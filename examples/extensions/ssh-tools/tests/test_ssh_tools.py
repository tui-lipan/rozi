from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).parents[1] / "bin" / "ssh_tools.py"


def load_script():
    spec = importlib.util.spec_from_file_location("ssh_tools_example", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


ssh_tools = load_script()


class SshConfigTests(unittest.TestCase):
    def test_include_aliases_are_recursive_deduplicated_and_concrete(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            ssh_dir = Path(temporary_directory)
            included = ssh_dir / "conf.d"
            included.mkdir()
            config = ssh_dir / "config"
            config.write_text(
                """
                Include conf.d/*.conf
                Host root-local *.wild !negative host[12]
                """,
                encoding="utf-8",
            )
            (included / "10-hosts.conf").write_text(
                """
                Host Build build other
                Include config
                """,
                encoding="utf-8",
            )
            unreadable = included / "20-invalid.conf"
            unreadable.write_bytes(b"\xff\xfe")

            aliases, warnings = ssh_tools.read_aliases(config)

        self.assertEqual(aliases, ["Build", "other", "root-local"])
        self.assertEqual(len(warnings), 1)
        self.assertEqual(warnings[0][0], unreadable)
        self.assertEqual(warnings[0][1], "Not UTF-8")

    def test_directive_parser_accepts_openssh_assignment_forms(self) -> None:
        self.assertEqual(
            ssh_tools.parse_directive('Include = "conf dir/*.conf" # comment'),
            ("include", ["conf dir/*.conf"]),
        )
        self.assertEqual(
            ssh_tools.parse_directive("Host=production staging"),
            ("host", ["production", "staging"]),
        )
        self.assertIsNone(ssh_tools.parse_directive('Host "unterminated'))

    def test_pane_launch_preserves_ssh_path_and_alias_as_direct_arguments(self) -> None:
        ssh = "/opt/Open SSH/bin/ssh"
        alias = "prod; touch /tmp/unsafe $(id) ' quoted"

        with patch.object(ssh_tools.subprocess, "run") as run:
            run.return_value.returncode = 0
            ssh_tools.open_host(ssh, alias)

        self.assertEqual(
            run.call_args.args[0],
            [ssh_tools.ROZI, "new-pane", "--focus", "--argv", ssh, "--", alias],
        )


if __name__ == "__main__":
    unittest.main()
