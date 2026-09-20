use tui_lipan::Result;

/// Print the control API contract implemented by this binary.
pub(crate) fn run_api_describe_cli() -> Result<()> {
    let description = crate::control::ApiDescription::current();
    println!(
        "{}",
        serde_json::to_string_pretty(&description).expect("API description serializes")
    );
    Ok(())
}
