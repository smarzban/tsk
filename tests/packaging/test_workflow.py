"""Pin the owner-approved draft-only release handoff without calling GitHub.

This deliberately guards the workflow's current one-line gh invocation. Changing
that handoff's shape requires updating/reviewing this safety test too.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/release.yml"


class WorkflowTests(unittest.TestCase):
    def test_public_pr_validation_is_read_only_and_secret_free(self):
        for name in ["ci", "site", "installer"]:
            with self.subTest(workflow=name):
                source = (ROOT / f".github/workflows/{name}.yml").read_text()
                triggers = source.split("on:\n", 1)[1].split("\npermissions:", 1)[0]
                events = dict(re.findall(r"^  (\w+):\n((?:    .*\n|\n)*)", triggers, re.M))
                self.assertEqual(set(events), {"push", "pull_request"})
                self.assertEqual(events["push"].strip(), events["pull_request"].strip())
                self.assertIn("branches: [main]", events["pull_request"])
                self.assertIn("paths-ignore:" if name == "ci" else "paths:", events["pull_request"])
                self.assertIn("permissions:\n  contents: read", source)
                self.assertEqual(source.count("permissions:"), 1)
                self.assertNotRegex(source, r"secrets\s*[.\[]")
                self.assertIn("persist-credentials: false", source)

    def test_vercel_secret_is_only_available_to_main_push_deployment(self):
        source = (ROOT / ".github/workflows/vercel.yml").read_text()
        triggers = source.split("on:\n", 1)[1].split("\npermissions:", 1)[0]
        self.assertEqual(re.findall(r"^  ([a-z_]+):", triggers, re.M), ["push"])
        self.assertIn("branches: [main]", triggers)
        self.assertIn("if: github.event_name == 'push' && github.ref == 'refs/heads/main'", source)
        self.assertIn("permissions:\n  contents: read", source)
        self.assertEqual(source.count("permissions:"), 1)

    def test_packaging_checks_cover_rust_and_installer_changes_not_site_builds(self):
        site = (ROOT / ".github/workflows/site.yml").read_text()
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        installer = (ROOT / ".github/workflows/installer.yml").read_text()
        for token in ["actions/setup-python", "Packaging contract tests", "python3 -m unittest discover -s tests/packaging"]:
            self.assertNotIn(token, site)
            self.assertIn(token, ci)
            self.assertIn(token, installer)
        # The installer must be gated before merge, not only after: the site serves it from main.
        triggers = installer.split("on:\n", 1)[1].split("\npermissions:", 1)[0]
        self.assertEqual(re.findall(r"^  ([a-z_]+):", triggers, re.M), ["push", "pull_request"])
        self.assertEqual(triggers.count("branches: [main]"), 2)
        for path in ["site/public/install.sh", "site/public/install.ps1", "scripts/release.py", "tests/packaging/**"]:
            self.assertEqual(triggers.count(f'- "{path}"'), 2, path)
        self.assertIn("shellcheck site/public/install.sh", installer)
        self.assertIn("runs-on: windows-2025", installer)
        self.assertIn("powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File site/public/install.ps1 -Help", installer)
        self.assertIn("TSK_TEST_BINARY: ${{ github.workspace }}/target/release/tsk", ci)
        self.assertLess(ci.index("name: Verify"), ci.index("name: Packaging contract tests"))

    def test_release_builds_and_packages_windows_natively(self):
        source = WORKFLOW.read_text()
        self.assertRegex(source, r"- os: windows-[^\n]+\n\s+target: x86_64-pc-windows-msvc")
        self.assertIn('target/${{ matrix.target }}/release/tsk${{ matrix.exe_suffix }}', source)
        self.assertIn("dist-release/*", source)
        self.assertIn("cargo test --locked --target", source)
        self.assertIn("powershell.exe -NoProfile -ExecutionPolicy Bypass -File site/public/install.ps1 -Help", source)
        self.assertRegex(source, r"python(?:3)? -m unittest discover -s tests/packaging")

    def test_release_handoff_creates_only_a_draft_for_an_existing_tag(self):
        source = WORKFLOW.read_text()
        commands = re.findall(r"^        run: (gh release create[^\n]+)$", source, re.M)
        self.assertEqual(len(commands), 1, "release creation must use the reviewed handoff")
        # Do not execute arbitrary future workflow shell in the test environment.
        # An unnamed release is titled by GitHub with the tagged commit's subject
        # ("release: v0.10.0 (#102)"), so the draft must carry the tag as its name.
        self.assertEqual(commands[0], 'gh release create "$TAG" dist-release/* --verify-tag --draft --title "$TAG" --notes-file packaging/RELEASE-NOTES.md')
        self.assertEqual(source.count("gh release "), 1)
        triggers = source.split("on:\n", 1)[1].split("\npermissions:", 1)[0]
        self.assertEqual(re.findall(r"^  ([a-z_]+):", triggers, re.M), ["workflow_dispatch"])
        self.assertEqual(source.count("ref: ${{ needs.resolve.outputs.commit }}"), 2)
        self.assertIn('test "$(gh api "repos/$GITHUB_REPOSITORY/commits/refs%2Ftags%2F$TAG" --jq .sha)" = "$RELEASE_COMMIT"', source)
        with tempfile.TemporaryDirectory(prefix="tsk-draft-handoff-") as temp:
            root = Path(temp)
            commands_dir = root / "bin"
            commands_dir.mkdir()
            mock = commands_dir / "gh"
            mock.write_text('#!/usr/bin/env python3\nimport json, os, pathlib, sys\npathlib.Path(os.environ["GH_CAPTURE"]).write_text(json.dumps(sys.argv[1:]))\n')
            mock.chmod(0o755)
            assets = root / "dist-release"
            assets.mkdir()
            names = ["tsk-v1.2.3-aarch64-apple-darwin.tar.gz", "tsk-v1.2.3-x86_64-pc-windows-msvc.zip", "SHA256SUMS", "tsk.rb", "install.sh", "install.ps1"]
            for name in names:
                (assets / name).write_text("fixture")
            capture = root / "args.json"
            environment = dict(os.environ, PATH=f"{commands_dir}:{os.environ['PATH']}", TAG="v1.2.3", GH_CAPTURE=str(capture))
            for key in ["GH_TOKEN", "GITHUB_TOKEN"]:
                environment.pop(key, None)
            subprocess.run(["bash", "-eu", "-c", commands[0]], cwd=root, env=environment, check=True, capture_output=True)
            arguments = json.loads(capture.read_text())
            self.assertEqual(arguments[:3], ["release", "create", "v1.2.3"])
            self.assertEqual(set(arguments[3:3 + len(names)]), {f"dist-release/{name}" for name in names})
            self.assertEqual(arguments[3 + len(names):], ["--verify-tag", "--draft", "--title", "v1.2.3", "--notes-file", "packaging/RELEASE-NOTES.md"])


if __name__ == "__main__":
    unittest.main()
