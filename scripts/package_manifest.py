#!/usr/bin/env python3
"""Write or verify PACKAGE_MANIFEST.json.

    python3 -B scripts/package_manifest.py [package root]           # write (overwrite)
    python3 -B scripts/package_manifest.py --check [package root]   # file list and sizes; exit 1 on any difference
    python3 -B scripts/package_manifest.py --check <installed root> --against <repository> --ref <tag>
        # also compare every installed file byte for byte with the tag's blob

The manifest says which files make up the package, with path, bytes and mode. It
holds no digests: whether an installed copy is intact is checked with git, or by
comparing its bytes with the tagged blobs directly (`--against`). The check compares
the file list and sizes; mode is one of the two values Git keeps (0644/0755) and is
recorded only, since some file systems cannot hold the executable bit. The manifest
itself and INVENTORY_EXCLUDES are not listed. Format: manifest_version 2.0.0.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

MANIFEST = "PACKAGE_MANIFEST.json"
MANIFEST_VERSION = "2.0.0"
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
        stat = (root / rel).stat()
        mode = "0755" if stat.st_mode & 0o111 else "0644"
        files.append({"path": rel, "bytes": stat.st_size, "mode": mode})
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
    """Paths whose presence or size differs from the manifest, and <version> when the version does."""
    have = json.loads((root / MANIFEST).read_text(encoding="utf-8"))
    want = build(root)
    out = [] if have.get("version") == want["version"] else ["<version>"]
    h = {f["path"]: f.get("bytes") for f in have["files"]}
    w = {f["path"]: f["bytes"] for f in want["files"]}
    out += sorted(p for p in set(h) | set(w) if h.get(p) != w.get(p))
    return out


class GitError(Exception):
    pass


def git(repo: Path, *args: str | bytes) -> bytes:
    try:
        done = subprocess.run(["git", "-C", str(repo), *args], capture_output=True)
    except OSError as exc:
        raise GitError(f"git: {exc}") from exc
    if done.returncode != 0:
        shown = " ".join(os.fsdecode(a) for a in args)
        raise GitError(f"git {shown}: {done.stderr.decode(errors='replace').strip()}")
    return done.stdout


def against(root: Path, repo: Path, reference: str) -> list[str]:
    """What differs between the installed files at `root` and the blobs of `reference` in the git
    repository `repo`, compared byte for byte: paths missing from the copy, paths the tag does not
    have, and paths whose bytes differ. Git errors are errors, not findings."""
    listing = git(repo, "ls-tree", "-r", "-z", "--name-only", reference)
    installed = set(inventory(root))
    if (root / MANIFEST).is_file():
        installed.add(MANIFEST)
    listed = set()
    out = []
    for raw in (n for n in listing.split(b"\0") if n):
        shown = os.fsdecode(raw)
        listed.add(shown)
        if shown not in installed:
            out.append(f"missing against {reference}: {shown}")
            continue
        blob = git(repo, "show", reference.encode() + b":" + raw)
        try:
            have = (root / shown).read_bytes()
        except OSError as exc:
            raise GitError(f"{root / shown}: {exc}") from exc
        if have != blob:
            out.append(f"differs from {reference}: {shown}")
    out += [f"not in {reference}: {p}" for p in sorted(installed - listed)]
    return out


def parse_args(args: list[str]) -> tuple[bool, str | None, str | None, list[str]]:
    check, repo, reference, rest = False, None, None, []
    it = iter(args)
    for arg in it:
        if arg == "--check":
            check = True
        elif arg in ("--against", "--ref"):
            value = next(it, None)
            if value is None:
                raise ValueError(f"{arg} needs a value")
            if arg == "--against":
                repo = value
            else:
                reference = value
        else:
            rest.append(arg)
    if (repo is None) != (reference is None):
        raise ValueError("--against and --ref go together")
    if repo is not None and not check:
        raise ValueError("--against compares an installed copy; it needs --check")
    return check, repo, reference, rest


def main() -> int:
    try:
        check, repo, reference, rest = parse_args(sys.argv[1:])
    except ValueError as exc:
        print(exc, file=sys.stderr)
        return 2
    root = Path(rest[0]).resolve() if rest else DEFAULT_ROOT
    if check:
        if not (root / MANIFEST).is_file():
            print(f"MANIFEST MISSING: {MANIFEST}")
            return 1
        found = diff(root)
        same = ""
        if repo is not None:
            try:
                found += against(root, Path(repo).resolve(), reference)
            except GitError as exc:
                print(exc, file=sys.stderr)
                return 1
            same = f", same bytes as {reference}"
        if found:
            print(f"MANIFEST MISMATCH: {', '.join(found)}")
            return 1
        print(f"MANIFEST OK ({build(root)['file_count']} files{same})")
        return 0
    manifest = build(root)
    (root / MANIFEST).write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"MANIFEST written ({manifest['file_count']} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
