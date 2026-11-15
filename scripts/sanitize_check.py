#!/usr/bin/env python3
"""Distribution hygiene check (for the publisher).

Scans the package tree for site names, personal home paths, private IPs, e-mail
addresses and the identifiers of the projects this package was extracted from.
Usage: python3 -B scripts/sanitize_check.py [package root]
Exit 0 = no hit, 1 = hits found. The forbidden patterns live in FORBIDDEN only.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

DEFAULT_ROOT = Path(__file__).resolve().parents[1]
EXCLUDED_DIRS = {".git", "__pycache__", ".cache", "node_modules"}
# Written in pieces so this file does not match itself.
SITE_NAMES = ["\uacbd\ub9ac\ub098\ub77c", "kyung" + "rinara"]
PROJECT_NAMES = ["val" + "_agent", "easy" + "[_-]vllm", "vllm" + "[_-]simulator", "her" + "mes[_-]subagent"]

FORBIDDEN: dict[str, re.Pattern[str]] = {
    "site_name": re.compile("|".join(SITE_NAMES), re.IGNORECASE),
    "home_path": re.compile(r"/home/[a-z_][a-z0-9_-]*/"),
    "private_ip": re.compile(r"(?<![0-9.])(?:10\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])|192\.168)\.\d{1,3}\.\d{1,3}(?![0-9])"),
    "email": re.compile(r"[A-Za-z0-9._%+-]+@(?!example\.(?:invalid|com|org)\b)[A-Za-z0-9.-]+\.[A-Za-z]{2,}"),
    "project_identifier": re.compile("|".join(PROJECT_NAMES), re.IGNORECASE),
}


def scan(root: Path = DEFAULT_ROOT) -> list[tuple[str, int, str]]:
    hits = []
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root)
        if any(part in EXCLUDED_DIRS for part in rel.parts) or not path.is_file():
            continue
        data = path.read_bytes()
        if b"\0" in data:
            continue
        for number, line in enumerate(data.decode("utf-8", errors="replace").splitlines(), 1):
            for name, pattern in FORBIDDEN.items():
                if pattern.search(line):
                    hits.append((rel.as_posix(), number, name))
    return hits


def main() -> int:
    root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_ROOT
    hits = scan(root)
    for rel, number, name in hits:
        print(f"{rel}:{number}: {name}")
    print(f"SANITIZE {'OK' if not hits else 'FAIL'} ({len(hits)} hits)")
    return 0 if not hits else 1


if __name__ == "__main__":
    sys.exit(main())
