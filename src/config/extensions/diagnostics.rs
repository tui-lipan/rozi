use serde::Serialize;

use super::{ExtensionInfo, ExtensionLaunchDiagnostic, ExtensionSettings, ExtensionStatus};

pub const EXTENSION_DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct ExtensionListDocument {
    pub schema_version: u32,
    pub extensions: Vec<ExtensionInfo>,
}

impl ExtensionListDocument {
    pub(crate) fn new(extensions: Vec<ExtensionInfo>) -> Self {
        Self {
            schema_version: EXTENSION_DIAGNOSTICS_SCHEMA_VERSION,
            extensions,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ExtensionCheckDocument {
    pub schema_version: u32,
    pub extension: ExtensionInfo,
}

impl ExtensionCheckDocument {
    pub(crate) fn new(extension: ExtensionInfo) -> Self {
        Self {
            schema_version: EXTENSION_DIAGNOSTICS_SCHEMA_VERSION,
            extension,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReportSection {
    pub title: &'static str,
    pub rows: Vec<ReportRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReportRow {
    pub label: String,
    pub value: String,
    pub tone: ReportTone,
    pub kind: ReportKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReportTone {
    Plain,
    Accent,
    Success,
    Warning,
    Error,
    Muted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReportKind {
    Info,
    Command(String),
    Setting(String),
    Error,
}

pub(crate) fn report_sections(
    info: &ExtensionInfo,
    merged: &ExtensionSettings,
) -> Vec<ReportSection> {
    let mut sections = vec![ReportSection {
        title: "Overview",
        rows: vec![
            ReportRow {
                label: "Status".to_string(),
                value: info.status.as_str().to_string(),
                tone: status_tone(info.status),
                kind: ReportKind::Info,
            },
            optional_info_row("Version", info.version.clone()),
            optional_info_row("API", info.api.map(|api| api.to_string())),
            optional_info_row("ID", info.id.clone()),
            info_row("Directory", info.path.clone()),
            info_row("Manifest", info.manifest_path.clone()),
        ],
    }];

    push_section(
        &mut sections,
        "Commands",
        info.command_details
            .iter()
            .map(|command| ReportRow {
                label: command.id.clone(),
                value: format!(
                    "launch: {}\ncwd: {}\nenv: {}",
                    format_launch(&command.launch),
                    command.cwd,
                    format_env(&command.injected_env)
                ),
                tone: ReportTone::Accent,
                kind: ReportKind::Command(command.id.clone()),
            })
            .collect(),
    );
    push_section(
        &mut sections,
        "Services",
        info.service_details
            .iter()
            .map(|service| {
                let mut value = format!(
                    "launch: {}\ncwd: {}\nrestart: {}\nenv: {}",
                    format_launch(&service.launch),
                    service.cwd,
                    service.restart,
                    format_env(&service.injected_env)
                );
                if !service.configured_env_keys.is_empty() {
                    value.push_str("\nmanifest env: ");
                    value.push_str(&service.configured_env_keys.join(", "));
                    value.push_str(" (values redacted)");
                }
                ReportRow {
                    label: service.id.clone(),
                    value,
                    tone: ReportTone::Accent,
                    kind: ReportKind::Info,
                }
            })
            .collect(),
    );
    push_section(
        &mut sections,
        "Agents",
        info.agents
            .iter()
            .map(|id| info_row("Agent", id.clone()))
            .collect(),
    );
    push_section(
        &mut sections,
        "Sidebar tabs",
        info.sidebar_tabs
            .iter()
            .map(|id| info_row("Tab", id.clone()))
            .collect(),
    );
    push_section(
        &mut sections,
        "Settings",
        merged
            .iter()
            .map(|(key, value)| ReportRow {
                label: key.clone(),
                value: serde_json::to_string(value).unwrap_or_else(|_| "?".to_string()),
                tone: ReportTone::Plain,
                kind: ReportKind::Setting(key.clone()),
            })
            .collect(),
    );
    push_section(
        &mut sections,
        "Errors",
        info.errors
            .iter()
            .map(|error| ReportRow {
                label: if error.starts_with("Config warning:") {
                    "Warning"
                } else {
                    "Error"
                }
                .to_string(),
                value: error.clone(),
                tone: if error.starts_with("Config warning:") {
                    ReportTone::Warning
                } else {
                    ReportTone::Error
                },
                kind: ReportKind::Error,
            })
            .collect(),
    );

    sections
}

pub(crate) fn report_text(sections: &[ReportSection]) -> String {
    use std::fmt::Write as _;

    let mut output = String::new();
    for (section_index, section) in sections.iter().enumerate() {
        if section_index > 0 {
            output.push('\n');
        }
        let _ = writeln!(output, "{}", section.title);
        for row in &section.rows {
            if row.value.contains('\n') {
                let _ = writeln!(output, "  {}:", row.label);
                for line in row.value.lines() {
                    let _ = writeln!(output, "    {line}");
                }
            } else {
                let _ = writeln!(output, "  {}: {}", row.label, row.value);
            }
        }
    }
    output
}

fn push_section(sections: &mut Vec<ReportSection>, title: &'static str, rows: Vec<ReportRow>) {
    if !rows.is_empty() {
        sections.push(ReportSection { title, rows });
    }
}

fn info_row(label: &str, value: String) -> ReportRow {
    ReportRow {
        label: label.to_string(),
        value,
        tone: ReportTone::Plain,
        kind: ReportKind::Info,
    }
}

fn optional_info_row(label: &str, value: Option<String>) -> ReportRow {
    match value {
        Some(value) => info_row(label, value),
        None => ReportRow {
            label: label.to_string(),
            value: "—".to_string(),
            tone: ReportTone::Muted,
            kind: ReportKind::Info,
        },
    }
}

fn status_tone(status: ExtensionStatus) -> ReportTone {
    match status {
        ExtensionStatus::Loaded => ReportTone::Success,
        ExtensionStatus::Disabled => ReportTone::Muted,
        ExtensionStatus::Invalid | ExtensionStatus::Incompatible | ExtensionStatus::Duplicate => {
            ReportTone::Error
        }
    }
}

fn format_launch(launch: &ExtensionLaunchDiagnostic) -> String {
    match launch {
        ExtensionLaunchDiagnostic::Direct { argv } => {
            serde_json::to_string(argv).unwrap_or_else(|_| "[]".to_string())
        }
        ExtensionLaunchDiagnostic::Shell { command } => format!("shell {command:?}"),
        ExtensionLaunchDiagnostic::Send { text } => format!("send {text:?}"),
    }
}

fn format_env(env: &std::collections::BTreeMap<String, String>) -> String {
    if env.is_empty() {
        return "none".to_string();
    }
    env.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{
        ExtensionCommandDiagnostic, ExtensionServiceDiagnostic, ExtensionSettingValue,
    };

    fn info() -> ExtensionInfo {
        ExtensionInfo {
            id: Some("tasks".to_string()),
            title: Some("Tasks".to_string()),
            description: None,
            version: Some("1.2.3".to_string()),
            api: Some(1),
            path: "/data/extensions/tasks".to_string(),
            manifest_path: "/data/extensions/tasks/extension.toml".to_string(),
            enabled: true,
            status: ExtensionStatus::Loaded,
            commands: vec!["tasks.run".to_string()],
            services: vec!["tasks.watch".to_string()],
            agents: vec!["tasks.worker".to_string()],
            sidebar_tabs: vec!["tasks.list".to_string()],
            settings: [(
                "runner".to_string(),
                ExtensionSettingValue::String("auto".to_string()),
            )]
            .into_iter()
            .collect(),
            command_details: vec![ExtensionCommandDiagnostic {
                id: "tasks.run".to_string(),
                launch: ExtensionLaunchDiagnostic::Send {
                    text: "run".to_string(),
                },
                cwd: "focused-pane-input".to_string(),
                injected_env: BTreeMap::new(),
            }],
            service_details: vec![ExtensionServiceDiagnostic {
                id: "tasks.watch".to_string(),
                launch: ExtensionLaunchDiagnostic::Shell {
                    command: "watch".to_string(),
                },
                cwd: ".".to_string(),
                restart: "on-failure".to_string(),
                injected_env: BTreeMap::new(),
                configured_env_keys: vec!["TOKEN".to_string()],
            }],
            command_paths: BTreeMap::new(),
            service_paths: BTreeMap::new(),
            errors: vec!["one problem".to_string()],
        }
    }

    #[test]
    fn report_sections_have_the_planned_order_and_actionable_row_kinds() {
        let merged = [(
            "runner".to_string(),
            ExtensionSettingValue::String("just".to_string()),
        )]
        .into_iter()
        .collect();
        let sections = report_sections(&info(), &merged);
        assert_eq!(
            sections
                .iter()
                .map(|section| section.title)
                .collect::<Vec<_>>(),
            [
                "Overview",
                "Commands",
                "Services",
                "Agents",
                "Sidebar tabs",
                "Settings",
                "Errors"
            ]
        );
        assert_eq!(
            sections[1].rows[0].kind,
            ReportKind::Command("tasks.run".to_string())
        );
        assert_eq!(
            sections[5].rows[0].kind,
            ReportKind::Setting("runner".to_string())
        );
        assert_eq!(sections[6].rows[0].kind, ReportKind::Error);
        assert_eq!(sections[5].rows[0].value, "\"just\"");
    }

    #[test]
    fn report_omits_empty_sections_and_plain_text_keeps_multiline_details() {
        let mut empty = info();
        empty.command_details.clear();
        empty.service_details.clear();
        empty.agents.clear();
        empty.sidebar_tabs.clear();
        empty.errors.clear();
        let sections = report_sections(&empty, &ExtensionSettings::new());
        assert_eq!(
            sections
                .iter()
                .map(|section| section.title)
                .collect::<Vec<_>>(),
            ["Overview"]
        );

        let sections = report_sections(&info(), &info().settings);
        let text = report_text(&sections);
        assert!(text.starts_with("Overview\n  Status: loaded\n"));
        assert!(text.contains(
            "Commands\n  tasks.run:\n    launch: send \"run\"\n    cwd: focused-pane-input\n"
        ));
        assert!(text.contains("manifest env: TOKEN (values redacted)"));
        assert!(text.ends_with("Errors\n  Error: one problem\n"));
    }
}
