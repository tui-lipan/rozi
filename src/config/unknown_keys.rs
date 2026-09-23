use toml::Table;

const TOP_LEVEL_KEYS: &[&str] = &[
    "shell",
    "command_shell",
    "shell_integration",
    "environment",
    "cwd",
    "scrollback",
    "frame_rate",
    "nerd_icons",
    "input",
    "animations",
    "theme",
    "profile",
    "worktrees",
    "session",
    "remote",
    "layout",
    "pane",
    "clipboard",
    "updates",
    "notifications",
    "sounds",
    "navigation",
    "confirm",
    "scratchpad",
    "sidebar",
    "workbar",
    "agents",
    "rules",
    "hints",
    "hooks",
    "commands",
    "services",
    "extensions",
    "logging",
    "keys",
];

const INPUT_KEYS: &[&str] = &["modifier", "prefix", "modifier_shortcuts", "which_key"];

const ANIMATION_KEYS: &[&str] = &[
    "enabled",
    "spawn",
    "close",
    "fullscreen",
    "tile_float",
    "axis_change",
    "sidebar",
    "workspace",
    "workspace_ms",
    "session",
    "focus_chrome",
    "pane_style",
    "geometry_ms",
    "close_ms",
    "focus_chrome_ms",
    "alert_pulse_ms",
    "open_delay_ms",
    "curve",
    "close_curve",
    "fade",
    "scale_from",
    "portal_origin",
    "scan_direction",
];

const SESSION_KEYS: &[&str] = &[
    "autosave",
    "path",
    "startup",
    "resurrect",
    "resurrect_foreground",
    "resurrect_agents",
    "allow_takeover",
];

const REMOTE_KEYS: &[&str] = &[
    "default_host",
    "connection_timeout_secs",
    "server_alive_interval_secs",
    "server_alive_count_max",
    "install",
    "batch_mode",
    "hosts",
];

const REMOTE_HOST_KEYS: &[&str] = &[
    "host",
    "user",
    "port",
    "identity_file",
    "ssh_args",
    "binary_path",
];

const PANE_KEYS: &[&str] = &[
    "resize_debounce_ms",
    "hold_on_exit",
    "highlight_focused_background",
    "highlight_focused_border",
    "highlight_focused_titlebar",
    "focus_on_hover",
    "focus_on_hover_pause_modifier",
    "show_workbar",
    "workbar_gap",
    "workbar_background",
    "workbar_at_bottom",
    "show_titles",
    "border_mode",
    "alert_border",
    "alert",
    "keep_special_borders",
    "background_follows_terminal",
    "border_style",
    "float_border_style",
    "scratch_border_style",
    "fullscreen_border_style",
    "picker_border_style",
    "picker_tab_background",
    "picker_tab_style",
    "picker_selection_style",
    "padding",
    "titlebar",
    "title_style",
    "workbar_badge_style",
    "workbar_powerline",
    "workbar_tab_style",
    "workbar_style",
    "toast_opacity",
];

const PANE_ALERT_KEYS: &[&str] = &["blocked", "finished", "working", "idle"];

const NOTIFICATION_KEYS: &[&str] = &[
    "enabled",
    "pane_exit",
    "pane_exit_error",
    "bell",
    "pane_blocked",
    "pane_done",
];

const SOUND_KEYS: &[&str] = &[
    "enabled",
    "bell",
    "blocked",
    "done",
    "error",
    "throttle_ms",
    "bell_file",
    "blocked_file",
    "done_file",
    "error_file",
    "player",
];

const CONFIRM_KEYS: &[&str] = &[
    "close_pane",
    "kill_workspace",
    "kill_session",
    "quit_ephemeral",
    "new_temporary_session",
    "load_profile",
];

const SIDEBAR_KEYS: &[&str] = &[
    "visible",
    "width",
    "position",
    "tabs",
    "panels",
    "split",
    "split_ratio",
    "background_follows_canvas",
    "gap",
    "background",
    "tab_style",
];

const SIDEBAR_TAB_KEYS: &[&str] = &[
    "name",
    "label",
    "entries",
    "command",
    "interval",
    "on_click",
    "group_prefix",
    "root",
    "show_hidden",
    "icons",
    "explorer",
    "diff_stats",
    "max_entries",
];

const LAUNCHER_ENTRY_KEYS: &[&str] = &["label", "group", "run", "send", "popup", "keep_open"];

const USER_COMMAND_KEYS: &[&str] = &["label", "run", "send", "popup", "exec", "keep_open"];

const COMMAND_KEYS: &[&str] = &["id", "label", "run", "send", "popup", "exec", "keep_open"];

const BINDING_TABLE_KEYS: &[&str] = &["add", "label", "run", "send", "popup", "exec", "keep_open"];

const WORKBAR_KEYS: &[&str] = &["left", "right", "clock_format", "alert"];

const WORKBAR_ALERT_KEYS: &[&str] = &[
    "bell", "blocked", "finished", "working", "idle", "mode", "paint",
];

const WORKBAR_SEGMENT_KEYS: &[&str] = &["segment", "color"];

const RULE_KEYS: &[&str] = &[
    "match",
    "match_regex",
    "float",
    "width",
    "height",
    "position",
    "workspace",
    "focus",
    "fullscreen",
];

const SERVICE_KEYS: &[&str] = &["name", "run", "cwd", "restart", "env"];

pub(super) fn collect_unknown_keys(table: &Table) -> Vec<String> {
    let mut unknown = Vec::new();
    report_unknown(table, "", TOP_LEVEL_KEYS, &mut unknown);
    collect_nested(table, &mut unknown);
    unknown
}

fn collect_nested(table: &Table, unknown: &mut Vec<String>) {
    collect_named_table(table, "", "shell_integration", &["mode"], unknown);
    collect_named_table(table, "", "environment", &["forward"], unknown);
    collect_named_table(table, "", "input", INPUT_KEYS, unknown);
    collect_named_table(table, "", "animations", ANIMATION_KEYS, unknown);
    collect_named_table(table, "", "theme", &["name"], unknown);
    collect_named_table(table, "", "profile", &["default"], unknown);
    collect_named_table(table, "", "worktrees", &["profile", "directory"], unknown);
    collect_named_table(table, "", "session", SESSION_KEYS, unknown);
    collect_remote(table, unknown);
    collect_named_table(
        table,
        "",
        "layout",
        &["split_width_multiplier", "default"],
        unknown,
    );
    collect_pane(table, unknown);
    collect_named_table(
        table,
        "",
        "clipboard",
        &[
            "copy_on_select",
            "middle_click_paste",
            "right_click",
            "enable_osc52",
        ],
        unknown,
    );
    collect_named_table(table, "", "updates", &["check", "interval_hours"], unknown);
    collect_named_table(table, "", "notifications", NOTIFICATION_KEYS, unknown);
    collect_named_table(table, "", "sounds", SOUND_KEYS, unknown);
    collect_named_table(table, "", "navigation", &["editors"], unknown);
    collect_named_table(table, "", "confirm", CONFIRM_KEYS, unknown);
    collect_named_table(
        table,
        "",
        "scratchpad",
        &["command", "cwd", "height"],
        unknown,
    );
    collect_sidebar(table, unknown);
    collect_workbar(table, unknown);
    collect_array_tables(table, "", "rules", RULE_KEYS, unknown);
    collect_array_tables(table, "", "hints", &["pattern", "open"], unknown);
    collect_array_tables(table, "", "hooks", &["event", "run"], unknown);
    collect_array_tables(table, "", "commands", COMMAND_KEYS, unknown);
    collect_services(table, unknown);
    collect_named_table(table, "", "logging", &["dir", "max_bytes"], unknown);
    collect_keys(table, unknown);
}

fn collect_pane(table: &Table, unknown: &mut Vec<String>) {
    collect_named_table(table, "", "pane", PANE_KEYS, unknown);
    let Some(pane) = named_table(table, "pane") else {
        return;
    };
    collect_named_table(pane, "pane", "alert", PANE_ALERT_KEYS, unknown);
}

fn collect_sidebar(table: &Table, unknown: &mut Vec<String>) {
    collect_named_table(table, "", "sidebar", SIDEBAR_KEYS, unknown);
    let Some(sidebar) = named_table(table, "sidebar") else {
        return;
    };
    collect_tab_list(sidebar, unknown);
}

fn collect_tab_list(sidebar: &Table, unknown: &mut Vec<String>) {
    let Some(tabs) = sidebar.get("tabs").and_then(toml::Value::as_array) else {
        return;
    };
    for (index, tab) in tabs.iter().enumerate() {
        let Some(tab_table) = tab.as_table() else {
            continue;
        };
        let path = format!("sidebar.tabs[{index}]");
        report_unknown(tab_table, &path, SIDEBAR_TAB_KEYS, unknown);
        collect_array_tables(tab_table, &path, "entries", LAUNCHER_ENTRY_KEYS, unknown);
        collect_named_table(tab_table, &path, "on_click", USER_COMMAND_KEYS, unknown);
    }
}

fn collect_workbar(table: &Table, unknown: &mut Vec<String>) {
    collect_named_table(table, "", "workbar", WORKBAR_KEYS, unknown);
    let Some(workbar) = named_table(table, "workbar") else {
        return;
    };
    collect_named_table(workbar, "workbar", "alert", WORKBAR_ALERT_KEYS, unknown);
    collect_segment_list(workbar, "left", unknown);
    collect_segment_list(workbar, "right", unknown);
}

fn collect_segment_list(workbar: &Table, name: &str, unknown: &mut Vec<String>) {
    let Some(items) = workbar.get(name).and_then(toml::Value::as_array) else {
        return;
    };
    for (index, item) in items.iter().enumerate() {
        let Some(item_table) = item.as_table() else {
            continue;
        };
        report_unknown(
            item_table,
            &format!("workbar.{name}[{index}]"),
            WORKBAR_SEGMENT_KEYS,
            unknown,
        );
    }
}

fn collect_remote(table: &Table, unknown: &mut Vec<String>) {
    collect_named_table(table, "", "remote", REMOTE_KEYS, unknown);
    let Some(remote) = named_table(table, "remote") else {
        return;
    };
    let Some(hosts) = named_table(remote, "hosts") else {
        return;
    };
    for (name, host) in hosts {
        let Some(host_table) = host.as_table() else {
            continue;
        };
        report_unknown(
            host_table,
            &format!("remote.hosts.{name}"),
            REMOTE_HOST_KEYS,
            unknown,
        );
    }
}

fn collect_services(table: &Table, unknown: &mut Vec<String>) {
    collect_array_tables(table, "", "services", SERVICE_KEYS, unknown);
}

fn collect_keys(table: &Table, unknown: &mut Vec<String>) {
    let Some(keys) = named_table(table, "keys") else {
        return;
    };
    for (action, spec) in keys {
        let Some(spec_table) = spec.as_table() else {
            continue;
        };
        report_unknown(
            spec_table,
            &format!("keys.{action}"),
            BINDING_TABLE_KEYS,
            unknown,
        );
    }
}

fn collect_named_table(
    parent: &Table,
    path: &str,
    name: &str,
    keys: &[&str],
    unknown: &mut Vec<String>,
) {
    let Some(table) = named_table(parent, name) else {
        return;
    };
    report_unknown(table, &qualify(path, name), keys, unknown);
}

fn collect_array_tables(
    parent: &Table,
    path: &str,
    name: &str,
    keys: &[&str],
    unknown: &mut Vec<String>,
) {
    let Some(items) = parent.get(name).and_then(toml::Value::as_array) else {
        return;
    };
    let prefix = qualify(path, name);
    for (index, item) in items.iter().enumerate() {
        let Some(table) = item.as_table() else {
            continue;
        };
        report_unknown(table, &format!("{prefix}[{index}]"), keys, unknown);
    }
}

fn named_table<'a>(parent: &'a Table, name: &str) -> Option<&'a Table> {
    parent.get(name).and_then(toml::Value::as_table)
}

fn report_unknown(table: &Table, path: &str, known: &[&str], unknown: &mut Vec<String>) {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            unknown.push(qualify(path, key));
        }
    }
}

fn qualify(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::collect_unknown_keys;

    fn parse(text: &str) -> Vec<String> {
        let value: toml::Value = toml::from_str(text).expect("toml parses");
        collect_unknown_keys(value.as_table().expect("root table"))
    }

    #[test]
    fn known_keys_are_silent() {
        assert!(parse("[sidebar]\nwidth = 42\n[theme]\nname = \"rozi\"\n").is_empty());
    }

    #[test]
    fn unknown_nested_key_is_reported() {
        let unknown = parse("[sidebar]\nwidth = 42\nwidht = 7\n");
        assert_eq!(unknown, ["sidebar.widht"]);
    }

    #[test]
    fn unknown_top_level_key_is_reported() {
        let unknown = parse("theem = \"rozi\"\nframe_rate = 60\n");
        assert_eq!(unknown, ["theem"]);
    }

    #[test]
    fn on_click_id_is_unknown() {
        let unknown = parse(
            "[sidebar]\ntabs = [{ name = \"rows\", label = \"Rows\", on_click = { id = \"x\", send = \"hi\" } }]\n",
        );
        assert_eq!(unknown, ["sidebar.tabs[0].on_click.id"]);
    }

    #[test]
    fn unknown_sidebar_tab_field_is_reported() {
        let unknown = parse(
            "[sidebar]\ntabs = [{ name = \"x\", label = \"X\", entries = [], typo = true }]\n",
        );
        assert_eq!(unknown, ["sidebar.tabs[0].typo"]);
    }

    #[test]
    fn open_extension_tables_are_not_unknown() {
        assert!(
            parse("[extensions]\ndisabled = []\n[extensions.tasks]\nrunner = \"just\"\n")
                .is_empty()
        );
    }

    #[test]
    fn open_key_action_ids_are_not_unknown() {
        assert!(parse("[keys]\ncopy-mode = \"b\"\ncustom = { run = \"echo hi\" }\n").is_empty());
    }
}
