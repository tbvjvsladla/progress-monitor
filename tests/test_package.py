#!/usr/bin/env python3
"""Tests for the distribution side: project root detection, init, hygiene, manifest, install smoke.

Run from the package (skill) folder:
    python3 -B -m unittest discover -s tests -v
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

PACKAGE = Path(__file__).resolve().parents[1]
SCRIPTS = PACKAGE / "scripts"
SKIP = {".git", "__pycache__", ".cache"}


def run(args: list[str], cwd: Path, env: dict | None = None) -> subprocess.CompletedProcess:
    base = {k: v for k, v in os.environ.items() if not k.startswith("PM_")}
    return subprocess.run(args, cwd=str(cwd), capture_output=True, text=True, env={**base, **(env or {})},
                          timeout=300)


class Installed:
    """A temporary project with the package copied to <root>/<place>/progress-monitor."""

    def __init__(self, case: unittest.TestCase, place: str = ".claude/skills"):
        self.tmp = Path(tempfile.mkdtemp(prefix="pm-pkg-"))
        case.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.root = self.tmp / "project"
        self.skill = self.root / place / "progress-monitor"
        shutil.copytree(PACKAGE, self.skill, ignore=lambda _d, names: {n for n in names if n in SKIP})

    def pm(self, *args: str, cwd: Path | None = None) -> tuple[int, dict]:
        done = run([sys.executable, "-B", str(self.skill / "scripts" / "pm.py"), *args], cwd or self.root)
        return done.returncode, json.loads(done.stdout)


class RootDetection(unittest.TestCase):
    def test_root_for_each_install_place(self):
        for place in (".claude/skills", ".agents/skills", "skills"):
            with self.subTest(place=place):
                inst = Installed(self, place)
                code, out = inst.pm("init", cwd=inst.tmp)
                self.assertEqual(code, 0, out)
                self.assertEqual(Path(out["root"]), inst.root.resolve())
                self.assertEqual(out["campaigns"], "campaigns")

    def test_env_overrides_root(self):
        inst = Installed(self)
        other = inst.tmp / "other"
        other.mkdir()
        done = run([sys.executable, "-B", str(inst.skill / "scripts" / "pm.py"), "init"], inst.root,
                   {"PM_PROJECT_ROOT": str(other)})
        self.assertEqual(Path(json.loads(done.stdout)["root"]), other.resolve())


class Init(unittest.TestCase):
    def test_dry_run_writes_nothing(self):
        inst = Installed(self)
        code, out = inst.pm("init")
        self.assertEqual((code, out["mode"]), (0, "dry-run"))
        self.assertIn("campaigns/README.md", out["created"])
        self.assertIn("campaigns/_template/campaign.json", out["created"])
        self.assertFalse((inst.root / "campaigns").exists())
        self.assertFalse((inst.root / ".claude" / "settings.local.json").exists())

    def test_apply_then_again_keeps_everything(self):
        inst = Installed(self)
        _, first = inst.pm("init", "--apply")
        for rel in first["created"]:
            source = inst.skill / "assets" / rel
            self.assertEqual((inst.root / rel).read_bytes(), source.read_bytes(), rel)
        self.assertEqual({h["state"] for h in first["hooks"]}, {"created"})
        (inst.root / "campaigns" / "README.md").write_text("mine\n", encoding="utf-8")
        _, second = inst.pm("init", "--apply")
        self.assertEqual(second["created"], [])
        self.assertEqual(sorted(second["kept"]), sorted(first["created"]))
        self.assertEqual({h["state"] for h in second["hooks"]}, {"present"})
        self.assertEqual((inst.root / "campaigns" / "README.md").read_text(encoding="utf-8"), "mine\n")

    def test_existing_hook_file_is_not_modified(self):
        inst = Installed(self)
        settings = inst.root / ".claude" / "settings.local.json"
        settings.parent.mkdir(parents=True, exist_ok=True)
        settings.write_text('{"permissions": {"allow": []}}\n', encoding="utf-8")
        _, out = inst.pm("init", "--apply", "--harness", "claude")
        self.assertEqual(settings.read_text(encoding="utf-8"), '{"permissions": {"allow": []}}\n')
        hook = out["hooks"][0]
        self.assertEqual(hook["state"], "exists-not-modified")
        command = hook["snippet"]["hooks"]["Stop"][0]["hooks"][0]["command"]
        self.assertIn("${CLAUDE_PROJECT_DIR}/.claude/skills/progress-monitor/scripts/pm.py", command)
        self.assertTrue(command.endswith("stop-hook --harness claude"))

    def test_codex_hook_points_at_the_skill(self):
        inst = Installed(self, ".agents/skills")
        _, out = inst.pm("init", "--apply", "--harness", "codex")
        data = json.loads((inst.root / ".codex" / "hooks.json").read_text(encoding="utf-8"))
        command = data["hooks"]["Stop"][0]["hooks"][0]["command"]
        self.assertEqual(command, "python3 -B .agents/skills/progress-monitor/scripts/pm.py stop-hook --harness codex")
        self.assertEqual([h["harness"] for h in out["hooks"]], ["codex"])

    def test_laid_template_passes_lint(self):
        inst = Installed(self)
        inst.pm("init", "--apply")
        done = run([sys.executable, "-B", str(inst.skill / "scripts" / "check_campaign.py")], inst.root)
        self.assertEqual(done.returncode, 0, done.stdout)
        out = json.loads(done.stdout)
        self.assertEqual((out["status"], out["mode"], out["target"]), ("PASS", "template", "campaigns/_template"))


class Distribution(unittest.TestCase):
    def test_sanitize_has_no_hits(self):
        done = run([sys.executable, "-B", str(SCRIPTS / "sanitize_check.py")], PACKAGE)
        self.assertEqual(done.returncode, 0, done.stdout)
        self.assertIn("SANITIZE OK (0 hits)", done.stdout)

    def test_sanitize_catches_planted_identifiers(self):
        tmp = Path(tempfile.mkdtemp(prefix="pm-san-"))
        self.addCleanup(shutil.rmtree, tmp, ignore_errors=True)
        # Built from pieces so the hygiene scan of this file stays clean.
        planted = ["/" + "home/someone/x", "10." + "1.2.3", "me" + "@" + "corp.io", "val" + "_agent"]
        (tmp / "a.md").write_text("\n".join(planted) + "\n", encoding="utf-8")
        done = run([sys.executable, "-B", str(SCRIPTS / "sanitize_check.py"), str(tmp)], PACKAGE)
        self.assertEqual(done.returncode, 1)
        for name in ("home_path", "private_ip", "email", "project_identifier"):
            self.assertIn(name, done.stdout)

    def test_manifest_matches_tree(self):
        if not (PACKAGE / "PACKAGE_MANIFEST.json").is_file():
            self.skipTest("PACKAGE_MANIFEST.json is written at release time")
        done = run([sys.executable, "-B", str(SCRIPTS / "package_manifest.py"), "--check"], PACKAGE)
        self.assertEqual(done.returncode, 0, done.stdout)

    def test_versions_agree(self):
        sys.path.insert(0, str(SCRIPTS))
        import package_manifest  # noqa: E402
        version = package_manifest.package_version(PACKAGE)
        self.assertRegex(version, r"^\d+\.\d+\.\d+$")
        readme = (PACKAGE / "README.md").read_text(encoding="utf-8")
        self.assertIn(f"배포 버전 **{version}**", readme)
        manifest = PACKAGE / "PACKAGE_MANIFEST.json"
        if manifest.is_file():
            self.assertEqual(json.loads(manifest.read_text(encoding="utf-8"))["version"], version)

    def test_install_smoke(self):
        done = run([sys.executable, "-B", str(SCRIPTS / "smoke_install.py")], PACKAGE)
        self.assertEqual(done.returncode, 0, done.stdout[-3000:])
        self.assertEqual(done.stdout.strip().splitlines()[-1], "SMOKE-PASS")


if __name__ == "__main__":
    unittest.main()
