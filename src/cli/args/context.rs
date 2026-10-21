use super::*;

pub(super) fn parse(
    iter: &mut impl Iterator<Item = String>,
    cli: CliArgs,
    socket: Option<PathBuf>,
) -> Result<ParsedCli, String> {
    let mut output_format = None;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--format" => {
                let value = require_value(iter, "--format requires text or json")?;
                if output_format
                    .replace(parse_list_format(&value, "context")?)
                    .is_some()
                {
                    return Err("context --format specified more than once".into());
                }
            }
            other => return Err(format!("unexpected context argument `{other}`")),
        }
    }
    let command = control::ControlCommand::Context;
    control_endpoint(&cli, socket.clone(), &command)?;
    if cli.attach_session.is_some() || cli.remote.is_some() || socket.is_some() {
        return Err(
            "context describes the caller; --session, --remote and --socket do not apply".into(),
        );
    }
    Ok(ParsedCli::Control(ControlCli {
        config_path: cli.config_path,
        endpoint: ControlEndpoint::CallerContext,
        request: control_request(command),
        output_format,
        output: None,
    }))
}
