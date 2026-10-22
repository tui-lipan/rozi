#!/usr/bin/env python3
"""Check the shared Discover corpus against a checkout of the Python index generator."""

import argparse
import importlib.util
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("index_script", type=Path, help="path to scripts/build_index.py")
    parser.add_argument(
        "--corpus", type=Path,
        default=Path(__file__).resolve().parents[1] / "tests/fixtures/discover-eligibility.json",
    )
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location("build_index", args.index_script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    repo = {
        "full_name": "example/extension",
        "clone_url": "https://github.com/example/extension.git",
        "updated_at": "2026-01-01T00:00:00Z",
        "stargazers_count": 0,
    }
    cases = json.loads(args.corpus.read_text(encoding="utf-8"))
    failures = []
    for case in cases:
        try:
            module.catalog_entry(repo, "a" * 40, case["manifest"].encode("utf-8"))
            eligible = True
        except ValueError:
            eligible = False
        if eligible != case["eligible"]:
            failures.append(case["name"])
    if failures:
        raise SystemExit("Eligibility mismatch: " + ", ".join(failures))
    print(f"All {len(cases)} Discover cases passed.")


if __name__ == "__main__":
    main()
