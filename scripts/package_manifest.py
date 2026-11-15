#!/usr/bin/env python3
"""Write or verify PACKAGE_MANIFEST.json (for the publisher).

    python3 -B scripts/package_manifest.py [package root]           # write (overwrite)
    python3 -B scripts/package_manifest.py --check [package root]   # verify; exit 1 on any difference

Each file is listed with path, bytes, mode and sha256. The manifest itself and
INVENTORY_EXCLUDES are not listed. mode is one of the two values Git keeps
(0644/0755), so a manifest made from the work tree also verifies an install
taken with `git archive`. Format: manifest_version 1.0.0 (same as wiki-desk).
"""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

MANIFEST = "PACKAGE_MANIFEST.json"
MANIFEST_VERSION = "1.0.0"
PACKAGE_NAME = "progress-monitor"
INVENTORY_EXCLUDES = [MANIFEST, ".git", "__pycache__", ".cache"]
DEFAULT_ROOT = Path(__file__).resolve().parents[1]


def inventory(root: Path) -> list[str]:
    out = []
    def walk(folder: Path) -> None:
        for entry in sorted(folder.iterdir()):
            if entry.name in INVENTORY_EXCLUDES:
                continue
            if entry.is_symlink() or not (entry.is_dir() or entry.is_file()):
                raise SystemExit(f"symlinks and special files cannot be in the package: {entry.relative_to(root)}")
            if entry.is_dir():
                walk(entry)
            else:
                out.append(entry.relative_to(root).as_posix())
    walk(root)
    return sorted(out)


def build(root: Path) -> dict:
    files = []
    for rel in inventory(root):
        path = root / rel
        data = path.read_bytes()
        mode = "0755" if path.stat().st_mode & 0o111 else "0644"
        files.append({"path": rel, "bytes": len(data), "mode": mode, "sha256": hashlib.sha256(data).hexdigest()})
    return {"manifest_version": MANIFEST_VERSION, "package": PACKAGE_NAME, "version": package_version(root),
            "inventory_excludes": INVENTORY_EXCLUDES, "file_count": len(files), "files": files}


def package_version(root: Path) -> str:
    """The version written in SKILL.md metadata (the one place the package version lives)."""
    for line in (root / "SKILL.md").read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("version:"):
            return stripped.split(":", 1)[1].strip().strip('"')
    raise SystemExit("SKILL.md metadata has no version")


def diff(root: Path) -> list[str]:
    have = json.loads((root / MANIFEST).read_text(encoding="utf-8"))
    want = build(root)
    out = [] if have.get("version") == want["version"] else ["<version>"]
    h = {f["path"]: f for f in have["files"]}
    w = {f["path"]: f for f in want["files"]}
    out += sorted(p for p in set(h) | set(w) if h.get(p) != w.get(p))
    return out


def main() -> int:
    args = sys.argv[1:]
    check = "--check" in args
    rest = [a for a in args if a != "--check"]
    root = Path(rest[0]).resolve() if rest else DEFAULT_ROOT
    if check:
        if not (root / MANIFEST).is_file():
            print(f"MANIFEST MISSING: {MANIFEST}")
            return 1
        found = diff(root)
        print(f"MANIFEST OK ({build(root)['file_count']} files)" if not found else f"MANIFEST MISMATCH: {', '.join(found)}")
        return 0 if not found else 1
    manifest = build(root)
    (root / MANIFEST).write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"MANIFEST written ({manifest['file_count']} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
