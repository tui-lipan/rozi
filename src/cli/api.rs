use std::io::{self, Write};

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

/// Print the JSON Schema of the control API this binary implements.
pub(crate) fn run_api_schema_cli() -> Result<()> {
    write_ignoring_closed_pipe(
        &mut io::stdout().lock(),
        crate::control::API_SCHEMA_DOCUMENT,
    )?;
    Ok(())
}

/// The document is far larger than a pipe's buffer, so a reader such as `head` that stops early is
/// an ordinary way for this to end rather than a failure.
fn write_ignoring_closed_pipe(out: &mut impl Write, text: &str) -> io::Result<()> {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}
