#!/usr/bin/env python3
"""Prepare deterministic release evidence and validate generated release notes."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Callable, Sequence


SECTIONS = ("Added", "Changed", "Fixed", "Compatibility", "Security")
FORBIDDEN_PUNCTUATION = {
    "—": "em dash",
    "“": "curly double quote",
    "”": "curly double quote",
    "‘": "curly single quote",
    "’": "curly single quote",
}
RELEASE_METADATA_PATHS = {
    "Cargo.lock",
    "Cargo.toml",
    "docs/installation.md",
}


class ReleaseNotesError(RuntimeError):
    """A release-note invariant was not satisfied."""


@dataclass(frozen=True)
class Commit:
    sha: str
    subject: str
    paths: tuple[str, ...]
    included: bool
    exclusion_reason: str | None


def run(command: Sequence[str]) -> str:
    try:
        result = subprocess.run(
            command,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except subprocess.CalledProcessError as error:
        detail = error.stderr.strip() or error.stdout.strip() or f"exit {error.returncode}"
        raise ReleaseNotesError(f"{' '.join(command)} failed: {detail}") from error
    return result.stdout


def git(*arguments: str) -> str:
    return run(("git", *arguments))


def github_releases(repository: str) -> list[dict[str, Any]]:
    pages = json.loads(
        run(
            (
                "gh",
                "api",
                "--paginate",
                "--slurp",
                f"repos/{repository}/releases?per_page=100",
            )
        )
    )
    if not isinstance(pages, list):
        raise ReleaseNotesError("GitHub releases response is not a list")
    releases: list[dict[str, Any]] = []
    for page in pages:
        if not isinstance(page, list):
            raise ReleaseNotesError("GitHub releases response contains a malformed page")
        releases.extend(item for item in page if isinstance(item, dict))
    return releases


def select_previous_release(releases: Sequence[dict[str, Any]], to_tag: str) -> str:
    candidates = [
        release
        for release in releases
        if not release.get("draft", False)
        and isinstance(release.get("tag_name"), str)
        and release["tag_name"].startswith("v")
        and release["tag_name"] != to_tag
        and (release.get("published_at") or release.get("created_at"))
    ]
    if not candidates:
        raise ReleaseNotesError("no previous published, non-draft v-tagged release exists")
    previous = max(
        candidates,
        key=lambda release: (
            release.get("published_at") or release.get("created_at"),
            release.get("id", 0),
        ),
    )
    return str(previous["tag_name"])


def resolve_tag(tag: str) -> str:
    return git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}").strip()


def changed_paths(sha: str) -> tuple[str, ...]:
    return tuple(
        path
        for path in git(
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            sha,
        ).splitlines()
        if path
    )


def paths_are(paths: Sequence[str], predicate: Callable[[str], bool]) -> bool:
    return bool(paths) and all(predicate(path) for path in paths)


def exclusion_reason(subject: str, paths: Sequence[str]) -> str | None:
    lowered = subject.lower()
    if re.match(r"^ci(?:\([^)]*\))?:", lowered) and paths_are(
        paths,
        lambda path: path.startswith(".github/")
        or path.startswith(".agents/")
        or path == "docs/release-process.md",
    ):
        return "CI-only change"
    if re.match(r"^test(?:\([^)]*\))?:", lowered) and paths_are(
        paths,
        lambda path: path.startswith("tests/")
        or path.startswith("benches/")
        or "/fixtures/" in path,
    ):
        return "test-only change"
    if re.match(r"^docs(?:\([^)]*\))?:", lowered) and paths_are(
        paths,
        lambda path: path.startswith("docs/")
        or path == "README.md",
    ):
        return "documentation-only change"
    if re.match(r"^chore(?:\(release\))?:\s+release\b", lowered) and set(
        paths
    ).issubset(RELEASE_METADATA_PATHS):
        return "release metadata only"
    return None


def collect_commits(from_commit: str, to_commit: str) -> list[Commit]:
    shas = [
        sha
        for sha in git(
            "rev-list",
            "--reverse",
            "--topo-order",
            f"{from_commit}..{to_commit}",
        ).splitlines()
        if sha
    ]
    if not shas:
        raise ReleaseNotesError("release range contains no commits")
    commits = []
    for sha in shas:
        subject = git("show", "-s", "--format=%s", sha).strip()
        paths = changed_paths(sha)
        reason = exclusion_reason(subject, paths)
        commits.append(
            Commit(
                sha=sha,
                subject=subject,
                paths=paths,
                included=reason is None,
                exclusion_reason=reason,
            )
        )
    return commits


def write_input(
    path: Path,
    from_tag: str,
    from_commit: str,
    to_tag: str,
    to_commit: str,
    commits: Sequence[Commit],
) -> None:
    lines = [
        "# Deterministic release-note input",
        "",
        f"- FROM tag: `{from_tag}`",
        f"- FROM commit: `{from_commit}`",
        f"- TO tag: `{to_tag}`",
        f"- TO commit: `{to_commit}`",
        f"- Exact range: `{from_tag}..{to_tag}`",
        "",
        "## Candidate commits",
        "",
    ]
    included = [commit for commit in commits if commit.included]
    excluded = [commit for commit in commits if not commit.included]
    if not included:
        raise ReleaseNotesError("deterministic filtering removed every commit in the release")

    for index, commit in enumerate(included, start=1):
        lines.extend(
            [
                f"### Candidate {index}: `{commit.sha}`",
                "",
                f"Subject: {commit.subject}",
                "",
                "Changed files:",
                *[f"- `{changed}`" for changed in commit.paths],
                "",
            ]
        )

    lines.extend(["## Deterministically excluded commits", ""])
    if excluded:
        lines.extend(
            f"- `{commit.sha}` — {commit.subject} ({commit.exclusion_reason})"
            for commit in excluded
        )
    else:
        lines.append("- None.")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def normalize_notes(raw: str) -> str:
    notes = raw.replace("\r\n", "\n").strip()
    if not notes:
        raise ReleaseNotesError("generated release notes are empty")
    if len(notes.encode("utf-8")) > 32_768:
        raise ReleaseNotesError("generated release notes exceed 32 KiB")
    if "```" in notes:
        raise ReleaseNotesError("generated release notes contain a code fence")
    for character, label in FORBIDDEN_PUNCTUATION.items():
        if character in notes:
            raise ReleaseNotesError(f"generated release notes contain a {label}")
    return notes


def parse_heading(line: str, line_number: int) -> str | None:
    if not line.startswith("#"):
        return None
    if not line.startswith("## "):
        raise ReleaseNotesError(f"invalid heading on line {line_number}: {line}")
    heading = line[3:].strip()
    if heading not in SECTIONS:
        raise ReleaseNotesError(f"unknown section on line {line_number}: {heading}")
    return heading


def collect_headings(lines: Sequence[str]) -> list[tuple[int, str]]:
    headings: list[tuple[int, str]] = []
    for line_number, line in enumerate(lines, start=1):
        heading = parse_heading(line, line_number)
        if heading is not None:
            headings.append((line_number, heading))
    return headings


def validate_heading_order(headings: Sequence[tuple[int, str]]) -> None:
    if not headings:
        raise ReleaseNotesError("generated release notes contain no sections")
    names = [heading for _, heading in headings]
    if len(names) != len(set(names)):
        raise ReleaseNotesError("generated release notes repeat a section")
    expected_order = sorted(names, key=SECTIONS.index)
    if names != expected_order:
        raise ReleaseNotesError(
            f"sections are out of order: expected {expected_order}, got {names}"
        )


def is_bullet_line(line: str) -> bool:
    return line.startswith("- ") or line.startswith("  ")


def validate_section_body(heading: str, body: Sequence[str]) -> None:
    content = [line for line in body if line.strip()]
    if not content:
        raise ReleaseNotesError(f"{heading} section has no bullet")
    if not any(line.startswith("- ") for line in content):
        raise ReleaseNotesError(f"{heading} section has no bullet")
    invalid = next((line for line in content if not is_bullet_line(line)), None)
    if invalid is not None:
        raise ReleaseNotesError(f"{heading} contains non-bullet content: {invalid}")


def validate_notes(raw: str) -> str:
    notes = normalize_notes(raw)
    lines = notes.splitlines()
    headings = collect_headings(lines)
    validate_heading_order(headings)
    first_content = next(line for line in lines if line.strip())
    if not first_content.startswith("## "):
        raise ReleaseNotesError("generated release notes have text before the first section")
    for index, (line_number, heading) in enumerate(headings):
        end = headings[index + 1][0] - 1 if index + 1 < len(headings) else len(lines)
        validate_section_body(heading, lines[line_number:end])
    return notes + "\n"


def prepare(arguments: argparse.Namespace) -> None:
    releases = github_releases(arguments.repository)
    from_tag = select_previous_release(releases, arguments.to_tag)
    from_commit = resolve_tag(from_tag)
    to_commit = resolve_tag(arguments.to_tag)
    if arguments.expected_to_sha and to_commit != arguments.expected_to_sha:
        raise ReleaseNotesError(
            f"{arguments.to_tag} resolves to {to_commit}, expected {arguments.expected_to_sha}"
        )
    try:
        subprocess.run(
            ("git", "merge-base", "--is-ancestor", from_commit, to_commit),
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except subprocess.CalledProcessError as error:
        raise ReleaseNotesError(
            f"previous release {from_tag} ({from_commit}) is not an ancestor of "
            f"{arguments.to_tag} ({to_commit})"
        ) from error

    commits = collect_commits(from_commit, to_commit)
    input_path = Path(arguments.input)
    metadata_path = Path(arguments.metadata)
    write_input(
        input_path,
        from_tag,
        from_commit,
        arguments.to_tag,
        to_commit,
        commits,
    )
    metadata_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "from_tag": from_tag,
                "from_commit": from_commit,
                "to_tag": arguments.to_tag,
                "to_commit": to_commit,
                "commits": [asdict(commit) for commit in commits],
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(
        f"prepared {sum(commit.included for commit in commits)} candidate commits "
        f"from {from_tag}..{arguments.to_tag}"
    )


def validate(arguments: argparse.Namespace) -> None:
    raw = Path(arguments.input).read_text(encoding="utf-8")
    Path(arguments.output).write_text(validate_notes(raw), encoding="utf-8")
    print(f"validated release notes in {arguments.output}")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)

    prepare_parser = commands.add_parser("prepare")
    prepare_parser.add_argument("--repository", required=True)
    prepare_parser.add_argument("--to-tag", required=True)
    prepare_parser.add_argument("--expected-to-sha")
    prepare_parser.add_argument("--input", required=True)
    prepare_parser.add_argument("--metadata", required=True)
    prepare_parser.set_defaults(function=prepare)

    validate_parser = commands.add_parser("validate")
    validate_parser.add_argument("--input", required=True)
    validate_parser.add_argument("--output", required=True)
    validate_parser.set_defaults(function=validate)
    return root


def main() -> int:
    arguments = parser().parse_args()
    try:
        arguments.function(arguments)
    except (OSError, ValueError, ReleaseNotesError) as error:
        print(f"release notes: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
