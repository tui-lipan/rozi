//! Managed-installation subcommands: `install`, `update`, and the startup recovery a managed
//! binary runs before anything else.

use super::args::UpdateCommand;
use super::output::{OutputStyles, OutputTone};
use crate::platform::install::InstallError;
use crate::platform::install_source::{InstallSource, SourceEnv};
use crate::platform::paths::{self, PlatformEnv};

pub(crate) fn recover_managed_installation() -> std::result::Result<(), String> {
    crate::platform::install::from_process()
        .recover_if_managed()
        .map(|_| ())
        .map_err(|error| format!("managed installation recovery failed: {error}"))
}

pub(crate) fn run_install_cli() -> std::result::Result<(), String> {
    let styles = OutputStyles::detect();
    let installation = crate::platform::install::from_process();
    let result = installation
        .install()
        .map_err(|error| format!("installation failed: {error}"))?;
    if result.changed {
        println!(
            "{} rozi {}",
            styles.paint("Installed", OutputTone::Success),
            styles.paint(&format!("v{}", result.version), OutputTone::Accent)
        );
    } else {
        println!(
            "rozi {} is already installed and verified",
            styles.paint(&format!("v{}", result.version), OutputTone::Accent)
        );
    }
    println!(
        "{}  {}",
        styles.paint("Command", OutputTone::Muted),
        installation.command_path().display()
    );
    Ok(())
}

/// Classify the running binary's distribution channel.
///
/// Falls back to [`InstallSource::Unknown`] when the platform will not name the executable, which
/// is the same answer as an unrecognised layout: say nothing rather than guess a command.
fn detect_install_source() -> InstallSource {
    let source_env = SourceEnv::from_platform_env(&PlatformEnv::from_process());
    paths::current_binary()
        .map(|executable| InstallSource::detect(&executable, &source_env))
        .unwrap_or(InstallSource::Unknown)
}

/// Name what owns an install `relswap` does not.
///
/// `relswap` refuses these correctly but can only say "managed installation is not present", which
/// tells the user what did not happen and nothing about what would. Reaching the real command is
/// the entire point of detecting the channel.
fn unmanaged_channel_clause() -> String {
    let source = detect_install_source();
    match source.upgrade_command() {
        Some(command) => format!(
            "this rozi was installed with {}, which owns its updates - run: {command}",
            source.label()
        ),
        None if source == InstallSource::SystemPackage => {
            "this rozi is owned by a system package manager, which owns its updates".to_string()
        }
        None => format!(
            "this rozi was not installed by its managed installer - see {}",
            env!("CARGO_PKG_HOMEPAGE")
        ),
    }
}

pub(crate) fn run_update_cli(command: UpdateCommand) -> std::result::Result<(), String> {
    let styles = OutputStyles::detect();
    let installation = crate::platform::install::from_process();
    match command {
        UpdateCommand::Check => {
            let result = installation
                .check_latest()
                .map_err(|error| format!("update check failed: {error}"))?;
            let running = semver::Version::parse(env!("CARGO_PKG_VERSION"))
                .map_err(|error| format!("invalid running version: {error}"))?;
            let current = result.current.as_ref().unwrap_or(&running);
            // `result.managed` is the authority on whether `rozi update` can act; the detected
            // channel only names what does own this install when it cannot.
            let channel = (!result.managed).then(detect_install_source);
            println!(
                "{}  {} ({})",
                styles.paint("Current", OutputTone::Muted),
                styles.paint(&format!("v{current}"), OutputTone::Accent),
                match channel {
                    None => "managed",
                    // Detection disagreeing with the engine is not a licence to claim this install
                    // is managed: the engine already said it cannot act on it.
                    Some(InstallSource::Managed) => InstallSource::Unknown.label(),
                    Some(source) => source.label(),
                }
            );
            println!(
                "{}   {}",
                styles.paint("Latest", OutputTone::Muted),
                styles.paint(&format!("v{}", result.latest), OutputTone::Accent)
            );
            let (status, tone) = if result.latest > *current {
                ("update available", OutputTone::Warning)
            } else if result.latest == *current {
                ("up to date", OutputTone::Success)
            } else {
                ("running version is newer", OutputTone::Warning)
            };
            println!(
                "{}   {}",
                styles.paint("Status", OutputTone::Muted),
                styles.paint(status, tone)
            );
            // Only worth printing when there is something to act on: an install this engine does
            // not own, and a newer version to move to.
            if let Some(command) = channel.and_then(InstallSource::upgrade_command)
                && result.latest > *current
            {
                println!(
                    "{}   {}",
                    styles.paint("Update", OutputTone::Muted),
                    styles.paint(command, OutputTone::Accent)
                );
            }
        }
        UpdateCommand::Apply => {
            // The archive is several megabytes and used to arrive in silence. A streaming
            // downloader draws one rewritten row while it lands, then erases it so the outcome
            // below is what stays in the scrollback.
            let row = std::sync::Arc::new(crate::platform::progress::StatusRow::new("Downloading"));
            let result = crate::platform::install::from_process_with_progress(row.clone())
                .update()
                .inspect(|_| row.finish())
                .inspect_err(|_| row.finish())
                .map_err(|error| match error {
                    InstallError::Unmanaged => unmanaged_channel_clause(),
                    error => format!("update failed: {error}"),
                })?;
            if result.changed {
                println!(
                    "{} rozi to {}",
                    styles.paint("Updated", OutputTone::Success),
                    styles.paint(&format!("v{}", result.version), OutputTone::Accent)
                );
            } else {
                println!(
                    "rozi {} is {}",
                    styles.paint(&format!("v{}", result.version), OutputTone::Accent),
                    styles.paint("up to date", OutputTone::Success)
                );
            }
        }
        UpdateCommand::Rollback => {
            let result = installation.rollback().map_err(|error| match error {
                InstallError::Unmanaged => format!(
                    "rollback needs a managed install; {}",
                    unmanaged_channel_clause()
                ),
                error => format!("rollback failed: {error}"),
            })?;
            println!(
                "{} rozi to {}",
                styles.paint("Rolled back", OutputTone::Success),
                styles.paint(&format!("v{}", result.version), OutputTone::Accent)
            );
        }
    }
    Ok(())
}
