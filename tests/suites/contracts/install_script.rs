//! Contract tests for the bootstrap installers.
//!
//! Unix checks `install.sh` syntax and the PATH hint. Windows drives the functions out of the
//! real `install.ps1`, so a regression in what ships is what fails - a transcription of the
//! logic into Rust would keep passing while the installer rotted next to it.

use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
fn install_sh() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

/// Load every function from `install.sh` without running `main`, then evaluate `body`.
#[cfg(unix)]
fn in_unix_installer_scope(body: &str) -> String {
    let script = format!(
        "source <(sed '/^main() {{/,$d' \"$1\")\n{body}",
        body = body
    );
    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .arg("install-script-test")
        .arg(install_sh())
        .output()
        .expect("run bash installer scope");
    assert!(
        output.status.success(),
        "bash exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .replace("\r\n", "\n")
}

#[cfg(unix)]
#[test]
fn unix_installer_has_valid_syntax() {
    let status = Command::new("bash")
        .arg("-n")
        .arg(install_sh())
        .status()
        .expect("run bash -n");
    assert!(status.success(), "install.sh failed bash -n");
}

#[cfg(unix)]
#[test]
fn command_hint_on_path_is_a_single_run_line() {
    let printed = in_unix_installer_scope(
        r#"PATH="$HOME/.local/bin:$PATH"
command_hint"#,
    );
    assert_eq!(
        printed, "  Run  rozi",
        "on-PATH hint was not the compact Run line:\n{printed}"
    );
}

#[cfg(unix)]
#[test]
fn command_hint_off_path_prints_run_and_add() {
    let printed = in_unix_installer_scope(
        r#"PATH="/usr/bin:/bin"
command_hint"#,
    );
    let home = std::env::var("HOME").expect("HOME");
    let expected = format!(
        "\n! Not on PATH\n  Run     {home}/.local/bin/rozi\n  Add     export PATH=\"$HOME/.local/bin:$PATH\""
    );
    assert_eq!(
        printed, expected,
        "off-PATH hint was not the compact Run/Add block:\n{printed}"
    );
}

#[cfg(windows)]
fn install_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("install.ps1")
}

/// Dot-source every function in `install.ps1` and run `body` with them in scope.
///
/// Loading only the function definitions is what keeps the script's top-level body - which
/// downloads and installs - from running during a test. Loading *all* of them, rather than a list
/// each test names, is deliberate: a named list silently turns "this function gained a callee"
/// into an opaque PowerShell failure inside the harness, which is exactly how this helper first
/// broke.
#[cfg(windows)]
fn in_installer_scope(body: &str) -> String {
    let script = format!(
        "$ErrorActionPreference = 'Stop'
$ast = [System.Management.Automation.Language.Parser]::ParseFile('{path}', [ref]$null, [ref]$null)
$ast.FindAll({{ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst]
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
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .replace("\r\n", "\n")
}

/// A shell that already had the entry added to it is not the same as a shell that never will.
///
/// The persisted user PATH reaches new terminals; the process PATH is what resolves a command in
/// this one. They diverge for every shell that was open when the entry was added, and reporting
/// that shell as simply "not on PATH" would send someone to fix an environment that is already
/// correct.
#[cfg(windows)]
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
    let states = in_installer_scope(&body);

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
#[cfg(windows)]
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
    let matches = in_installer_scope(&body);

    assert_eq!(
        matches.lines().collect::<Vec<_>>(),
        vec!["True", "True", "True", "True", "False", "False"],
        "PATH entry matching did not behave as expected:\n{matches}"
    );
}

/// Every PATH change the hint prints has to be safe to paste twice.
///
/// An installer hint is read as a recipe, and a recipe that appends unconditionally leaves a
/// duplicate entry behind on the second run.
#[cfg(windows)]
#[test]
fn the_printed_path_remediation_is_guarded_against_running_twice() {
    let body = r#"foreach ($state in 'absent','stale-session') {
    $parsed = [System.Management.Automation.Language.Parser]::ParseInput(
        ((Get-PathRemediation $state) -join "`n"), [ref]$null, [ref]$null)
    $writes = @($parsed.FindAll({ param($n)
        ($n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
         $n.Left.Extent.Text -like '*env:Path*') -or
        ($n -is [System.Management.Automation.Language.InvokeMemberExpressionAst] -and
         $n.Member.Extent.Text -eq 'SetEnvironmentVariable') }, $true))
    foreach ($write in $writes) {
        $guarded = $false
        $parent = $write.Parent
        while ($null -ne $parent) {
            if ($parent -is [System.Management.Automation.Language.IfStatementAst] -and
                $parent.Clauses[0].Item1.Extent.Text -match '-notcontains') { $guarded = $true; break }
            $parent = $parent.Parent
        }
        Write-Output "$state|$guarded"
    }
}"#;
    let reported = in_installer_scope(body);

    let writes: Vec<&str> = reported.lines().filter(|line| !line.is_empty()).collect();
    assert!(
        writes.len() >= 3,
        "expected a persisted and a session write for 'absent' and a session write for \
         'stale-session', got:\n{reported}"
    );
    for line in &writes {
        let (state, guarded) = line.split_once('|').expect("state-tagged line");
        assert_eq!(
            guarded, "True",
            "a PATH write in the {state} block is not inside a `-notcontains` check, so pasting \
             that block twice duplicates the entry"
        );
    }
}

/// Each block has to stand on its own, because people paste one of them and not the other.
///
/// The two were once printed as a pair that shared a `$bin` defined only by the first, so anyone
/// who needed just the session fix - a terminal one entry behind, the common case - pasted a
/// snippet that died on an undefined variable. Comparing the variables a block *reads* against the
/// ones it *assigns* is what catches that; running the block cannot, because the persisted half
/// writes to the user's real environment.
#[cfg(windows)]
#[test]
fn path_remediation_defines_every_variable_it_uses() {
    let body = r#"foreach ($state in 'absent','stale-session') {
    $parsed = [System.Management.Automation.Language.Parser]::ParseInput(
        ((Get-PathRemediation $state) -join "`n"), [ref]$null, [ref]$null)
    $assigned = @($parsed.FindAll({ param($n)
        $n -is [System.Management.Automation.Language.AssignmentStatementAst] }, $true) |
        ForEach-Object { $_.Left } |
        Where-Object { $_ -is [System.Management.Automation.Language.VariableExpressionAst] } |
        ForEach-Object { $_.VariablePath.UserPath })
    $used = @($parsed.FindAll({ param($n)
        $n -is [System.Management.Automation.Language.VariableExpressionAst] }, $true) |
        Where-Object { -not $_.VariablePath.IsDriveQualified } |
        ForEach-Object { $_.VariablePath.UserPath } |
        Where-Object { $_ -notin @('true','false','null','_') } | Select-Object -Unique)
    Write-Output "$state|$(@($used | Where-Object { $_ -notin $assigned }) -join ',')"
}"#;
    let reported = in_installer_scope(body);

    for line in reported.lines().filter(|line| !line.is_empty()) {
        let (state, undefined) = line.split_once('|').expect("state-tagged line");
        assert!(
            undefined.is_empty(),
            "the {state} block reads variables it never defines ({undefined}), \
             so pasting it on its own fails"
        );
    }
}

/// The hint must not send anyone back through the installer to change PATH.
///
/// Re-running re-downloads the archive and re-verifies its checksum and signature to append one
/// string to the registry - and it does that work after the payload probe, so on a machine whose
/// application-control policy refuses the payload the re-run fails before ever reaching the PATH
/// code. `-AddToPath` stays correct at install time; it is not remediation.
#[cfg(windows)]
#[test]
fn the_hint_does_not_prescribe_reinstalling_to_fix_path() {
    let body = "$script:CDim = ''
$script:CReset = ''
Write-CommandHint 'C:\\U\\AppData\\Local\\rozi\\bin'";
    let printed = in_installer_scope(body);

    assert!(
        !printed.contains("-AddToPath"),
        "the PATH hint told the user to re-run the installer:\n{printed}"
    );
    assert!(
        !printed.contains("scriptblock]::Create"),
        "the PATH hint told the user to re-fetch and re-run the installer:\n{printed}"
    );
    assert!(
        printed.contains("Not on PATH"),
        "the PATH hint did not use the compact Not on PATH header:\n{printed}"
    );
    assert!(
        printed.contains("  Run     ") && printed.contains("  Add     "),
        "the PATH hint did not use the compact Run/Add labels:\n{printed}"
    );
    assert!(
        !printed.contains("Start it with"),
        "the PATH hint still used the padded Start it with copy:\n{printed}"
    );
}
