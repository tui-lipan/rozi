//! Extension subcommands: scaffolding a new extension, listing installed ones, and checking a
//! manifest before it is installed.

use tui_lipan::Result;

use super::output::{OutputStyles, OutputTone, TableCell, format_table};

pub(crate) fn run_install_extension_cli(source: &str, link: bool) -> Result<()> {
    let request = if link {
        crate::extension_installation::InstallRequest::Link(std::path::PathBuf::from(source))
    } else {
        crate::extension_installation::InstallRequest::Source(source.to_string())
    };
    let mut progress = crate::platform::progress::ActivityRow::new("Installing extension");
    let installed = crate::extension_installation::install(request);
    progress.finish();
    let installed = installed.map_err(std::io::Error::other)?;
    let styles = OutputStyles::detect();
    let action = match installed.kind {
        crate::extension_installation::InstallKind::Local => "Copied",
        crate::extension_installation::InstallKind::Git => "Cloned",
        crate::extension_installation::InstallKind::Link => "Linked",
    };
    println!(
        "{} {}",
        styles.paint("Installed", OutputTone::Success),
        styles.paint(&installed.id, OutputTone::Accent)
    );
    println!(
        "{}  {} to {}",
        styles.paint("Source", OutputTone::Muted),
        action,
        installed.destination.display()
    );
    if let Some(summary) = installed_extension_summary(&installed.id) {
        if summary.navigation_programs > 0 {
            println!(
                "{}  {} program{}",
                styles.paint("Navigation targets", OutputTone::Muted),
                summary.navigation_programs,
                if summary.navigation_programs == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
        if summary.keybindings > 0 {
            let mut counts = Vec::new();
            if summary.active > 0 {
                counts.push(format!("{} active", summary.active));
            }
            if summary.conflicts > 0 {
                counts.push(format!(
                    "{} conflict{}",
                    summary.conflicts,
                    if summary.conflicts == 1 { "" } else { "s" }
                ));
            }
            if summary.suppressed > 0 {
                counts.push(format!("{} suppressed", summary.suppressed));
            }
            println!(
                "{}  {}",
                styles.paint("Keybindings", OutputTone::Muted),
                counts.join(", ")
            );
        }
    }
    println!(
        "{}  {}",
        styles.paint("Reload", OutputTone::Muted),
        styles.paint("rozi run-action reload-extensions", OutputTone::Accent)
    );
    Ok(())
}

struct InstalledExtensionSummary {
    navigation_programs: usize,
    keybindings: usize,
    active: usize,
    conflicts: usize,
    suppressed: usize,
}

fn installed_extension_summary(id: &str) -> Option<InstalledExtensionSummary> {
    let loaded = crate::config::load_config();
    let mut entries = crate::config::scan_extensions_for_cli().entries();
    crate::config::apply_suggested_keybinding_resolutions(
        &mut entries,
        &loaded.config.suggested_keybinding_resolutions,
    );
    let extension = entries
        .iter()
        .find(|extension| extension.id.as_deref() == Some(id))?;
    Some(InstalledExtensionSummary {
        navigation_programs: extension
            .navigation_targets
            .iter()
            .map(|target| target.programs.len())
            .sum(),
        keybindings: extension.suggested_keybindings.len(),
        active: extension
            .suggested_keybindings
            .iter()
            .filter(|binding| {
                binding.status == crate::config::ExtensionSuggestedKeybindingStatus::Active
            })
            .count(),
        conflicts: extension
            .suggested_keybindings
            .iter()
            .filter(|binding| {
                binding.status == crate::config::ExtensionSuggestedKeybindingStatus::Conflict
            })
            .count(),
        suppressed: extension
            .suggested_keybindings
            .iter()
            .filter(|binding| {
                binding.status == crate::config::ExtensionSuggestedKeybindingStatus::Suppressed
            })
            .count(),
    })
}

pub(crate) fn run_remove_extension_cli(id: &str) -> Result<()> {
    let removed = crate::extension_installation::remove(id).map_err(std::io::Error::other)?;
    let styles = OutputStyles::detect();
    println!(
        "{} {}",
        styles.paint(
            if removed.linked {
                "Unlinked"
            } else {
                "Removed"
            },
            OutputTone::Success
        ),
        styles.paint(&removed.id, OutputTone::Accent)
    );
    if removed.linked {
        println!(
            "{}  The source checkout was not changed.",
            styles.paint("Source", OutputTone::Muted)
        );
    }
    println!(
        "{}  {}",
        styles.paint("Reload", OutputTone::Muted),
        styles.paint("rozi run-action reload-extensions", OutputTone::Accent)
    );
    Ok(())
}

pub(crate) fn run_update_extension_cli(id: &str) -> Result<()> {
    let mut progress = crate::platform::progress::ActivityRow::new("Updating extension");
    let updated = crate::extension_installation::update(id);
    progress.finish();
    let updated = updated.map_err(std::io::Error::other)?;
    let styles = OutputStyles::detect();
    println!(
        "{} {}",
        styles.paint(
            if updated.changed {
                "Updated"
            } else {
                "Up to date"
            },
            if updated.changed {
                OutputTone::Success
            } else {
                OutputTone::Muted
            }
        ),
        styles.paint(&updated.id, OutputTone::Accent)
    );
    if updated.changed {
        println!(
            "{}  {}",
            styles.paint("Reload", OutputTone::Muted),
            styles.paint("rozi run-action reload-extensions", OutputTone::Accent)
        );
    }
    Ok(())
}

pub(crate) fn run_new_extension_cli(id: &str) -> Result<()> {
    let styles = OutputStyles::detect();
    let parent = std::env::current_dir()?;
    let destination =
        crate::config::create_extension_scaffold(id, &parent).map_err(std::io::Error::other)?;
    println!(
        "{} {}",
        styles.paint("Created", OutputTone::Success),
        destination.display()
    );
    println!(
        "{}  {}",
        styles.paint("Validate", OutputTone::Muted),
        styles.paint(
            &format!("rozi extensions check {}", destination.display()),
            OutputTone::Accent
        )
    );
    println!(
        "{}    {}",
        styles.paint("Invoke", OutputTone::Muted),
        styles.paint(&format!("rozi run-action {id}.hello"), OutputTone::Accent)
    );
    Ok(())
}

fn extension_status_tone(status: crate::config::ExtensionStatus) -> OutputTone {
    match status {
        crate::config::ExtensionStatus::Loaded => OutputTone::Success,
        crate::config::ExtensionStatus::Disabled => OutputTone::Muted,
        crate::config::ExtensionStatus::Invalid
        | crate::config::ExtensionStatus::Incompatible
        | crate::config::ExtensionStatus::Duplicate => OutputTone::Error,
    }
}

pub(super) fn format_extensions_text(
    entries: &[crate::config::ExtensionInfo],
    root: &std::path::Path,
    verbose: bool,
    styles: OutputStyles,
) -> String {
    // Unlike an empty session or pane report, an empty extension report has a location that is
    // almost always the answer: extensions live under the data directory, and the neighbouring
    // config directory is the obvious wrong guess.
    if entries.is_empty() {
        return format!(
            "{}\n",
            styles.paint(
                &format!("No extensions found in {}.", root.display()),
                OutputTone::Muted
            )
        );
    }
    let rows = entries
        .iter()
        .map(|extension| {
            vec![
                TableCell::new(extension.display_name(), OutputTone::Accent),
                TableCell::plain(extension.title.as_deref().unwrap_or("—")),
                TableCell::plain(extension.version.as_deref().unwrap_or("—")),
                TableCell::plain(extension.commands.len().to_string()),
                TableCell::plain(extension.services.len().to_string()),
                TableCell::new(
                    extension.status_detail(),
                    extension_status_tone(extension.status),
                ),
            ]
        })
        .collect::<Vec<_>>();
    let mut out = format_table(
        &["NAME", "TITLE", "VERSION", "COMMANDS", "SERVICES", "STATUS"],
        &rows,
        styles,
    );
    if !verbose {
        return out;
    }

    for extension in entries {
        out.push('\n');
        out.push_str(&styles.paint(extension.display_name(), OutputTone::Heading));
        out.push('\n');
        let mut field = |label: &str, value: &str| {
            out.push_str("  ");
            out.push_str(&format!("{label:<10}"));
            out.push_str(value);
            out.push('\n');
        };
        field("directory", &extension.path);
        field("manifest", &extension.manifest_path);
        field("id", extension.id.as_deref().unwrap_or("<unresolved>"));
        field("title", extension.title.as_deref().unwrap_or("—"));
        field(
            "api",
            &extension
                .api
                .map(|api| api.to_string())
                .unwrap_or_else(|| "—".to_string()),
        );
        if !extension.commands.is_empty() {
            out.push_str("  commands\n");
            for id in &extension.commands {
                out.push_str("    ");
                out.push_str(id);
                if let Some(path) = extension.command_paths.get(id) {
                    out.push_str("  ");
                    out.push_str(path);
                }
                out.push('\n');
            }
        }
        if !extension.services.is_empty() {
            out.push_str("  services\n");
            for id in &extension.services {
                out.push_str("    ");
                out.push_str(id);
                if let Some(path) = extension.service_paths.get(id) {
                    out.push_str("  ");
                    out.push_str(path);
                }
                out.push('\n');
            }
        }
        if !extension.agents.is_empty() {
            out.push_str("  agents\n");
            for id in &extension.agents {
                out.push_str("    ");
                out.push_str(id);
                out.push('\n');
            }
        }
        if !extension.settings.is_empty() {
            out.push_str("  settings\n");
            for (key, value) in &extension.settings {
                out.push_str("    ");
                out.push_str(key);
                out.push_str("  ");
                out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "?".to_string()));
                out.push('\n');
            }
        }
        if !extension.sidebar_tabs.is_empty() {
            out.push_str("  sidebar tabs\n");
            for id in &extension.sidebar_tabs {
                out.push_str("    ");
                out.push_str(id);
                out.push('\n');
            }
        }
        if !extension.navigation_targets.is_empty() {
            out.push_str("  navigation targets\n");
            for target in &extension.navigation_targets {
                out.push_str("    ");
                out.push_str(&target.name);
                out.push_str("  ");
                out.push_str(&target.programs.join(", "));
                out.push('\n');
            }
        }
        if !extension.suggested_keybindings.is_empty() {
            out.push_str("  suggested keybindings\n");
            for binding in &extension.suggested_keybindings {
                out.push_str("    ");
                out.push_str(&format!("{:<10}  {:<24}  ", binding.key, binding.action));
                out.push_str(binding.status.as_str());
                if let Some(detail) = binding.detail.as_deref() {
                    out.push_str(": ");
                    out.push_str(detail);
                }
                out.push('\n');
            }
        }
        for error in &extension.errors {
            out.push_str("  error     ");
            out.push_str(error);
            out.push('\n');
        }
    }
    out
}

pub(crate) fn run_list_extensions_cli(json: bool, verbose: bool) -> Result<()> {
    let root = crate::config::extensions_dir_path();
    let scan = crate::config::scan_extensions_for_cli();
    for error in &scan.root_errors {
        eprintln!("rozi: {error}");
    }
    let mut entries = scan.entries();
    let loaded = crate::config::load_config();
    crate::config::apply_suggested_keybinding_resolutions(
        &mut entries,
        &loaded.config.suggested_keybinding_resolutions,
    );
    if json {
        let document = crate::config::ExtensionListDocument::new(entries);
        super::output::print_or_stop(&format!(
            "{}\n",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        ));
        return Ok(());
    }
    super::output::print_or_stop(&format_extensions_text(
        &entries,
        &root,
        verbose,
        OutputStyles::detect(),
    ));
    Ok(())
}

pub(crate) fn run_check_extension_cli(path: &std::path::Path, json: bool) -> Result<bool> {
    let extension = crate::config::check_extension(path);
    let info = &extension.info;
    if json {
        let document = crate::config::ExtensionCheckDocument::new(info.clone());
        super::output::print_or_stop(&format!(
            "{}\n",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        ));
        return Ok(info.status == crate::config::ExtensionStatus::Loaded);
    }
    let styles = OutputStyles::detect();
    let sections = crate::config::report_sections(info, &info.settings);
    super::output::print_or_stop(&format_check_text(&sections, styles));
    Ok(info.status == crate::config::ExtensionStatus::Loaded)
}

/// The whole `extensions check` report as one string.
///
/// Built rather than printed line by line so it reaches stdout in a single write: `println!` panics
/// when the reader has gone, and `rozi extensions check … | head` is an ordinary thing to type.
fn format_check_text(sections: &[crate::config::ReportSection], styles: OutputStyles) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for (section_index, section) in sections.iter().enumerate() {
        if section_index > 0 {
            out.push('\n');
        }
        let _ = writeln!(
            out,
            "{}",
            styles.paint(&section.title.to_ascii_uppercase(), OutputTone::Heading)
        );
        for row in &section.rows {
            if row.value.contains('\n') {
                let _ = writeln!(out, "  {}", styles.paint(&row.label, report_tone(row.tone)));
                for line in row.value.lines() {
                    let _ = writeln!(out, "    {line}");
                }
            } else {
                let label_tone = match &row.kind {
                    crate::config::ReportKind::Command(_)
                    | crate::config::ReportKind::Setting(_) => OutputTone::Accent,
                    crate::config::ReportKind::Error | crate::config::ReportKind::Info => {
                        OutputTone::Muted
                    }
                };
                let _ = writeln!(
                    out,
                    "  {}  {}",
                    styles.paint(&row.label, label_tone),
                    styles.paint(&row.value, report_tone(row.tone))
                );
            }
        }
    }
    out
}

fn report_tone(tone: crate::config::ReportTone) -> OutputTone {
    match tone {
        crate::config::ReportTone::Plain => OutputTone::Plain,
        crate::config::ReportTone::Accent => OutputTone::Accent,
        crate::config::ReportTone::Success => OutputTone::Success,
        crate::config::ReportTone::Warning => OutputTone::Warning,
        crate::config::ReportTone::Error => OutputTone::Error,
        crate::config::ReportTone::Muted => OutputTone::Muted,
    }
}
