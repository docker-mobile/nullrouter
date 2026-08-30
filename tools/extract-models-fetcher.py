#!/usr/bin/env python3
"""Add each provider's `modelsFetcher` to `crates/providers/data/registry.json`.

The registry is generated from the upstream checkout rather than hand-transcribed, so this
field is extracted too rather than typed in. Eight providers publish a model catalogue at a
fixed URL; the dashboard's "suggested models" list is that catalogue, filtered.

Why extraction matters beyond fidelity: taking these URLs from the registry is what lets
`/api/providers/suggested-models` stop fetching a URL the *caller* supplied. Upstream's
route fetches whatever it is given, which is a server-side request forgery primitive behind
dashboard auth. With the registry as the authority the route can refuse anything it does not
already know, and lose nothing -- the dashboard only ever passes these eight.

Usage:
    python3 tools/extract-models-fetcher.py            # patch in place
    python3 tools/extract-models-fetcher.py --check    # verify, write nothing (exit 1 on drift)

Reads the gitignored `inspire/` reference checkout. Not needed to build or test: the
generated JSON is committed.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
REGISTRY_JSON = REPO / "crates" / "providers" / "data" / "registry.json"
UPSTREAM_REGISTRY = REPO / "inspire" / "open-sse" / "providers" / "registry"

# `modelsFetcher: { url: "...", type: "..." }` on one line, which is how every entry is
# written upstream. A multi-line entry would be missed, so the count is asserted below.
FETCHER = re.compile(
    r"""modelsFetcher:\s*\{\s*url:\s*["']([^"']+)["']\s*,\s*type:\s*["']([^"']+)["']\s*,?\s*\}"""
)
# The `id:` of the entry, which is the first one in the file.
ENTRY_ID = re.compile(r"""^\s*id:\s*["']([^"']+)["']""", re.MULTILINE)

# Upstream has exactly this many. If the count changes, the upstream registry changed and
# this script should be re-read rather than trusted.
EXPECTED = 8


def extract() -> dict[str, dict[str, str]]:
    if not UPSTREAM_REGISTRY.is_dir():
        sys.exit(
            f"no upstream checkout at {UPSTREAM_REGISTRY}.\n"
            "  git clone https://github.com/decolua/9router inspire"
        )

    found: dict[str, dict[str, str]] = {}
    for path in sorted(UPSTREAM_REGISTRY.glob("*.js")):
        text = path.read_text(encoding="utf-8")
        fetcher = FETCHER.search(text)
        if not fetcher:
            continue
        entry_id = ENTRY_ID.search(text)
        if not entry_id:
            print(f"warning: {path.name} has a modelsFetcher but no id", file=sys.stderr)
            continue
        found[entry_id.group(1)] = {"url": fetcher.group(1), "type": fetcher.group(2)}
    return found


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="verify without writing")
    args = parser.parse_args()

    found = extract()
    if len(found) != EXPECTED:
        print(
            f"expected {EXPECTED} providers with a modelsFetcher, found {len(found)}: "
            f"{sorted(found)}\n"
            "The upstream registry changed shape. Re-read this script before trusting it.",
            file=sys.stderr,
        )
        return 1

    registry = json.loads(REGISTRY_JSON.read_text(encoding="utf-8"))
    entries = registry if isinstance(registry, list) else registry.get("providers", [])
    by_id = {entry["id"]: entry for entry in entries if isinstance(entry, dict) and "id" in entry}

    missing = sorted(set(found) - set(by_id))
    if missing:
        print(f"upstream providers absent from the generated registry: {missing}", file=sys.stderr)
        return 1

    changed = []
    for provider_id, fetcher in sorted(found.items()):
        entry = by_id[provider_id]
        if entry.get("modelsFetcher") != fetcher:
            changed.append(provider_id)
            if not args.check:
                entry["modelsFetcher"] = fetcher

    if args.check:
        if changed:
            print(f"registry.json is missing or has stale modelsFetcher for: {changed}")
            return 1
        print(f"registry.json carries all {len(found)} modelsFetcher entries")
        return 0

    if not changed:
        print("no change needed")
        return 0

    # One-space indent and a trailing newline, matching the committed file exactly, so the
    # diff is the added fields and nothing else. Checked against the file before writing:
    # a reformat of 117 entries would bury the change.
    REGISTRY_JSON.write_text(
        json.dumps(registry, indent=1, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"patched {len(changed)} entries: {changed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
