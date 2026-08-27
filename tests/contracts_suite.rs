#[path = "suites/contracts/cli_startup.rs"]
mod cli_startup;
#[path = "suites/contracts/extensions_cli.rs"]
mod extensions_cli;
#[path = "suites/contracts/extensions_conformance.rs"]
mod extensions_conformance;
#[path = "suites/contracts/extensions_smoke.rs"]
mod extensions_smoke;
#[path = "suites/contracts/no_sleeping_pool_tasks.rs"]
mod no_sleeping_pool_tasks;
// The bootstrap script's own contracts. Windows-only: it drives `install.ps1` through PowerShell,
// and the file it tests is the one that ships.
#[cfg(windows)]
#[path = "suites/contracts/install_script.rs"]
mod install_script;
