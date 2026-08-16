from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).parents[1] / "bin" / "docker_tools.py"


def load_script():
    spec = importlib.util.spec_from_file_location("docker_tools_example", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


docker_tools = load_script()


def docker_result(records: list[dict[str, object]]) -> subprocess.CompletedProcess[str]:
    stdout = "\n".join(json.dumps(record) for record in records)
    return subprocess.CompletedProcess(["docker"], 0, stdout=stdout, stderr="")


class DockerDiscoveryTests(unittest.TestCase):
    def test_line_delimited_json_is_normalized_grouped_and_sorted(self) -> None:
        result = docker_result(
            [
                {
                    "ID": "B" * 12,
                    "Names": "stopped-zeta",
                    "Image": "busybox:latest",
                    "State": "exited",
                    "Status": "Exited (0)",
                },
                {
                    "ID": "a" * 64,
                    "Names": "running-alpha",
                    "Image": "alpine:3",
                    "State": "RUNNING",
                    "Status": "Up 2 minutes",
                },
                {
                    "ID": "c" * 12,
                    "Names": "",
                    "Image": "",
                    "State": "paused",
                    "Status": "",
                },
            ]
        )

        with mock.patch.object(docker_tools, "docker", return_value=result) as docker:
            containers = docker_tools.discover_containers()

        docker.assert_called_once_with(
            "container",
            "ls",
            "--all",
            "--no-trunc",
            "--format",
            "{{json .}}",
        )
        self.assertEqual(
            [container.name for container in containers],
            ["cccccccccccc", "running-alpha", "stopped-zeta"],
        )
        self.assertEqual(
            [container.group for container in containers],
            ["Running", "Running", "Stopped"],
        )
        self.assertEqual(containers[1].id, "a" * 64)
        self.assertEqual(containers[0].image, "unknown image")
        self.assertEqual(containers[0].disabled, "Paused")

    def test_invalid_json_or_container_id_is_rejected(self) -> None:
        invalid_json = subprocess.CompletedProcess(
            ["docker"], 0, stdout="{not json}\n", stderr=""
        )
        with mock.patch.object(docker_tools, "docker", return_value=invalid_json):
            with self.assertRaisesRegex(
                docker_tools.DockerError, "invalid container data"
            ):
                docker_tools.discover_containers()

        invalid_id = docker_result([{"ID": "--not-an-id", "Names": "unsafe"}])
        with mock.patch.object(docker_tools, "docker", return_value=invalid_id):
            with self.assertRaisesRegex(
                docker_tools.DockerError, "invalid container data"
            ):
                docker_tools.discover_containers()

    def test_missing_daemon_becomes_a_disabled_status_row(self) -> None:
        with mock.patch.object(
            docker_tools,
            "docker",
            side_effect=docker_tools.DockerError("Docker daemon unavailable"),
        ):
            rows, containers = docker_tools.snapshot()

        self.assertEqual(containers, {})
        self.assertEqual(rows, [
            {
                "id": "__docker_unavailable__",
                "label": "Docker unavailable",
                "group": "Status",
                "description": "Docker daemon unavailable",
                "disabled": "Docker daemon unavailable",
            }
        ])

    def test_daemon_connection_errors_are_concise(self) -> None:
        self.assertEqual(
            docker_tools.concise_error(
                "Cannot connect to the Docker daemon at unix:///run/docker.sock. "
                "Is the docker daemon running?"
            ),
            "Docker daemon unavailable",
        )


if __name__ == "__main__":
    unittest.main()
