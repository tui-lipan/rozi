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
    println!(
        "{}  {}",
        styles.paint("Access", OutputTone::Muted),
        styles.paint(
            crate::extension_installation::UNSANDBOXED_NOTICE,
            OutputTone::Warning
        )
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
    if let Some(warning) = &installed.warning {
        println!(
            "{}  {}",
            styles.paint("Warning", OutputTone::Warning),
            styles.paint(warning, OutputTone::Warning)
        );
    }
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

/// An extension's logs go with it: they describe an installation that no longer exists. Failing
/// to delete them is not worth failing the removal for.
fn delete_logs(id: &str) {
    let _ = crate::platform::paths::remove_extension_logs(
        &crate::platform::paths::PlatformEnv::from_process(),
        id,
    );
}

/// `extensions logs`: what an extension's processes on this machine printed, one block per unit.
pub(crate) fn run_logs_extension_cli(id: &str, unit: Option<&str>, lines: usize) -> Result<()> {
    use crate::extension_runtime::log;

    if !crate::platform::paths::valid_extension_id(id) {
        return Err(std::io::Error::other(format!("`{id}` is not an extension id")).into());
    }
    let available = log::units(id)?;
    let shown: Vec<&String> = match unit {
        None => available.iter().collect(),
        Some(unit) => {
            let wanted = log::stem(unit);
            let matched: Vec<&String> = available.iter().filter(|name| **name == wanted).collect();
            if matched.is_empty() && !available.is_empty() {
                return Err(std::io::Error::other(format!(
                    "`{id}` has no log for `{unit}` here (units: {})",
                    available.join(", ")
                ))
                .into());
            }
            matched
        }
    };
    let styles = OutputStyles::detect();
    if shown.is_empty() {
        println!(
            "{}",
            styles.paint(
                &format!(
                    "No output recorded for `{id}` on this machine. An extension's output is kept \
                     on the machine its process ran on; run this there for placed processes."
                ),
                OutputTone::Muted
            )
        );
        return Ok(());
    }
    for (index, name) in shown.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!(
            "{}",
            styles.paint(&format!("==> {name} <=="), OutputTone::Heading)
        );
        for line in log::tail(id, name, lines)? {
            println!("{line}");
        }
    }
    Ok(())
}

pub(crate) fn run_remove_extension_cli(id: &str, purge: bool) -> Result<()> {
    // `--purge` also clears what an extension removed earlier left behind.
    if purge && !crate::extension_installation::is_installed(id) {
        let purged =
            crate::extension_installation::purge_state(id).map_err(std::io::Error::other)?;
        delete_logs(id);
        if !purged {
            return Err(std::io::Error::other(format!(
                "Extension `{id}` is not installed and has no state on this machine"
            ))
            .into());
        }
        let styles = OutputStyles::detect();
        println!(
            "{} {}",
            styles.paint("Purged", OutputTone::Success),
            styles.paint(id, OutputTone::Accent)
        );
        return Ok(());
    }
    let removed = crate::extension_installation::remove(id).map_err(std::io::Error::other)?;
    delete_logs(id);
    let purged = purge
        .then(|| crate::extension_installation::purge_state(id))
        .transpose()
        .map_err(std::io::Error::other)?;
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
    let state = match purged {
        Some(true) => Some("Deleted on this machine.".to_string()),
        Some(false) => None,
        None => crate::platform::paths::extension_state_dir(
            &crate::platform::paths::PlatformEnv::from_process(),
            id,
        )
        .exists()
        .then(|| format!("Kept. Delete it with `rozi extensions remove {id} --purge`.")),
    };
    if let Some(state) = state {
        println!("{}   {state}", styles.paint("State", OutputTone::Muted));
    }
    if let Some(warning) = &removed.warning {
        println!(
            "{}  {}",
            styles.paint("Warning", OutputTone::Warning),
            styles.paint(warning, OutputTone::Warning)
        );
    }
    Ok(())
}

/// Updates one Git-managed extension, or every one when `id` is `None`. A failure does not stop
/// the rest, but makes the command fail once they have all run.
pub(crate) fn run_update_extension_cli(id: Option<&str>) -> Result<()> {
    let Some(id) = id else {
        return run_update_all_extensions_cli();
    };
    update_one_extension(id).map_err(std::io::Error::other)?;
    Ok(())
}

fn run_update_all_extensions_cli() -> Result<()> {
    let listed =
        crate::extension_installation::list_git_installations().map_err(std::io::Error::other)?;
    if listed.ids.is_empty() && listed.failures.is_empty() {
        println!("No Git-managed extensions to update");
        return Ok(());
    }
    let styles = OutputStyles::detect();
    let mut failed = listed.failures.len();
    for (id, error) in &listed.failures {
        report_not_updated(&styles, id, error);
    }
    for id in &listed.ids {
        match update_one_extension(id) {
            Ok(()) => {}
            Err(error) => {
                failed += 1;
                report_not_updated(&styles, id, &error);
            }
        }
    }
    if failed > 0 {
        let total = listed.ids.len() + listed.failures.len();
        return Err(std::io::Error::other(format!(
            "{failed} of {total} extensions could not be updated"
        ))
        .into());
    }
    Ok(())
}

fn report_not_updated(styles: &OutputStyles, id: &str, error: &str) {
    eprintln!(
        "{} {}: {error}",
        styles.paint("Not updated", OutputTone::Error),
        styles.paint(id, OutputTone::Accent)
    );
}

/// Updates `id` and prints its outcome.
fn update_one_extension(id: &str) -> std::result::Result<(), String> {
    let mut progress = crate::platform::progress::ActivityRow::new(format!("Updating {id}"));
    let updated = crate::extension_installation::update(id);
    progress.finish();
    let updated = updated?;
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
    if let Some(warning) = &updated.warning {
        println!(
            "{}  {}",
            styles.paint("Warning", OutputTone::Warning),
            styles.paint(warning, OutputTone::Warning)
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
    for (index, step) in scaffold_next_steps(id).iter().enumerate() {
        let label = if index == 0 { "Next" } else { "" };
        println!(
            "{}  {}",
            styles.paint(&format!("{label:<4}"), OutputTone::Muted),
            styles.paint(step, OutputTone::Accent)
        );
    }
    Ok(())
}

/// From a fresh scaffold to its command running in the current `rozi`.
fn scaffold_next_steps(id: &str) -> [String; 4] {
    [
        format!("cd {id}"),
        "rozi extensions check .".to_string(),
        "rozi extensions install --link .".to_string(),
        format!("rozi run-action {id}.hello"),
    ]
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
                TableCell::new(extension.display_name(), OutputTone::Key),
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
        // Discovery metadata: shown only when declared, so an extension that says nothing about
        // where it runs does not grow three em dashes explaining that.
        if let Some(min_rozi) = extension.min_rozi.as_deref() {
            field("min rozi", min_rozi);
        }
        if !extension.platforms.is_empty() {
            field("platforms", &extension.platforms.join(", "));
        }
        if let Some(homepage) = extension.homepage.as_deref() {
            field("homepage", homepage);
        }
        if !extension.screenshots.is_empty() {
            field("screenshots", &extension.screenshots.join(", "));
        }
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
    let runtime = (json || verbose)
        .then(super::control::live_extension_runtime)
        .flatten();
    if json {
        let document = crate::config::ExtensionListDocument::new(entries, runtime);
        super::output::print_or_stop(&format!(
            "{}\n",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        ));
        return Ok(());
    }
    let styles = OutputStyles::detect();
    let mut text = format_extensions_text(&entries, &root, verbose, styles);
    if let Some(runtime) = runtime
        && !(runtime.runtimes.is_empty() && runtime.instances.is_empty())
    {
        text.push('\n');
        text.push_str(&super::output::format_extension_runtime_text(
            serde_json::to_value(&runtime).ok().as_ref(),
            styles,
        ));
    }
    super::output::print_or_stop(&text);
    Ok(())
}

pub(crate) fn run_check_extension_cli(path: &std::path::Path, json: bool) -> Result<bool> {
    let extension = crate::config::check_extension(path);
    let info = &extension.info;
    let hints = crate::config::authoring_hints(info);
    let discover = crate::config::discover_readiness(info);
    if json {
        let document = crate::config::ExtensionCheckDocument::new(info.clone(), hints, discover);
        super::output::print_or_stop(&format!(
            "{}\n",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        ));
        return Ok(info.status == crate::config::ExtensionStatus::Loaded);
    }
    let styles = OutputStyles::detect();
    let mut sections = crate::config::report_sections(info, &info.settings);
    if !hints.is_empty() {
        sections.push(crate::config::ReportSection {
            title: "Hints",
            rows: hints
                .into_iter()
                .map(|hint| crate::config::ReportRow {
                    label: "Hint".to_string(),
                    value: hint,
                    tone: crate::config::ReportTone::Warning,
                    kind: crate::config::ReportKind::Info,
                })
                .collect(),
        });
    }
    sections.push(discover_section(discover));
    super::output::print_or_stop(&format_check_text(&sections, styles));
    Ok(info.status == crate::config::ExtensionStatus::Loaded)
}

fn discover_section(discover: crate::config::DiscoverReadiness) -> crate::config::ReportSection {
    let row = |label: &str, value: String, tone| crate::config::ReportRow {
        label: label.to_string(),
        value,
        tone,
        kind: crate::config::ReportKind::Info,
    };
    let rows = if discover.ready {
        vec![row(
            "Ready",
            "add the `rozi-extension` topic to the public GitHub repository; the index updates daily"
                .to_string(),
            crate::config::ReportTone::Success,
        )]
    } else {
        discover
            .problems
            .into_iter()
            .map(|problem| row("Not listed", problem, crate::config::ReportTone::Muted))
            .collect()
    };
    crate::config::ReportSection {
        title: "Discover",
        rows,
    }
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
