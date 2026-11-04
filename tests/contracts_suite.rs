#[path = "suites/contracts/cli_startup.rs"]
mod cli_startup;
#[path = "suites/contracts/domain_floor.rs"]
mod domain_floor;
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
#[path = "suites/contracts/pick_stream.rs"]
mod pick_stream;
// Shared source reading for the contracts that scan crate sources.
#[path = "suites/contracts/source_scan.rs"]
mod source_scan;
