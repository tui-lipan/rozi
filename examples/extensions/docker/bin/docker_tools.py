#!/usr/bin/env python3
"""Canonical Docker extension using only Rozi's public environment and CLI."""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ROZI = os.environ.get("ROZI_BIN", "rozi")
CONTAINER_ID = re.compile(r"^[0-9a-fA-F]{12,64}$")
RUNNING_STATES = {"running", "paused", "restarting"}
DISABLED_STATES = {
    "paused": "Paused",
    "restarting": "Restarting",
    "removing": "Removing",
    "dead": "Dead",
    "unknown": "Unknown state",
}


class DockerError(RuntimeError):
    """A concise error suitable for a picker row or toast."""


@dataclass(frozen=True)
class Container:
    id: str
    name: str
    image: str
    state: str
    status: str

    @property
    def running(self) -> bool:
        return self.state == "running"

    @property
    def group(self) -> str:
        return "Running" if self.state in RUNNING_STATES else "Stopped"

    @property
    def disabled(self) -> str | None:
        return DISABLED_STATES.get(self.state)


def concise_error(stderr: str) -> str:
    detail = next((line.strip() for line in stderr.splitlines() if line.strip()), "")
    lowered = detail.lower()
    if "permission denied" in lowered:
        return "Docker daemon permission denied"
    if (
        "cannot connect to the docker daemon" in lowered
        or "error during connect" in lowered
        or "is the docker daemon running" in lowered
    ):
        return "Docker daemon unavailable"
    for prefix in ("Error response from daemon:", "docker:"):
        if detail.lower().startswith(prefix.lower()):
            detail = detail[len(prefix) :].strip()
            break
    return detail[:180] or "Docker command failed"


def docker(*args: str) -> subprocess.CompletedProcess[str]:
    try:
        process = subprocess.run(
            ["docker", *args],
            text=True,
            capture_output=True,
            check=False,
        )
    except FileNotFoundError as error:
        raise DockerError("Docker CLI not found") from error
    if process.returncode != 0:
        raise DockerError(concise_error(process.stderr))
    return process


def discover_containers() -> list[Container]:
    output = docker(
        "container",
        "ls",
        "--all",
        "--no-trunc",
        "--format",
        "{{json .}}",
    ).stdout
    containers = []
    for line in output.splitlines():
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise DockerError("Docker returned invalid container data") from error
        container_id = str(record.get("ID", ""))
        if not CONTAINER_ID.fullmatch(container_id):
            raise DockerError("Docker returned invalid container data")
        containers.append(
            Container(
                id=container_id.lower(),
                name=str(record.get("Names") or container_id[:12]),
                image=str(record.get("Image") or "unknown image"),
                state=str(record.get("State") or "unknown").lower(),
                status=str(record.get("Status") or ""),
            )
        )
    return sorted(
        containers,
        key=lambda container: (
            container.group != "Running",
            container.name.casefold(),
            container.id,
        ),
    )


def container_row(container: Container) -> dict[str, object]:
    description = container.image
    if container.status:
        description = f"{description} · {container.status}"
    row: dict[str, object] = {
        "id": container.id,
        "label": container.name,
        "group": container.group,
        "description": description,
        "active": container.running,
    }
    if container.disabled:
        row["disabled"] = container.disabled
    return row


def snapshot() -> tuple[list[dict[str, object]], dict[str, Container]]:
    try:
        containers = discover_containers()
    except DockerError as error:
        message = str(error)
        return (
            [
                {
                    "id": "__docker_unavailable__",
                    "label": "Docker unavailable",
                    "group": "Status",
                    "description": message,
                    "disabled": message,
                }
            ],
            {},
        )
    if not containers:
        return (
            [
                {
                    "id": "__docker_empty__",
                    "label": "No containers",
                    "group": "Status",
                    "description": "docker container ls --all",
                    "disabled": "Nothing to select",
                }
            ],
            {},
        )
    return (
        [container_row(container) for container in containers],
        {container.id: container for container in containers},
    )


def picker_request(rows: list[dict[str, object]]) -> dict[str, object]:
    return {
        "title": "Docker containers",
        "placeholder": "Filter containers…",
        "width": 92,
        "actions": [
            {"id": "refresh", "key": "r", "label": "refresh"},
            {"id": "start", "key": "s", "label": "start"},
            {"id": "stop", "key": "x", "label": "stop"},
            {"id": "restart", "key": "ctrl-r", "label": "restart"},
            {"id": "inspect", "key": "i", "label": "inspect", "close": True},
            {"id": "logs", "key": "l", "label": "logs", "close": True},
            {"id": "shell", "key": "e", "label": "shell", "close": True},
            {
                "id": "remove",
                "key": "ctrl-d",
                "label": "remove",
                "confirm": True,
            },
        ],
        "rows": rows,
    }


def notify(message: str) -> None:
    try:
        subprocess.run(
            [ROZI, "notify", message, "--title", "Docker", "--level", "error"],
            check=False,
        )
    except OSError:
        print(message, file=sys.stderr)


def selected_container(
    selected: object, containers: dict[str, Container]
) -> Container | None:
    if not isinstance(selected, str) or not CONTAINER_ID.fullmatch(selected):
        return None
    return containers.get(selected.lower())


def mutation_error(action: str, container: Container) -> str | None:
    if container.disabled:
        return container.disabled
    if action == "start" and container.running:
        return "Container already running"
    if action in {"stop", "restart"} and not container.running:
        return "Container not running"
    if action == "remove" and container.state in RUNNING_STATES:
        return "Stop container before removal"
    return None


def mutate(action: str, container: Container) -> None:
    command = {
        "start": ("container", "start", "--", container.id),
        "stop": ("container", "stop", "--", container.id),
        "restart": ("container", "restart", "--", container.id),
        "remove": ("container", "rm", "--", container.id),
    }[action]
    docker(*command)


def open_pane(mode: str, container: Container) -> None:
    if mode == "shell" and not container.running:
        notify("Container not running")
        return
    if container.disabled:
        notify(container.disabled)
        return
    script = str(Path(os.path.abspath(__file__)))
    try:
        process = subprocess.run(
            [
                ROZI,
                "split",
                "--focus",
                "--argv",
                sys.executable,
                script,
                "--pane",
                mode,
                container.id,
            ],
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError:
        notify("Could not launch Rozi pane")
        return
    if process.returncode != 0:
        notify(concise_error(process.stderr))


def run_picker() -> int:
    rows, containers = snapshot()
    try:
        process = subprocess.Popen(
            [ROZI, "pick", "--json"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
    except OSError as error:
        print(f"could not start rozi pick: {error}", file=sys.stderr)
        return 2
    assert process.stdin is not None and process.stdout is not None

    def send(payload: dict[str, object]) -> bool:
        try:
            process.stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
            process.stdin.flush()
        except (BrokenPipeError, OSError):
            return False
        return True

    if not send(picker_request(rows)):
        return process.wait()

    cancelled = False
    for line in process.stdout:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("cancelled"):
            cancelled = True
            break

        action = event.get("action")
        if action == "refresh":
            rows, containers = snapshot()
            if not send({"rows": rows}):
                break
            continue

        container = selected_container(event.get("selected"), containers)
        if container is None:
            notify("Container no longer available")
            if action in {"start", "stop", "restart", "remove"}:
                rows, containers = snapshot()
                if not send({"rows": rows}):
                    break
            continue

        if action in {"start", "stop", "restart", "remove"}:
            rejection = mutation_error(str(action), container)
            if rejection:
                notify(rejection)
            else:
                try:
                    mutate(str(action), container)
                except DockerError as error:
                    notify(str(error))
            rows, containers = snapshot()
            if not send({"rows": rows}):
                break
            continue

        if action in {"inspect", "logs", "shell"}:
            open_pane(str(action), container)
            break

        if event.get("selected"):
            open_pane("inspect", container)
            break

    try:
        process.stdin.close()
    except OSError:
        pass
    return_code = process.wait()
    return 0 if cancelled and return_code == 1 else return_code


def pane_docker(mode: str, container_id: str) -> int:
    if not CONTAINER_ID.fullmatch(container_id):
        print("invalid container id", file=sys.stderr)
        return 2
    commands = {
        "inspect": ["container", "inspect", "--", container_id],
        "logs": ["container", "logs", "--tail", "200", "--follow", "--", container_id],
        "shell": ["container", "exec", "-it", "--", container_id, "sh"],
    }
    command = commands.get(mode)
    if command is None:
        print(f"unknown pane mode: {mode}", file=sys.stderr)
        return 2
    try:
        docker("version", "--format", "{{.Server.Version}}")
    except DockerError as error:
        print(error, file=sys.stderr)
        return 1
    try:
        result = subprocess.run(["docker", *command], check=False)
    except FileNotFoundError:
        print("Docker CLI not found", file=sys.stderr)
        return 127
    if mode == "inspect" and sys.stdin.isatty():
        try:
            input("\nPress Enter to close")
        except (EOFError, KeyboardInterrupt):
            pass
    return result.returncode


def main() -> int:
    if len(sys.argv) == 4 and sys.argv[1] == "--pane":
        return pane_docker(sys.argv[2], sys.argv[3])
    if os.environ.get("ROZI_EXTENSION") != "docker":
        print("docker extension must be launched by Rozi", file=sys.stderr)
        return 2
    mode = sys.argv[1] if len(sys.argv) > 1 else "containers"
    if mode != "containers":
        print(f"unknown mode: {mode}", file=sys.stderr)
        return 2
    return run_picker()


if __name__ == "__main__":
    raise SystemExit(main())
