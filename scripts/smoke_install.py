#!/usr/bin/env python3
"""Install the package into an empty temporary project and run the synthetic campaign end to end.

Steps: copy this folder to <tmp>/.claude/skills/progress-monitor (and link it from
.agents/skills for Codex), compare the copy with the package byte for byte, check an
archive install of a tag with `package_manifest --check --against` (no digests),
`init` dry-run then `--apply` then again (nothing is overwritten), lint the laid template, run the demo campaign from
assets/examples through prepare -> change -> gate -> final, lint the instance,
and call the Stop hook with and without campaigns/ACTIVE.

Prints SMOKE-PASS and exits 0 only when every step held. Needs python3 and git.
Usage: python3 -B scripts/smoke_install.py [--keep]
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

PACKAGE = Path(__file__).resolve().parents[1]
SKIP = {".git", "__pycache__", ".cache", "node_modules"}
GIT = ["git", "-c", "user.name=smoke", "-c", "user.email=smoke@example.invalid", "-c", "init.defaultBranch=main"]


class Smoke:
    def __init__(self, root: Path):
        self.root = root
        self.skill = root / ".claude" / "skills" / "progress-monitor"
        self.steps: list[str] = []

    def fail(self, step: str, detail: object) -> None:
        print(json.dumps({"status": "FAIL", "step": step, "detail": detail, "passed": self.steps},
                         ensure_ascii=False, indent=2))
        sys.exit(1)

    def run(self, args: list[str], cwd: Path | None = None, stdin: str | None = None) -> subprocess.CompletedProcess:
        env = {k: v for k, v in os.environ.items() if not k.startswith("PM_")}
        env["PM_EXECUTOR"] = "smoke"
        if args and args[0] == "git":
            env["GIT_CONFIG_GLOBAL"] = os.devnull
            env["GIT_CONFIG_NOSYSTEM"] = "1"
        return subprocess.run(args, cwd=str(cwd or self.root), input=stdin, capture_output=True, text=True,
                              env=env, timeout=300)

    def pm(self, *args: str, code: int = 0, stdin: str | None = None) -> dict:
        done = self.run([sys.executable, "-B", str(self.skill / "scripts" / "pm.py"), *args], stdin=stdin)
        if done.returncode != code:
            self.fail(f"pm {' '.join(args)}", {"exit": done.returncode, "stdout": done.stdout[-2000:],
                                               "stderr": done.stderr[-2000:]})
        if args and args[0] == "stop-hook":
            return {"stdout": done.stdout}
        try:
            return json.loads(done.stdout)
        except json.JSONDecodeError:
            self.fail(f"pm {' '.join(args)} output", done.stdout[-2000:])
        return {}

    def lint(self, *args: str) -> dict:
        done = self.run([sys.executable, "-B", str(self.skill / "scripts" / "check_campaign.py"), *args])
        if done.returncode != 0:
            self.fail(f"lint {' '.join(args)}", done.stdout[-3000:])
        return json.loads(done.stdout)

    def git(self, cwd: Path, *args: str) -> None:
        done = self.run([*GIT, *args], cwd=cwd)
        if done.returncode != 0:
            self.fail(f"git {' '.join(args)}", done.stderr)

    def ok(self, step: str) -> None:
        self.steps.append(step)


def copy_package(dest: Path) -> None:
    def ignore(_dir: str, names: list[str]) -> set[str]:
        return {n for n in names if n in SKIP}
    shutil.copytree(PACKAGE, dest, ignore=ignore)


def manifest_steps(s: Smoke, tmp: Path) -> None:
    """The installed copy has the package's bytes, and an archive install of a tag passes
    `package_manifest --check --against` while one changed byte fails it."""
    sys.path.insert(0, str(PACKAGE / "scripts"))
    from package_manifest import inventory
    for rel in inventory(PACKAGE):
        if (PACKAGE / rel).read_bytes() != (s.skill / rel).read_bytes():
            s.fail("installed copy has the package's bytes", rel)
    s.ok("installed copy has the package's bytes (direct comparison, no digest)")

    repo = tmp / "release-repo"
    copy_package(repo)
    s.git(repo, "init", "-q", ".")
    s.git(repo, "add", "-A")
    s.git(repo, "commit", "-q", "-m", "release")
    s.git(repo, "tag", "-a", "t1", "-m", "t1")
    installed = tmp / "archive-install"
    installed.mkdir()
    env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
    archive = subprocess.Popen([*GIT, "-C", str(repo), "archive", "t1"], stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=env)
    untar = subprocess.run(["tar", "-x", "-C", str(installed)], stdin=archive.stdout, capture_output=True)
    archive.stdout.close()
    archive_err = archive.stderr.read()
    if archive.wait() != 0 or untar.returncode != 0:
        s.fail("git archive t1 | tar -x", {"git": archive_err.decode(errors="replace"),
                                          "tar": untar.stderr.decode(errors="replace")})
    s.ok("git archive t1 | tar -x makes an archive install")
    cache = installed / "scripts" / "__pycache__"
    cache.mkdir(parents=True, exist_ok=True)
    (cache / "pm.cpython-0.pyc").write_bytes(b"made after install\n")

    tool = [sys.executable, "-B", str(s.skill / "scripts" / "package_manifest.py"),
            "--check", str(installed), "--against", str(repo), "--ref", "t1"]
    done = s.run(tool)
    if done.returncode != 0 or "same bytes as t1" not in done.stdout:
        s.fail("package_manifest --check --against", {"exit": done.returncode, "stdout": done.stdout[-2000:],
                                                      "stderr": done.stderr[-2000:]})
    s.ok("package_manifest --against agrees with the archive install (__pycache__ made after install is ignored)")

    skill_md = installed / "SKILL.md"
    skill_md.write_text(skill_md.read_text(encoding="utf-8").replace("progress-monitor", "progress-monitoq", 1),
                        encoding="utf-8")
    done = s.run(tool)
    if done.returncode != 1 or "differs from t1: SKILL.md" not in done.stdout:
        s.fail("package_manifest --against notices one changed byte", {"exit": done.returncode,
                                                                       "stdout": done.stdout[-2000:]})
    s.ok("package_manifest --against notices one changed byte with the size unchanged")


def main() -> int:
    keep = "--keep" in sys.argv[1:]
    tmp = Path(tempfile.mkdtemp(prefix="pm-smoke-"))
    try:
        s = Smoke(tmp / "project")
        s.root.mkdir()
        copy_package(s.skill)
        agents = s.root / ".agents" / "skills"
        agents.mkdir(parents=True)
        (agents / "progress-monitor").symlink_to(Path("..") / ".." / ".claude" / "skills" / "progress-monitor")
        s.ok("install")
        manifest_steps(s, tmp)

        dry = s.pm("init")
        if dry["mode"] != "dry-run" or not dry["created"] or (s.root / "campaigns").exists():
            s.fail("init dry-run", dry)
        if any(h["state"] != "would-create" for h in dry["hooks"]):
            s.fail("init dry-run hooks", dry["hooks"])
        s.ok("init dry-run writes nothing")

        applied = s.pm("init", "--apply")
        laid = [s.root / p for p in applied["created"]]
        if not laid or not all(p.is_file() for p in laid):
            s.fail("init --apply files", applied)
        for hook in applied["hooks"]:
            text = (s.root / hook["file"]).read_text(encoding="utf-8")
            if hook["state"] != "created" or f"stop-hook --harness {hook['harness']}" not in text:
                s.fail("init --apply hooks", applied["hooks"])
        claude_cmd = json.loads((s.root / ".claude" / "settings.local.json").read_text())["hooks"]["Stop"][0]["hooks"][0]["command"]
        if ".claude/skills/progress-monitor/scripts/pm.py" not in claude_cmd:
            s.fail("hook command path", claude_cmd)
        s.ok("init --apply lays campaigns/ and hooks")

        (s.root / "campaigns" / "README.md").write_text("local edit\n", encoding="utf-8")
        again = s.pm("init", "--apply")
        if again["created"] or any(h["state"] != "present" for h in again["hooks"]):
            s.fail("init idempotent", again)
        if (s.root / "campaigns" / "README.md").read_text(encoding="utf-8") != "local edit\n":
            s.fail("init overwrote a file", "campaigns/README.md")
        shutil.copy2(s.skill / "assets" / "campaigns" / "README.md", s.root / "campaigns" / "README.md")
        s.ok("init keeps existing files")

        template = s.lint()
        if template["mode"] != "template" or template["status"] != "PASS":
            s.fail("lint template", template)
        s.ok("lint template")

        examples = s.skill / "assets" / "examples"
        target = s.root / "demo-target"
        shutil.copytree(examples / "demo-target", target)
        s.git(target, "init", "-q", ".")
        s.git(target, "add", "-A")
        s.git(target, "commit", "-q", "-m", "demo target")
        shutil.copytree(examples / "docs", s.root / "docs")
        shutil.copytree(examples / "demo-campaign", s.root / "campaigns" / "demo")
        (s.root / "campaigns" / "ACTIVE").write_text("demo\n", encoding="utf-8")
        s.pm("open")
        s.ok("open demo")

        hook_in = json.dumps({"session_id": "smoke", "stop_hook_active": False, "background_tasks": []})
        guided = s.pm("stop-hook", "--harness", "claude", stdin=hook_in)["stdout"]
        if "additionalContext" not in guided and "decision" not in guided:
            s.fail("stop-hook with work left", guided)
        s.ok("stop-hook keeps the session going")
        s.pm("monitor", "reset")

        edits = {"greet": ("greet/message.txt", "hello, campaign\n"), "count": ("count/value.txt", "2\n")}
        for cell, (rel, text) in edits.items():
            s.pm("phase", "start", cell, "prepare")
            s.pm("phase", "close", cell, "prepare")
            s.pm("phase", "start", cell, "change")
            (target / rel).write_text(text, encoding="utf-8")
            s.git(target, "add", "-A")
            s.git(target, "commit", "-q", "-m", f"[{cell}/change] demo edit")
            s.pm("phase", "close", cell, "change")
        s.ok("cells prepare -> change")

        gate = s.pm("gate", "run", "g-demo")
        record = json.loads((s.root / "campaigns" / "demo" / "gates" / "g-demo.status.json").read_text())
        if record["state"] != "passed" or {c["result"] for c in record["checks"]} != {"PASS"}:
            s.fail("gate g-demo", {"out": gate, "record": record})
        s.ok("gate g-demo passed")

        s.pm("final", "run")
        final = json.loads((s.root / "campaigns" / "demo" / "gates" / "_final.status.json").read_text())
        if final["state"] != "passed":
            s.fail("final", final)
        s.ok("final passed")

        instance = s.lint("campaigns/demo")
        if instance["mode"] != "instance" or instance["status"] != "PASS":
            s.fail("lint instance", instance)
        s.ok("lint instance")

        metrics = s.pm("metrics")
        (s.root / "campaigns" / "ACTIVE").unlink()
        silent = s.pm("stop-hook", "--harness", "codex", stdin=hook_in)["stdout"]
        if silent.strip():
            s.fail("stop-hook without ACTIVE", silent)
        s.ok("stop-hook silent without ACTIVE")

        print(json.dumps({"status": "PASS", "passed": s.steps, "metrics": metrics.get("totals", metrics)},
                         ensure_ascii=False, indent=2))
        print("SMOKE-PASS")
        return 0
    finally:
        if keep:
            print(f"kept: {tmp}", file=sys.stderr)
        else:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
