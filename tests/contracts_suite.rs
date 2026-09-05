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
// The bootstrap scripts' own contracts. Unix drives `install.sh`; Windows drives `install.ps1`.
#[path = "suites/contracts/install_script.rs"]
mod install_script;
