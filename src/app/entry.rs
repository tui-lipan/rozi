use std::time::Duration;

use tui_lipan::prelude::*;

use crate::config::Config;
use crate::{cli, config, control, ops, platform};

use super::{AppRoot, startup::StartupPlan};

fn clipboard_config(config: &Config) -> ClipboardConfig {
    // OSC52 always targets the *local* terminal emulator that hosts this client. Under `--remote`
    // that is what we want: copy from a remote pane reaches the local clipboard. Disabling
    // `enable_osc52` drops OSC52 without redirecting copies to the remote host.
    ClipboardConfig {
        enable_osc52: config.clipboard.enable_osc52,
        ..ClipboardConfig::default()
    }
}

pub(crate) fn clipboard_copy_feedback_duration(config: &Config) -> Duration {
    Duration::from_millis(clipboard_config(config).copy_feedback_duration_ms as u64)
}

/// Point config loading at `--config <PATH>` before any command reads it.
///
/// Every entry point that loads config goes through here, so a server, a remote `sessions list`,
/// and the UI all honour the same flag rather than the UI alone.
fn apply_config_path(path: Option<String>) {
    if let Some(path) = path {
        unsafe {
            std::env::set_var("ROZI_CONFIG", path);
        }
    }
}

pub fn run() -> Result<()> {
    // Ahead of argument parsing: `ssh` re-executes this binary as its askpass helper with a prompt
    // where rozi expects a subcommand. The environment it was handed says so unambiguously, and
    // only `session::remote::askpass::configure` ever sets it.
    if let Some(helper) = crate::session::remote::askpass::helper_invocation() {
        helper.run();
    }

    let parsed = match cli::parse_cli_args(std::env::args().skip(1).collect()) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("Run `rozi --help` for usage.");
            std::process::exit(1);
        }
    };

    // Pure extension diagnostics, namespace help, and the built-in skill do not need a runnable
    // terminal host or a healthy managed binary. Keeping them first makes them useful while
    // repairing either.
    let parsed = match parsed {
        cli::ParsedCli::Extensions(command) => match command {
            cli::ExtensionsCommand::List {
                json,
                verbose,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_list_extensions_cli(json, verbose);
            }
            cli::ExtensionsCommand::New { id } => {
                return cli::run_new_extension_cli(&id);
            }
            cli::ExtensionsCommand::Check { path, json } => {
                if !cli::run_check_extension_cli(&path, json)? {
                    std::process::exit(1);
                }
                return Ok(());
            }
            cli::ExtensionsCommand::Install {
                source,
                link,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_install_extension_cli(&source, link);
            }
            cli::ExtensionsCommand::Remove { id, config_path } => {
                apply_config_path(config_path);
                return cli::run_remove_extension_cli(&id);
            }
            cli::ExtensionsCommand::Update { id, config_path } => {
                apply_config_path(config_path);
                return cli::run_update_extension_cli(&id);
            }
        },
        cli::ParsedCli::ExtensionsHelp => {
            cli::print_extensions_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsCheckHelp => {
            cli::print_extensions_check_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsInstallHelp => {
            cli::print_extensions_install_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsRemoveHelp => {
            cli::print_extensions_remove_help();
            return Ok(());
        }
        cli::ParsedCli::ExtensionsUpdateHelp => {
            cli::print_extensions_update_help();
            return Ok(());
        }
        cli::ParsedCli::SessionsHelp => {
            cli::print_sessions_help();
            return Ok(());
        }
        cli::ParsedCli::AgentsHelp => {
            cli::print_agents_help();
            return Ok(());
        }
        cli::ParsedCli::SkillHelp => {
            cli::print_skill_help();
            return Ok(());
        }
        cli::ParsedCli::Skill(command) => {
            if let Err(message) = cli::run_skill_cli(command) {
                eprintln!("rozi: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        // Ahead of recovery on purpose. Both print and exit without consulting the managed layout,
        // and they are what someone runs to work out why an installation is unhappy - a misconfigured
        // one must not be able to take them down with it.
        cli::ParsedCli::Help { advanced } => {
            cli::print_help(advanced);
            return Ok(());
        }
        cli::ParsedCli::Version => {
            cli::print_version();
            return Ok(());
        }
        cli::ParsedCli::ApiDescribe => return cli::run_api_describe_cli(),
        parsed => parsed,
    };

    // Control clients only bridge one request to an already-running UI. Managed-installation
    // recovery verifies retained binaries and can take hundreds of milliseconds; doing that before
    // every `run-action` makes editor edge navigation visibly stall without making the request any
    // safer. The serving UI already passed recovery when it started.
    let parsed = match parsed {
        cli::ParsedCli::Control(command) => return cli::run_control_cli(command),
        cli::ParsedCli::Publish(command) => return cli::run_publish_cli(command),
        cli::ParsedCli::Subscribe(command) => return cli::run_subscribe_cli(command),
        cli::ParsedCli::Pick(command) => return cli::run_pick_cli(command),
        parsed => parsed,
    };

    // Reconcile a managed pointer before commands can update or start the application. This keeps
    // updater recovery ahead of ConPTY checks, endpoints, sessions, and the TUI.
    if let Err(message) = cli::recover_managed_installation() {
        eprintln!("rozi: {message}");
        std::process::exit(1);
    }

    let parsed = match parsed {
        cli::ParsedCli::Install => {
            if let Err(message) = cli::run_install_cli() {
                eprintln!("rozi: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        cli::ParsedCli::Update(command) => {
            if let Err(message) = cli::run_update_cli(command) {
                eprintln!("rozi: {message}");
                std::process::exit(1);
            }
            return Ok(());
        }
        parsed => parsed,
    };

    // Runtime/session commands receive the host support check. Pure installation and extension
    // diagnostics above intentionally remain usable on a host that cannot launch the TUI.
    if let Err(reason) = platform::server_lifecycle::check_host_supported() {
        eprintln!("rozi: {reason}");
        std::process::exit(1);
    }

    let cli = match parsed {
        cli::ParsedCli::Server {
            name,
            fresh,
            config_path,
            startup_nonce,
        } => {
            apply_config_path(config_path);
            return cli::run_server_cli(&name, fresh, startup_nonce);
        }
        cli::ParsedCli::RemoteServe { name, autostart } => {
            return cli::run_remote_serve_cli(&name, autostart);
        }
        cli::ParsedCli::RemoteControl { name } => {
            return cli::run_remote_control_cli(&name);
        }
        cli::ParsedCli::Sessions(command) => match command {
            cli::SessionsCommand::Watch { config_path } => {
                apply_config_path(config_path);
                return crate::session::remote::monitor::serve(
                    &mut std::io::stdin().lock(),
                    &mut std::io::stdout().lock(),
                )
                .map_err(Into::into);
            }
            cli::SessionsCommand::List {
                format,
                remote,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_list_sessions_cli(format, remote.as_deref());
            }
            cli::SessionsCommand::Kill {
                name,
                remote,
                config_path,
            } => {
                apply_config_path(config_path);
                return cli::run_kill_session_cli(&name, remote.as_deref());
            }
        },
        cli::ParsedCli::Run(args) => args,
        cli::ParsedCli::Control(_)
        | cli::ParsedCli::Publish(_)
        | cli::ParsedCli::Subscribe(_)
        | cli::ParsedCli::Pick(_)
        | cli::ParsedCli::Help { .. }
        | cli::ParsedCli::Version
        | cli::ParsedCli::ApiDescribe
        | cli::ParsedCli::Skill(_)
        | cli::ParsedCli::SkillHelp
        | cli::ParsedCli::SessionsHelp
        | cli::ParsedCli::AgentsHelp
        | cli::ParsedCli::Extensions(_)
        | cli::ParsedCli::ExtensionsHelp
        | cli::ParsedCli::ExtensionsCheckHelp
        | cli::ParsedCli::ExtensionsInstallHelp
        | cli::ParsedCli::ExtensionsRemoveHelp
        | cli::ParsedCli::ExtensionsUpdateHelp
        | cli::ParsedCli::Install
        | cli::ParsedCli::Update(_) => unreachable!("early CLI command returned above"),
    };

    apply_config_path(cli.config_path.clone());

    let mut plan = StartupPlan::resolve(&cli, config::load_config());
    let startup_host_colors = query_host_colors();
    let terminal_bg = startup_host_colors.map(|colors| colors.bg);
    let startup_system_theme = startup_host_colors.map(ops::theme::system_theme_from_host_colors);
    let resolved_theme =
        config::resolve_theme(&plan.config.theme.name, startup_system_theme.as_ref());
    plan.messages.extend(resolved_theme.warnings);
    let theme = ops::theme::apply_backdrop_policy(
        resolved_theme.theme,
        terminal_bg,
        plan.config.pane.background_follows_terminal,
    );

    let (control_listener, control_guard) = match control::bind_control_socket() {
        Ok((listener, guard)) => (Some(listener), Some(guard)),
        Err(err) => {
            plan.messages
                .push(format!("Control socket unavailable: {err}"));
            (None, None)
        }
    };

    let app = App::new()
        .title("rozi")
        .theme(theme.clone())
        .terminal_bg(terminal_bg)
        .live_host_terminal_colors(true)
        .toast_placement(ToastPlacement::BottomEnd)
        .toast_margin((1, 2, 1, 1))
        .clipboard_config(clipboard_config(&plan.config))
        // Read once at startup: the runner turns this into its poll interval, so a live config
        // reload cannot move it. Detaching and reattaching picks up a new value.
        .frame_rate(plan.config.frame_rate)
        .mouse(true)
        // Leader chords (`ctrl-a c`) and WM-modifier chords (`alt-c`) are executable command
        // shortcuts (see `commands/`), not a framework keymap file - resolve them ahead of
        // focused widgets/terminal passthrough so they win regardless of what has focus.
        .key_dispatch_policy(KeyDispatchPolicy::AppCommandsFirst)
        .terminal_key_policy(TerminalKeyPolicy::AppCommandsThenTerminal)
        // Pressing the prefix is an explicit entry into rozi's command state, so the next key
        // belongs to rozi whatever it turns out to be: it runs a binding, cancels with Esc, is
        // forwarded by the explicit `<prefix> <prefix>` command, or - being unbound - does nothing.
        // The default policy instead replays an unbound key into the pane, which makes a mistyped
        // chord type a stray character into the shell.
        .chord_mismatch_policy(ChordMismatchPolicy::CancelOnly)
        // How long the prefix is held before the which-key strip appears. Only that strip reads the
        // delayed signal; the PREFIX badge, the withheld caret, and prefix mouse gestures all stay
        // on the instant `command_chord_pending`.
        .command_chord_reveal_delay(plan.config.input.which_key.reveal_delay())
        // Ctrl-q is unbound: rozi's own `quit`/`detach` commands own client lifecycle exits.
        .global_quit(None);

    // Probe / prompt / install before the TUI takes stdin (install = "prompt").
    if let Some(ref target) = plan.remote {
        let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if let Err(err) =
            crate::session::remote::ensure_remote_binary(target, &plan.config.remote, interactive)
        {
            eprintln!("rozi: {err}");
            std::process::exit(1);
        }
    }

    let outcome = app
        .mount(AppRoot::new(
            plan.config,
            theme,
            startup_system_theme,
            plan.profile,
            plan.messages,
            control_listener,
            control_guard,
            plan.attach_session,
            plan.autostart,
            plan.create_only,
            cli.read_only,
            plan.remote,
            plan.want_picker,
            plan.last_session,
        ))
        .exit_view(crate::view::exit::exit_view)
        .run();
    // The control socket has a guard the app owns; the askpass endpoint is reached from worker
    // threads with no such owner, so it is retired here.
    crate::session::remote::askpass::shutdown();
    outcome
}
