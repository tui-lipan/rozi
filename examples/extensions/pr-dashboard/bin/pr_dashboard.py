#!/usr/bin/env python3
"""Focused-repository PR dashboard using only Rozi's public CLI."""

from __future__ import annotations

import argparse
import collections
import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


ROZI = os.environ.get("ROZI_BIN", "rozi")
GH_TIMEOUT_SECONDS = 20
MIN_EVENT_REFRESH_SECONDS = 5
SUBSCRIBED_EVENTS = [
    "focus-changed",
    "config-reloaded",
    "pane-exited",
    "session-attached",
    "session-detached",
    "workspace-switched",
]
PR_FIELDS = ",".join(
    [
        "number",
        "title",
        "headRefName",
        "statusCheckRollup",
        "reviewDecision",
        "isDraft",
        "mergeable",
        "mergeStateStatus",
        "url",
        "state",
    ]
)
FAILURE_STATES = {
    "ACTION_REQUIRED",
    "CANCELLED",
    "ERROR",
    "FAILURE",
    "STARTUP_FAILURE",
    "TIMED_OUT",
}
PENDING_STATES = {"EXPECTED", "IN_PROGRESS", "PENDING", "QUEUED", "REQUESTED", "WAITING"}
SUCCESS_STATES = {"NEUTRAL", "SKIPPED", "STALE", "SUCCESS"}


@dataclass(frozen=True)
class Diagnostic:
    code: str
    title: str
    reason: str
    error: bool = False
    notify: bool = True


@dataclass(frozen=True)
class PullRequest:
    number: int
    title: str
    branch: str
    url: str
    group: str
    status: str
    reason: str
    current: bool = False


@dataclass
class Dashboard:
    cwd: Path | None
    repo: str | None = None
    repo_url: str | None = None
    pull_requests: list[PullRequest] = field(default_factory=list)
    diagnostic: Diagnostic | None = None

    def publish_rows(self) -> list[dict[str, object]]:
        if self.diagnostic is not None:
            return [
                {
                    "id": f"diagnostic:{self.diagnostic.code}",
                    "title": self.diagnostic.title,
                    "status": "blocked" if self.diagnostic.error else "idle",
                    "reason": self.diagnostic.reason,
                    "active": False,
                }
            ]
        if not self.pull_requests:
            return [
                {
                    "id": f"empty:{self.repo or 'unknown'}",
                    "title": self.repo or "PR dashboard",
                    "status": "idle",
                    "reason": "No current, authored, or review-requested PRs",
                    "active": False,
                }
            ]
        return [
            {
                "id": row_id(self.repo, pr.number),
                "title": f"#{pr.number} {pr.title}",
                "status": pr.status,
                "reason": " · ".join(
                    part for part in (self.repo, pr.branch, pr.reason) if part
                ),
                "active": pr.current,
            }
            for pr in self.pull_requests
        ]

    def picker_rows(self) -> list[dict[str, object]]:
        if self.diagnostic is not None:
            return [
                {
                    "id": f"diagnostic:{self.diagnostic.code}",
                    "label": self.diagnostic.title,
                    "description": self.diagnostic.reason,
                    "disabled": self.diagnostic.reason,
                }
            ]
        if not self.pull_requests:
            return [
                {
                    "id": f"empty:{self.repo or 'unknown'}",
                    "label": self.repo or "PR dashboard",
                    "disabled": "No relevant open pull requests",
                }
            ]
        return [
            {
                "id": row_id(self.repo, pr.number),
                "label": f"#{pr.number} {pr.title}",
                "description": " · ".join(
                    part for part in (pr.branch, pr.reason) if part
                ),
                "group": pr.group,
                "active": pr.current,
            }
            for pr in self.pull_requests
        ]


class CommandFailure(Exception):
    def __init__(self, message: str, stderr: str = "") -> None:
        super().__init__(message)
        self.stderr = stderr


def bounded_env_int(name: str, default: int, minimum: int, maximum: int) -> int:
    try:
        value = int(os.environ.get(name, str(default)))
    except ValueError:
        return default
    return max(minimum, min(maximum, value))


def row_id(repo: str | None, number: int) -> str:
    return f"pr:{repo or 'unknown'}#{number}"


def run(
    argv: list[str],
    *,
    cwd: Path | None = None,
    timeout: int = GH_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            argv,
            cwd=str(cwd) if cwd is not None else None,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise CommandFailure(f"{Path(argv[0]).name} timed out") from error
    except OSError as error:
        raise CommandFailure(str(error)) from error


def run_json(argv: list[str], *, cwd: Path | None = None) -> Any:
    result = run(argv, cwd=cwd)
    if result.returncode != 0:
        raise CommandFailure(
            f"{Path(argv[0]).name} exited with status {result.returncode}",
            result.stderr.strip(),
        )
    candidates = [result.stdout, *reversed(result.stdout.splitlines())]
    for candidate in candidates:
        candidate = candidate.strip()
        if not candidate:
            continue
        try:
            return json.loads(candidate)
        except json.JSONDecodeError:
            continue
    raise CommandFailure(
        f"{Path(argv[0]).name} returned invalid JSON", result.stderr.strip()
    )


def auth_is_ready() -> bool:
    try:
        data = run_json(
            [
                shutil.which("gh") or "gh",
                "auth",
                "status",
                "--active",
                "--json",
                "hosts",
            ]
        )
    except CommandFailure:
        return False
    if not isinstance(data, dict) or not isinstance(data.get("hosts"), dict):
        return False
    return any(
        isinstance(account, dict)
        and account.get("active") is True
        and account.get("state") == "success"
        for accounts in data["hosts"].values()
        if isinstance(accounts, list)
        for account in accounts
    )


def failure_detail(error: CommandFailure) -> str:
    for line in error.stderr.splitlines():
        line = line.strip()
        if line:
            return line[:160]
    return str(error)[:160]


def repository_failure(cwd: Path, error: CommandFailure) -> Diagnostic:
    if not auth_is_ready():
        return Diagnostic(
            "auth",
            "GitHub authentication required",
            "Run gh auth login",
            error=True,
        )
    detail = failure_detail(error)
    lowered = detail.lower()
    no_repository_markers = (
        "not a git repository",
        "no git remotes",
        "unable to determine repository",
        "does not have any remotes",
    )
    if any(marker in lowered for marker in no_repository_markers):
        return Diagnostic(
            "repository",
            "No GitHub repository",
            cwd.name or str(cwd),
            error=False,
        )
    return Diagnostic("repository-error", "GitHub repository unavailable", detail, error=True)


def check_state(check: dict[str, Any]) -> str:
    for key in ("conclusion", "state", "status"):
        value = check.get(key)
        if isinstance(value, str) and value:
            return value.upper()
    return "UNKNOWN"


def summarize_pr(raw: dict[str, Any], group: str, current: bool) -> PullRequest | None:
    number = raw.get("number")
    title = raw.get("title")
    if not isinstance(number, int) or not isinstance(title, str):
        return None
    checks = raw.get("statusCheckRollup")
    checks = checks if isinstance(checks, list) else []
    states = [check_state(check) for check in checks if isinstance(check, dict)]
    failed = sum(state in FAILURE_STATES for state in states)
    pending = sum(state in PENDING_STATES for state in states)
    passed = sum(state in SUCCESS_STATES for state in states)
    pending += len(states) - failed - pending - passed
    review = str(raw.get("reviewDecision") or "").upper()
    mergeable = str(raw.get("mergeable") or "").upper()
    merge_state = str(raw.get("mergeStateStatus") or "").upper()

    if mergeable == "CONFLICTING" or merge_state == "DIRTY":
        status, reason = "blocked", "merge conflicts"
    elif review == "CHANGES_REQUESTED":
        status, reason = "blocked", "changes requested"
    elif failed:
        reason = f"{failed} failed"
        if passed:
            reason += f" · {passed} passed"
        status = "blocked"
    elif pending:
        noun = "check" if pending == 1 else "checks"
        reason = f"{pending} {noun} running"
        if passed:
            reason += f" · {passed} passed"
        status = "working"
    elif raw.get("isDraft") is True:
        status, reason = "idle", "draft"
    elif review == "REVIEW_REQUIRED":
        suffix = " · checks passed" if passed else ""
        status, reason = "idle", f"review needed{suffix}"
    elif checks:
        noun = "check" if passed == 1 else "checks"
        suffix = " · approved" if review == "APPROVED" else ""
        status, reason = "done", f"{passed} {noun} passed{suffix}"
    elif review == "APPROVED":
        status, reason = "done", "approved · no checks"
    else:
        status, reason = "idle", "no checks"

    branch = raw.get("headRefName")
    url = raw.get("url")
    return PullRequest(
        number=number,
        title=" ".join(title.split()),
        branch=branch if isinstance(branch, str) else "",
        url=url if isinstance(url, str) else "",
        group=group,
        status=status,
        reason=reason,
        current=current,
    )


def fetch_dashboard(cwd: Path | None, max_rows: int) -> Dashboard:
    if cwd is None:
        return Dashboard(
            None,
            diagnostic=Diagnostic(
                "focus",
                "PR dashboard",
                "Waiting for pane focus",
                notify=False,
            ),
        )
    gh = shutil.which("gh")
    if gh is None:
        return Dashboard(
            cwd,
            diagnostic=Diagnostic(
                "missing-gh",
                "GitHub CLI not found",
                "Install gh, then reload config",
                error=True,
            ),
        )
    try:
        repository = run_json([gh, "repo", "view", "--json", "nameWithOwner,url"], cwd=cwd)
    except CommandFailure as error:
        return Dashboard(cwd, diagnostic=repository_failure(cwd, error))
    if not isinstance(repository, dict) or not isinstance(
        repository.get("nameWithOwner"), str
    ):
        return Dashboard(
            cwd,
            diagnostic=Diagnostic(
                "repository-json",
                "GitHub repository unavailable",
                "gh returned incomplete repository data",
                error=True,
            ),
        )
    repo = repository["nameWithOwner"]
    try:
        status = run_json([gh, "pr", "status", "--json", PR_FIELDS], cwd=cwd)
    except CommandFailure as error:
        diagnostic = (
            Diagnostic("auth", "GitHub authentication required", "Run gh auth login", error=True)
            if not auth_is_ready()
            else Diagnostic("gh-error", "Pull request refresh failed", failure_detail(error), error=True)
        )
        return Dashboard(cwd, repo=repo, diagnostic=diagnostic)

    groups = [
        ("Current branch", "currentBranch", True),
        ("Authored by you", "createdBy", False),
        ("Review requested", "needsReview", False),
    ]
    seen: set[int] = set()
    pull_requests: list[PullRequest] = []
    if isinstance(status, dict):
        for group, key, current in groups:
            values = status.get(key)
            if isinstance(values, dict):
                values = [values]
            if not isinstance(values, list):
                continue
            for raw in values:
                if not isinstance(raw, dict):
                    continue
                pr = summarize_pr(raw, group, current)
                if pr is None or pr.number in seen:
                    continue
                seen.add(pr.number)
                pull_requests.append(pr)
                if len(pull_requests) >= max_rows:
                    break
            if len(pull_requests) >= max_rows:
                break
    repo_url = repository.get("url")
    return Dashboard(
        cwd,
        repo=repo,
        repo_url=repo_url if isinstance(repo_url, str) else None,
        pull_requests=pull_requests,
    )


def notify(message: str, *, error: bool = False) -> None:
    argv = [ROZI, "notify", message]
    if error:
        argv.extend(["--title", "PR dashboard", "--level", "error"])
    try:
        subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        pass


def notify_diagnostic(
    diagnostic: Diagnostic | None, previous_signature: str | None
) -> str | None:
    if diagnostic is None or not diagnostic.notify:
        return None
    signature = f"{diagnostic.code}\0{diagnostic.reason}"
    if signature != previous_signature:
        notify(f"{diagnostic.title}: {diagnostic.reason}", error=diagnostic.error)
    return signature


def notify_transitions(previous: Dashboard | None, current: Dashboard) -> None:
    if previous is None or previous.repo != current.repo:
        return
    old = {pr.number: pr for pr in previous.pull_requests}
    for pr in current.pull_requests:
        prior = old.get(pr.number)
        if prior is None or prior.status == pr.status:
            continue
        if pr.status == "blocked":
            notify(f"#{pr.number} {pr.reason} · {current.repo}", error=True)
        elif pr.status == "done" and prior.status in {"blocked", "working"}:
            notify(f"#{pr.number} ready · {current.repo}")


def list_pane_cwd(pane_id: object) -> Path | None:
    try:
        response = run_json([ROZI, "list-panes"])
    except CommandFailure:
        return None
    panes = response.get("data") if isinstance(response, dict) else None
    if not isinstance(panes, list):
        return None
    wanted = str(pane_id)
    for pane in panes:
        if not isinstance(pane, dict) or str(pane.get("id")) != wanted:
            continue
        cwd = pane.get("cwd")
        return Path(cwd) if isinstance(cwd, str) and cwd else None
    return None


def open_pr(dashboard: Dashboard, selected: object, *, checks: bool = False) -> None:
    if not isinstance(selected, str) or dashboard.repo is None:
        return
    prefix = f"pr:{dashboard.repo}#"
    if not selected.startswith(prefix):
        return
    number = selected.removeprefix(prefix)
    if not number.isdigit():
        return
    gh = shutil.which("gh")
    if gh is None:
        return
    argv = [gh, "pr", "checks" if checks else "view", number, "--web", "--repo", dashboard.repo]
    result = run(argv, cwd=dashboard.cwd)
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()
        notify(detail[0][:160] if detail else "Could not open pull request", error=True)


def run_picker(cwd: Path | None, max_rows: int) -> int:
    dashboard = fetch_dashboard(cwd, max_rows)
    process = subprocess.Popen(
        [ROZI, "pick", "--json"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert process.stdin is not None and process.stdout is not None

    def send(payload: dict[str, object]) -> bool:
        try:
            process.stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
            process.stdin.flush()
            return True
        except (BrokenPipeError, OSError):
            return False

    request = {
        "title": f"Pull requests · {dashboard.repo or 'focused pane'}",
        "placeholder": "Filter pull requests…",
        "width": 88,
        "actions": [
            {"id": "refresh", "key": "r", "label": "refresh"},
            {"id": "checks", "key": "c", "label": "checks", "close": True},
        ],
        "rows": dashboard.picker_rows(),
    }
    if not send(request):
        return process.wait()
    for line in process.stdout:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("cancelled"):
            break
        action = event.get("action")
        selected = event.get("selected")
        if action == "refresh":
            dashboard = fetch_dashboard(cwd, max_rows)
            if not send({"rows": dashboard.picker_rows()}):
                break
        elif action == "checks":
            open_pr(dashboard, selected, checks=True)
            break
        elif selected:
            open_pr(dashboard, selected)
            break
    try:
        process.stdin.close()
    except OSError:
        pass
    return process.wait()


def stream_reader(
    name: str,
    process: subprocess.Popen[str],
    messages: queue.Queue[tuple[str, object]],
) -> None:
    assert process.stdout is not None
    for line in process.stdout:
        try:
            messages.put((name, json.loads(line)))
        except json.JSONDecodeError:
            continue
    messages.put(("closed", (name, process)))


def stop_process(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_service() -> int:
    if os.environ.get("ROZI_EXTENSION") != "pr-dashboard":
        print("pr-dashboard must be launched by Rozi", file=sys.stderr)
        return 2

    poll_seconds = bounded_env_int("PR_DASHBOARD_POLL_SECONDS", 120, 60, 900)
    max_rows = bounded_env_int("PR_DASHBOARD_MAX_ROWS", 12, 1, 30)
    messages: queue.Queue[tuple[str, object]] = queue.Queue()
    try:
        subscriber = subprocess.Popen(
            [ROZI, "subscribe", *SUBSCRIBED_EVENTS],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        publisher = subprocess.Popen(
            [ROZI, "publish"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except OSError as error:
        print(f"could not start Rozi stream: {error}", file=sys.stderr)
        return 1
    assert publisher.stdin is not None
    threading.Thread(
        target=stream_reader, args=("event", subscriber, messages), daemon=True
    ).start()
    threading.Thread(
        target=stream_reader, args=("activation", publisher, messages), daemon=True
    ).start()

    dashboard = fetch_dashboard(None, max_rows)
    focused_pane: object = None
    next_refresh = float("inf")
    last_refresh = 0.0
    last_diagnostic: str | None = None
    picker_lock = threading.Lock()
    activation_targets: collections.OrderedDict[str, Dashboard] = (
        collections.OrderedDict()
    )

    def publish() -> bool:
        rows = dashboard.publish_rows()
        try:
            publisher.stdin.write(
                json.dumps({"rows": rows}, separators=(",", ":")) + "\n"
            )
            publisher.stdin.flush()
        except (BrokenPipeError, OSError):
            return False
        for row in rows:
            identifier = row.get("id")
            if not isinstance(identifier, str):
                continue
            activation_targets[identifier] = dashboard
            activation_targets.move_to_end(identifier)
        while len(activation_targets) > 100:
            activation_targets.popitem(last=False)
        return True

    def open_picker_for(snapshot: Dashboard) -> None:
        if not picker_lock.acquire(blocking=False):
            return

        def worker() -> None:
            try:
                run_picker(snapshot.cwd, max_rows)
            except (CommandFailure, OSError) as error:
                notify(f"Picker failed: {error}", error=True)
            finally:
                picker_lock.release()

        threading.Thread(target=worker, daemon=True).start()

    try:
        if not publish():
            return publisher.wait()
        while True:
            timeout = None if next_refresh == float("inf") else max(0.0, next_refresh - time.monotonic())
            try:
                kind, payload = messages.get(timeout=timeout)
            except queue.Empty:
                kind, payload = "refresh", None

            if kind == "closed":
                _, process = payload
                try:
                    return process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    return 1
            if kind == "activation" and isinstance(payload, dict):
                activated = payload.get("activate")
                if isinstance(activated, str):
                    open_picker_for(activation_targets.get(activated, dashboard))
                continue
            if kind == "event" and isinstance(payload, dict):
                event = payload.get("event")
                raw_data = payload.get("data")
                data = raw_data if isinstance(raw_data, dict) else {}
                if event == "focus-changed":
                    focused_pane = data.get("pane")
                    cwd = list_pane_cwd(focused_pane)
                    if cwd != dashboard.cwd:
                        dashboard = fetch_dashboard(cwd, max_rows)
                        notify_transitions(None, dashboard)
                        last_diagnostic = notify_diagnostic(dashboard.diagnostic, last_diagnostic)
                        last_refresh = time.monotonic()
                        next_refresh = last_refresh + poll_seconds
                        if not publish():
                            return publisher.wait()
                        continue
                elif event == "pane-exited" and str(data.get("pane")) == str(focused_pane):
                    focused_pane = None
                    dashboard = fetch_dashboard(None, max_rows)
                    next_refresh = float("inf")
                    if not publish():
                        return publisher.wait()
                    continue
                elif event in {"session-attached", "session-detached"}:
                    focused_pane = None
                    dashboard = fetch_dashboard(None, max_rows)
                    next_refresh = float("inf")
                    if not publish():
                        return publisher.wait()
                    continue
                if dashboard.cwd is not None:
                    next_refresh = min(
                        next_refresh,
                        max(
                            time.monotonic(),
                            last_refresh + MIN_EVENT_REFRESH_SECONDS,
                        ),
                    )
                continue
            if kind != "refresh" or dashboard.cwd is None:
                continue

            previous = dashboard
            dashboard = fetch_dashboard(previous.cwd, max_rows)
            notify_transitions(previous, dashboard)
            last_diagnostic = notify_diagnostic(dashboard.diagnostic, last_diagnostic)
            last_refresh = time.monotonic()
            next_refresh = last_refresh + poll_seconds
            if dashboard != previous and not publish():
                return publisher.wait()
    finally:
        stop_process(subscriber)
        stop_process(publisher)


def main() -> int:
    parser = argparse.ArgumentParser(description="Rozi pull-request dashboard")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--service", action="store_true")
    mode.add_argument("--pick", action="store_true")
    args = parser.parse_args()
    if args.service:
        return run_service()
    if os.environ.get("ROZI_EXTENSION") != "pr-dashboard":
        print("pr-dashboard must be launched by Rozi", file=sys.stderr)
        return 2
    max_rows = bounded_env_int("PR_DASHBOARD_MAX_ROWS", 12, 1, 30)
    try:
        return run_picker(Path.cwd(), max_rows)
    except (CommandFailure, OSError) as error:
        notify(f"PR dashboard failed: {error}", error=True)
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
