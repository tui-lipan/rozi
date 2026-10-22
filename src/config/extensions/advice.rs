//! The fixes `rozi extensions check` suggests beside its report.
//!
//! A hint never changes whether an extension loads: some name a mistake that already made it
//! invalid, and others a manifest that loads but will not do what its author meant.

use std::path::Path;

use super::{EXTENSION_API_VERSION, ExtensionInfo, ExtensionManifestFile};

pub(crate) fn authoring_hints(info: &ExtensionInfo) -> Vec<String> {
    let mut hints = Vec::new();
    let Ok(text) = std::fs::read_to_string(&info.manifest_path) else {
        return hints;
    };
    let directory = Path::new(&info.path);
    if let Ok(value) = toml::from_str::<toml::Value>(&text) {
        manifest_hints(&value, directory, &mut hints);
    }
    if let Err(error) = toml::from_str::<ExtensionManifestFile>(&text)
        && let Some(hint) = unknown_key_hint(error.message())
    {
        hints.push(hint);
    }
    for error in &info.errors {
        if let Some(hint) = error_hint(error, directory) {
            hints.push(hint);
        }
    }
    let mut seen = std::collections::HashSet::new();
    hints.retain(|hint| seen.insert(hint.clone()));
    hints
}

fn manifest_hints(value: &toml::Value, directory: &Path, hints: &mut Vec<String>) {
    match value.get("extension") {
        None => hints.push(
            "Start the manifest with an `[extension]` table that names `id` and `api`; \
             `rozi extensions new <id>` writes one."
                .to_string(),
        ),
        Some(extension) => {
            if extension.get("api").is_none() {
                hints.push(format!(
                    "Add `api = {EXTENSION_API_VERSION}` to `[extension]`."
                ));
            }
            if let Some(id) = extension.get("id").and_then(toml::Value::as_str)
                && let Some(suggestion) = suggested_id(id)
            {
                hints.push(format!(
                    "An extension ID uses lowercase letters, digits, `-`, and `_`: try \
                     `id = \"{suggestion}\"`."
                ));
            }
        }
    }
    if value.get("extensions").is_some() {
        hints.push(
            "`[extensions]` belongs in Rozi's config.toml. An extension declares what its users \
             may configure under `[settings]`."
                .to_string(),
        );
    }
    for (kind, tables) in ["commands", "services"]
        .into_iter()
        .filter_map(|kind| Some((kind, value.get(kind)?.as_array()?)))
    {
        for (index, table) in tables.iter().enumerate() {
            let name = table
                .get(if kind == "commands" { "id" } else { "name" })
                .and_then(toml::Value::as_str)
                .map_or_else(|| format!("#{}", index + 1), |name| format!("`{name}`"));
            let noun = if kind == "commands" {
                "command"
            } else {
                "service"
            };
            if let Some(line) = table.get("exec").and_then(toml::Value::as_str) {
                let argv = line
                    .split_whitespace()
                    .map(|word| {
                        let word =
                            in_extension(word, directory).unwrap_or_else(|| word.to_string());
                        format!("{word:?}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                hints.push(format!(
                    "The {noun} {name} gives `exec` a string. `exec` is a list of arguments, such \
                     as `exec = [{argv}]`; use `shell` for a command line."
                ));
            }
            if kind != "commands" {
                continue;
            }
            if ["run", "popup"].iter().any(|key| table.get(key).is_some()) {
                hints.push(format!(
                    "The command {name} uses `run` or `popup`, which are for commands in \
                     config.toml. An extension command runs with `exec`, `shell`, or `send`."
                ));
            }
            let on_client = table
                .get("placement")
                .and_then(toml::Value::as_str)
                .is_none_or(|placement| placement == "client");
            let arguments = table
                .get("exec")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .skip(1)
                .filter_map(toml::Value::as_str);
            for argument in arguments.filter(|_| on_client) {
                if let Some(fixed) = in_extension(argument, directory) {
                    hints.push(format!(
                        "The command {name} passes `{argument}`, which is found from the focused \
                         pane's directory, not the extension's. Write `\"{fixed}\"`."
                    ));
                }
            }
        }
    }
}

/// `argument` rewritten through `{extension_dir}` when it is a relative path to a file the
/// extension ships, and so almost certainly meant the extension's copy.
fn in_extension(argument: &str, directory: &Path) -> Option<String> {
    if argument.starts_with('-')
        || argument.starts_with('~')
        || argument.contains('{')
        || argument.contains('=')
        || Path::new(argument).is_absolute()
    {
        return None;
    }
    let relative = argument
        .strip_prefix("./")
        .or_else(|| argument.strip_prefix(".\\"))
        .unwrap_or(argument);
    if relative.starts_with("..") || !directory.join(relative).is_file() {
        return None;
    }
    Some(format!("{{extension_dir}}/{}", relative.replace('\\', "/")))
}

fn suggested_id(id: &str) -> Option<String> {
    let valid = |character: char| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '-'
            || character == '_'
    };
    if !id.is_empty() && id.chars().all(valid) {
        return None;
    }
    let mut suggestion = String::new();
    for character in id
        .trim()
        .chars()
        .map(|character| character.to_ascii_lowercase())
    {
        if valid(character) {
            suggestion.push(character);
        } else if !suggestion.ends_with('-') {
            suggestion.push('-');
        }
    }
    let suggestion = suggestion.trim_matches('-').to_string();
    (!suggestion.is_empty()).then_some(suggestion)
}

/// "Did you mean" for a key the manifest does not know, from the parser's own list of the keys it
/// expected there.
fn unknown_key_hint(message: &str) -> Option<String> {
    let rest = message.strip_prefix("unknown field ")?;
    let mut names = rest.split('`').skip(1).step_by(2);
    let unknown = names.next()?;
    if matches!(unknown, "run" | "popup" | "extensions") {
        return None;
    }
    let (distance, closest) = names
        .map(|name| (edit_distance(unknown, name), name))
        .min()?;
    (distance <= 2 && distance < unknown.len())
        .then(|| format!("Unknown key `{unknown}`: did you mean `{closest}`?"))
}

fn error_hint(error: &str, directory: &Path) -> Option<String> {
    if let Some((_, path)) = error.split_once("declared executable is not executable: ") {
        let (relative, argument) = match Path::new(path).strip_prefix(directory) {
            Ok(relative) => {
                let relative = relative.display().to_string();
                let argument = format!("{{extension_dir}}/{relative}");
                (relative, argument)
            }
            Err(_) => (path.to_string(), path.to_string()),
        };
        let interpreter = match Path::new(path).extension().and_then(|kind| kind.to_str()) {
            Some("py") => Some("python3"),
            Some("sh") => Some("sh"),
            Some("js" | "mjs") => Some("node"),
            _ => None,
        };
        return Some(match interpreter {
            Some(interpreter) => format!(
                "Make `{relative}` executable with `chmod +x {relative}`, or run it through its \
                 interpreter: `exec = [\"{interpreter}\", \"{argument}\"]`."
            ),
            None => format!("Make `{relative}` executable with `chmod +x {relative}`."),
        });
    }
    let program = error
        .split_once("executable `")?
        .1
        .strip_suffix("` was not found on PATH")?;
    directory.join(program).is_file().then(|| {
        format!(
            "`{program}` is a file in the extension. Write `./{program}` to run the extension's \
             copy rather than look it up on PATH."
        )
    })
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left) in left.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left != *right);
            current.push(
                substitution
                    .min(previous[column + 1] + 1)
                    .min(current[column] + 1),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints_for(manifest: &str, files: &[(&str, bool)]) -> Vec<String> {
        let temp = tempfile::tempdir().unwrap();
        for (file, executable) in files {
            let path = temp.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if *executable { 0o755 } else { 0o644 };
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            #[cfg(not(unix))]
            let _ = executable;
        }
        std::fs::write(temp.path().join("extension.toml"), manifest).unwrap();
        let checked = crate::config::check_extension(temp.path());
        authoring_hints(&checked.info)
    }

    const HEADER: &str = "[extension]\nid = \"x\"\ntitle = \"x\"\nversion = \"0.1.0\"\napi = 1\n";

    #[test]
    fn a_valid_scaffold_needs_no_hints() {
        let manifest = format!(
            "{HEADER}[[commands]]\nid = \"a\"\nexec = [\"sh\", \"{{extension_dir}}/bin/run.sh\"]\n"
        );
        assert_eq!(
            hints_for(&manifest, &[("bin/run.sh", false)]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_script_named_from_the_pane_directory_is_pointed_at_the_extension() {
        let manifest =
            format!("{HEADER}[[commands]]\nid = \"a\"\nexec = [\"sh\", \"bin/run.sh\"]\n");
        assert_eq!(
            hints_for(&manifest, &[("bin/run.sh", false)]),
            [
                "The command `a` passes `bin/run.sh`, which is found from the focused pane's \
              directory, not the extension's. Write `\"{extension_dir}/bin/run.sh\"`."
            ]
        );
        let placed = format!(
            "{HEADER}[[commands]]\nid = \"a\"\nexec = [\"sh\", \"bin/run.sh\"]\n\
             placement = \"active-session\"\n"
        );
        assert!(
            hints_for(&placed, &[("bin/run.sh", false)])
                .iter()
                .all(|hint| !hint.contains("focused pane")),
            "a placed command's arguments are not resolved on the client"
        );
    }

    #[test]
    fn a_misspelled_key_gets_the_one_it_meant() {
        let manifest = format!("{HEADER}[[command]]\nid = \"a\"\nexec = [\"true\"]\n");
        assert_eq!(
            hints_for(&manifest, &[]),
            ["Unknown key `command`: did you mean `commands`?"]
        );
        let unrelated = format!("{HEADER}[[zzzzzzzz]]\nid = \"a\"\n");
        assert!(hints_for(&unrelated, &[]).is_empty());
    }

    #[test]
    fn a_string_exec_is_shown_as_the_list_it_should_be() {
        let manifest = format!("{HEADER}[[commands]]\nid = \"a\"\nexec = \"python3 bin/a.py\"\n");
        assert_eq!(
            hints_for(&manifest, &[("bin/a.py", false)]),
            [
                "The command `a` gives `exec` a string. `exec` is a list of arguments, such as \
              `exec = [\"python3\", \"{extension_dir}/bin/a.py\"]`; use `shell` for a command \
              line."
            ]
        );
    }

    #[test]
    fn config_file_command_keys_and_sections_are_redirected() {
        let manifest = format!(
            "{HEADER}[[commands]]\nid = \"a\"\nrun = \"htop\"\n[extensions]\ndisabled = []\n"
        );
        let hints = hints_for(&manifest, &[]);
        assert_eq!(hints.len(), 2, "{hints:?}");
        assert!(hints[0].starts_with("`[extensions]` belongs in Rozi's config.toml"));
        assert!(hints[1].contains("uses `run` or `popup`"));
    }

    #[test]
    fn missing_metadata_and_a_bad_id_say_what_to_write() {
        let hints = hints_for("[extension]\nid = \"My Tools\"\n", &[]);
        assert_eq!(
            hints,
            [
                "Add `api = 1` to `[extension]`.",
                "An extension ID uses lowercase letters, digits, `-`, and `_`: try \
                 `id = \"my-tools\"`."
            ]
        );
        assert!(hints_for("[[commands]]\nid = \"a\"\n", &[])[0].starts_with("Start the manifest"));
    }

    #[test]
    fn a_program_path_says_how_to_make_it_run() {
        let bare = format!("{HEADER}[[commands]]\nid = \"a\"\nexec = [\"bin/run\"]\n");
        assert_eq!(
            hints_for(&bare, &[("bin/run", true)]),
            [
                "`bin/run` is a file in the extension. Write `./bin/run` to run the extension's \
              copy rather than look it up on PATH."
            ]
        );
        #[cfg(unix)]
        {
            let script = format!("{HEADER}[[commands]]\nid = \"a\"\nexec = [\"./bin/run.py\"]\n");
            assert_eq!(
                hints_for(&script, &[("bin/run.py", false)]),
                [
                    "Make `bin/run.py` executable with `chmod +x bin/run.py`, or run it through its \
                  interpreter: `exec = [\"python3\", \"{extension_dir}/bin/run.py\"]`."
                ]
            );
        }
    }
}
