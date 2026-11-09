//! Client slots held across a client relaunch.
//!
//! A client about to be replaced by a new process - the same UI started again on a newer build -
//! reserves its slot in two steps, like a transaction. [`ClientMessage::PrepareHandoff`] creates an
//! inert reservation and returns its token; nothing changes if the client then stays, or leaves,
//! which discards it. [`ClientMessage::CommitHandoff`] commits it, and the client leaves only once
//! every commit is answered; [`ClientMessage::AbortHandoff`] takes a commit back when the relaunch
//! fails. Once a committed client leaves, the server treats the client as in transit rather than gone: the
//! layout-control lease stays with it instead of passing to another client, nobody is told it left,
//! and an ephemeral session does not start counting down to its own shutdown. The process that
//! replaces it presents the token when it attaches and takes the slot over. A reservation nobody
//! claims expires, and the client is then treated exactly as if it had left when it detached.

use super::*;

/// How long a slot is held for a client that asks for no particular time: from the reservation to
/// the commit, and from the client leaving to the replacement claiming it.
pub(super) const HANDOFF_HOLD: Duration = Duration::from_secs(120);

/// The longest hold a client may ask for. The lease is held this long against every other client
/// when a relaunch fails after its commit, so a client that needs longer - many sessions to reach
/// over SSH, each allowed time for a password - is refused before it lets go, not trusted.
pub(super) const MAX_HANDOFF_HOLD: Duration = Duration::from_secs(30 * 60);

/// How long a committed client may stay connected. It commits as the very last thing before it
/// leaves, so one still here is a relaunch that stopped; its commit lapses rather than turn a
/// later, ordinary departure into a held slot.
pub(crate) const COMMITTED_DEPARTURE_WINDOW: Duration = Duration::from_secs(30);

/// One client's reserved slot.
pub(super) struct Reservation {
    token: String,
    client: ClientId,
    label: Option<String>,
    issued: Instant,
    /// How long the replacement has to claim the slot once the client has left.
    hold: Duration,
    /// When [`ClientMessage::CommitHandoff`] committed it. Until then the reservation holds
    /// nothing.
    committed: Option<Instant>,
    departure: Option<Departure>,
}

/// What the client held when its connection went.
#[derive(Clone, Copy)]
struct Departure {
    at: Instant,
    parked: bool,
    held_lease: bool,
}

impl Reservation {
    fn expired(&self, now: Instant) -> bool {
        let (since, lasts) = match (self.departure, self.committed) {
            (Some(departure), _) => (departure.at, self.hold),
            (None, Some(committed)) => (committed, COMMITTED_DEPARTURE_WINDOW),
            (None, None) => (self.issued, HANDOFF_HOLD),
        };
        now.duration_since(since) >= lasts
    }

    fn is_committed(&self) -> bool {
        self.committed.is_some()
    }
}

/// What claiming a reservation hands the new connection.
pub(super) struct Claimed {
    pub(super) parked: bool,
    pub(super) held_lease: bool,
}

#[derive(Default)]
pub(super) struct Reservations {
    held: Vec<Reservation>,
}

impl Reservations {
    /// Whether any client is in transit: gone, with a replacement still expected.
    pub(super) fn any_in_transit(&self) -> bool {
        self.held.iter().any(|held| held.departure.is_some())
    }

    fn position_for_client(&self, client: ClientId) -> Option<usize> {
        self.held.iter().position(|held| held.client == client)
    }

    pub(super) fn forget_client(&mut self, client: ClientId) {
        self.held.retain(|held| held.client != client);
    }
}

fn new_token() -> io::Result<String> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    Ok(crate::extension_runtime::bundle::hex_digest(&random))
}

impl SessionServer {
    /// Reserve `client_id`'s slot and give it the token that claims it. A second request replaces
    /// the first reservation, so only the newest token is ever valid.
    pub(super) fn handle_prepare_handoff(
        &mut self,
        client_id: ClientId,
        request_id: u64,
        hold_ms: u64,
    ) -> Vec<(Target, ServerMessage)> {
        let refused = |error: &str| {
            vec![(
                Target::Sender,
                ServerMessage::HandoffPrepared {
                    request_id,
                    token: String::new(),
                    hold_ms: 0,
                    error: Some(error.to_string()),
                },
            )]
        };
        let Some(label) = self
            .clients
            .iter()
            .find(|client| client.id == client_id && client.attached)
            .map(|client| client.label.clone())
        else {
            return refused("only an attached client can reserve its slot");
        };
        let token = match new_token() {
            Ok(token) => token,
            Err(error) => return refused(&format!("no randomness for a token: {error}")),
        };
        let hold = match hold_ms {
            0 => HANDOFF_HOLD,
            asked => Duration::from_millis(asked).min(MAX_HANDOFF_HOLD),
        };
        self.handoffs.forget_client(client_id);
        self.handoffs.held.push(Reservation {
            token: token.clone(),
            client: client_id,
            label,
            issued: Instant::now(),
            hold,
            committed: None,
            departure: None,
        });
        vec![(
            Target::Sender,
            ServerMessage::HandoffPrepared {
                request_id,
                token,
                hold_ms: u64::try_from(hold.as_millis()).unwrap_or(u64::MAX),
                error: None,
            },
        )]
    }

    /// Commit `client`'s reservation `token`, and say whether it was. A token the client does not
    /// hold, or one that expired, commits nothing.
    pub(super) fn handle_commit_handoff(
        &mut self,
        client: ClientId,
        request_id: u64,
        token: &str,
    ) -> Vec<(Target, ServerMessage)> {
        let now = Instant::now();
        let reservation = self
            .handoffs
            .held
            .iter_mut()
            .find(|held| held.client == client && held.token == token && !held.expired(now));
        let error = match reservation {
            Some(reservation) => {
                reservation.committed = Some(now);
                None
            }
            None => Some("no reservation for that token".to_string()),
        };
        vec![(
            Target::Sender,
            ServerMessage::HandoffCommitted { request_id, error },
        )]
    }

    /// Discard `client`'s reservation `token`, and confirm that none remains: the relaunch failed
    /// and the client stays. A token already gone is confirmed all the same.
    pub(super) fn handle_abort_handoff(
        &mut self,
        client: ClientId,
        request_id: u64,
        token: &str,
    ) -> Vec<(Target, ServerMessage)> {
        self.handoffs
            .held
            .retain(|held| !(held.client == client && held.token == token));
        vec![(Target::Sender, ServerMessage::HandoffAborted { request_id })]
    }

    /// Record that a reserving client's connection went, and say whether its slot is now held. A
    /// held slot keeps the lease where it is; the caller skips promotion and the `client-left`
    /// event. An uncommitted reservation is discarded: the client left without handing over.
    pub(super) fn hold_departing_client(&mut self, client: ClientId, parked: bool) -> bool {
        let now = Instant::now();
        let held_lease = self.controller == Some(client);
        let Some(index) = self.handoffs.position_for_client(client) else {
            return false;
        };
        let reservation = &mut self.handoffs.held[index];
        if !reservation.is_committed() || reservation.expired(now) {
            self.handoffs.held.remove(index);
            return false;
        }
        reservation.departure = Some(Departure {
            at: now,
            parked,
            held_lease,
        });
        true
    }

    /// Take over the slot `token` reserves for `claimer`. A reserving client that has not finished
    /// leaving yet - its close is still unread - is removed first, so its slot passes over whole.
    pub(super) fn claim_handoff(&mut self, claimer: ClientId, token: &str) -> Option<Claimed> {
        let now = Instant::now();
        // Only a committed reservation can be claimed: before the commit, the client that made it
        // may still decide to stay.
        let index = self.handoffs.held.iter().position(|held| {
            held.token == token && held.client != claimer && held.is_committed()
        })?;
        if self.handoffs.held[index].expired(now) {
            let expired = self.handoffs.held.remove(index);
            self.release_reservation(expired);
            return None;
        }
        let reserving = self.handoffs.held[index].client;
        if self.client_attached(reserving) {
            self.remove_client(reserving);
        }
        let index = self.handoffs.position_for_client(reserving)?;
        let reservation = self.handoffs.held.remove(index);
        let departure = reservation.departure?;
        if departure.held_lease && self.controller == Some(reserving) {
            self.controller = Some(claimer);
        }
        Some(Claimed {
            parked: departure.parked,
            held_lease: departure.held_lease,
        })
    }

    /// Drop reservations whose time ran out. A client that already left is now treated as having
    /// left for good: the lease moves on and its departure is announced.
    pub(super) fn expire_handoffs(&mut self) {
        let now = Instant::now();
        let (expired, held): (Vec<_>, Vec<_>) = std::mem::take(&mut self.handoffs.held)
            .into_iter()
            .partition(|held| held.expired(now));
        self.handoffs.held = held;
        for reservation in expired {
            self.release_reservation(reservation);
        }
    }

    /// Finish the departure a reservation postponed.
    fn release_reservation(&mut self, reservation: Reservation) {
        if reservation.departure.is_none() {
            return;
        }
        let count = self.client_roster().len().to_string();
        self.record_session_event(
            events::EventKind::ClientLeft,
            vec![
                ("client_id", reservation.client.to_string()),
                ("client_name", reservation.label.unwrap_or_default()),
                ("count", count),
            ],
        );
        if self.controller != Some(reservation.client) {
            return;
        }
        let messages = self.release_departed_lease(ControllerChangeReason::Expired);
        for message in messages {
            self.broadcast_control(&message);
        }
    }
}

#[cfg(all(test, unix))]
impl Reservations {
    /// Move every reservation `by` into the past, as if that much time had gone by.
    pub(super) fn age_for_test(&mut self, by: Duration) {
        for held in &mut self.held {
            held.issued -= by;
            if let Some(committed) = &mut held.committed {
                *committed -= by;
            }
            if let Some(departure) = &mut held.departure {
                departure.at -= by;
            }
        }
    }
}
