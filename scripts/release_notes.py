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
SUMMARY_MAX_CHARS = 4_000
# Reviewer-facing sections that never reach the notes, whichever section a PR uses for its summary.
NON_RELEASE_SECTIONS = {
    "checklist",
    "notes for reviewers",
    "screenshots",
    "test plan",
    "testing",
    "validation",
    "verification",
}
UPGRADE_SECTION_RE = re.compile(r"\b(breaking|migrat|upgrad|compatibility)", re.IGNORECASE)
BREAKING_SECTION_RE = re.compile(r"\bbreaking\b", re.IGNORECASE)
NO_BREAKING_RE = re.compile(r"^(none|n/a|no\b)", re.IGNORECASE)
TITLE_RE = re.compile(r"^(?P<type>[a-z]+)(?:\([^)]*\))?(?P<breaking>!)?: \S")
# Commit types that are user-visible unless every changed path says otherwise.
USER_FACING_TYPES = {"feat", "fix", "perf"}
COMMIT_TYPES = USER_FACING_TYPES | {
    "bench",
    "build",
    "chore",
    "ci",
    "deps",
    "docs",
    "refactor",
    "release",
    "revert",
    "style",
    "test",
}
NON_USER_FACING_PATH_PREFIXES = (
    ".agents/",
    ".githooks/",
    ".github/",
    ".opencode/",
    "benches/",
    "dev/",
    "docs/",
    "scripts/",
    "tests/",
)
PR_LINK_RE = re.compile(r"/pull/(\d+)\b")
TRAILER_RE = re.compile(
    r"^(Signed-off-by|Co-authored-by|Reviewed-by|Acked-by):",
    re.IGNORECASE,
)
HEADING_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
FENCE_RE = re.compile(r"^\s*(```|~~~)")
PR_TRAILER_RE = re.compile(r"\(#(\d+)\)\s*$")


class ReleaseNotesError(RuntimeError):
    """A release-note invariant was not satisfied."""


@dataclass(frozen=True)
class Commit:
    sha: str
    subject: str
    summary: str
    paths: tuple[str, ...]
    included: bool
    exclusion_reason: str | None
    breaking: bool = False


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
        or path.startswith(".opencode/")
        or path.startswith("scripts/")
        or path == "dev/release-process.md",
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


@dataclass(frozen=True)
class Section:
    level: int
    title: str | None
    lines: tuple[str, ...]

    @property
    def name(self) -> str:
        return (self.title or "").strip().rstrip(":").strip().lower()

    def text(self) -> str:
        return "\n".join(self.lines).strip()

    def content(self) -> str:
        return "\n".join(self.lines[1:] if self.title is not None else self.lines).strip()


def clean_body(body: str) -> list[str]:
    text = re.sub(r"<!--.*?-->", "", body, flags=re.DOTALL)
    lines = text.replace("\r\n", "\n").splitlines()
    while lines and (
        not lines[-1].strip()
        or TRAILER_RE.match(lines[-1].strip())
        or set(lines[-1].strip()) <= {"-"}
    ):
        lines.pop()
    return lines


def split_sections(lines: Sequence[str]) -> list[Section]:
    """Split at headings, ignoring `#` lines inside fenced code."""
    sections: list[Section] = []
    level = 0
    title: str | None = None
    current: list[str] = []
    fence: str | None = None
    for line in lines:
        fence_match = FENCE_RE.match(line)
        if fence_match is not None:
            marker = fence_match.group(1)
            fence = marker if fence is None else (None if fence == marker else fence)
        heading = None if fence is not None else HEADING_RE.match(line)
        if heading is None:
            current.append(line)
            continue
        sections.append(Section(level, title, tuple(current)))
        level = len(heading.group(1))
        title = heading.group(2)
        current = [line]
    sections.append(Section(level, title, tuple(current)))
    return sections


def summary_range(sections: Sequence[Section]) -> range | None:
    """The Summary section together with the subsections nested under it."""
    start = next((i for i, section in enumerate(sections) if section.name == "summary"), None)
    if start is None:
        return None
    end = start + 1
    while end < len(sections) and sections[end].level > sections[start].level:
        end += 1
    return range(start, end)


def has_breaking_section(sections: Sequence[Section]) -> bool:
    return any(
        BREAKING_SECTION_RE.search(section.name)
        and section.content()
        and not NO_BREAKING_RE.match(section.content())
        for section in sections
    )


def is_breaking(subject: str, body: str) -> bool:
    title = TITLE_RE.match(subject)
    if title is not None and title.group("breaking"):
        return True
    return has_breaking_section(split_sections(clean_body(body)))


def commit_summary(body: str) -> str:
    """Keep what the notes need from a PR body and drop what only reviewers need.

    Prefers the Summary section with its subsections, then the preamble, then every
    section that is not reviewer-facing, so a PR without a Summary still reaches the
    notes. Upgrade guidance is kept wherever it is written, and survives truncation.
    """
    sections = split_sections(clean_body(body))
    chosen = summary_range(sections)
    if chosen is not None:
        main = [sections[i] for i in chosen if i != chosen.start]
        lead = sections[chosen.start].content()
    elif sections[0].text():
        main = []
        lead = sections[0].text()
    else:
        main = [section for section in sections[1:] if section.name not in NON_RELEASE_SECTIONS]
        lead = ""
    upgrade = [
        section
        for section in sections
        if UPGRADE_SECTION_RE.search(section.name) and section.content()
    ]
    body_parts = [lead] + [
        section.text()
        for section in main
        if section not in upgrade and section.name not in NON_RELEASE_SECTIONS
    ]
    upgrade_text = "\n\n".join(section.text() for section in upgrade)
    summary = "\n\n".join(part for part in body_parts if part).strip()
    room = SUMMARY_MAX_CHARS - len(upgrade_text)
    if len(summary) > room:
        summary = summary[: max(room, 0)].rsplit("\n", 1)[0].strip()
    return "\n\n".join(part for part in (summary, upgrade_text) if part)


def pull_request_url(repository: str, subject: str) -> str | None:
    match = PR_TRAILER_RE.search(subject)
    if match is None:
        return None
    return f"https://github.com/{repository}/pull/{match.group(1)}"


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
        body = git("show", "-s", "--format=%b", sha)
        summary = commit_summary(body)
        paths = changed_paths(sha)
        reason = exclusion_reason(subject, paths)
        commits.append(
            Commit(
                sha=sha,
                subject=subject,
                summary=summary,
                paths=paths,
                included=reason is None,
                exclusion_reason=reason,
                breaking=is_breaking(subject, body),
            )
        )
    return commits


def write_input(
    path: Path,
    repository: str,
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
                f"Breaking: {'yes' if commit.breaking else 'no'}",
                "",
                f"PR: {pull_request_url(repository, commit.subject) or '(none)'}",
                "",
                "Summary:",
                "",
                commit.summary if commit.summary else "(none)",
                "",
                "Changed files:",
                *[f"- `{changed}`" for changed in commit.paths],
                "",
            ]
        )

    lines.extend(["## Deterministically excluded commits", ""])
    if excluded:
        lines.extend(
            f"- `{commit.sha}` - {commit.subject} ({commit.exclusion_reason})"
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


def release_body(raw: str, from_tag: str) -> str:
    """Return the published body, with a fixed note when no commit was user-facing."""
    if not raw.strip():
        return f"No user-facing changes since {from_tag}.\n"
    return validate_notes(raw)


def requires_note(commit: dict[str, Any]) -> bool:
    """Whether a release-range commit must appear in the notes."""
    if not commit.get("included"):
        return False
    title = TITLE_RE.match(commit["subject"])
    if commit.get("breaking"):
        return True
    if title is None or title.group("type") not in USER_FACING_TYPES:
        return False
    return not paths_are(
        commit.get("paths", ()),
        lambda path: path.startswith(NON_USER_FACING_PATH_PREFIXES)
        or path in {"Cargo.lock", "Cargo.toml"},
    )


def section_text(notes: str, heading: str) -> str:
    match = re.search(rf"^## {heading}\n(.*?)(?=^## |\Z)", notes, re.DOTALL | re.MULTILINE)
    return match.group(1) if match else ""


def validate_coverage(notes: str, commits: Sequence[dict[str, Any]]) -> None:
    """Fail when the notes drop a change, hide a break, or cite an unknown PR."""
    numbers = {
        commit["sha"]: match.group(1)
        for commit in commits
        if (match := PR_TRAILER_RE.search(commit["subject"])) is not None
    }
    cited = set(PR_LINK_RE.findall(notes))
    compatibility = set(PR_LINK_RE.findall(section_text(notes, "Compatibility")))
    problems = []
    for commit in commits:
        number = numbers.get(commit["sha"])
        if number is None or not requires_note(commit):
            continue
        if number not in cited:
            problems.append(f"#{number} is missing: {commit['subject']}")
        elif commit.get("breaking") and number not in compatibility:
            problems.append(f"#{number} is breaking but not in Compatibility: {commit['subject']}")
    unknown = cited - set(numbers.values())
    problems.extend(f"#{number} is not in this release range" for number in sorted(unknown))
    if problems:
        raise ReleaseNotesError(
            "generated release notes do not match the release range:\n- " + "\n- ".join(problems)
        )


def check_pull_request(title: str, body: str) -> list[str]:
    """What stops this PR from producing a correct release note, if anything."""
    match = TITLE_RE.match(title)
    if match is None or match.group("type") not in COMMIT_TYPES:
        return [f"title is not a Conventional Commit: {title}"]
    breaking = match.group("breaking") is not None
    if match.group("type") not in USER_FACING_TYPES and not breaking:
        return []
    sections = split_sections(clean_body(body))
    problems = []
    chosen = summary_range(sections)
    if chosen is None:
        problems.append("add a `## Summary` section: release notes are written from it")
    elif not "".join(sections[i].content() for i in chosen).strip():
        problems.append("the `## Summary` section is empty: release notes are written from it")
    if breaking and not has_breaking_section(sections):
        problems.append(
            "a `!` title needs a `### Breaking changes` section saying what breaks and how to migrate"
        )
    return problems


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
        arguments.repository,
        from_tag,
        from_commit,
        arguments.to_tag,
        to_commit,
        commits,
    )
    metadata_path.write_text(
        json.dumps(
            {
                "schema_version": 2,
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
    metadata = json.loads(Path(arguments.metadata).read_text(encoding="utf-8"))
    body = release_body(raw, metadata["from_tag"])
    validate_coverage(body, metadata["commits"])
    Path(arguments.output).write_text(body, encoding="utf-8")
    print(f"validated release notes in {arguments.output}")


def check_pr(arguments: argparse.Namespace) -> None:
    body = Path(arguments.body_file).read_text(encoding="utf-8")
    problems = check_pull_request(arguments.title, body)
    if problems:
        raise ReleaseNotesError("pull request description:\n- " + "\n- ".join(problems))
    print("pull request description can produce a release note")


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
    validate_parser.add_argument("--metadata", required=True)
    validate_parser.set_defaults(function=validate)

    check_pr_parser = commands.add_parser("check-pr")
    check_pr_parser.add_argument("--title", required=True)
    check_pr_parser.add_argument("--body-file", required=True)
    check_pr_parser.set_defaults(function=check_pr)
    return root


def main() -> int:
    arguments = parser().parse_args()
    try:
        arguments.function(arguments)
    except (OSError, ValueError, KeyError, ReleaseNotesError) as error:
        print(f"release notes: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
