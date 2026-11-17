//! Cross-user session sharing grants, shared listener management, and live access updates.

use std::collections::BTreeMap;
use std::io;
#[cfg(test)]
use std::path::PathBuf;

use super::*;
use crate::control::{ControlErrorCode, ControlResponse, ShareGrantInfo, ShareState};
use crate::platform::user::{
    UserAccount, UserId, account_of, current_user_id, lookup_user, session_sharing_unavailable,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Grant {
    pub(super) user: String,
    pub(super) role: protocol::SessionRole,
}

#[derive(Default)]
pub(super) struct Shares {
    grants: BTreeMap<UserId, Grant>,
    listener: Option<IpcListener>,
    endpoint: Option<IpcEndpoint>,
    #[cfg(test)]
    test_shared_dir: Option<PathBuf>,
}

impl Shares {
    pub(super) fn grant_for(&self, uid: UserId) -> Option<&Grant> {
        self.grants.get(&uid)
    }

    pub(super) fn state(&self, clients: &[ClientConn]) -> ShareState {
        let endpoint = self
            .endpoint
            .as_ref()
            .map(|ep| ep.path().to_string_lossy().into_owned());
        let grants = self
            .grants
            .iter()
            .map(|(uid, grant)| {
                let connected = clients
                    .iter()
                    .filter(|client| {
                        client.attached
                            && !client.close_after_flush
                            && client.guest.as_ref().is_some_and(|g| g.id == *uid)
                    })
                    .count();
                ShareGrantInfo {
                    user: grant.user.clone(),
                    uid: uid.as_raw(),
                    role: grant.role,
                    connected,
                }
            })
            .collect();
        ShareState { endpoint, grants }
    }

    #[cfg(all(test, unix))]
    pub(super) fn set_test_shared_dir(&mut self, dir: PathBuf) {
        self.test_shared_dir = Some(dir);
    }

    #[cfg(all(test, unix))]
    pub(super) fn endpoint_path(&self) -> Option<&std::path::Path> {
        self.endpoint.as_ref().map(IpcEndpoint::path)
    }

    fn bind_for_session(&self, session_name: &str) -> io::Result<(IpcListener, IpcEndpoint)> {
        #[cfg(test)]
        if let Some(dir) = &self.test_shared_dir {
            let endpoint = EndpointRegistry::shared_session_endpoint(dir, session_name);
            let bound = endpoint.bind()?;
            let listener = bound.into_listener();
            listener.set_nonblocking(true)?;
            return Ok((listener, endpoint));
        }

        let shared_dir = crate::platform::paths::ensure_own_shared_dir()?;
        let endpoint = EndpointRegistry::shared_session_endpoint(&shared_dir, session_name);
        let bound = endpoint.bind_shared()?;
        let listener = bound.into_listener();
        listener.set_nonblocking(true)?;
        Ok((listener, endpoint))
    }

    pub(super) fn close_listener(&mut self) {
        // Retire the socket path while this server still holds the listener, matching private
        // endpoint retirement in `shutdown_listener`.
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.remove_stale();
        }
        self.listener.take();
    }

    pub(super) fn prepare_rebind(
        &self,
        session_name: &str,
    ) -> io::Result<Option<(IpcListener, IpcEndpoint)>> {
        if self.listener.is_none() {
            return Ok(None);
        }
        self.bind_for_session(session_name).map(Some)
    }

    pub(super) fn commit_rebind(&mut self, pending: Option<(IpcListener, IpcEndpoint)>) {
        let Some((listener, endpoint)) = pending else {
            return;
        };
        if let Some(old_endpoint) = self.endpoint.replace(endpoint) {
            old_endpoint.remove_stale();
        }
        self.listener = Some(listener);
    }

    fn account_name_for(&self, uid: UserId) -> String {
        self.grants
            .get(&uid)
            .map(|grant| grant.user.clone())
            .or_else(|| account_of(uid).map(|account| account.name))
            .unwrap_or_else(|| uid.to_string())
    }
}

/// How long a connection to the shared endpoint may stay silent before its first frame.
const SHARED_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections the shared endpoint takes in one pump, so a connect flood cannot stall the session.
const MAX_SHARED_ACCEPTS_PER_PUMP: usize = 32;
/// Connections one granted account may hold at once.
const MAX_CLIENTS_PER_GUEST: usize = 16;
/// Connections from accounts without a grant that may wait to hear they are refused.
const MAX_UNSHARED_PEERS: usize = 4;

impl SessionServer {
    pub(super) fn accept_shared(&mut self) -> io::Result<bool> {
        let Some(listener) = self.shares.listener.as_ref() else {
            return Ok(false);
        };
        let mut accepted_streams = Vec::new();
        while accepted_streams.len() < MAX_SHARED_ACCEPTS_PER_PUMP {
            match listener.accept() {
                Ok(stream) => accepted_streams.push(stream),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err),
            }
        }
        let accepted = !accepted_streams.is_empty();
        for stream in accepted_streams {
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let Some(peer_uid) = stream.peer_user() else {
                // Drop connections whose OS identity cannot be verified by the kernel.
                continue;
            };
            if !self.shared_peer_has_room(peer_uid) {
                continue;
            }
            let id = self.next_client_id;
            self.next_client_id += 1;
            let conn = self.client_conn_from_shared(id, stream, peer_uid);
            self.clients.push(conn);
        }
        Ok(accepted)
    }

    /// Whether another connection from `peer_uid` fits. Anyone may connect to the shared socket, so
    /// an account without a grant only gets a few slots, each closed after its refusal or when it
    /// stays silent, and a granted account gets a bounded number of clients.
    pub(super) fn shared_peer_has_room(&self, peer_uid: UserId) -> bool {
        if peer_uid == current_user_id() {
            return true;
        }
        let granted = self.shares.grant_for(peer_uid).is_some();
        let held = self
            .clients
            .iter()
            .filter(|client| {
                client.guest.as_ref().is_some_and(|guest| {
                    if granted {
                        guest.id == peer_uid
                    } else {
                        self.shares.grant_for(guest.id).is_none()
                    }
                })
            })
            .count();
        held < if granted {
            MAX_CLIENTS_PER_GUEST
        } else {
            MAX_UNSHARED_PEERS
        }
    }

    /// Close shared-endpoint connections that never sent a frame, and revoked clients that have not
    /// taken their eviction notice in time.
    pub(super) fn expire_shared_connections(&mut self, now: Instant) {
        let silent: Vec<ClientId> = self
            .clients
            .iter()
            .filter(|client| {
                client.handshake_deadline.is_some_and(|at| now >= at)
                    || client
                        .revoked_at
                        .is_some_and(|at| now >= at + SHARED_HANDSHAKE_TIMEOUT)
            })
            .map(|client| client.id)
            .collect();
        for id in silent {
            self.remove_client(id);
        }
    }

    pub(super) fn client_conn_from_shared(
        &self,
        id: ClientId,
        stream: IpcConnection,
        peer_uid: UserId,
    ) -> ClientConn {
        let mut conn = ClientConn::new(id, stream);
        conn.handshake_deadline = Some(Instant::now() + SHARED_HANDSHAKE_TIMEOUT);
        if peer_uid != current_user_id() {
            let name = self.shares.account_name_for(peer_uid);
            conn.access = protocol::ClientAccess::guest(protocol::SessionRole::Viewer, true);
            conn.guest = Some(UserAccount { id: peer_uid, name });
        }
        conn
    }

    pub(super) fn share_grant(
        &mut self,
        user: &str,
        role: protocol::SessionRole,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        self.share_grant_with(user, role, lookup_user, broadcasts)
    }

    pub(super) fn share_grant_with(
        &mut self,
        user: &str,
        role: protocol::SessionRole,
        lookup: impl FnOnce(&str) -> io::Result<Option<UserAccount>>,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        if let Some(reason) = session_sharing_unavailable() {
            return ControlResponse::error_with(ControlErrorCode::Unsupported, reason);
        }
        if role == protocol::SessionRole::Owner {
            return ControlResponse::error_with(
                ControlErrorCode::InvalidRequest,
                "cannot grant the owner role",
            );
        }
        let account = match lookup(user) {
            Ok(Some(account)) => account,
            Ok(None) => {
                return ControlResponse::error_with(
                    ControlErrorCode::InvalidArgument,
                    format!("no account named `{user}` on this host"),
                );
            }
            Err(err) if err.kind() == io::ErrorKind::Unsupported => {
                return ControlResponse::error_with(ControlErrorCode::Unsupported, err.to_string());
            }
            Err(err) => {
                return ControlResponse::error_with(
                    ControlErrorCode::RequestFailed,
                    err.to_string(),
                );
            }
        };
        if account.id == current_user_id() {
            return ControlResponse::error_with(
                ControlErrorCode::InvalidRequest,
                "cannot share a session with yourself",
            );
        }
        if self.shares.listener.is_none() {
            match self.shares.bind_for_session(&self.session_name) {
                Ok((listener, endpoint)) => {
                    self.shares.listener = Some(listener);
                    self.shares.endpoint = Some(endpoint);
                }
                Err(err) if err.kind() == io::ErrorKind::Unsupported => {
                    return ControlResponse::error_with(
                        ControlErrorCode::Unsupported,
                        err.to_string(),
                    );
                }
                Err(err) => {
                    return ControlResponse::error_with(
                        ControlErrorCode::RequestFailed,
                        format!("could not bind shared endpoint: {err}"),
                    );
                }
            }
        }
        self.shares.grants.insert(
            account.id,
            Grant {
                user: account.name.clone(),
                role,
            },
        );
        self.apply_live_role_change(&account, role, broadcasts);
        ControlResponse::ok(self.shares.state(&self.clients))
    }

    fn apply_live_role_change(
        &mut self,
        account: &UserAccount,
        role: protocol::SessionRole,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) {
        let mut any_attached_updated = false;
        let mut lease_lost_by_controller = false;
        for client in &mut self.clients {
            if client.guest.as_ref().is_some_and(|g| g.id == account.id) {
                client.guest = Some(account.clone());
                if client.attached {
                    let new_access =
                        protocol::ClientAccess::guest(role, client.requested_read_only);
                    if client.access != new_access {
                        client.access = new_access;
                        any_attached_updated = true;
                    }
                    if !client.access.allows(protocol::Permission::Layout) {
                        client.requesting_control = false;
                        client.last_request_notify = None;
                        if self.controller == Some(client.id) {
                            lease_lost_by_controller = true;
                        }
                    }
                } else {
                    client.access = protocol::ClientAccess::guest(role, false);
                }
            }
        }
        if lease_lost_by_controller {
            broadcasts.extend(self.release_controller());
        } else if any_attached_updated {
            broadcasts.push((Target::Broadcast, self.clients_changed()));
        }
    }

    pub(super) fn share_revoke(
        &mut self,
        user: Option<&str>,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        self.share_revoke_with(user, lookup_user, broadcasts)
    }

    pub(super) fn share_revoke_with(
        &mut self,
        user: Option<&str>,
        lookup: impl FnOnce(&str) -> io::Result<Option<UserAccount>>,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        let revoked_uids: Vec<UserId> = match user {
            None => self.shares.grants.keys().copied().collect(),
            Some(spec) => {
                let parsed_uid = spec.parse::<u32>().ok().map(UserId::from_raw);
                let matched = self
                    .shares
                    .grants
                    .iter()
                    .find(|(uid, grant)| grant.user == spec || parsed_uid == Some(**uid))
                    .map(|(uid, _)| *uid)
                    .or_else(|| {
                        lookup(spec).ok().flatten().and_then(|acc| {
                            self.shares.grants.contains_key(&acc.id).then_some(acc.id)
                        })
                    });
                matched.into_iter().collect()
            }
        };

        for uid in &revoked_uids {
            self.shares.grants.remove(uid);
        }
        if self.shares.grants.is_empty() {
            self.shares.close_listener();
        }

        let mut targets = Vec::new();
        for client in &mut self.clients {
            if client
                .guest
                .as_ref()
                .is_some_and(|g| revoked_uids.contains(&g.id))
            {
                client.access = protocol::ClientAccess::guest(protocol::SessionRole::Viewer, true);
                client.requesting_control = false;
                client.last_request_notify = None;
                targets.push((client.id, client.attached));
            }
        }
        let mut controller_revoked = false;
        for (target, attached) in targets {
            self.handoffs.forget_client(target);
            self.set_close_after_flush(target);
            if attached {
                self.enqueue(
                    target,
                    Target::Sender,
                    ServerMessage::Error {
                        code: protocol::EVICTED_ERROR_CODE.to_string(),
                        message: "access to this session was revoked".to_string(),
                        peer: None,
                    },
                );
            }
            // Revocation takes effect now, not when the outbox drains: whatever the session
            // produces from here on, and any reply still held for this client, is not queued.
            if let Some(client) = self.client_mut(target) {
                client.revoked_at = Some(Instant::now());
            }
            if self.controller == Some(target) {
                controller_revoked = true;
            }
        }
        if controller_revoked {
            broadcasts.extend(self.release_controller());
        }
        self.release_revoked_handoffs(&revoked_uids, broadcasts);
        ControlResponse::ok(self.shares.state(&self.clients))
    }

    pub(super) fn share_list(&self) -> ControlResponse {
        ControlResponse::ok(self.shares.state(&self.clients))
    }
}
