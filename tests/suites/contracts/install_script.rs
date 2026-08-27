//! Windows contract tests for `install.ps1`.
//!
//! The bootstrap script is the only Windows-specific surface a user meets before rozi has run at
//! all, and it carried no tests: three separate defects shipped in it inside a day, each one
//! reasoned about rather than exercised. These drive the functions out of the real file, so a
//! regression in what ships is what fails - a transcription of the logic into Rust would keep
//! passing while the installer rotted next to it.

use std::path::PathBuf;
use std::process::Command;

fn install_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("install.ps1")
}

/// Dot-source `functions` out of `install.ps1` and run `body` with them in scope.
///
/// Parsing the file and loading only its function definitions is what keeps the script's top-level
/// body - which downloads and installs - from running during a test.
fn in_installer_scope(functions: &[&str], body: &str) -> String {
    let wanted = functions
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "$ErrorActionPreference = 'Stop'
$ast = [System.Management.Automation.Language.Parser]::ParseFile('{path}', [ref]$null, [ref]$null)
$wanted = @({wanted})
$ast.FindAll({{ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
    $wanted -contains $node.Name
}}, $true) | ForEach-Object {{ . ([scriptblock]::Create($_.Extent.Text)) }}
{body}",
        path = install_script().display(),
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("run powershell");
    assert!(
        output.status.success(),
        "powershell exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().replace("\r\n", "\n")
}

/// A shell that already had the entry added to it is not the same as a shell that never will.
///
/// The persisted user PATH reaches new terminals; the process PATH is what resolves a command in
/// this one. They diverge for every shell that was open when the entry was added, and reporting
/// that shell as simply "not on PATH" would send someone to fix an environment that is already
/// correct.
#[test]
fn path_state_separates_a_stale_session_from_a_missing_entry() {
    let bin = r"C:\U\AppData\Local\rozi\bin";
    let body = format!(
        "$bin = '{bin}'
Write-Output (Get-CommandHintState $bin \"C:\\a;$bin\" \"C:\\a;$bin\")
Write-Output (Get-CommandHintState $bin \"C:\\a;$bin\" 'C:\\a')
Write-Output (Get-CommandHintState $bin 'C:\\a' \"C:\\a;$bin\")
Write-Output (Get-CommandHintState $bin 'C:\\a' 'C:\\a')
Write-Output (Get-CommandHintState $bin '' '')"
    );
    let states = in_installer_scope(&["Test-PathContainsDirectory", "Get-CommandHintState"], &body);

    assert_eq!(
        states.lines().collect::<Vec<_>>(),
        vec!["ready", "ready", "stale-session", "absent", "absent"],
        "PATH states did not classify as expected:\n{states}"
    );
}

/// PATH entries are compared the way Windows resolves them, and only that way.
///
/// A directory that merely starts with the same text is a different directory: matching it would
/// suppress the hint for someone who has `...\rozi\bin2` and no working command.
#[test]
fn path_entry_matching_ignores_case_and_trailing_separators_but_not_prefixes() {
    let bin = r"C:\U\AppData\Local\rozi\bin";
    let body = format!(
        "$bin = '{bin}'
Write-Output (Test-PathContainsDirectory \"C:\\a;$bin;C:\\b\" $bin)
Write-Output (Test-PathContainsDirectory \"$bin\\\" $bin)
Write-Output (Test-PathContainsDirectory $bin.ToUpper() $bin)
Write-Output (Test-PathContainsDirectory \" $bin \" $bin)
Write-Output (Test-PathContainsDirectory \"${{bin}}2\" $bin)
Write-Output (Test-PathContainsDirectory 'C:\\a;C:\\b' $bin)"
    );
    let matches = in_installer_scope(&["Test-PathContainsDirectory"], &body);

    assert_eq!(
        matches.lines().collect::<Vec<_>>(),
        vec!["True", "True", "True", "True", "False", "False"],
        "PATH entry matching did not behave as expected:\n{matches}"
    );
}

/// Every PATH change the hint prints has to be safe to paste twice.
///
/// An installer hint is read as a recipe, and a recipe that appends unconditionally leaves a
/// duplicate entry behind on the second run. The guard is asserted on the printed text rather than
/// on a copy of it here, because the text is what a user actually runs.
#[test]
fn the_printed_path_remediation_is_guarded_against_running_twice() {
    let body = "$script:CDim = ''
$script:CReset = ''
Write-CommandHint 'C:\\U\\AppData\\Local\\rozi\\bin'";
    let printed = in_installer_scope(
        &[
            "Test-PathContainsDirectory",
            "Get-CommandHintState",
            "Write-CommandHint",
        ],
        body,
    );

    let mutating: Vec<&str> = printed
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("$env:Path +=") || line.contains("SetEnvironmentVariable"))
        .collect();
    assert!(
        !mutating.is_empty(),
        "the hint printed no PATH remediation at all:\n{printed}"
    );
    for line in &mutating {
        assert!(
            line.contains("-notcontains"),
            "a printed PATH change is not guarded, so pasting it twice duplicates the entry: {line}"
        );
    }
}

/// The hint must not send anyone back through the installer to change PATH.
///
/// Re-running re-downloads the archive and re-verifies its checksum and signature to append one
/// string to the registry - and it does that work after the payload probe, so on a machine whose
/// application-control policy refuses the payload the re-run fails before ever reaching the PATH
/// code. `-AddToPath` stays correct at install time; it is not remediation.
#[test]
fn the_hint_does_not_prescribe_reinstalling_to_fix_path() {
    let body = "$script:CDim = ''
$script:CReset = ''
Write-CommandHint 'C:\\U\\AppData\\Local\\rozi\\bin'";
    let printed = in_installer_scope(
        &[
            "Test-PathContainsDirectory",
            "Get-CommandHintState",
            "Write-CommandHint",
        ],
        body,
    );

    assert!(
        !printed.contains("-AddToPath"),
        "the PATH hint told the user to re-run the installer:\n{printed}"
    );
    assert!(
        !printed.contains("scriptblock]::Create"),
        "the PATH hint told the user to re-fetch and re-run the installer:\n{printed}"
    );
}
