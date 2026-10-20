use std::cell::{Cell, RefCell};
use std::time::Instant;

use tui_lipan::prelude::{FloatRect, Key, ManagedTerminalStatus, Span, Text, TextSource};

use crate::pane::{TerminalPane, shell_title_parts};

use super::{DEFAULT_SCROLLABLE_WIDTH, PaneId, PaneIdentity};

pub struct Pane {
    pub id: PaneId,
    pub pty_generation: u64,
    pub title: String,
    pub identity: PaneIdentity,
    pub floating: bool,
    pub fullscreen: bool,
    pub floating_rect: FloatRect,
    /// Whether `floating_rect` is user-visible floating geometry rather than only the default size
    /// prepared for a tiled pane's first lift-off. Local presentation state; never serialized.
    pub floating_rect_initialized: bool,
    /// Scrollable layout column width as a fraction of the tile viewport.
    pub scrollable_width: f32,
    pub opening: bool,
    /// Resolved at the start of opening so a reload cannot change the effect under a live pane.
    pub opening_animation: Option<crate::layout::anim::PaneAnimationSnapshot>,
    /// Tile edge this pane slides in from and back out toward, under
    /// [`PaneAnimationStyle::Slide`](crate::layout::anim::PaneAnimationStyle::Slide). Set from the split that
    /// placed the pane and never changed after, so the close is the open in reverse. Purely local
    /// view state: it is never shared, persisted, or sent over the session protocol.
    pub slide_edge: crate::layout::anim::SlideEdge,
    pub terminal_active: bool,
    /// Removed from the layout but still described, so its close animation can run.
    pub closing: bool,
    /// Resolved at the start of closing so retention and rendering share one immutable recipe.
    pub closing_animation: Option<crate::layout::anim::PaneAnimationSnapshot>,
    /// Local handoff of the fade when the pane moves into its retained closing layer.
    pub(crate) fade_closing: Cell<bool>,
    pub(crate) paint_clock: Cell<Option<crate::layout::anim::PanePaintClock>>,
    pub(crate) fade_stage: Cell<crate::layout::anim::FadeStage>,
    pub logging: bool,
    pub activity: PaneActivity,
    pub agents: Vec<crate::session::protocol::AgentRecord>,
    pub terminal: TerminalPane,
    pub(crate) live_title: LivePaneTitle,
    /// This pane's element keys, built once here rather than formatted in every `view()`.
    ///
    /// Both are functions of [`id`](Self::id) alone, which never changes for a live pane, so a
    /// cached key cannot go stale. The window key is deliberately absent: it also depends on
    /// `pty_generation`, which is a public field written from many places, and a cache behind a
    /// field that anything may assign is a correctness trap rather than a saving.
    pub keys: PaneKeys,
}

/// The title label's live content and the fixed presentation prefix chosen by the view.
#[derive(Default)]
pub(crate) struct LivePaneTitle {
    source: TextSource,
    prefix: RefCell<String>,
    marker: RefCell<Option<String>>,
    pub bound: Cell<bool>,
}

impl LivePaneTitle {
    pub fn bind(&self, prefix: &str, title: &str, marker: Option<&str>) -> Text {
        *self.prefix.borrow_mut() = prefix.to_string();
        *self.marker.borrow_mut() = marker.map(str::to_string);
        self.source
            .set([Span::new(prefix.to_string()), Span::new(title.to_string())]);
        self.bound.set(true);
        Text::from_source(self.source.clone())
    }

    pub fn refresh(&self, title: String) {
        let title = match self.marker.borrow().as_deref() {
            Some(marker) => format!("{marker} · {title}"),
            None => title,
        };
        self.source
            .set([Span::new(self.prefix.borrow().clone()), Span::new(title)]);
    }
}

/// A chrome colour a pane animates independently.
///
/// Each slot is one animation identity, so the set is closed and its names are part of the key
/// format rather than free text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeSlot {
    /// Titlebar background.
    TitleBg,
    /// Titlebar text.
    TitleFg,
    /// Pane frame background.
    FrameBg,
    /// Pane frame border.
    FrameFg,
    /// The divider drawn between this pane and its neighbour.
    DividerFg,
}

impl ChromeSlot {
    const ALL: [Self; 5] = [
        Self::TitleBg,
        Self::TitleFg,
        Self::FrameBg,
        Self::FrameFg,
        Self::DividerFg,
    ];

    /// The slot's name as it appears in the animation key. Changing one restarts that fade.
    const fn name(self) -> &'static str {
        match self {
            Self::TitleBg => "title-bg",
            Self::TitleFg => "title-fg",
            Self::FrameBg => "frame-bg",
            Self::FrameFg => "frame-fg",
            Self::DividerFg => "divider-fg",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// Per-pane element and animation keys, cached for the lifetime of the pane.
///
/// Rebuilding these in `view()` cost a `String` from `format!` plus the `Arc<str>` that `Key`
/// copies it into, for every pane on every frame.
pub struct PaneKeys {
    /// Keys the pane body element, the container holding the terminal and its chrome.
    pub body: Key,
    /// Keys the terminal widget itself; focus routing looks the pane up by this.
    pub terminal: Key,
    /// Stable wrapper key for the pane's optional full-size paint effect.
    pub effect_scope: Key,
    /// Tracks opacity independently of the pane's live or closing widget parent.
    pub(crate) opacity: Key,
    /// One animation key per [`ChromeSlot`], indexed by the slot.
    chrome: [Key; ChromeSlot::ALL.len()],
}

impl PaneKeys {
    fn new(id: PaneId) -> Self {
        Self {
            body: crate::view::pane_body_key(id).into(),
            terminal: crate::view::pane_terminal_key(id).into(),
            effect_scope: Key::from(format!("rozi-pane-effect-{id}")),
            opacity: Key::from(format!("rozi-pane-opacity-{id}")),
            chrome: ChromeSlot::ALL
                .map(|slot| Key::from(format!("rozi-pane-chrome-{id}-{}", slot.name()))),
        }
    }

    /// The animation key for one chrome slot.
    pub fn chrome(&self, slot: ChromeSlot) -> &Key {
        &self.chrome[slot.index()]
    }
}

/// Where an agent works, as the Activity sidebar and Agents view group and label it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentPlace {
    pub cwd: Option<String>,
    /// Host of a remote `cwd`; `None` for this machine.
    pub host: Option<String>,
    /// Git project containing `cwd`: the grouping key wherever it exists.
    pub project_root: Option<String>,
    /// Primary checkout when `project_root` is a linked worktree; the group is named after it.
    pub repository: Option<String>,
    pub branch: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneActivity {
    pub last_activity: Option<Instant>,
    pub has_unseen_output: bool,
    pub bell: bool,
}

impl Pane {
    pub fn new(id: PaneId, scrollback: usize, floating_rect: FloatRect) -> Self {
        Self {
            id,
            pty_generation: 0,
            title: "shell".to_string(),
            identity: PaneIdentity::default(),
            floating: false,
            fullscreen: false,
            floating_rect,
            floating_rect_initialized: false,
            scrollable_width: DEFAULT_SCROLLABLE_WIDTH,
            opening: true,
            opening_animation: None,
            slide_edge: crate::layout::anim::SlideEdge::default(),
            terminal_active: false,
            closing: false,
            closing_animation: None,
            fade_closing: Cell::new(false),
            paint_clock: Cell::new(None),
            fade_stage: Cell::new(crate::layout::anim::FadeStage::Running),
            logging: false,
            activity: PaneActivity::default(),
            live_title: LivePaneTitle::default(),
            agents: Vec::new(),
            keys: PaneKeys::new(id),
            terminal: {
                let mut terminal = TerminalPane::new(scrollback);
                terminal.bind_session(id, 0);
                terminal
            },
        }
    }

    /// The titlebar's display title including local exit/log markers.
    pub(crate) fn status_title(&self, remote_attached: bool) -> String {
        let mut title = self.titlebar_title(remote_attached);
        if self.closing {
            return title;
        }
        if let ManagedTerminalStatus::Exited(code) = self.terminal.status {
            title.push_str(&format!(" [exited {code}]"));
        }
        if self.logging {
            title.push_str(" [log]");
        }
        title
    }

    /// The semantic runtime the shared projections read, rebuilt from this client's copy.
    fn semantic_runtime(&self) -> crate::session::protocol::PaneRuntimeState {
        crate::session::protocol::PaneRuntimeState {
            status: self.terminal.reported_status.clone(),
            detected_agent: self.terminal.detected_agent.clone(),
            work_started_at: self.terminal.work_started_at,
            rows: self.terminal.published_rows.clone(),
            integration: self.terminal.agent_integration.clone(),
            ..crate::session::protocol::PaneRuntimeState::default()
        }
    }

    /// The agent projection: this pane's agents, for the Agents view and `rozi agents`.
    pub fn agent_runtimes(&self) -> Vec<crate::session::protocol::AgentRuntime> {
        crate::session::protocol::effective_agent_runtimes(&self.semantic_runtime(), &self.agents)
    }

    /// The agents this pane shows: its own, and of its published rows the ones on screen. Focusing
    /// the pane looks at these and only these - a conversation behind another tab of the same
    /// program has not been seen.
    pub fn agents_on_screen(&self) -> Vec<crate::session::protocol::AgentRuntime> {
        let rows = &self.terminal.published_rows;
        self.agent_runtimes()
            .into_iter()
            .filter(|runtime| {
                runtime
                    .reference
                    .slot
                    .as_ref()
                    .is_none_or(|slot| rows.iter().any(|row| row.has_key(slot) && row.active))
            })
            .collect()
    }

    /// Whether this pane finished something nobody has looked at: an agent it shows, by
    /// `attention`, or its own activity status. Drives the pane's finished border and its
    /// workspace's marker.
    pub fn finished_unseen(&self, attention: &crate::state::AgentAttention) -> bool {
        self.terminal.finished_unseen
            || self
                .agents_on_screen()
                .iter()
                .any(|runtime| attention.runtime_finished_unseen(runtime))
    }

    /// Whether one published row finished something nobody has looked at: by `attention` for an
    /// agent row, by the pane's own record of it for an activity row.
    pub fn row_finished_unseen(
        &self,
        key: &crate::session::protocol::RowKey,
        attention: &crate::state::AgentAttention,
    ) -> bool {
        match self
            .agent_runtimes()
            .into_iter()
            .find(|runtime| runtime.reference.slot.as_ref() == Some(key))
        {
            Some(runtime) => attention.runtime_finished_unseen(&runtime),
            None => self
                .terminal
                .published_row_ui
                .get(key)
                .is_some_and(|ui| ui.finished_unseen),
        }
    }

    /// The activity projection: everything happening in this pane, agents and activities alike,
    /// for the Activity sidebar.
    ///
    /// Program status records are split by who reports them first. The records an agent in the
    /// pane reports (`app=claude-code` from Claude Code, and the child records inheriting that app)
    /// describe that agent, so they ride on its occupant - of several conversations, the one on
    /// screen - rather than becoming a second row for it. Every other record is an activity, so
    /// a blocked `cargo` beside a working agent neither hides the agent's report nor takes it on.
    pub fn occupants(&self) -> Vec<crate::session::protocol::PaneOccupant> {
        let mut occupants = crate::session::protocol::pane_occupants(&self.semantic_runtime());
        let snapshot = &self.terminal.program_status;
        let agents: Vec<crate::session::protocol::AgentIdentity> = occupants
            .iter()
            .filter_map(|occupant| occupant.agent.clone())
            .collect();
        let reported_by =
            |record: &tui_lipan::ProgramStatusRecord,
             agent: &crate::session::protocol::AgentIdentity| {
                snapshot
                    .app_for(record)
                    .is_some_and(|app| program_status_names_agent(app, agent))
            };
        let rows = &self.terminal.published_rows;
        for occupant in &mut occupants {
            let Some(agent) = occupant.agent.as_ref() else {
                continue;
            };
            if occupant
                .slot
                .as_ref()
                .is_some_and(|slot| !rows.iter().any(|row| row.has_key(slot) && row.active))
            {
                continue;
            }
            occupant.program_status = crate::session::protocol::most_urgent_program_status(
                snapshot
                    .records
                    .iter()
                    .filter(|record| reported_by(record, agent)),
            )
            .cloned();
        }
        let Some(record) = crate::session::protocol::most_urgent_program_status(
            snapshot
                .records
                .iter()
                .filter(|record| !agents.iter().any(|agent| reported_by(record, agent))),
        ) else {
            return occupants;
        };
        let app = snapshot.app_for(record);
        let agent = app.and_then(|app| {
            crate::agent_detection::AgentCatalog::shared_builtin()
                .by_name(app)
                .map(|definition| definition.identity.label.clone())
        });
        occupants.push(crate::session::protocol::PaneOccupant {
            slot: None,
            agent: None,
            label: record
                .title
                .as_deref()
                .or(agent.as_deref())
                .or(app)
                .unwrap_or("")
                .to_string(),
            state: crate::session::protocol::ActivityState::from_status(record.state.as_str()),
            reason: record.msg.as_deref().map(str::to_string),
            source: crate::session::protocol::AgentAuthority::Reported,
            program_status: Some(record.clone()),
        });
        occupants
    }

    /// Where one of this pane's occupants is working.
    ///
    /// A published row that names its own `cwd` is somewhere else than the pane: a program that
    /// runs several sessions behind one terminal typically gives each its own Git worktree. Every
    /// other occupant is wherever the pane is.
    pub(crate) fn agent_place(
        &self,
        slot: Option<&crate::session::protocol::RowKey>,
    ) -> AgentPlace {
        let host = self.terminal.cwd_host.clone();
        let row = slot.and_then(|slot| {
            self.terminal
                .published_rows
                .iter()
                .find(|row| row.has_key(slot))
        });
        if let Some(row) = row
            && let Some(cwd) = row.cwd.clone()
        {
            let project = row.project.as_ref();
            return AgentPlace {
                cwd: Some(cwd),
                host,
                project_root: project.map(|project| project.root.clone()),
                repository: project.and_then(|project| project.repository.clone()),
                branch: project.and_then(|project| project.branch.clone()),
            };
        }
        let cwd = self
            .terminal
            .cwd
            .clone()
            .filter(|cwd| !cwd.trim().is_empty());
        let known = cwd.is_some();
        AgentPlace {
            host: known.then_some(host).flatten(),
            project_root: known.then(|| self.terminal.project_root.clone()).flatten(),
            repository: known.then(|| self.terminal.repository.clone()).flatten(),
            branch: known.then(|| self.terminal.git_branch.clone()).flatten(),
            cwd,
        }
    }

    pub(crate) fn begin_open_animation(
        &mut self,
        animations: crate::layout::anim::WindowAnimationConfig,
    ) {
        self.paint_clock.set(None);
        self.opening_animation = Some(crate::layout::anim::snapshot_for_open(
            animations,
            self.floating,
        ));
    }

    pub(crate) fn begin_close_animation(
        &mut self,
        animations: crate::layout::anim::WindowAnimationConfig,
    ) {
        self.paint_clock.set(None);
        self.closing_animation = Some(crate::layout::anim::snapshot_for_close(
            animations,
            self.floating,
        ));
    }

    pub fn display_title(&self, terminal_title: Option<String>) -> String {
        self.identity
            .custom_title
            .clone()
            .or(terminal_title)
            .unwrap_or_else(|| self.title.clone())
    }

    /// Titlebar text in user name → application title → contextual cwd → generic fallback order.
    /// A custom/application primary title is qualified with the project-relative cwd when known.
    /// Conventional shell `user@host:cwd` titles count as cwd metadata rather than application
    /// titles; their hostname is omitted, while a user changed after shell launch remains visible.
    pub fn titlebar_title(&self, remote_attached: bool) -> String {
        let terminal_title = self.terminal.title();
        let shell_title = terminal_title.as_deref().and_then(shell_title_parts);
        let primary = self.identity.custom_title.clone().or_else(|| {
            shell_title
                .is_none()
                .then(|| terminal_title.clone())
                .flatten()
        });

        let path = self
            .terminal
            .display_path
            .clone()
            .or_else(|| shell_title.map(|(_, cwd)| cwd.to_string()))
            .or_else(|| self.live_cwd())
            .or_else(|| self.identity.cwd.clone())
            .map(|cwd| {
                if remote_attached || self.terminal.display_path.is_some() {
                    cwd
                } else {
                    crate::platform::paths::compress_home(&cwd)
                }
            });
        let switched_user = shell_title.and_then(|(user, _)| user).filter(|user| {
            self.terminal
                .original_user
                .as_deref()
                .is_some_and(|original| original != *user)
        });
        let location = path.map(|path| match switched_user {
            Some(user) => format!("{user} · {path}"),
            None => path,
        });

        match (primary, location) {
            (Some(primary), Some(location)) if primary != location => {
                format!("{primary} · {location}")
            }
            (Some(primary), _) => primary,
            (None, Some(location)) => location,
            (None, None) => self.title.clone(),
        }
    }

    pub fn set_custom_title(&mut self, title: impl AsRef<str>) {
        self.identity.set_custom_title(title);
    }

    pub fn clear_custom_title(&mut self) {
        self.identity.custom_title = None;
    }

    /// The shell's current working directory if it can be discovered live, else `None`.
    pub fn live_cwd(&self) -> Option<String> {
        self.terminal.working_directory()
    }

    /// A local working directory safe to reuse for spawning, falling back to launch identity.
    pub fn local_cwd(&self) -> Option<String> {
        self.local_cwd_ref().map(str::to_string)
    }

    /// [`local_cwd`](Self::local_cwd) without the allocation, for callers that only compare it.
    /// The sidebar checks this on every message, including output from off-screen panes that
    /// otherwise cost nothing to handle, so that check must not allocate a string to throw away.
    pub fn local_cwd_ref(&self) -> Option<&str> {
        self.terminal
            .cwd
            .as_deref()
            .filter(|_| self.terminal.cwd_host.is_none())
            .or(self.identity.cwd.as_deref())
    }

    /// The working directory as the *session server* sees it, without the `cwd_host` filter.
    ///
    /// Under `--remote` every reported path is server-relative, so the filter that keeps
    /// [`local_cwd_ref`](Self::local_cwd_ref) honest would discard exactly the path the remote file
    /// tree needs. Callers must already know the server is remote.
    pub fn server_cwd_ref(&self) -> Option<&str> {
        self.terminal
            .cwd
            .as_deref()
            .or(self.identity.cwd.as_deref())
    }

    /// Whether this pane is blocked and still worth acting on: its own status, or any agent it
    /// runs. The hooks' report speaks for one conversation only, so another conversation the
    /// publisher calls blocked must still count.
    pub fn awaits_input(&self) -> bool {
        !self.closing
            && !matches!(self.terminal.status, ManagedTerminalStatus::Exited(_))
            && (self.terminal.is_blocked()
                || self
                    .agent_runtimes()
                    .iter()
                    .any(|runtime| runtime.state == crate::session::protocol::AgentState::Blocked))
    }
}

/// Whether an OSC 7501 `app` names `agent`: its definition ID, or a name the agent catalog knows it
/// by (`claude-code` for `claude`).
fn program_status_names_agent(app: &str, agent: &crate::session::protocol::AgentIdentity) -> bool {
    agent.id.eq_ignore_ascii_case(app)
        || crate::agent_detection::AgentCatalog::shared_builtin()
            .by_name(app)
            .is_some_and(|definition| definition.id() == agent.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        )
    }

    #[test]
    fn pane_display_title_prefers_custom_title() {
        let mut pane = pane();
        pane.title = "terminal title".to_string();
        pane.set_custom_title("custom title");

        assert_eq!(
            pane.display_title(Some("terminal title".to_string())),
            "custom title"
        );
    }

    #[test]
    fn titlebar_uses_cwd_without_the_original_user_or_host() {
        let mut pane = pane();
        pane.terminal.title = Some("razuer@workstation:~/Work/Projects/rozi".to_string());
        pane.terminal.original_user = Some("razuer".to_string());
        pane.terminal.cwd = Some("/home/razuer/Work/Projects/rozi".to_string());

        assert_eq!(pane.titlebar_title(false), "~/Work/Projects/rozi");
    }

    #[test]
    fn titlebar_qualifies_cwd_after_switching_users() {
        let mut pane = pane();
        pane.terminal.title = Some("root@workstation:/etc/nginx".to_string());
        pane.terminal.original_user = Some("razuer".to_string());
        pane.terminal.cwd = Some("/etc/nginx".to_string());

        assert_eq!(pane.titlebar_title(false), "root · /etc/nginx");
    }

    #[test]
    fn titlebar_qualifies_a_custom_name_with_the_project_path() {
        let mut pane = pane();
        pane.set_custom_title("logs");
        pane.terminal.title = Some("razuer@workstation:/work/rozi/services/api".to_string());
        pane.terminal.original_user = Some("razuer".to_string());
        pane.terminal.cwd = Some("/work/rozi/services/api".to_string());
        pane.terminal.display_path = Some("rozi/services/api".to_string());

        assert_eq!(pane.titlebar_title(false), "logs · rozi/services/api");
    }

    #[test]
    fn titlebar_qualifies_an_application_title_with_the_project_path() {
        let mut pane = pane();
        pane.terminal.title = Some("nvim src/main.rs".to_string());
        pane.terminal.cwd = Some("/work/rozi/src".to_string());
        pane.terminal.display_path = Some("rozi/src".to_string());

        assert_eq!(pane.titlebar_title(false), "nvim src/main.rs · rozi/src");
    }

    #[test]
    fn titlebar_uses_structured_cwd_without_a_terminal_title() {
        let mut pane = pane();
        pane.terminal.cwd = Some("/work/rozi".to_string());

        assert_eq!(pane.titlebar_title(false), "/work/rozi");
    }

    #[test]
    fn titlebar_uses_the_generic_fallback_when_no_identity_is_available() {
        let mut pane = pane();
        pane.title = "Shell".to_string();

        assert_eq!(pane.titlebar_title(false), "Shell");
    }

    #[test]
    fn pane_display_title_uses_terminal_title_before_fallback() {
        let mut pane = pane();
        pane.title = "fallback title".to_string();

        assert_eq!(
            pane.display_title(Some("terminal title".to_string())),
            "terminal title"
        );
    }

    #[test]
    fn empty_custom_title_is_cleared() {
        let mut pane = pane();
        pane.set_custom_title("custom title");
        pane.set_custom_title("   ");

        assert_eq!(pane.identity.custom_title, None);
        assert_eq!(pane.display_title(None), "shell");
    }

    #[test]
    fn awaits_input_ignores_closing_and_exited_panes() {
        let mut pane = pane();
        pane.terminal.reported_status = Some(crate::session::protocol::PaneStatus {
            value: "blocked".into(),
            reason: None,
            set_at: 1,
        });
        assert!(pane.awaits_input());
        pane.closing = true;
        assert!(!pane.awaits_input());
        pane.closing = false;
        pane.terminal.status = ManagedTerminalStatus::Exited(0);
        assert!(!pane.awaits_input());
    }

    /// The hooks follow one conversation and speak only for its row, so another conversation the
    /// publisher calls blocked still leaves the pane waiting on the user.
    #[test]
    fn a_blocked_conversation_beside_the_one_the_hooks_follow_awaits_input() {
        use crate::session::protocol::{
            AgentIdentity, AgentIntegrationReport, AgentRef, AgentState, PaneRef, PublishedRow,
            PublisherKind, SessionInstanceId,
        };
        let row = |id: &str, status: &str, active: bool| PublishedRow {
            publisher: "sessions".into(),
            kind: PublisherKind::Agent,
            live_session: None,
            reported: false,
            id: id.into(),
            title: id.into(),
            status: status.into(),
            reason: None,
            active,
            work_started_at: None,
            cwd: None,
            project: None,
            native_session: Some(format!("native-{id}")),
        };
        let reference = |slot, incarnation| AgentRef {
            pane: PaneRef {
                session_instance: SessionInstanceId::for_test("server"),
                pane_id: 1,
                generation: 0,
            },
            slot,
            incarnation,
        };
        let mut pane = pane();
        let _ = pane.terminal.apply_rows(
            vec![row("shown", "idle", true), row("behind", "blocked", false)],
            Some(Box::new(AgentIntegrationReport {
                integration: "hook".into(),
                identity: AgentIdentity::new("claude", "Claude Code"),
                reference: reference(None, 9),
                state: AgentState::Done,
                reason: None,
                native_session: Some("native-shown".into()),
                seq: 1,
                reported_at_unix_ms: 1,
            })),
        );
        pane.agents = pane
            .terminal
            .published_rows
            .iter()
            .enumerate()
            .map(|(index, row)| reference(Some(row.key()), index as u64 + 2).into())
            .collect();

        assert!(!pane.terminal.is_blocked(), "the hooks' row is done");
        assert!(pane.awaits_input());
    }
}
