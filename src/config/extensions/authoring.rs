use std::path::{Path, PathBuf};

use super::EXTENSION_API_VERSION;

pub(crate) fn create_extension_scaffold(id: &str, parent: &Path) -> Result<PathBuf, String> {
    super::validation::validate_requested_extension_id(id)?;
    let python = scaffold_python()?;
    let destination = parent.join(id);
    std::fs::create_dir(&destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!("destination already exists: {}", destination.display())
        } else {
            format!(
                "could not create extension directory {}: {error}",
                destination.display()
            )
        }
    })?;

    let result = write_scaffold(id, &destination, &python);
    if result.is_err() {
        cleanup_partial_scaffold(&destination);
    }
    result.map(|()| destination)
}

struct PythonLaunch {
    program: &'static str,
    prefix_arg: Option<&'static str>,
}

fn scaffold_python() -> Result<PythonLaunch, String> {
    let candidates: &[PythonLaunch] = if cfg!(windows) {
        &[
            PythonLaunch {
                program: "python",
                prefix_arg: None,
            },
            PythonLaunch {
                program: "py",
                prefix_arg: Some("-3"),
            },
            PythonLaunch {
                program: "python3",
                prefix_arg: None,
            },
        ]
    } else {
        &[
            PythonLaunch {
                program: "python3",
                prefix_arg: None,
            },
            PythonLaunch {
                program: "python",
                prefix_arg: None,
            },
        ]
    };
    candidates
        .iter()
        .find(|candidate| crate::platform::command::program_exists(candidate.program))
        .map(|candidate| PythonLaunch {
            program: candidate.program,
            prefix_arg: candidate.prefix_arg,
        })
        .ok_or_else(|| {
            "`rozi extensions new` requires Python 3 (`python3`, `python`, or Windows `py`)"
                .to_string()
        })
}

fn write_scaffold(id: &str, destination: &Path, python: &PythonLaunch) -> Result<(), String> {
    let bin = destination.join("bin");
    std::fs::create_dir(&bin)
        .map_err(|error| format!("could not create {}: {error}", bin.display()))?;

    let prefix_arg = python
        .prefix_arg
        .map(|argument| format!("{argument:?}, "))
        .unwrap_or_default();
    let manifest = format!(
        "[extension]\n\
         id = \"{id}\"\n\
         title = \"{id}\"\n\
         description = \"A Rozi extension\"\n\
         version = \"0.1.0\"\n\
         api = {EXTENSION_API_VERSION}\n\n\
         [[commands]]\n\
         id = \"hello\"\n\
         label = \"Hello from {id}\"\n\
         exec = [{program:?}, {prefix_arg}\"{{extension_dir}}/bin/hello.py\"]\n",
        program = python.program,
    );
    write_file(&destination.join("extension.toml"), &manifest)?;

    let script = format!(
        "from __future__ import annotations\n\n\
         import os\n\
         import subprocess\n\
         import sys\n\n\
         rozi = os.environ.get(\"ROZI_BIN\", \"rozi\")\n\
         extension = os.environ.get(\"ROZI_EXTENSION\")\n\
         if extension != {id:?}:\n\
         \x20   print(\"launch this command through Rozi\", file=sys.stderr)\n\
         \x20   raise SystemExit(2)\n\
         raise SystemExit(subprocess.run(\n\
         \x20   [rozi, \"notify\", f\"Hello from {{extension}}\"], check=False\n\
         ).returncode)\n"
    );
    write_file(&bin.join("hello.py"), &script)?;

    let readme = format!(
        "# {id}\n\n\
         Generated Rozi extension scaffold.\n\n\
         ## Develop\n\n\
         ```bash\n\
         rozi extensions check .\n\
         # Copy this directory below Rozi's user extension directory, then:\n\
         rozi run-action reload-extensions\n\
         rozi extensions list --verbose\n\
         rozi run-action {id}.hello\n\
         ```\n\n\
         The example uses structured `exec` and calls the running Rozi binary through\n\
         `ROZI_BIN`. It requires Python 3; this scaffold selected `{program}` for the current\n\
         machine. Adjust the manifest argv if the extension is shared with a platform whose\n\
         Python launcher differs.\n\n\
         Installed extensions are trusted local executable code. Rozi validates the manifest but\n\
         does not sandbox the program.\n",
        program = python.program,
    );
    write_file(&destination.join("README.md"), &readme)
}

fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn cleanup_partial_scaffold(destination: &Path) {
    let _ = std::fs::remove_file(destination.join("extension.toml"));
    let _ = std::fs::remove_file(destination.join("README.md"));
    let _ = std::fs::remove_file(destination.join("bin").join("hello.py"));
    let _ = std::fs::remove_dir(destination.join("bin"));
    let _ = std::fs::remove_dir(destination);
}
