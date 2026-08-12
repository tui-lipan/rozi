//! Shell-integration injection (cross-platform plan Phase 8).
//!
//! Ships `assets/shell-integration/hyprmux.{bash,zsh,fish,ps1}` (embedded via `include_str!`, no
//! install step required) and, when [`ShellIntegrationMode::Auto`] recognizes the resolved
//! *interactive* shell (never the one-off `command_shell` runner - see the plan's "do not inject
//! into noninteractive command-runner processes" rule), adjusts that pane's spawn argv/env so the
//! child picks up the matching script automatically:
//!
//! - bash: appends `--rcfile <generated wrapper>`, which chains the user's real `~/.bashrc` (and
//!   `/etc/bash.bashrc`) before sourcing `hyprmux.bash`. Skipped for a configured login shell
//!   (`-l`/`--login`): bash ignores `--rcfile` for login shells entirely, and there is no
//!   automatic non-dotfile-editing equivalent for that case, per the plan's call to "handle login
//!   vs non-login shells explicitly" - this is that handling: recognized and skipped rather than
//!   silently doing nothing while claiming to have injected.
//! - zsh: sets `ZDOTDIR` to a generated shim directory and `ROZI_ORIG_ZDOTDIR` to the child's
//!   real one (its inherited `$ZDOTDIR`, or `$HOME` if unset) so the shim's `.zshenv`/`.zshrc`
//!   chain to the user's real files before sourcing `hyprmux.zsh`.
//! - fish: prepends the install directory to `XDG_DATA_DIRS` so fish's own `vendor_conf.d`
//!   auto-discovery picks up `hyprmux.fish` - no dotfile or wrapper needed.
//! - PowerShell: appends `-NoExit -Command ". <hyprmux.ps1>"`. The plan assumed no clean
//!   non-dotfile injection point existed here and settled for env markers plus a documented
//!   `$PROFILE` edit; there is one. PowerShell runs `$PROFILE` *before* `-Command`, so the script
//!   sees the user's finished prompt and PSReadLine configuration and wraps them, and `-NoExit`
//!   keeps the session interactive afterwards. (This is the same mechanism VS Code's own PowerShell
//!   integration uses.) The `$PROFILE` route still works and is still documented, for shells hyprmux
//!   did not launch; the script is idempotent, so having both costs nothing.
//! - cmd.exe: sets `PROMPT` to a variant carrying OSC 9;9 (cwd) and OSC 133 A/B (prompt boundaries)
//!   markers around the user's `$P$G`. Command lifecycle (`C`/`D`, and therefore exit status and
//!   foreground program) is *not* available: cmd has no preexec hook, and the plan rules out the
//!   `AutoRun` registry key. Users who want the rest should install Clink. This is the one shell
//!   where integration is partial by design.
//!
//! Every script itself no-ops in a non-interactive shell and guards against being loaded twice
//! (`ROZI_SHELL_INTEGRATION_LOADED`), so this module's job is purely "make sure the right file
//! gets sourced/discovered", not deduplication - see each script's own header comment for the
//! composition rules (never overwrites an existing hook mechanism outright).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::ShellIntegrationMode;
use crate::platform::command::ShellCommand;

const BASH_SCRIPT: &str = include_str!("../../assets/shell-integration/hyprmux.bash");
const ZSH_SCRIPT: &str = include_str!("../../assets/shell-integration/hyprmux.zsh");
const FISH_SCRIPT: &str = include_str!("../../assets/shell-integration/hyprmux.fish");
const POWERSHELL_SCRIPT: &str = include_str!("../../assets/shell-integration/hyprmux.ps1");

/// cmd.exe's prompt, instrumented. `$E` is ESC and `$P` the current directory (both cmd's own
/// `PROMPT` escapes, so the value re-expands on every prompt with no hook needed); `$E\` is the
/// string terminator and `$P$G` is cmd's stock `C:\dir>` prompt.
///
/// This is the whole of cmd's integration. There is no `C`/`D` here because cmd offers nothing to
/// hang them on - see the module doc comment.
const CMD_PROMPT: &str = r"$E]133;A$E\$E]9;9;$P$E\$P$G$E]133;B$E\";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellKind {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
    Other,
}

fn detect_shell_kind(program: &str) -> ShellKind {
    // Split on both separators rather than going through `Path::file_name`, which only recognizes
    // the *host's* separator: `%COMSPEC%` is a backslash path that a Linux-hosted test (or a
    // profile written on Windows and opened on Linux) must still recognize. Windows program names
    // are also case-insensitive, and `%COMSPEC%` is not always spelled in lowercase.
    let basename = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    match basename.as_str() {
        "bash" => ShellKind::Bash,
        "zsh" => ShellKind::Zsh,
        "fish" => ShellKind::Fish,
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe" => ShellKind::PowerShell,
        "cmd" | "cmd.exe" => ShellKind::Cmd,
        _ => ShellKind::Other,
    }
}

/// Apply shell-integration injection to a resolved interactive [`ShellCommand`], returning the
/// (possibly unchanged) command plus any extra environment variables the child needs. `env`
/// supplies the values this needs to read from the *current* environment (real `ZDOTDIR`, etc.);
/// injectable for testability rather than reading `std::env` directly.
pub fn inject(
    shell: ShellCommand,
    mode: ShellIntegrationMode,
    env: &InjectionEnv,
) -> (ShellCommand, Vec<(String, String)>) {
    if mode == ShellIntegrationMode::Off || env.already_loaded {
        return (shell, Vec::new());
    }
    let Some(install_dir) = install_dir() else {
        return (shell, Vec::new());
    };
    match detect_shell_kind(&shell.program) {
        ShellKind::Bash => inject_bash(shell, &install_dir),
        ShellKind::Zsh => inject_zsh(shell, &install_dir, env),
        ShellKind::Fish => inject_fish(shell, &install_dir, env),
        ShellKind::PowerShell => inject_powershell(shell, &install_dir),
        ShellKind::Cmd => inject_cmd(shell),
        ShellKind::Other => (shell, Vec::new()),
    }
}

/// Inputs from the ambient environment that injection needs but must not read from `std::env`
/// directly, so decisions stay testable and so a detached server (which resolves its own launch
/// policy from its own process environment only as a resurrection fallback - see
/// `platform::command`'s module doc comment) is not surprised by a client-side env it never saw.
impl InjectionEnv {
    pub fn from_process() -> Self {
        Self {
            zdotdir: non_empty_env("ZDOTDIR"),
            xdg_data_dirs: non_empty_env("XDG_DATA_DIRS"),
            already_loaded: false,
        }
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// Resolve the interactive-shell launch policy and apply shell-integration injection in one call -
/// the combinator every pane-spawn call site should use instead of calling
/// [`crate::platform::command::resolve_interactive_shell`] directly.
pub fn resolve_interactive_shell(
    configured: Option<&[String]>,
    shell_env: &crate::platform::command::ShellEnv,
    mode: ShellIntegrationMode,
    injection_env: &InjectionEnv,
) -> (ShellCommand, Vec<(String, String)>) {
    let shell = crate::platform::command::resolve_interactive_shell(configured, shell_env);
    inject(shell, mode, injection_env)
}

#[derive(Clone, Debug, Default)]
pub struct InjectionEnv {
    /// The child's inherited `ZDOTDIR`, if any (used as the zsh shim's fallback-through target).
    pub zdotdir: Option<String>,
    /// The child's inherited `XDG_DATA_DIRS`, if any (fish vendor `conf.d` discovery searches
    /// every directory listed here plus its own compiled-in defaults).
    pub xdg_data_dirs: Option<String>,
    /// Set when the spawning client already knows integration is active for this pane (currently
    /// never set client-side; reserved so a future nested-hyprmux-nested-hyprmux nesting case, or
    /// a nested `command_shell` re-exec, has an explicit opt-out without needing to guess from
    /// argv). Scripts themselves already guard against a *sourced-twice* double-injection; this
    /// guards the rarer *wrapped-twice at the argv/env level* case (e.g. a pane whose `command`
    /// itself invokes `hyprmux`).
    pub already_loaded: bool,
}

fn inject_bash(shell: ShellCommand, install_dir: &Path) -> (ShellCommand, Vec<(String, String)>) {
    if shell.args.iter().any(|arg| arg == "-l" || arg == "--login") {
        // bash ignores `--rcfile` entirely for a login shell; there is no automatic
        // non-dotfile-editing equivalent for that case (see the module doc comment).
        return (shell, Vec::new());
    }
    let rcfile = install_dir.join("bash-rcfile");
    let shell = shell
        .arg("--rcfile")
        .arg(rcfile.to_string_lossy().into_owned());
    (shell, Vec::new())
}

fn inject_zsh(
    shell: ShellCommand,
    install_dir: &Path,
    env: &InjectionEnv,
) -> (ShellCommand, Vec<(String, String)>) {
    let shim_dir = install_dir.join("zsh-zdotdir");
    let mut extra_env = vec![(
        "ZDOTDIR".to_string(),
        shim_dir.to_string_lossy().into_owned(),
    )];
    if let Some(orig) = &env.zdotdir {
        extra_env.push(("ROZI_ORIG_ZDOTDIR".to_string(), orig.clone()));
    }
    (shell, extra_env)
}

fn inject_fish(
    shell: ShellCommand,
    install_dir: &Path,
    env: &InjectionEnv,
) -> (ShellCommand, Vec<(String, String)>) {
    let vendor_parent = install_dir.join("fish-vendor");
    let mut dirs = vendor_parent.to_string_lossy().into_owned();
    if let Some(existing) = &env.xdg_data_dirs
        && !existing.is_empty()
    {
        dirs.push(':');
        dirs.push_str(existing);
    }
    (shell, vec![("XDG_DATA_DIRS".to_string(), dirs)])
}

/// PowerShell: dot-source `hyprmux.ps1` after the user's `$PROFILE` has run, and stay interactive.
///
/// Skipped when the configured shell already carries a `-Command`, `-File`, or `-EncodedCommand`
/// argument: those say "run this and exit", so the pane is not the interactive session this is for,
/// and appending a second `-Command` would either be rejected or silently override the user's.
fn inject_powershell(
    shell: ShellCommand,
    install_dir: &Path,
) -> (ShellCommand, Vec<(String, String)>) {
    let already_directed = shell.args.iter().any(|arg| {
        let lowered = arg.to_ascii_lowercase();
        // PowerShell accepts any unambiguous prefix of a parameter name (`-Comm`, `-c`, ...), which
        // is why this matches on a prefix rather than the full spelling.
        ["-c", "-f", "-e", "/c"]
            .iter()
            .any(|prefix| lowered.starts_with(prefix))
    });
    if already_directed {
        return (shell, Vec::new());
    }
    let script = install_dir.join("hyprmux.ps1");
    let command = format!(". {}", powershell_quote(&script.to_string_lossy()));
    let shell = shell.arg("-NoExit").arg("-Command").arg(command);
    (shell, Vec::new())
}

/// cmd.exe: hand the child an instrumented `PROMPT`. Nothing else about the launch changes - no
/// `/K`, no wrapper batch file, and emphatically no `AutoRun` registry key.
fn inject_cmd(shell: ShellCommand) -> (ShellCommand, Vec<(String, String)>) {
    (shell, vec![("PROMPT".to_string(), CMD_PROMPT.to_string())])
}

/// Single-quote a path for embedding in a PowerShell command string, escaping any literal `'` by
/// doubling it (PowerShell's own escape, not a backslash).
fn powershell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

/// Idempotently (re)write every generated script/wrapper this module's injection points reference,
/// returning the shared install directory. Installed once per process (`OnceLock`) since the
/// content is static for a given binary - repeat pane spawns must not repeat this I/O.
fn install_dir() -> Option<PathBuf> {
    static INSTALLED: OnceLock<Option<PathBuf>> = OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            let env = crate::platform::paths::PlatformEnv::from_process();
            let dir = crate::platform::paths::cache_dir(&env).join("shell-integration");
            install_assets(&dir).ok().map(|()| dir)
        })
        .clone()
}

fn install_assets(dir: &Path) -> std::io::Result<()> {
    crate::platform::fs_security::ensure_private_dir(dir)?;
    write_if_changed(&dir.join("hyprmux.bash"), BASH_SCRIPT)?;
    write_if_changed(&dir.join("hyprmux.zsh"), ZSH_SCRIPT)?;
    write_if_changed(&dir.join("hyprmux.fish"), FISH_SCRIPT)?;

    let bash_target = shell_quote(&dir.join("hyprmux.bash").to_string_lossy());
    write_if_changed(
        &dir.join("bash-rcfile"),
        &format!(
            "# Generated by hyprmux; regenerated on every launch, safe to delete.\n\
             [ -f /etc/bash.bashrc ] && . /etc/bash.bashrc\n\
             [ -f \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n\
             . {bash_target}\n"
        ),
    )?;

    let zsh_shim = dir.join("zsh-zdotdir");
    crate::platform::fs_security::ensure_private_dir(&zsh_shim)?;
    let zsh_target = shell_quote(&dir.join("hyprmux.zsh").to_string_lossy());
    write_if_changed(
        &zsh_shim.join(".zshenv"),
        "# Generated by hyprmux; regenerated on every launch, safe to delete.\n\
         if [ -n \"$ROZI_ORIG_ZDOTDIR\" ] && [ -f \"$ROZI_ORIG_ZDOTDIR/.zshenv\" ]; then\n\
         \t. \"$ROZI_ORIG_ZDOTDIR/.zshenv\"\n\
         elif [ -z \"$ROZI_ORIG_ZDOTDIR\" ] && [ -f \"$HOME/.zshenv\" ]; then\n\
         \t. \"$HOME/.zshenv\"\n\
         fi\n",
    )?;
    write_if_changed(
        &zsh_shim.join(".zshrc"),
        &format!(
            "# Generated by hyprmux; regenerated on every launch, safe to delete.\n\
             if [ -n \"$ROZI_ORIG_ZDOTDIR\" ] && [ -f \"$ROZI_ORIG_ZDOTDIR/.zshrc\" ]; then\n\
             \t. \"$ROZI_ORIG_ZDOTDIR/.zshrc\"\n\
             elif [ -z \"$ROZI_ORIG_ZDOTDIR\" ] && [ -f \"$HOME/.zshrc\" ]; then\n\
             \t. \"$HOME/.zshrc\"\n\
             fi\n\
             . {zsh_target}\n"
        ),
    )?;

    let fish_vendor_conf_d = dir.join("fish-vendor").join("fish").join("vendor_conf.d");
    crate::platform::fs_security::ensure_private_dir(&fish_vendor_conf_d)?;
    write_if_changed(&fish_vendor_conf_d.join("hyprmux.fish"), FISH_SCRIPT)?;

    write_if_changed(&dir.join("hyprmux.ps1"), POWERSHELL_SCRIPT)?;

    // cmd's integration is a single `PROMPT` value, which [`inject_cmd`] hands the child directly.
    // This file is that same value in runnable form, for a cmd session hyprmux did not launch (one
    // reached through a `command =` pane, say). Generated rather than shipped as an asset so the
    // string cannot drift from the one actually injected.
    write_if_changed(
        &dir.join("hyprmux.cmd"),
        &format!(
            "@echo off\r\n\
             REM Generated by hyprmux; regenerated on every launch, safe to delete.\r\n\
             REM Adds OSC 9;9 (cwd) and OSC 133 A/B (prompt boundary) markers to cmd's prompt.\r\n\
             REM Command lifecycle (exit status, foreground program) needs Clink; see docs/terminal.md.\r\n\
             set PROMPT={CMD_PROMPT}\r\n"
        ),
    )?;

    Ok(())
}

/// Single-quote a path for embedding in a generated `sh`-family script, escaping any literal `'`
/// (paths containing one are rare but not impossible).
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rozi-shell-integration-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn detects_recognized_shell_kinds_by_basename_only() {
        assert_eq!(detect_shell_kind("/usr/bin/bash"), ShellKind::Bash);
        assert_eq!(detect_shell_kind("zsh"), ShellKind::Zsh);
        assert_eq!(detect_shell_kind("/opt/homebrew/bin/fish"), ShellKind::Fish);
        assert_eq!(detect_shell_kind("/bin/sh"), ShellKind::Other);
        assert_eq!(detect_shell_kind("pwsh.exe"), ShellKind::PowerShell);
        assert_eq!(
            detect_shell_kind(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            ShellKind::PowerShell
        );
        // Windows program names are case-insensitive; `%COMSPEC%` in particular is often uppercase.
        assert_eq!(
            detect_shell_kind(r"C:\Windows\system32\CMD.EXE"),
            ShellKind::Cmd
        );
    }

    #[test]
    fn powershell_gets_a_noexit_dot_source_after_the_users_profile() {
        let shell = ShellCommand::new("pwsh.exe").arg("-NoLogo");
        let (shell, extra_env) =
            inject(shell, ShellIntegrationMode::Auto, &InjectionEnv::default());
        assert_eq!(shell.program, "pwsh.exe");
        // The user's own arguments are preserved, and ours are appended after them.
        assert_eq!(shell.args[0], "-NoLogo");
        assert_eq!(shell.args[1], "-NoExit");
        assert_eq!(shell.args[2], "-Command");
        assert!(shell.args[3].starts_with(". '"));
        assert!(shell.args[3].ends_with("hyprmux.ps1'"));
        assert!(extra_env.is_empty());
    }

    #[test]
    fn powershell_already_told_to_run_something_is_left_untouched() {
        // `-Command`/`-File` mean "run this and exit" - not an interactive session, so there is no
        // prompt to instrument, and a second `-Command` would fight with the user's.
        for directed in ["-Command", "-File", "-EncodedCommand", "-c"] {
            let shell = ShellCommand::new("powershell.exe").arg(directed).arg("x");
            let (result, extra_env) = inject(
                shell.clone(),
                ShellIntegrationMode::Auto,
                &InjectionEnv::default(),
            );
            assert_eq!(result, shell, "{directed} must be left alone");
            assert!(extra_env.is_empty());
        }
    }

    #[test]
    fn cmd_gets_an_instrumented_prompt_and_no_argv_change() {
        let shell = ShellCommand::new(r"C:\Windows\system32\cmd.exe");
        let (result, extra_env) = inject(
            shell.clone(),
            ShellIntegrationMode::Auto,
            &InjectionEnv::default(),
        );
        assert_eq!(result, shell, "cmd's argv must not be touched");
        let prompt = extra_env
            .iter()
            .find(|(key, _)| key == "PROMPT")
            .map(|(_, value)| value.as_str())
            .expect("PROMPT set");
        // The cwd report, both prompt boundaries, and cmd's own stock prompt text.
        assert!(prompt.contains("]9;9;$P"));
        assert!(prompt.contains("]133;A"));
        assert!(prompt.contains("]133;B"));
        assert!(prompt.contains("$P$G"));
    }

    #[test]
    fn powershell_quoting_escapes_an_apostrophe_by_doubling_it() {
        assert_eq!(powershell_quote(r"C:\it's\here"), r"'C:\it''s\here'");
    }

    #[test]
    fn off_mode_never_touches_argv_or_env() {
        let shell = ShellCommand::new("bash");
        let (shell, extra_env) = inject(
            shell.clone(),
            ShellIntegrationMode::Off,
            &InjectionEnv::default(),
        );
        assert_eq!(shell, ShellCommand::new("bash"));
        assert!(extra_env.is_empty());
    }

    #[test]
    fn bash_login_shell_is_left_untouched() {
        let shell = ShellCommand::new("bash").arg("--login");
        let (shell, extra_env) = inject(
            shell.clone(),
            ShellIntegrationMode::Auto,
            &InjectionEnv::default(),
        );
        assert_eq!(shell, ShellCommand::new("bash").arg("--login"));
        assert!(extra_env.is_empty());
    }

    #[test]
    fn bash_non_login_shell_gets_an_rcfile_flag() {
        let shell = ShellCommand::new("bash");
        let (shell, extra_env) =
            inject(shell, ShellIntegrationMode::Auto, &InjectionEnv::default());
        assert_eq!(shell.program, "bash");
        assert_eq!(shell.args.first().map(String::as_str), Some("--rcfile"));
        assert!(
            shell
                .args
                .get(1)
                .is_some_and(|arg| arg.ends_with("bash-rcfile"))
        );
        assert!(extra_env.is_empty());
    }

    #[test]
    fn zsh_gets_zdotdir_pointed_at_the_shim_and_preserves_the_original() {
        let shell = ShellCommand::new("zsh");
        let env = InjectionEnv {
            zdotdir: Some("/home/user/.config/zsh".to_string()),
            ..InjectionEnv::default()
        };
        let (shell, extra_env) = inject(shell, ShellIntegrationMode::Auto, &env);
        assert_eq!(shell, ShellCommand::new("zsh"));
        let zdotdir = extra_env
            .iter()
            .find(|(key, _)| key == "ZDOTDIR")
            .map(|(_, value)| value.clone());
        assert!(zdotdir.is_some_and(|value| value.ends_with("zsh-zdotdir")));
        assert_eq!(
            extra_env
                .iter()
                .find(|(key, _)| key == "ROZI_ORIG_ZDOTDIR")
                .map(|(_, value)| value.clone()),
            Some("/home/user/.config/zsh".to_string())
        );
    }

    #[test]
    fn fish_prepends_the_vendor_dir_to_existing_xdg_data_dirs() {
        let shell = ShellCommand::new("fish");
        let env = InjectionEnv {
            xdg_data_dirs: Some("/usr/share:/usr/local/share".to_string()),
            ..InjectionEnv::default()
        };
        let (shell, extra_env) = inject(shell, ShellIntegrationMode::Auto, &env);
        assert_eq!(shell, ShellCommand::new("fish"));
        let dirs = extra_env
            .iter()
            .find(|(key, _)| key == "XDG_DATA_DIRS")
            .map(|(_, value)| value.clone())
            .expect("XDG_DATA_DIRS set");
        assert!(dirs.ends_with("fish-vendor:/usr/share:/usr/local/share"));
    }

    #[test]
    fn already_loaded_short_circuits_regardless_of_mode() {
        let shell = ShellCommand::new("zsh");
        let env = InjectionEnv {
            already_loaded: true,
            ..InjectionEnv::default()
        };
        let (shell, extra_env) = inject(shell, ShellIntegrationMode::Auto, &env);
        assert_eq!(shell, ShellCommand::new("zsh"));
        assert!(extra_env.is_empty());
    }

    #[test]
    fn install_assets_writes_every_expected_file_and_is_idempotent() {
        let dir = temp_dir("install");
        install_assets(&dir).expect("install");
        for relative in [
            "hyprmux.bash",
            "hyprmux.zsh",
            "hyprmux.fish",
            "hyprmux.ps1",
            "hyprmux.cmd",
            "bash-rcfile",
            "zsh-zdotdir/.zshenv",
            "zsh-zdotdir/.zshrc",
            "fish-vendor/fish/vendor_conf.d/hyprmux.fish",
        ] {
            assert!(dir.join(relative).is_file(), "missing {relative}");
        }
        let bash_script_before = std::fs::read_to_string(dir.join("hyprmux.bash")).unwrap();
        // Re-running must not error and must reproduce identical content (write-if-changed).
        install_assets(&dir).expect("reinstall");
        let bash_script_after = std::fs::read_to_string(dir.join("hyprmux.bash")).unwrap();
        assert_eq!(bash_script_before, bash_script_after);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_bash_rcfile_chains_real_bashrc_then_the_hyprmux_script() {
        let dir = temp_dir("rcfile-content");
        install_assets(&dir).expect("install");
        let rcfile = std::fs::read_to_string(dir.join("bash-rcfile")).unwrap();
        assert!(rcfile.contains(".bashrc"));
        assert!(rcfile.contains("hyprmux.bash"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_zsh_shim_falls_through_to_the_real_zdotdir_when_set() {
        let dir = temp_dir("zsh-content");
        install_assets(&dir).expect("install");
        let zshrc = std::fs::read_to_string(dir.join("zsh-zdotdir").join(".zshrc")).unwrap();
        assert!(zshrc.contains("ROZI_ORIG_ZDOTDIR"));
        assert!(zshrc.contains("hyprmux.zsh"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
