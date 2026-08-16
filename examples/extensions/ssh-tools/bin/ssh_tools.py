#!/usr/bin/env python3
"""Canonical SSH host picker using only Rozi's public extension CLI."""

from __future__ import annotations

import base64
import glob
import json
import os
import shlex
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


ROZI = os.environ.get("ROZI_BIN", "rozi")
DESCRIPTION_WORKERS = 8
DESCRIPTION_TIMEOUT_SECONDS = 2
CONNECT_CODE = (
    "import base64, os, sys; "
    "decode=lambda value: base64.urlsafe_b64decode(value).decode('utf-8'); "
    "executable=decode(sys.argv[1]); host=decode(sys.argv[2]); "
    "os.execv(executable,[executable,'--',host])"
)


def notify_error(message: str) -> None:
    subprocess.run(
        [ROZI, "notify", message, "--title", "SSH tools", "--level", "error"],
        check=False,
    )


def parse_directive(line: str) -> tuple[str, list[str]] | None:
    try:
        tokens = shlex.split(line, comments=True, posix=True)
    except ValueError:
        return None
    if not tokens:
        return None

    keyword = tokens.pop(0)
    if "=" in keyword:
        keyword, first_value = keyword.split("=", 1)
        if first_value:
            tokens.insert(0, first_value)
    elif tokens and tokens[0] == "=":
        tokens.pop(0)
    elif tokens and tokens[0].startswith("="):
        tokens[0] = tokens[0][1:]
    return keyword.casefold(), [token for token in tokens if token]


def include_paths(pattern: str, ssh_dir: Path) -> list[Path]:
    expanded = pattern.replace("%d", str(Path.home()))
    expanded = os.path.expanduser(expanded)
    candidate = Path(expanded)
    if not candidate.is_absolute():
        candidate = ssh_dir / candidate
    return [
        Path(match)
        for match in sorted(glob.glob(str(candidate)))
        if Path(match).is_file()
    ]


def is_concrete_alias(pattern: str) -> bool:
    return (
        bool(pattern)
        and not pattern.startswith("!")
        and not any(character in pattern for character in "*?[")
    )


def read_aliases(config_path: Path) -> tuple[list[str], list[tuple[Path, str]]]:
    ssh_dir = config_path.parent
    aliases: list[str] = []
    alias_keys: set[str] = set()
    visited: set[str] = set()
    warnings: list[tuple[Path, str]] = []

    def visit(path: Path) -> None:
        key = os.path.normcase(os.path.realpath(path))
        if key in visited:
            return
        visited.add(key)
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            warnings.append((path, error.strerror or str(error)))
            return
        except UnicodeError:
            warnings.append((path, "Not UTF-8"))
            return

        for line in text.lstrip("\ufeff").splitlines():
            directive = parse_directive(line)
            if directive is None:
                continue
            keyword, values = directive
            if keyword == "include":
                for pattern in values:
                    for included in include_paths(pattern, ssh_dir):
                        visit(included)
            elif keyword == "host":
                for alias in values:
                    folded = alias.casefold()
                    if is_concrete_alias(alias) and folded not in alias_keys:
                        alias_keys.add(folded)
                        aliases.append(alias)

    visit(config_path)
    return aliases, warnings


def effective_description(ssh: str, alias: str) -> str:
    try:
        result = subprocess.run(
            [ssh, "-G", "--", alias],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=DESCRIPTION_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ""
    if result.returncode != 0:
        return ""

    fields: dict[str, str] = {}
    for line in result.stdout.splitlines():
        parts = line.split(None, 1)
        if len(parts) == 2:
            fields.setdefault(parts[0].casefold(), parts[1])

    hostname = fields.get("hostname")
    if not hostname:
        return ""
    port = fields.get("port")
    if port and port != "22":
        display_host = f"[{hostname}]" if ":" in hostname else hostname
        hostname = f"{display_host}:{port}"
    user = fields.get("user")
    target = f"{user}@{hostname}" if user else hostname
    proxyjump = fields.get("proxyjump")
    if proxyjump and proxyjump.casefold() != "none":
        target += f" · via {proxyjump}"
    return target


def describe_aliases(ssh: str, aliases: list[str]) -> list[str]:
    if not aliases:
        return []
    workers = min(DESCRIPTION_WORKERS, len(aliases))
    with ThreadPoolExecutor(max_workers=workers) as executor:
        return list(executor.map(lambda alias: effective_description(ssh, alias), aliases))


def picker_rows(
    ssh: str, config_path: Path
) -> tuple[list[dict[str, object]], set[str]]:
    if not config_path.is_file():
        return (
            [
                {
                    "id": "__missing_config",
                    "label": "SSH config",
                    "group": "Unavailable",
                    "disabled": "Not found",
                }
            ],
            set(),
        )

    aliases, warnings = read_aliases(config_path)
    descriptions = describe_aliases(ssh, aliases)
    rows: list[dict[str, object]] = []
    for alias, description in zip(aliases, descriptions):
        row: dict[str, object] = {
            "id": alias,
            "label": alias,
            "group": "Hosts",
        }
        if description:
            row["description"] = description
        rows.append(row)

    if not aliases:
        rows.append(
            {
                "id": "__no_aliases",
                "label": "Concrete Host aliases",
                "group": "Unavailable",
                "disabled": "None found",
            }
        )
    for path, reason in warnings:
        rows.append(
            {
                "id": f"__warning_{len(rows)}",
                "label": str(path),
                "group": "Unreadable config",
                "disabled": reason,
            }
        )
    return rows, set(aliases)


def encode_argument(value: str) -> str:
    return base64.urlsafe_b64encode(value.encode("utf-8")).decode("ascii")


def pane_command(ssh: str, alias: str) -> str:
    # Rozi currently accepts one command string at this boundary. Keep every dynamic value in
    # URL-safe base64, then recover structured argv in the pane before replacing Python with ssh.
    argv = [
        sys.executable,
        "-c",
        CONNECT_CODE,
        encode_argument(ssh),
        encode_argument(alias),
    ]
    if os.name == "nt":
        return subprocess.list2cmdline(argv)
    return shlex.join(argv)


def open_host(ssh: str, alias: str) -> None:
    try:
        result = subprocess.run(
            [ROZI, "new-pane", "--focus", pane_command(ssh, alias)],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
    except OSError as error:
        notify_error(f"could not run rozi new-pane: {error}")
        return
    if result.returncode == 0:
        return

    detail = result.stderr.strip()
    if not detail and result.stdout.strip():
        try:
            detail = str(json.loads(result.stdout).get("error", "")).strip()
        except (AttributeError, json.JSONDecodeError):
            detail = result.stdout.strip()
    notify_error(detail or "could not open SSH pane")


def run_picker(ssh: str, config_path: Path) -> int:
    try:
        process = subprocess.Popen(
            [ROZI, "pick", "--json"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            encoding="utf-8",
            bufsize=1,
        )
    except OSError as error:
        print(f"could not run rozi pick: {error}", file=sys.stderr)
        return 2
    assert process.stdin is not None and process.stdout is not None

    def send(payload: dict[str, object]) -> bool:
        try:
            process.stdin.write(json.dumps(payload, ensure_ascii=False) + "\n")
            process.stdin.flush()
            return True
        except (BrokenPipeError, OSError):
            return False

    rows, selectable = picker_rows(ssh, config_path)
    if not send(
        {
            "title": "SSH hosts",
            "placeholder": "Filter hosts…",
            "actions": [{"id": "refresh", "key": "r", "label": "refresh"}],
            "rows": rows,
        }
    ):
        return process.wait()

    for line in process.stdout:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("cancelled"):
            break
        if event.get("action") == "refresh":
            rows, selectable = picker_rows(ssh, config_path)
            if not send({"rows": rows}):
                break
            continue
        selected = event.get("selected")
        if isinstance(selected, str) and selected in selectable:
            open_host(ssh, selected)
            break

    try:
        process.stdin.close()
    except OSError:
        pass
    return process.wait()


def main() -> int:
    if os.environ.get("ROZI_EXTENSION") != "ssh-tools":
        print("ssh-tools must be launched by Rozi", file=sys.stderr)
        return 2

    discovered_ssh = shutil.which("ssh")
    if discovered_ssh is None:
        notify_error("OpenSSH client `ssh` was not found on PATH")
        return 0
    ssh = os.path.abspath(discovered_ssh)
    try:
        config_path = Path.home() / ".ssh" / "config"
    except RuntimeError as error:
        notify_error(f"could not locate the user SSH config: {error}")
        return 0
    return run_picker(ssh, config_path)


if __name__ == "__main__":
    raise SystemExit(main())
