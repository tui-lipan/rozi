use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub(super) fn normalize_direct_argv(
    mut argv: Vec<String>,
    base: &Path,
    public_id: &str,
    resolved: &mut BTreeMap<String, String>,
    errors: &mut Vec<String>,
) -> Option<Vec<String>> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        errors.push(format!(
            "`{public_id}` direct `exec` must contain a program"
        ));
        return None;
    }
    let extension_dir = base.to_string_lossy();
    for argument in &mut argv {
        if argument.contains("$ROZI_EXTENSION_DIR")
            || argument.contains("${ROZI_EXTENSION_DIR}")
            || argument.contains("%ROZI_EXTENSION_DIR%")
        {
            errors.push(format!(
                "`{public_id}` direct `exec` uses shell environment expansion; use `{{extension_dir}}`"
            ));
        }
        *argument = argument.replace("{extension_dir}", &extension_dir);
    }

    let original_program = argv[0].clone();
    if is_declared_path(&original_program) {
        let path = resolve_declared_path(base, &original_program);
        let path_text = path.to_string_lossy().to_string();
        resolved.insert(public_id.to_string(), path_text.clone());
        validate_target(&path, public_id, true, errors);
        argv[0] = path_text;
    } else if !crate::platform::command::program_exists(&original_program) {
        errors.push(format!(
            "`{public_id}` executable `{original_program}` was not found on PATH"
        ));
    }

    if !resolved.contains_key(public_id) {
        for argument in argv.iter().skip(1) {
            if let Some(suffix) = argument.strip_prefix(extension_dir.as_ref())
                && !suffix.is_empty()
            {
                let path = normalize_path(Path::new(argument));
                resolved.insert(public_id.to_string(), path.to_string_lossy().to_string());
                validate_target(&path, public_id, false, errors);
                break;
            }
        }
    }
    Some(argv)
}

pub(super) fn resolve_declared_path(base: &Path, value: &str) -> PathBuf {
    let expanded = if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        super::super::expand_path(value)
    } else {
        PathBuf::from(value)
    };
    if expanded.is_absolute() {
        normalize_path(&expanded)
    } else {
        normalize_path(&base.join(expanded))
    }
}

pub(super) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(
            &std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path),
        )
    }
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn validate_target(
    path: &Path,
    public_id: &str,
    require_executable: bool,
    errors: &mut Vec<String>,
) {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            if require_executable
                && !crate::platform::command::program_exists(&path.to_string_lossy())
            {
                errors.push(format!(
                    "`{public_id}` declared executable is not executable: {}",
                    path.display()
                ));
            }
        }
        Ok(_) => errors.push(format!(
            "`{public_id}` declared path is not a file: {}",
            path.display()
        )),
        Err(error) => errors.push(format!(
            "`{public_id}` declared path is unavailable at {}: {error}",
            path.display()
        )),
    }
}

fn is_declared_path(program: &str) -> bool {
    let unix_relative = program.starts_with("./") || program.starts_with("../");
    let windows_relative = program.starts_with(".\\") || program.starts_with("..\\");
    unix_relative
        || windows_relative
        || program.starts_with('~')
        || Path::new(program).is_absolute()
}
