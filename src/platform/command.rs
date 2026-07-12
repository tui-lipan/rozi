//! Shell and command-runner resolution (cross-platform plan Phase 4).
//!
//! Two separate resolved launch policies, per the plan:
//!
//! - [`resolve_interactive_shell`] - the persistent shell a plain pane runs. Resolution order:
//!   configured `shell` -> Unix/macOS `$SHELL` -> `/bin/sh`; Windows `pwsh.exe` ->
//!   `powershell.exe` -> `%COMSPEC%` -> `cmd.exe`.
//! - [`resolve_command_shell`] - the shell used to run a one-off command line (pane/popup
//!   commands, hooks, workbar `command:` segments, `[keys] run`, profile commands, control-socket
//!   run requests). Deterministic and **never** detection-based, so a `command_shell`-invoked
//!   config snippet behaves identically on every machine: Linux/macOS `["/bin/sh", "-c"]`;
//!   Windows `[%COMSPEC%, "/D", "/S", "/C"]`.
//!
//! Both accept an argument-preserving `Vec<String>` (first element is the program, the rest are
//! fixed leading arguments) rather than a single string, so `shell = ["pwsh.exe", "-NoLogo"]`
//! round-trips exactly; the historical bare-string config form is normalized to a one-element
//! vector by the config loader (`config::file`), not here.
//!
//! Callers resolve these client-side (the controlling client has the live, possibly hot-reloaded
//! config) and send the resolved argv across the wire in `ClientMessage::SpawnPane`, so a
//! detached server never falls back to its own process environment or a stale on-disk config.

/// A resolved, ready-to-spawn shell or command-runner: `program` plus fixed leading `args`.
///
/// Wire representation is a single non-empty `Vec<String>` (`program` followed by `args`); see
/// [`ShellCommand::as_argv`] / [`ShellCommand::from_argv`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ShellCommand {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Flatten to the wire/argv form: `[program, ...args]`.
    pub fn as_argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(1 + self.args.len());
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }

    /// Parse the wire/argv form back into a [`ShellCommand`]. `None` for an empty argv (the
    /// caller should treat that as "not configured" and fall through to a resolver default rather
    /// than spawning a program with an empty name).
    pub fn from_argv(argv: &[String]) -> Option<Self> {
        let (program, args) = argv.split_first()?;
        if program.is_empty() {
            return None;
        }
        Some(Self {
            program: program.clone(),
            args: args.to_vec(),
        })
    }
}

/// Environment inputs needed to resolve shell launch policies, injectable for testability (mirrors
/// [`super::paths::PlatformEnv`]) rather than reading `std::env` directly inside the resolvers.
#[derive(Clone, Debug, Default)]
pub struct ShellEnv {
    /// `$SHELL` (Unix/macOS only), if set to a non-empty value.
    pub shell_var: Option<String>,
    /// `%COMSPEC%` (Windows only), if set to a non-empty value.
    pub comspec: Option<String>,
    /// Directories to search for a program on Windows, in `PATH` order.
    pub windows_path_dirs: Vec<std::path::PathBuf>,
    /// `%PATHEXT%` split into extensions (`.COM`, `.EXE`, `.BAT`, ...), in precedence order. Empty
    /// on Unix, which has no such concept.
    pub windows_path_exts: Vec<String>,
}

impl ShellEnv {
    pub fn from_process() -> Self {
        Self {
            shell_var: non_empty_env("SHELL"),
            comspec: non_empty_env("COMSPEC"),
            windows_path_dirs: std::env::var_os("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default(),
            windows_path_exts: windows_path_exts(),
        }
    }
}

/// `%PATHEXT%`, split and normalized. Falls back to the documented Windows default when the
/// variable is unset - which happens more often than one would like (a bare `CreateProcess`
/// environment, a service, a CI shell), and a `PATHEXT`-less lookup would find nothing at all.
fn windows_path_exts() -> Vec<String> {
    const DEFAULT: &str = ".COM;.EXE;.BAT;.CMD";
    if !cfg!(windows) {
        return Vec::new();
    }
    non_empty_env("PATHEXT")
        .unwrap_or_else(|| DEFAULT.to_string())
        .split(';')
        .map(|ext| ext.trim().to_string())
        .filter(|ext| ext.starts_with('.'))
        .collect()
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// Find `program` on the current process's `PATH`, honoring `%PATHEXT%` (cross-platform plan
/// Phase 10, "Windows `PATH`/`PATHEXT` command lookup").
///
/// Returns the resolved absolute path, or `None` if nothing matches. A `program` that already
/// contains a separator is treated as a path, not a name to search for - the same rule the OS's own
/// loader applies.
///
/// Windows-only: on Unix, `Command::new` resolves a bare program name against `PATH` itself, so
/// there is nothing for a caller here to do that the OS does not already do correctly.
#[cfg(windows)]
pub fn lookup_program(program: &str) -> Option<std::path::PathBuf> {
    lookup_program_in(program, &ShellEnv::from_process())
}

fn lookup_program_in(program: &str, env: &ShellEnv) -> Option<std::path::PathBuf> {
    if program.contains('/') || (cfg!(windows) && program.contains('\\')) {
        let path = std::path::PathBuf::from(program);
        return path.is_file().then_some(path);
    }
    for dir in &env.windows_path_dirs {
        // An explicit extension (`pwsh.exe`) is tried as given before any PATHEXT expansion, so a
        // caller that already knows the exact file name never pays for a fruitless `.COM` probe -
        // and, on Unix, so the extensionless name is the only thing tried at all.
        let exact = dir.join(program);
        if exact.is_file() {
            return Some(exact);
        }
        for ext in &env.windows_path_exts {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether `program` names an executable available to the current process.
pub fn program_exists(program: &str) -> bool {
    #[cfg(windows)]
    {
        lookup_program(program).is_some()
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let executable = |path: &std::path::Path| {
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        };
        if program.contains('/') {
            return executable(std::path::Path::new(program));
        }
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|dir| executable(&dir.join(program)))
        })
    }
}

/// Resolve the interactive-shell launch policy. `configured` is the user's `shell` config value
/// (already normalized to argv form by `config::file`); empty is treated as "not configured".
pub fn resolve_interactive_shell(configured: Option<&[String]>, env: &ShellEnv) -> ShellCommand {
    if let Some(command) = configured.and_then(ShellCommand::from_argv) {
        return command;
    }
    if cfg!(windows) {
        windows_interactive_shell(env)
    } else {
        ShellCommand::new(
            env.shell_var
                .clone()
                .unwrap_or_else(|| "/bin/sh".to_string()),
        )
    }
}

fn windows_interactive_shell(env: &ShellEnv) -> ShellCommand {
    for candidate in ["pwsh.exe", "powershell.exe"] {
        if lookup_program_in(candidate, env).is_some() {
            return ShellCommand::new(candidate).arg("-NoLogo");
        }
    }
    ShellCommand::new(env.comspec.clone().unwrap_or_else(|| "cmd.exe".to_string()))
}

/// Resolve the command-runner launch policy: the shell used to run one-off command lines
/// (pane/popup commands, hooks, workbar `command:` segments, `[keys] run`, profile commands,
/// control-socket run requests). Never probes the environment for "the best" shell - only the
/// user's explicit `command_shell` override (if any) or the fixed per-platform default - so a
/// config snippet using it behaves identically on every machine.
pub fn resolve_command_shell(configured: Option<&[String]>, env: &ShellEnv) -> ShellCommand {
    if let Some(command) = configured.and_then(ShellCommand::from_argv) {
        return command;
    }
    if cfg!(windows) {
        windows_command_shell(env)
    } else {
        ShellCommand::new("/bin/sh").arg("-c")
    }
}

fn windows_command_shell(env: &ShellEnv) -> ShellCommand {
    ShellCommand::new(env.comspec.clone().unwrap_or_else(|| "cmd.exe".to_string()))
        .arg("/D")
        .arg("/S")
        .arg("/C")
}

/// Resolve both launch policies at once, in the flattened argv form the wire protocol carries
/// (see `ClientMessage::SpawnPane`). Convenience wrapper over [`resolve_interactive_shell`] and
/// [`resolve_command_shell`] for call sites that only need the argv, not the [`ShellCommand`].
pub fn resolve_launch_argv(
    shell: Option<&[String]>,
    command_shell: Option<&[String]>,
    env: &ShellEnv,
) -> (Vec<String>, Vec<String>) {
    (
        resolve_interactive_shell(shell, env).as_argv(),
        resolve_command_shell(command_shell, env).as_argv(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn shell_command_round_trips_through_argv() {
        let command = ShellCommand::new("pwsh.exe").arg("-NoLogo");
        assert_eq!(command.as_argv(), vec!["pwsh.exe", "-NoLogo"]);
        assert_eq!(ShellCommand::from_argv(&command.as_argv()), Some(command));
    }

    #[test]
    fn from_argv_rejects_empty_argv_and_empty_program() {
        assert_eq!(ShellCommand::from_argv(&[]), None);
        assert_eq!(ShellCommand::from_argv(&argv(&[""])), None);
    }

    #[cfg(unix)]
    #[test]
    fn program_exists_requires_an_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "hyprmux-command-test-executable-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("editor");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        assert!(!program_exists(&path.to_string_lossy()));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(program_exists(&path.to_string_lossy()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn interactive_shell_prefers_configured_value_over_everything() {
        let env = ShellEnv {
            shell_var: Some("/bin/zsh".to_string()),
            ..ShellEnv::default()
        };
        let configured = argv(&["fish", "--login"]);
        assert_eq!(
            resolve_interactive_shell(Some(&configured), &env),
            ShellCommand::new("fish").arg("--login")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn interactive_shell_falls_back_to_shell_env_var_then_bin_sh() {
        let env = ShellEnv {
            shell_var: Some("/bin/zsh".to_string()),
            ..ShellEnv::default()
        };
        assert_eq!(
            resolve_interactive_shell(None, &env),
            ShellCommand::new("/bin/zsh")
        );
        assert_eq!(
            resolve_interactive_shell(None, &ShellEnv::default()),
            ShellCommand::new("/bin/sh")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn interactive_shell_ignores_empty_configured_argv() {
        let env = ShellEnv {
            shell_var: Some("/bin/zsh".to_string()),
            ..ShellEnv::default()
        };
        assert_eq!(
            resolve_interactive_shell(Some(&[]), &env),
            ShellCommand::new("/bin/zsh")
        );
    }

    #[test]
    fn lookup_program_expands_pathext_and_prefers_an_exact_name() {
        let dir = std::env::temp_dir().join(format!(
            "hyprmux-command-test-pathext-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tool.CMD"), b"").unwrap();
        std::fs::write(dir.join("tool.EXE"), b"").unwrap();

        let env = ShellEnv {
            windows_path_dirs: vec![dir.clone()],
            windows_path_exts: vec![".EXE".to_string(), ".CMD".to_string()],
            ..ShellEnv::default()
        };

        // PATHEXT order decides between two candidates that both exist.
        assert_eq!(lookup_program_in("tool", &env), Some(dir.join("tool.EXE")));
        // A name that already carries its extension is taken as-is, never re-expanded.
        assert_eq!(
            lookup_program_in("tool.CMD", &env),
            Some(dir.join("tool.CMD"))
        );
        assert_eq!(lookup_program_in("absent", &env), None);

        // Something that is already a path is checked, not searched for.
        let exact = dir.join("tool.EXE");
        assert_eq!(
            lookup_program_in(&exact.to_string_lossy(), &env),
            Some(exact)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_interactive_shell_prefers_pwsh_then_powershell_then_comspec_then_cmd() {
        let dir = std::env::temp_dir().join(format!(
            "hyprmux-command-test-windows-shell-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("powershell.exe"), b"").unwrap();
        std::fs::write(dir.join("pwsh.exe"), b"").unwrap();

        let mut env = ShellEnv {
            comspec: Some(r"C:\Windows\system32\cmd.exe".to_string()),
            windows_path_dirs: vec![dir.clone()],
            ..ShellEnv::default()
        };
        assert_eq!(
            windows_interactive_shell(&env),
            ShellCommand::new("pwsh.exe").arg("-NoLogo")
        );

        std::fs::remove_file(dir.join("pwsh.exe")).unwrap();
        assert_eq!(
            windows_interactive_shell(&env),
            ShellCommand::new("powershell.exe").arg("-NoLogo")
        );

        env.windows_path_dirs.clear();
        assert_eq!(
            windows_interactive_shell(&env),
            ShellCommand::new(r"C:\Windows\system32\cmd.exe")
        );

        env.comspec = None;
        assert_eq!(
            windows_interactive_shell(&env),
            ShellCommand::new("cmd.exe")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_shell_prefers_configured_value() {
        let configured = argv(&["bash", "-c"]);
        assert_eq!(
            resolve_command_shell(Some(&configured), &ShellEnv::default()),
            ShellCommand::new("bash").arg("-c")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn command_shell_default_is_deterministic_bin_sh_and_ignores_shell_env_var() {
        // Unlike resolve_interactive_shell, the command runner never reads $SHELL - it must
        // behave identically regardless of the invoking user's interactive shell choice.
        let env = ShellEnv {
            shell_var: Some("/bin/fish".to_string()),
            ..ShellEnv::default()
        };
        assert_eq!(
            resolve_command_shell(None, &env),
            ShellCommand::new("/bin/sh").arg("-c")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn resolve_launch_argv_resolves_both_policies_independently() {
        let env = ShellEnv {
            shell_var: Some("/bin/fish".to_string()),
            ..ShellEnv::default()
        };
        let (shell, command_shell) = resolve_launch_argv(None, None, &env);
        assert_eq!(shell, vec!["/bin/fish".to_string()]);
        assert_eq!(command_shell, vec!["/bin/sh".to_string(), "-c".to_string()]);
    }

    #[test]
    fn command_shell_windows_default_uses_comspec_with_fixed_flags() {
        // `windows_command_shell` has no OS-specific dependency (just string building), so it is
        // exercised directly here regardless of host OS; only the `cfg!(windows)` dispatch in
        // `resolve_command_shell`/`resolve_interactive_shell` is unverified on this Linux host.
        let env = ShellEnv {
            comspec: Some(r"C:\Windows\system32\cmd.exe".to_string()),
            ..ShellEnv::default()
        };
        assert_eq!(
            windows_command_shell(&env),
            ShellCommand::new(r"C:\Windows\system32\cmd.exe")
                .arg("/D")
                .arg("/S")
                .arg("/C")
        );
        assert_eq!(
            windows_command_shell(&ShellEnv::default()),
            ShellCommand::new("cmd.exe").arg("/D").arg("/S").arg("/C")
        );
    }
}
