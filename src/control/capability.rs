//! Features of this binary's control API, as `rozi api describe` lists them.
//!
//! These answer "what does this rozi accept from a control client or an extension", which is a
//! different question from the session wire's [`crate::session::protocol::Capability`]: a control
//! capability describes a whole binary, a wire capability one negotiated session connection. Both
//! keep their names forever; add a variant, never rename one.

/// One feature a control client or an extension can check for before relying on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ControlCapability {
    AgentPromptResults,
    /// `agents prompt` and `agents wait` return the agent's own reply when its integration reports
    /// one, instead of only a terminal capture.
    AgentResponses,
    AgentWaits,
    /// A UI's control socket accepts `record-start`, `record-stop`, `record-list`, and `record-mark`,
    /// forwarding them to the session server it is attached to.
    AttachedControl,
    /// `list-panes --scope attached`, and a service manifest's `session_scope`. A placed service runs
    /// the host's `rozi`, so its host has to offer this too, not only the client that loaded it.
    AttachedSessionScope,
    CallerContext,
    /// `capture-pane` honors `render`: `ansi` and `png` as well as text. An older binary ignores the
    /// field and answers with text.
    CaptureRender,
    /// `capture-pane` and `capture-ui` honor `scale` for a PNG. An older binary ignores the field and
    /// answers at scale 1.
    CaptureScale,
    /// `capture-pane` and `capture-ui` accept `render: "spans"`, and `image_pixels` with it. An older
    /// binary refuses the render as an invalid request.
    CaptureSpans,
    /// `capture-ui` returns the whole client as it is drawn, chrome and every visible pane.
    CaptureUi,
    /// `capture-pane`, `send-text`, and `send-keys` honor `wait`, holding the reply until the pane's
    /// screen shows some text or settles. An older binary ignores the field and answers at once.
    CaptureWait,
    /// A UI answers `command-visibility`, which shows or hides an extension's own command in the
    /// command palette, and extension manifests accept `hidden` on `[[commands]]`.
    CommandVisibility,
    /// `sessions new <NAME> --detach` starts a session server and returns without opening a UI.
    DetachedSessionCreate,
    /// Extension manifests accept link handlers.
    ExtensionLinkHandlers,
    /// This host's extension runtime gives every process it starts a state directory on that host.
    ExtensionStateDir,
    /// A UI answers `agents-overview`, every agent it knows about in every session and on every
    /// connected host, and `agent-go`, which goes to one of them.
    GlobalAgents,
    /// `layout get` reports workspaces and pane geometry from the shared layout document.
    LayoutControl,
    PaneControl,
    /// A UI answers `pane-reveal`, scrolling a Scrollable strip to a pane without moving focus.
    PaneReveal,
    /// A pick action's prompt object honors `from_query`, starting from the picker's filter text. An
    /// older binary ignores the field and starts from `value`.
    PickPromptFromQuery,
    /// A UI answers `prompt`, a modal text prompt with no picker behind it.
    Prompt,
    /// `publish` accepts `kind` and `name`: a publisher declares whether its rows are agents, and
    /// several publishers share a pane, each owning its own rows. Without it every published row is
    /// treated as an agent and a pane holds one publisher.
    PublishKind,
    PublishedActivity,
    /// A session server records a pane with `record-start`, `record-stop`, `record-list`, and
    /// `record-mark`, and `list-panes` and `metrics` report recordings.
    RecordPane,
    /// A UI records itself as it paints, chrome included, with `record-ui-start`, `record-ui-stop`,
    /// and `record-ui-mark`, into a file on the UI's own host.
    RecordUi,
    /// This binary can both forward a control command to a session on another host and serve one
    /// forwarded to it. Advertised by `api describe`, so a caller can check the far host's rozi before
    /// relying on it.
    RemoteControl,
    /// This host's Rozi can run placed extension processes.
    RemoteExtensionRuntime,
    /// `subscribe` and hooks accept the `agent-state-changed` and `foreground-changed` events. An older
    /// binary refuses a subscription naming either, and warns about a hook that does.
    RuntimeEvents,
    /// A session server runs deployed actions: `invoke`, `invocation-list`, `invocation-cancel`, and
    /// `actions-list`.
    SessionActions,
    SessionControl,
    /// A session server stores, verifies, lists, and removes extension deployments of its own, so a
    /// client can hand it the files an extension will run when no UI is attached.
    SessionDeployments,
    /// `sessions new <NAME> --detach --no-resurrect` starts a session that never writes a snapshot.
    SessionNoResurrect,
    /// A session server accepts `notify`, keeps a bounded inbox, and includes it in subscription
    /// snapshots.
    SessionNotifications,
    /// A session server runs the `placement = "session"` services of what is deployed to it, with no UI
    /// attached, under credentials it issues itself.
    SessionServices,
    /// `subscribe` on session endpoints streams authoritative session events, snapshots, and cursors.
    SessionSubscribe,
    /// `sidebar-publish` streams rows into a sidebar stream tab, several producers to a tab, and
    /// command tabs accept `format = "json"`.
    SidebarStreams,
    /// Extension manifests accept a `[settings]` entry's table form (`default`, `label`,
    /// `description`, `choices`, `min`, `max`) and `settings` on `[[sidebar_tabs]]`, offered in the tab
    /// settings picker. An older binary rejects both.
    SidebarTabSettings,
    /// `new-pane` honors `target` and `split_ratio`: it splits the named pane, and sizes the split it
    /// opens. An older binary ignores both and splits the caller's or the focused pane evenly.
    SplitPlacement,
    /// `new-pane` honors `size`, starting the pane at that terminal size. An older binary ignores the
    /// field and starts the pane at its default size.
    SplitSize,
}

impl ControlCapability {
    /// Every control capability this binary has, in name order.
    pub const ALL: [ControlCapability; 39] = [
        ControlCapability::AgentPromptResults,
        ControlCapability::AgentResponses,
        ControlCapability::AgentWaits,
        ControlCapability::AttachedControl,
        ControlCapability::AttachedSessionScope,
        ControlCapability::CallerContext,
        ControlCapability::CaptureRender,
        ControlCapability::CaptureScale,
        ControlCapability::CaptureSpans,
        ControlCapability::CaptureUi,
        ControlCapability::CaptureWait,
        ControlCapability::CommandVisibility,
        ControlCapability::DetachedSessionCreate,
        ControlCapability::ExtensionLinkHandlers,
        ControlCapability::ExtensionStateDir,
        ControlCapability::GlobalAgents,
        ControlCapability::LayoutControl,
        ControlCapability::PaneControl,
        ControlCapability::PaneReveal,
        ControlCapability::PickPromptFromQuery,
        ControlCapability::Prompt,
        ControlCapability::PublishKind,
        ControlCapability::PublishedActivity,
        ControlCapability::RecordPane,
        ControlCapability::RecordUi,
        ControlCapability::RemoteControl,
        ControlCapability::RemoteExtensionRuntime,
        ControlCapability::RuntimeEvents,
        ControlCapability::SessionActions,
        ControlCapability::SessionControl,
        ControlCapability::SessionDeployments,
        ControlCapability::SessionNoResurrect,
        ControlCapability::SessionNotifications,
        ControlCapability::SessionServices,
        ControlCapability::SessionSubscribe,
        ControlCapability::SidebarStreams,
        ControlCapability::SidebarTabSettings,
        ControlCapability::SplitPlacement,
        ControlCapability::SplitSize,
    ];

    /// The name this capability is listed under.
    pub const fn as_str(self) -> &'static str {
        match self {
            ControlCapability::AgentPromptResults => "agent-prompt-results",
            ControlCapability::AgentResponses => "agent-responses",
            ControlCapability::AgentWaits => "agent-waits",
            ControlCapability::AttachedControl => "attached-control",
            ControlCapability::AttachedSessionScope => "attached-session-scope",
            ControlCapability::CallerContext => "caller-context",
            ControlCapability::CaptureRender => "capture-render",
            ControlCapability::CaptureScale => "capture-scale",
            ControlCapability::CaptureSpans => "capture-spans",
            ControlCapability::CaptureUi => "capture-ui",
            ControlCapability::CaptureWait => "capture-wait",
            ControlCapability::CommandVisibility => "command-visibility",
            ControlCapability::DetachedSessionCreate => "detached-session-create",
            ControlCapability::ExtensionLinkHandlers => "extension-link-handlers",
            ControlCapability::ExtensionStateDir => "extension-state-dir",
            ControlCapability::GlobalAgents => "global-agents",
            ControlCapability::LayoutControl => "layout-control",
            ControlCapability::PaneControl => "pane-control",
            ControlCapability::PaneReveal => "pane-reveal",
            ControlCapability::PickPromptFromQuery => "pick-prompt-from-query",
            ControlCapability::Prompt => "prompt",
            ControlCapability::PublishKind => "publish-kind",
            ControlCapability::PublishedActivity => "published-activity",
            ControlCapability::RecordPane => "record-pane",
            ControlCapability::RecordUi => "record-ui",
            ControlCapability::RemoteControl => "remote-control",
            ControlCapability::RemoteExtensionRuntime => "remote-extension-runtime",
            ControlCapability::RuntimeEvents => "runtime-events",
            ControlCapability::SessionActions => "session-actions",
            ControlCapability::SessionControl => "session-control",
            ControlCapability::SessionDeployments => "session-deployments",
            ControlCapability::SessionNoResurrect => "session-no-resurrect",
            ControlCapability::SessionNotifications => "session-notifications",
            ControlCapability::SessionServices => "session-services",
            ControlCapability::SessionSubscribe => "session-subscribe",
            ControlCapability::SidebarStreams => "sidebar-streams",
            ControlCapability::SidebarTabSettings => "sidebar-tab-settings",
            ControlCapability::SplitPlacement => "split-placement",
            ControlCapability::SplitSize => "split-size",
        }
    }
}

impl std::fmt::Display for ControlCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_sorted() {
        let names: Vec<_> = ControlCapability::ALL
            .iter()
            .map(|cap| cap.as_str())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
    }
}
