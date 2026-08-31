//! Extension subcommands: scaffolding a new extension, listing installed ones, and checking a
//! manifest before it is installed.

use tui_lipan::Result;

use super::output::{OutputStyles, OutputTone, TableCell, format_table};

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
            &format!("rozi check-extension {}", destination.display()),
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
    verbose: bool,
    styles: OutputStyles,
) -> String {
    if entries.is_empty() {
        return format!(
            "{}\n",
            styles.paint("No extensions found.", OutputTone::Muted)
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
        for error in &extension.errors {
            out.push_str("  error     ");
            out.push_str(error);
            out.push('\n');
        }
    }
    out
}

pub(crate) fn run_list_extensions_cli(json: bool, verbose: bool) -> Result<()> {
    let scan = crate::config::scan_extensions_for_cli();
    for error in &scan.root_errors {
        eprintln!("rozi: {error}");
    }
    let entries = scan.entries();
    if json {
        let document = crate::config::ExtensionListDocument::new(entries);
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        );
        return Ok(());
    }
    print!(
        "{}",
        format_extensions_text(&entries, verbose, OutputStyles::detect())
    );
    Ok(())
}

pub(crate) fn run_check_extension_cli(path: &std::path::Path, json: bool) -> Result<bool> {
    let extension = crate::config::check_extension(path);
    let info = &extension.info;
    if json {
        let document = crate::config::ExtensionCheckDocument::new(info.clone());
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(std::io::Error::other)?
        );
        return Ok(info.status == crate::config::ExtensionStatus::Loaded);
    }
    let styles = OutputStyles::detect();
    println!(
        "{}  {}",
        styles.paint("Extension", OutputTone::Muted),
        styles.paint(info.display_name(), OutputTone::Accent)
    );
    println!(
        "{}    {}",
        styles.paint("Version", OutputTone::Muted),
        info.version.as_deref().unwrap_or("—")
    );
    println!(
        "{}        {}",
        styles.paint("API", OutputTone::Muted),
        info.api
            .map(|api| api.to_string())
            .unwrap_or_else(|| "—".to_string())
    );
    println!();
    if info.status == crate::config::ExtensionStatus::Loaded {
        println!("{}", styles.paint("CHECKS", OutputTone::Heading));
        for check in [
            "manifest valid".to_string(),
            "extension id valid".to_string(),
            format!("{} commands", info.commands.len()),
            format!("{} services", info.services.len()),
            "executable paths resolved".to_string(),
        ] {
            println!("  {} {check}", styles.paint("✓", OutputTone::Success));
        }
    } else {
        println!(
            "{}  {}",
            styles.paint("Status", OutputTone::Muted),
            styles.paint(info.status.as_str(), extension_status_tone(info.status))
        );
        for error in &info.errors {
            eprintln!("rozi: {error}");
        }
    }
    if !info.command_details.is_empty() {
        println!("\n{}", styles.paint("COMMANDS", OutputTone::Heading));
        for command in &info.command_details {
            println!("  {}", styles.paint(&command.id, OutputTone::Accent));
            println!("    launch: {}", format_extension_launch(&command.launch));
            println!("    cwd:    {}", command.cwd);
            println!(
                "    env:    {}",
                format_extension_env(&command.injected_env)
            );
        }
    }
    if !info.service_details.is_empty() {
        println!("\n{}", styles.paint("SERVICES", OutputTone::Heading));
        for service in &info.service_details {
            println!("  {}", styles.paint(&service.id, OutputTone::Accent));
            println!("    launch: {}", format_extension_launch(&service.launch));
            println!("    cwd:    {}", service.cwd);
            println!("    restart: {}", service.restart);
            println!(
                "    env:    {}",
                format_extension_env(&service.injected_env)
            );
            if !service.configured_env_keys.is_empty() {
                println!(
                    "    manifest env: {} (values redacted)",
                    service.configured_env_keys.join(", ")
                );
            }
        }
    }
    Ok(info.status == crate::config::ExtensionStatus::Loaded)
}

fn format_extension_launch(launch: &crate::config::ExtensionLaunchDiagnostic) -> String {
    match launch {
        crate::config::ExtensionLaunchDiagnostic::Direct { argv } => {
            serde_json::to_string(argv).unwrap_or_else(|_| "[]".to_string())
        }
        crate::config::ExtensionLaunchDiagnostic::Shell { command } => {
            format!("shell {command:?}")
        }
        crate::config::ExtensionLaunchDiagnostic::Send { text } => format!("send {text:?}"),
    }
}

fn format_extension_env(env: &std::collections::BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}
