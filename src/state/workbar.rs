use std::collections::HashMap;

use crate::config::WorkbarConfig;

/// Client-global runtime state for scheduled workbar command segments.
#[derive(Default)]
pub struct WorkbarState {
    /// Invalidates every scheduled or running command when configuration is reloaded.
    pub command_epoch: u64,
    /// The epoch of the run currently occupying each command's overlap guard.
    pub command_in_flight: HashMap<String, u64>,
    /// Cached first-line stdout, keyed by the raw configured command string.
    pub command_output: HashMap<String, String>,
}

impl WorkbarState {
    /// Reconcile command caches after a config reload while leaving running commands recorded until
    /// their matching result arrives. That prevents the replacement epoch from overlapping an old
    /// run, and the `(command, epoch)` value prevents an old result from clearing a newer guard.
    pub fn reconcile(&mut self, config: &WorkbarConfig) {
        self.command_epoch = self.command_epoch.wrapping_add(1);
        let commands: std::collections::HashSet<_> = config
            .command_specs()
            .into_iter()
            .map(|(command, _)| command)
            .collect();
        self.command_output
            .retain(|command, _| commands.contains(command));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WorkbarItem, WorkbarSegment};

    fn command_item(command: &str, interval_secs: u64) -> WorkbarItem {
        WorkbarItem {
            segment: WorkbarSegment::Command {
                command: command.to_string(),
                interval_secs,
            },
            color: None,
        }
    }

    #[test]
    fn reload_bumps_epoch_prunes_removed_output_and_preserves_running_guards() {
        let mut state = WorkbarState {
            command_epoch: 4,
            command_in_flight: HashMap::from([("kept".to_string(), 4), ("removed".to_string(), 4)]),
            command_output: HashMap::from([
                ("kept".to_string(), "old".to_string()),
                ("removed".to_string(), "gone".to_string()),
            ]),
        };
        let config = WorkbarConfig {
            left: vec![command_item("kept", 2)],
            ..WorkbarConfig::default()
        };

        state.reconcile(&config);

        assert_eq!(state.command_epoch, 5);
        assert_eq!(
            state.command_output.get("kept").map(String::as_str),
            Some("old")
        );
        assert!(!state.command_output.contains_key("removed"));
        assert_eq!(state.command_in_flight.get("kept"), Some(&4));
        assert_eq!(state.command_in_flight.get("removed"), Some(&4));
    }

    #[test]
    fn interval_only_reload_still_advances_command_epoch() {
        let mut state = WorkbarState::default();
        state.reconcile(&WorkbarConfig {
            left: vec![command_item("date", 10)],
            ..WorkbarConfig::default()
        });
        let first_epoch = state.command_epoch;

        state.reconcile(&WorkbarConfig {
            left: vec![command_item("date", 1)],
            ..WorkbarConfig::default()
        });

        assert_eq!(state.command_epoch, first_epoch.wrapping_add(1));
    }
}
