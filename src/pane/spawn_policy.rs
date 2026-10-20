//! Policy every `new-pane` request obeys, whichever endpoint received it.
//!
//! Two endpoints can open a pane: a UI client through [`crate::ops::control`], and a session
//! server through [`crate::session::server`]'s headless control path. They run in different
//! processes against different state, but they are answering the same request and must not answer
//! it differently — a `[[rules]]` entry that floats `btop` has to float it whether a person
//! pressed a key or a cron job asked for it.
//!
//! So the decisions both share live here, as functions over plain data rather than over `State` or
//! `SessionServer`: what the caller's `command`/`argv` means, which workspace the pane lands in
//! once rules and explicit overrides are combined, and what environment it starts with. What is
//! genuinely per-endpoint — where focus goes, which canvas the geometry is measured against, who
//! is allowed to ask — stays with the endpoint.

use std::path::Path;

use crate::config::RuleConfig;
use crate::pane::launch::PaneLaunch;
use crate::pane::lifecycle::SpawnPlacement;
use crate::state::{PaneId, WORKSPACE_COUNT};

/// Turn a request's `command`/`argv` pair into a launch.
///
/// The pair is deliberately exclusive rather than precedence-ordered: a caller that sent both has
/// a bug, and picking one silently would run something it did not ask for.
pub(crate) fn requested_launch(
    command: Option<String>,
    argv: Option<Vec<String>>,
) -> std::result::Result<Option<PaneLaunch>, String> {
    match (command, argv) {
        (Some(_), Some(_)) => {
            Err("new-pane accepts either `command` or `argv`, not both".to_string())
        }
        (Some(command), None) => Ok(Some(PaneLaunch::shell(command))),
        (None, Some(argv)) => PaneLaunch::direct(argv).map(Some),
        (None, None) => Ok(None),
    }
}

/// Convert a caller's one-based workspace number into an index.
pub(crate) fn workspace_index(index: usize) -> std::result::Result<usize, String> {
    if index == 0 || index > WORKSPACE_COUNT {
        Err(format!(
            "workspace index must be between 1 and {WORKSPACE_COUNT}"
        ))
    } else {
        Ok(index - 1)
    }
}

/// Combine `[[rules]]` with what the caller asked for explicitly.
///
/// `command` is the rule-matching text, which is [`PaneLaunch::display`] for a pane that has a
/// launch and `None` for a plain shell. Returns the rule's workspace (zero-based) only when the
/// caller named none: `[[rules]]` placement is a default for the panes a person opens, not an
/// override of an explicit instruction from automation. `focus` works the same way round, and an
/// endpoint with no focus to move ignores the answer entirely.
pub(crate) fn resolve_placement(
    rules: &[RuleConfig],
    command: Option<&str>,
    requested_workspace: Option<usize>,
    requested_focus: Option<bool>,
) -> (Option<usize>, SpawnPlacement) {
    let (rule_workspace, mut placement) = command
        .map(|command| crate::pane::rules::placement_for_command(rules, command))
        .unwrap_or_default();
    if let Some(focus) = requested_focus {
        placement.focus = focus;
    }
    (requested_workspace.or(rule_workspace), placement)
}

/// Who is opening a pane, and therefore what the pane can be told about its surroundings.
pub(crate) enum SpawnOrigin<'a> {
    /// A client, which forwards its own live environment and advertises itself.
    ///
    /// The environment is sampled from the *client* rather than the session server on purpose: a
    /// named server can outlive several clients, and the `DISPLAY` it was started with years of
    /// uptime ago names a session that is gone.
    Client {
        control_socket: Option<&'a Path>,
        /// `[environment] forward`, on top of the desktop allowlist.
        forward: &'a [String],
        /// Attached over SSH. The pane runs on the far host, where this client's socket, binary,
        /// and desktop variables all name things that are not there.
        remote: bool,
    },
    /// The session server itself, with no client in the picture (headless control, resurrection).
    ///
    /// Nothing is forwarded and nothing is advertised. `ROZI_SOCKET` and `ROZI_BIN` name a UI
    /// process and there is not one; the desktop variables would have to come either from the
    /// server's own environment, which is as old as the server, or from the one-shot CLI process
    /// that asked, which will be gone long before the pane is. A pane that inherits neither is
    /// wrong in a way its program can detect; one that inherits a dangling `DISPLAY` is not.
    Headless,
}

/// The environment a freshly spawned pane starts with.
///
/// `per_spawn` is [`crate::state::PaneIdentity::env`] — values that belong to this one spawn and
/// are never persisted. It goes last so a caller-supplied value wins over the standard set.
pub(crate) fn spawn_environment(
    origin: SpawnOrigin<'_>,
    pane_id: PaneId,
    per_spawn: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env = match &origin {
        SpawnOrigin::Client {
            forward,
            remote: false,
            ..
        } => crate::platform::environment::forwarded_client_environment(forward),
        SpawnOrigin::Client { remote: true, .. } | SpawnOrigin::Headless => Vec::new(),
    };
    env.extend([
        ("ROZI".to_string(), "1".to_string()),
        ("ROZI_PANE".to_string(), pane_id.to_string()),
    ]);
    if let SpawnOrigin::Client {
        control_socket,
        remote: false,
        ..
    } = &origin
    {
        if let Some(path) = control_socket {
            env.push(("ROZI_SOCKET".to_string(), path.display().to_string()));
        }
        if let Some(path) = crate::platform::paths::current_binary() {
            env.push(("ROZI_BIN".to_string(), path.display().to_string()));
        }
    }
    env.extend(per_spawn.iter().cloned());
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FloatPosition, RuleMatcher};

    fn float_rule(matches: &str, workspace: Option<usize>) -> RuleConfig {
        RuleConfig {
            matcher: RuleMatcher::Substring(matches.to_string()),
            float: true,
            width: Some(0.5),
            height: Some(0.4),
            workspace,
            focus: false,
            fullscreen: false,
            position: FloatPosition::Center,
        }
    }

    #[test]
    fn a_launch_request_names_exactly_one_valid_process_model() {
        assert!(
            requested_launch(Some("top".into()), Some(vec!["top".into()])).is_err(),
            "sending both must be refused, not silently resolved"
        );
        assert_eq!(requested_launch(None, None).unwrap(), None);
        assert_eq!(
            requested_launch(Some("top".into()), None).unwrap(),
            Some(PaneLaunch::shell("top"))
        );
        // Direct argv keeps its arguments literal: nothing here joins or quotes them.
        assert_eq!(
            requested_launch(None, Some(vec!["tool".into(), "space; $literal".into()])).unwrap(),
            Some(PaneLaunch::Direct {
                argv: vec!["tool".into(), "space; $literal".into()]
            })
        );
        assert!(requested_launch(None, Some(Vec::new())).is_err());
    }

    #[test]
    fn workspace_numbers_are_one_based_and_bounded() {
        assert_eq!(workspace_index(1), Ok(0));
        assert_eq!(workspace_index(WORKSPACE_COUNT), Ok(WORKSPACE_COUNT - 1));
        assert!(workspace_index(0).is_err());
        assert!(workspace_index(WORKSPACE_COUNT + 1).is_err());
    }

    #[test]
    fn an_explicit_workspace_beats_a_rule_but_a_rule_still_places_an_unplaced_pane() {
        let rules = vec![float_rule("btop", Some(2))];
        let launch = PaneLaunch::shell("btop --version");
        let command = launch.display();

        let (workspace, placement) = resolve_placement(&rules, Some(&command), None, None);
        assert_eq!(workspace, Some(2), "the rule places a pane nobody placed");
        assert!(placement.float.is_some());

        let (workspace, _) = resolve_placement(&rules, Some(&command), Some(8), None);
        assert_eq!(
            workspace,
            Some(8),
            "an explicit workspace is an instruction"
        );

        // Focus is the caller's to override in either direction, and a pane with no launch
        // matches no rule at all.
        let (workspace, placement) = resolve_placement(&rules, None, None, Some(false));
        assert_eq!(workspace, None);
        assert!(placement.float.is_none());
        assert!(!placement.focus);
    }

    #[test]
    fn a_headless_spawn_advertises_no_ui_and_forwards_nothing() {
        let forward = vec!["ROZI_SPAWN_POLICY_TEST".to_string()];
        let headless = spawn_environment(SpawnOrigin::Headless, 4, &[]);
        assert_eq!(
            headless,
            vec![
                ("ROZI".to_string(), "1".to_string()),
                ("ROZI_PANE".to_string(), "4".to_string()),
            ]
        );

        // A remote client is the same shape for the same reason: the pane is not on its machine.
        let remote = spawn_environment(
            SpawnOrigin::Client {
                control_socket: Some(Path::new("/run/rozi/control-1.sock")),
                forward: &forward,
                remote: true,
            },
            4,
            &[("EXTRA".to_string(), "1".to_string())],
        );
        assert!(remote.iter().all(|(key, _)| key != "ROZI_SOCKET"));
        assert!(remote.iter().all(|(key, _)| key != "ROZI_BIN"));
        assert!(remote.contains(&("EXTRA".to_string(), "1".to_string())));
    }

    #[test]
    fn a_local_client_advertises_the_endpoint_a_pane_can_actually_reach() {
        let env = spawn_environment(
            SpawnOrigin::Client {
                control_socket: Some(Path::new("/run/rozi/control-1.sock")),
                forward: &[],
                remote: false,
            },
            9,
            &[],
        );
        assert!(
            env.contains(&(
                "ROZI_SOCKET".to_string(),
                "/run/rozi/control-1.sock".to_string()
            )),
            "{env:?}"
        );
        assert!(env.contains(&("ROZI_PANE".to_string(), "9".to_string())));
    }
}
