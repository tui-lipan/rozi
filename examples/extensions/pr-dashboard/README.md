# PR dashboard extension

This independent extension watches the repository belonging to the most recently focused Rozi
pane. It uses only supervised `[[services]]`, the injected extension environment, and the public
`rozi subscribe`, `publish`, `notify`, `pick`, and `list-panes` commands. GitHub data comes from
machine-readable `gh --json` output; subprocesses always use argv arrays, so repository paths and
PR titles containing spaces are not interpreted by a shell.

The service publishes open PRs for the current branch, PRs authored by the active GitHub user, and
PRs awaiting that user's review. Rows are capped and deduplicated. Check failures, merge conflicts,
and requested changes are `blocked`; running checks are `working`; passed checks are `done`.
Initial state is silent. Later failure and recovery transitions notify once, while ordinary
refreshes do not.

The default cadence is 120 seconds. Focus changes refresh immediately, while config/workspace
events request a refresh no more often than every five seconds. `PR_DASHBOARD_POLL_SECONDS` is
clamped to 60-900 seconds and `PR_DASHBOARD_MAX_ROWS` to 1-30. Change the service `env` values in
`extension.toml` and reload config to tune them. Closing either the subscription or publication
stream exits the service so Rozi can retire it cleanly.

## Prerequisites

- Rozi with extension API 1 and an available control endpoint.
- Python 3.10 or newer.
- GitHub CLI (`gh`) with a host account authenticated by `gh auth login`.
- A focused pane whose current directory is inside a repository known to GitHub.

The extension itself has no Python packages to install. Missing `gh`, missing authentication,
non-repository directories, API failures, timeouts, and malformed JSON become inert Activity/picker
diagnostic rows instead of crashing the service. The supervised Python process cannot run when
`python` itself is absent; `rozi extensions check` and `extensions list --verbose` report that
missing executable before launch.

Check the prerequisites manually:

```bash
command -v python
python --version
command -v gh
gh auth status --active
gh repo view --json nameWithOwner,url
gh pr status --json number,title,headRefName,statusCheckRollup,reviewDecision,isDraft,mergeable,mergeStateStatus,url,state
```

Run the last two commands from the repository you want the dashboard to follow.

## Install and validate

From this repository checkout:

```bash
src="$PWD/examples/extensions/pr-dashboard"

rozi extensions check "$src"
python -c 'import ast, pathlib, sys; ast.parse(pathlib.Path(sys.argv[1]).read_text())' \
  "$src/bin/pr_dashboard.py"
rozi extensions install --link "$src"
rozi run-action reload-extensions
rozi extensions list --verbose
```

The PowerShell equivalent on Windows is:

```powershell
$src = Resolve-Path ".\examples\extensions\pr-dashboard"
rozi extensions check $src
rozi extensions install --link $src
rozi run-action reload-extensions
rozi extensions list --verbose
```

## Invoke and use

Focus a pane inside the desired repository, then move focus once after installation so the
subscriber receives a `focus-changed` event. Open the picker either way:

```bash
rozi run-action pr-dashboard.open
```

Or choose **Pull requests…** from Rozi's command palette. In the picker:

- `Enter` opens the selected PR in the browser;
- `c` opens its checks in the browser;
- `r` refreshes the rows without closing the picker;
- `Esc` closes it.

Activating a published Activity row opens the same picker for the repository represented by the
current dashboard snapshot.

## Manual behavior check

1. Focus a pane in an authenticated GitHub repository and switch away and back. Within one GitHub
   query round, Activity should show stable `#<number>` rows or one idle “no relevant PRs” row.
2. Compare the rows with the machine-readable `gh pr status --json ...` command above.
3. Open `pr-dashboard.open`, press `r`, select a PR, and verify that `Enter` opens that exact PR.
4. Focus a pane in a directory without a GitHub repository. Activity should show an idle
   **No GitHub repository** diagnostic and one informational notification, without a service
   restart.
5. Restore focus to the repository. A later transition from running checks to failure should emit
   one error notification; recovery to passed checks should emit one informational notification.
6. Detach or reload after changing the manifest. The old rows should disappear when Rozi closes
   the streams, and the supervised service should exit rather than keep polling.
