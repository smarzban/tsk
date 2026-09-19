"""Offline installer contract tests, no network or real HOME writes."""
import hashlib
import io
import json
import os
import platform
import shutil
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
INSTALLER = ROOT / "site/public/install.sh"


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bin = self.root / "commands"
        self.bin.mkdir()
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.env = dict(os.environ, HOME=str(self.root / "home"), PATH=f"{self.bin}:{os.environ['PATH']}", CURL_ARGS=str(self.root / "curl-args"), ASSETS=str(self.assets), REQUESTS=str(self.root / "requests"), MOCK_OS="Linux", MOCK_ARCH="x86_64")
        self.env["SHELL"] = "/bin/bash"
        self.env.pop("ZDOTDIR", None)
        self.env.pop("TSK_VERSION", None)
        self.env.pop("TSK_INSTALL_DIR", None)
        # Packaging CI sets CI=true; clear it so Herdr-prompt branches stay deterministic.
        self.env.pop("CI", None)
        self.setup_log = self.root / "setup-herdr.log"
        self.command("uname", '#!/bin/sh\ncase "$1" in -s) echo "$MOCK_OS";; -m) echo "$MOCK_ARCH";; esac\n')
        self.command("curl", '''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
url = args[-1]
with open(os.environ["CURL_ARGS"], "a") as out: out.write(json.dumps(args) + "\\n")
with open(os.environ["REQUESTS"], "a") as out: out.write(url + "\\n")
if os.environ.get("FAIL_DOWNLOAD"): sys.exit(22)
if url.endswith("/releases/latest"):
    print("https://github.com/smarzban/tsk/releases/tag/v1.2.3", end="")
else:
    assert "/releases/download/v1.2.3/" in url, url
    path = pathlib.Path(os.environ["ASSETS"]) / url.rsplit("/", 1)[-1]
    if not path.exists(): sys.exit(22)
    pathlib.Path(args[args.index("-o") + 1]).write_bytes(path.read_bytes())
''')

    def command(self, name, source):
        path = self.bin / name
        path.write_text(source)
        path.chmod(0o755)

    def archive(self, target="x86_64-unknown-linux-musl", bad_checksum=False, member="tsk", record_setup=False, legacy=False):
        archive = self.assets / f"tsk-v1.2.3-{target}.tar.gz"
        with tarfile.open(archive, "w:gz") as out:
            if legacy:
                # A release from before the probes existed: `setup --skill-states` and
                # `setup herdr --check` are usage errors (exit 2, nothing on stdout).
                data = b"""#!/bin/sh
if [ "${1:-}" = setup ] && [ "${2:-}" = --detected-ids ]; then
    if [ -n "${TSK_DETECT_AGENTS:-}" ]; then printf '%s\\n' "$TSK_DETECT_AGENTS"; fi
    exit 0
fi
if [ "${1:-}" = setup ] && { [ "${2:-}" = --skill-states ] || [ "${3:-}" = --check ]; }; then
    echo usage >&2
    exit 2
fi
if [ "${1:-}" = setup ]; then
    if [ -n "${TSK_SETUP_LOG:-}" ]; then printf '%s\\n' "$*" >> "$TSK_SETUP_LOG"; fi
    exit 0
fi
echo installed-fixture
"""
            elif record_setup:
                data = b"""#!/bin/sh
if [ "${1:-}" = setup ] && [ "${2:-}" = --detected-ids ]; then
    if [ -n "${TSK_DETECT_AGENTS:-}" ]; then
        printf '%s\\n' "$TSK_DETECT_AGENTS"
    fi
    exit 0
fi
if [ "${1:-}" = setup ] && [ "${2:-}" = --skill-states ]; then
    printf 'embedded\\t1.3.0\\n'
    if [ -n "${TSK_SKILL_STATES:-}" ]; then
        printf '%s\\n' "$TSK_SKILL_STATES"
    fi
    exit 0
fi
if [ "${1:-}" = setup ] && [ "${2:-}" = herdr ] && [ "${3:-}" = --check ]; then
    if [ -n "${TSK_HERDR_BOUND:-}" ]; then echo bound; else echo unbound; fi
    exit 0
fi
if [ "${1:-}" = setup ] && [ "${2:-}" = agents ] && [ "${3:-}" = --yes ]; then
    if [ -n "${TSK_SETUP_LOG:-}" ]; then
        printf '%s\\n' "$*" >> "$TSK_SETUP_LOG"
    fi
    exit 0
fi
if [ "${1:-}" = setup ] && [ "${2:-}" = herdr ]; then
    if [ -n "${TSK_SETUP_LOG:-}" ]; then
        printf '%s\\n' "$*" >> "$TSK_SETUP_LOG"
    fi
    if [ "${TSK_SETUP_FAIL:-}" = herdr ]; then echo "tsk setup: config.toml is not writable" >&2; exit 7; fi
    exit 0
fi
if [ "${1:-}" = setup ] && [ -n "${2:-}" ]; then
    if [ -n "${TSK_SETUP_LOG:-}" ]; then
        printf '%s\\n' "$*" >> "$TSK_SETUP_LOG"
    fi
    if [ -n "${TSK_SETUP_FAIL:-}" ] && [ "$2" = "$TSK_SETUP_FAIL" ]; then echo "tsk setup: skill dir is a symlink" >&2; exit 7; fi
    exit 0
fi
echo installed-fixture
"""
            else:
                data = b"#!/bin/sh\necho installed-fixture\n"
            info = tarfile.TarInfo(member)
            info.size = len(data)
            info.mode = 0o755
            out.addfile(info, io.BytesIO(data))
        digest = "0" * 64 if bad_checksum else hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.assets / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")

    def run_install(self, **env):
        return subprocess.run(["sh", str(INSTALLER)], env=dict(self.env, **env), cwd=self.root, text=True, capture_output=True, stdin=subprocess.DEVNULL, start_new_session=True)

    def run_install_with_answer(self, answer, **env):
        """Drive the Herdr prompt over a PTY so stdin is a TTY."""
        import errno
        import pty
        import select
        import time

        master, slave = pty.openpty()
        environment = dict(self.env, **env)
        process = None
        transcript = b""
        try:
            process = subprocess.Popen(
                ["sh", str(INSTALLER)],
                stdin=slave,
                stdout=slave,
                stderr=slave,
                env=environment,
                cwd=self.root,
                start_new_session=True,
            )
            os.close(slave)
            slave = None
            answers = [line + b"\n" for line in answer.split(b"\n") if line != b""]
            if not answers:
                answers = [b"\n"]
            answered = 0
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if not select.select([master], [], [], 0.1)[0]:
                    if process.poll() is not None:
                        break
                    continue
                try:
                    chunk = os.read(master, 8192)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not chunk:
                    break
                transcript += chunk
                seen = transcript.count(b"[y/N]") + transcript.count(b"[Y/n]")
                while answered < seen:
                    reply = answers[answered] if answered < len(answers) else answers[-1]
                    os.write(master, reply)
                    answered += 1
            code = process.wait(timeout=5)
            output = transcript.decode(errors="replace")
            return subprocess.CompletedProcess(["sh", str(INSTALLER)], code, output, "")
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
            if slave is not None:
                os.close(slave)
            os.close(master)

    @unittest.skipIf(os.name == "nt", "Unix installer smoke")
    @unittest.skipUnless(os.environ.get("TSK_TEST_BINARY"), "set TSK_TEST_BINARY to smoke a built executable")
    def test_installed_real_binary_runs_isolated_cli(self):
        system, arch = platform.system(), platform.machine()
        cpu = "aarch64" if arch in ("arm64", "aarch64") else "x86_64"
        target = cpu + ("-apple-darwin" if system == "Darwin" else "-unknown-linux-musl")
        archive = self.assets / f"tsk-v1.2.3-{target}.tar.gz"
        # v1.2.3 is the offline HTTP fixture tag, not a public release claim.
        with tarfile.open(archive, "w:gz") as bundle:
            bundle.add(os.environ["TSK_TEST_BINARY"], arcname="tsk")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.assets / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
        result = self.run_install(MOCK_OS=system, MOCK_ARCH=arch)
        self.assertEqual(result.returncode, 0, result.stderr)
        installed = self.root / "home/.local/bin/tsk"
        isolated = dict(self.env, TSK_STATE_DIR=str(self.root / "state"))
        subprocess.run([str(installed), "add", "--desk", "-t", "Installed smoke"], env=isolated, check=True, capture_output=True)
        listed = subprocess.run([str(installed), "list", "--desk"], env=isolated, check=True, text=True, capture_output=True)
        self.assertIn("Installed smoke", listed.stdout)

    def test_path_setup_preserves_config_and_is_idempotent(self):
        self.archive()
        home = Path(self.env["HOME"])
        home.mkdir()
        rc = home / ".bashrc"
        rc.write_text("# existing config without final newline")
        for _ in range(2):
            result = self.run_install()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Reopen your terminal", result.stdout)
            self.assertIn("export PATH=", result.stdout)
        self.assertTrue(rc.read_text().startswith("# existing config without final newline\n"))
        self.assertEqual(rc.read_text().count("# tsk PATH"), 1)
        self.assertTrue((home / ".profile").exists())
        # Sourcing both login and interactive config twice must not duplicate PATH.
        command = '. "$HOME/.profile"; . "$HOME/.bashrc"; . "$HOME/.bashrc"; printf "%s" "$PATH"'
        activated = subprocess.run(["sh", "-c", command], env=self.env, text=True, capture_output=True, check=True)
        self.assertEqual(activated.stdout.split(":").count(str(home / ".local/bin")), 1)

    def test_bash_uses_existing_login_profile_without_shadowing_it(self):
        self.archive()
        home = Path(self.env["HOME"])
        home.mkdir()
        profile = home / ".bash_login"
        profile.write_text("# existing login\n")
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((home / ".bash_profile").exists())
        self.assertIn("# tsk PATH", profile.read_text())

    def test_zsh_uses_zdotdir_and_printed_export_handles_shell_metacharacters(self):
        self.archive()
        zdot = self.root / "zsh-config"
        dest = self.root / "bin ' $(touch INJECTED) $x `touch INJECTED`"
        result = self.run_install(SHELL="/bin/zsh", ZDOTDIR=str(zdot), TSK_INSTALL_DIR=str(dest))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((zdot / ".zshrc").exists())
        self.assertFalse((Path(self.env["HOME"]) / ".zshrc").exists())
        export = next(line.strip() for line in result.stdout.splitlines() if line.strip().startswith("export PATH="))
        for command in [export, '. "$1"']:
            activated = subprocess.run(["sh", "-c", command + '; command -v tsk', "sh", str(zdot / ".zshrc")], env=self.env, cwd=self.root, text=True, capture_output=True, check=True)
            self.assertEqual(activated.stdout.strip(), str(dest / "tsk"))
        self.assertFalse((self.root / "INJECTED").exists())

    def test_existing_path_needs_no_shell_edits(self):
        self.archive()
        home = Path(self.env["HOME"])
        result = self.run_install(PATH=f"{home / '.local/bin'}:{self.env['PATH']}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((home / ".bashrc").exists())
        self.assertFalse((home / ".profile").exists())
        self.assertNotIn("Reopen your terminal", result.stdout)

    def test_failed_download_never_edits_shell_config(self):
        self.archive(bad_checksum=True)
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((Path(self.env["HOME"]) / ".bashrc").exists())
        self.assertFalse((Path(self.env["HOME"]) / ".profile").exists())

    def test_unsafe_or_unsupported_shell_config_keeps_install_and_gives_manual_guidance(self):
        self.archive()
        home = Path(self.env["HOME"])
        home.mkdir()
        target = self.root / "dotfile"
        target.write_text("# leave this alone\n")
        rc = home / ".zshrc"
        rc.symlink_to(target)
        result = self.run_install(SHELL="/bin/zsh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(target.read_text(), "# leave this alone\n")
        self.assertIn("Could not update", result.stderr)
        self.assertIn("export PATH=", result.stdout)
        self.assertNotIn("Reopen your terminal", result.stdout)
        rc.unlink()
        result = self.run_install(SHELL="/bin/fish")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("configure PATH manually", result.stderr)
        self.assertFalse(rc.exists())
        self.assertFalse((home / ".config/fish").exists())

    def test_piped_script_configures_default_zsh_and_new_shell_finds_tsk(self):
        self.archive()
        environment = dict(self.env, SHELL="/bin/zsh")
        result = subprocess.run(["sh"], input=INSTALLER.read_text(), env=environment, cwd=self.root, text=True, capture_output=True, start_new_session=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        home = Path(self.env["HOME"])
        self.assertTrue((home / ".zshrc").exists())
        # Execute the generated source, not just a substring assertion.
        activated = subprocess.run(["sh", "-c", '. "$HOME/.zshrc"; tsk'], env=environment, text=True, capture_output=True, check=True)
        self.assertEqual(activated.stdout.strip(), "installed-fixture")

    @unittest.skipIf(os.geteuid() == 0, "root bypasses write permissions")
    def test_unwritable_startup_file_keeps_binary_and_reports_manual_setup(self):
        self.archive()
        home = Path(self.env["HOME"])
        home.mkdir()
        rc = home / ".zshrc"
        rc.write_text("# read only\n")
        rc.chmod(0o400)
        result = self.run_install(SHELL="/bin/zsh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(rc.read_text(), "# read only\n")
        self.assertIn("configure PATH manually", result.stderr)
        self.assertNotIn("Reopen your terminal", result.stdout)
        self.assertTrue((home / ".local/bin/tsk").exists())

    def test_fresh_interactive_shells_find_the_installed_binary(self):
        self.archive()
        for name in ["bash", "zsh"]:
            with self.subTest(shell=name):
                shell = shutil.which(name)
                if not shell:
                    self.skipTest(f"{name} is not installed")
                result = self.run_install(SHELL=shell)
                self.assertEqual(result.returncode, 0, result.stderr)
                fresh = subprocess.run([shell, "-i", "-c", "tsk"], env=self.env, stdin=subprocess.DEVNULL, text=True, capture_output=True)
                self.assertEqual(fresh.returncode, 0, fresh.stderr)
                self.assertTrue(fresh.stdout.rstrip().endswith("installed-fixture"), fresh.stdout)

    def test_path_separator_directories_are_refused_before_asset_download(self):
        self.archive()
        for name in ["bad:bin", "bad\nbin"]:
            result = self.run_install(TSK_VERSION="v1.2.3", TSK_INSTALL_DIR=str(self.root / name))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("PATH separators", result.stderr)
            self.assertFalse((self.root / "requests").exists())
            self.assertFalse((self.root / name).exists())

    def test_glob_characters_in_install_directory_are_literal(self):
        self.archive()
        for name in ["tsk*", "tsk?", "tsk[ab]"]:
            with self.subTest(name=name):
                dest = self.root / name
                result = self.run_install(TSK_INSTALL_DIR=str(dest), PATH=f"{self.root / 'tsk-old'}:{self.root / 'tska'}:{self.env['PATH']}")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("Reopen your terminal", result.stdout)
                self.assertIn(str(dest), result.stdout)
                active = subprocess.run(["sh", "-c", '. "$HOME/.bashrc"; command -v tsk'], env=self.env, text=True, capture_output=True, check=True)
                self.assertEqual(active.stdout.strip(), str(dest / "tsk"))

    def test_partial_bash_setup_names_skipped_file_and_keeps_successful_edit(self):
        self.archive()
        home = Path(self.env["HOME"])
        home.mkdir()
        target = self.root / "login-config"
        target.write_text("# managed elsewhere\n")
        login = home / ".bash_profile"
        login.symlink_to(target)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("# tsk PATH", (home / ".bashrc").read_text())
        self.assertEqual(target.read_text(), "# managed elsewhere\n")
        self.assertIn(str(login), result.stderr)
        self.assertIn("successful edits were kept", result.stderr)
        self.assertIn("export PATH=", result.stdout)

    def test_latest_release_and_all_platforms(self):
        for system, arch, target in [("Linux", "x86_64", "x86_64-unknown-linux-musl"), ("Linux", "aarch64", "aarch64-unknown-linux-musl"), ("Darwin", "arm64", "aarch64-apple-darwin"), ("Darwin", "x86_64", "x86_64-apple-darwin")]:
            with self.subTest(target=target):
                self.archive(target)
                result = self.run_install(MOCK_OS=system, MOCK_ARCH=arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                installed = self.root / "home/.local/bin/tsk"
                self.assertTrue(os.access(installed, os.X_OK))
                self.assertIn("installed-fixture", installed.read_text())
        requests = (self.root / "requests").read_text()
        self.assertIn("/releases/latest", requests)
        self.assertNotIn("/main/", requests)

    def test_explicit_version_custom_directory_and_upgrade(self):
        self.archive()
        dest = self.root / "custom dir"
        dest.mkdir()
        (dest / "tsk").write_text("old binary")
        result = self.run_install(TSK_VERSION="v1.2.3", TSK_INSTALL_DIR=str(dest))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("/releases/latest", (self.root / "requests").read_text())
        self.assertIn("installed-fixture", (dest / "tsk").read_text())

    def test_corrupt_download_preserves_existing_binary(self):
        self.archive(bad_checksum=True)
        dest = self.root / "bin"
        dest.mkdir()
        (dest / "tsk").write_text("old binary")
        result = self.run_install(TSK_INSTALL_DIR=str(dest))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum", result.stderr)
        self.assertEqual((dest / "tsk").read_text(), "old binary")

    def test_refuses_unsupported_platform_and_invalid_version_without_network(self):
        for options in [dict(MOCK_OS="Windows"), dict(MOCK_ARCH="riscv64"), dict(TSK_VERSION="main"), dict(TSK_VERSION="v1.2.3/../../main"), dict(TSK_VERSION="v1.2.3-rc1")]:
            with self.subTest(options=options):
                result = self.run_install(**options)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((self.root / "requests").exists())

    def test_missing_asset_and_missing_executable_do_not_install(self):
        for member in [None, "not-tsk"]:
            if member:
                self.archive(member=member)
            result = self.run_install()
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((self.root / "home/.local/bin/tsk").exists())

    def test_refuses_symlink_destination(self):
        self.archive()
        target = self.root / "untouched"
        target.write_text("keep")
        dest = self.root / "bin"
        dest.mkdir()
        (dest / "tsk").symlink_to(target)
        result = self.run_install(TSK_INSTALL_DIR=str(dest))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(target.read_text(), "keep")

    def test_destination_guards_refuse_before_downloading_assets(self):
        self.archive()
        dest = self.root / "directory-destination"
        (dest / "tsk").mkdir(parents=True)
        marker = dest / "tsk/keep"
        marker.write_text("untouched")
        for directory, message in [("relative-bin", "absolute path"), (str(dest), "destination is a directory")]:
            with self.subTest(directory=directory):
                (self.root / "requests").unlink(missing_ok=True)
                result = self.run_install(TSK_VERSION="v1.2.3", TSK_INSTALL_DIR=directory)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertFalse((self.root / "requests").exists())
                self.assertFalse((self.root / "relative-bin").exists())
                self.assertEqual(list((dest / "tsk").iterdir()), [marker])
                self.assertEqual(marker.read_text(), "untouched")

    def test_every_download_restricts_protocol_redirects_and_tls(self):
        self.archive()
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in (self.root / "curl-args").read_text().splitlines()]
        self.assertEqual(len(calls), 3)
        self.assertTrue(calls[0][-1].endswith("/releases/latest"))
        self.assertTrue(calls[1][-1].endswith(".tar.gz"))
        self.assertTrue(calls[2][-1].endswith("/SHA256SUMS"))
        for args in calls:
            with self.subTest(url=args[-1]):
                for flag in ["--proto", "--proto-redir"]:
                    self.assertIn(flag, args)
                    self.assertEqual(args[args.index(flag) + 1], "=https")
                self.assertIn("--tlsv1.2", args)
                self.assertNotIn("--insecure", args)
                self.assertNotIn("-k", args)

    def test_help_and_unknown_arguments_do_not_download(self):
        for argument, expected in [("--help", 0), ("--version", 1)]:
            result = subprocess.run(["sh", str(INSTALLER), argument], env=self.env, text=True, capture_output=True)
            self.assertEqual(result.returncode, expected, result.stderr)
            self.assertFalse((self.root / "requests").exists())

    def test_network_failure_never_installs(self):
        result = self.run_install(FAIL_DOWNLOAD="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "home/.local/bin/tsk").exists())

    def test_duplicate_checksum_is_refused(self):
        self.archive()
        sums = self.assets / "SHA256SUMS"
        sums.write_text(sums.read_text() * 2)
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "home/.local/bin/tsk").exists())

    def test_herdr_absent_stays_silent_about_plugin_setup(self):
        self.archive(record_setup=True)
        result = self.run_install(TSK_SETUP_LOG=str(self.setup_log), PATH=f"{self.bin}:/usr/bin:/bin")
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("Herdr detected", combined)
        self.assertNotIn("Herdr plugin:", combined)
        self.assertNotIn("Agent skills:", combined)
        self.assertNotIn("prefix+t", combined)
        self.assertIn("Done. Run tsk in a project directory to open the board.", combined)
        self.assertFalse(self.setup_log.exists())

    def test_custom_install_directory_never_executes_the_published_binary(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        dest = self.root / "shared-bin"
        result = self.run_install(
            TSK_INSTALL_DIR=str(dest),
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_DETECT_AGENTS="cursor",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((dest / "tsk").exists())
        self.assertFalse(self.setup_log.exists())
        # T41: the closing block says why setup did not run and uses the full binary path.
        self.assertIn("Done. Run tsk in a project directory to open the board.", result.stdout)
        self.assertIn("Custom install directory: setup was not run. When you are ready:", result.stdout)
        self.assertIn(f"    Herdr plugin:  {dest}/tsk setup herdr", result.stdout)
        self.assertIn(f"    Agent skills:  {dest}/tsk setup", result.stdout)
        self.assertNotIn("[y/N]", result.stdout + result.stderr)

    def run_update(self, **env):
        env.setdefault("PATH", f"{self.bin}:/usr/bin:/bin")
        env.setdefault("TSK_INSTALL_DIR", str(self.root / "managed-bin"))
        env.setdefault("TSK_UPDATE", "1")
        env.setdefault("TSK_SETUP_LOG", str(self.setup_log))
        return self.run_install(**env)

    def setup_calls(self):
        return self.setup_log.read_text().splitlines() if self.setup_log.exists() else []

    def test_update_names_both_versions(self):
        self.archive(record_setup=True)
        result = self.run_update(TSK_CURRENT_VERSION="v1.0.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Verifying checksum... ok\n\nCurrent version v1.0.0\nInstalling tsk v1.2.3...", result.stdout)
        self.assertNotIn("Custom install directory", result.stdout)
        self.assertNotIn("Updated in place", result.stdout)

    def test_first_install_names_the_installed_version_only(self):
        self.archive()
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Installing tsk v1.2.3...", result.stdout)
        self.assertNotIn("Current version", result.stdout)

    def test_update_refreshes_a_bound_herdr_plugin_without_asking(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_update(TSK_HERDR_BOUND="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Herdr plugin refreshed.", result.stdout)
        self.assertIn("setup herdr", self.setup_calls())
        self.assertNotIn("[y/N]", result.stdout + result.stderr)
        self.assertNotIn("    Herdr plugin:", result.stdout)
        # The user's keys are kept, so the closing line must not promise prefix+t.
        self.assertNotIn("prefix+t", result.stdout)
        self.assertIn("Done. Run tsk in a project directory to open the board.", result.stdout)

    def test_update_with_unbound_herdr_falls_back_to_the_install_nudge(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_update()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("Herdr plugin refreshed", result.stdout)
        self.assertNotIn("setup herdr\n", self.setup_log.read_text() + "\n" if self.setup_log.exists() else "")
        self.assertIn("    Herdr plugin:  tsk setup herdr", result.stdout)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_update_with_unbound_herdr_asks_on_a_tty(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_install_with_answer(
            b"y\n",
            TSK_INSTALL_DIR=str(self.root / "managed-bin"),
            TSK_UPDATE="1",
            TSK_SETUP_LOG=str(self.setup_log),
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("Herdr detected. Set up the Herdr plugin now? [y/N]", result.stdout)
        self.assertIn("setup herdr", self.setup_calls())

    def test_update_refreshes_outdated_skills_unattended(self):
        self.archive(record_setup=True)
        result = self.run_update(TSK_SKILL_STATES="claude\toutdated\t1.2.0\t/h/.claude/skills/tsk-cli/SKILL.md\ncodex\toutdated\t1.2.0\t/h/.codex/skills/tsk-cli/SKILL.md\npi\tcurrent\t1.3.0\t/h/.pi/skills/tsk-cli/SKILL.md")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("[Y/n]", result.stdout + result.stderr)
        self.assertEqual(self.setup_calls(), ["setup claude", "setup codex"])
        self.assertIn("Updated the tsk skill for claude, codex.", result.stdout)
        self.assertNotIn("    Agent skills:", result.stdout)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_update_asks_before_refreshing_outdated_skills_and_enter_means_yes(self):
        self.archive(record_setup=True)
        result = self.run_install_with_answer(
            b"\n",
            TSK_INSTALL_DIR=str(self.root / "managed-bin"),
            TSK_UPDATE="1",
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_SKILL_STATES="claude\toutdated\t1.2.0\t/h/.claude/skills/tsk-cli/SKILL.md",
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("tsk skill installed for claude (v1.2.0); update to v1.3.0? [Y/n]", result.stdout)
        self.assertEqual(self.setup_calls(), ["setup claude"])
        self.assertIn("Updated the tsk skill for claude.", result.stdout)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_update_declined_skill_refresh_leaves_the_nudge(self):
        self.archive(record_setup=True)
        result = self.run_install_with_answer(
            b"n\n",
            TSK_INSTALL_DIR=str(self.root / "managed-bin"),
            TSK_UPDATE="1",
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_SKILL_STATES="claude\toutdated\t1.2.0\t/h/.claude/skills/tsk-cli/SKILL.md",
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("[Y/n]", result.stdout)
        self.assertEqual(self.setup_calls(), [])
        self.assertIn("    Agent skills:  tsk setup", result.stdout)

    def test_update_with_current_skills_stays_quiet_and_never_adds_agents(self):
        self.archive(record_setup=True)
        result = self.run_update(TSK_SKILL_STATES="claude\tcurrent\t1.3.0\t/h/.claude/skills/tsk-cli/SKILL.md\ncursor\tmissing\t-\t/h/.cursor/skills/tsk-cli/SKILL.md")
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertEqual(self.setup_calls(), [])
        self.assertNotIn("Agents detected", combined)
        self.assertNotIn("Updated the tsk skill", combined)
        self.assertNotIn("    Agent skills:", combined)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_update_with_no_skill_installed_offers_the_first_install_ask(self):
        self.archive(record_setup=True)
        result = self.run_install_with_answer(
            b"y\n",
            TSK_INSTALL_DIR=str(self.root / "managed-bin"),
            TSK_UPDATE="1",
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_SKILL_STATES="cursor\tmissing\t-\t/h/.cursor/skills/tsk-cli/SKILL.md\ncodex\tmissing\t-\t/h/.codex/skills/tsk-cli/SKILL.md",
            PATH=f"{self.bin}:/usr/bin:/bin",
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("Agents detected: cursor, codex. Install the tsk skill for them? [y/N]", result.stdout)
        self.assertEqual(self.setup_calls(), ["setup agents --yes"])

    def test_update_onto_a_release_without_the_probes_keeps_the_plain_nudges(self):
        # The site serves the newest installer to every `tsk update`, including ones that
        # land on a release predating the probes. Nothing may go silent there.
        self.archive(legacy=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_update(TSK_DETECT_AGENTS="cursor claude")
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("Herdr plugin refreshed", combined)
        self.assertNotIn("Updated the tsk skill", combined)
        self.assertNotIn("[Y/n]", combined)
        self.assertIn("    Herdr plugin:  tsk setup herdr", result.stdout)
        self.assertIn("    Agent skills:  tsk setup", result.stdout)
        self.assertEqual(self.setup_calls(), [])

    def test_update_onto_a_release_without_the_probes_stays_quiet_without_agents(self):
        self.archive(legacy=True)
        result = self.run_update()
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("    Agent skills:", combined)
        self.assertNotIn("Agents detected", combined)
        self.assertEqual(self.setup_calls(), [])

    def test_update_refuses_to_move_backwards(self):
        # releases/latest resolves to v1.2.3 in this rig; a copy built from a newer tag stays.
        # v1.10.0 and v1.2.10 sort before v1.2.3 lexically: the compare must be numeric.
        self.archive(record_setup=True)
        for current in ("v1.3.0", "v1.10.0", "v1.2.10", "v2.0.0", "v10.0.0"):
            result = self.run_update(TSK_CURRENT_VERSION=current)
            self.assertEqual(result.returncode, 1, (current, result.stdout))
            self.assertIn(f"Current version {current}", result.stdout)
            self.assertIn(f"latest published release is v1.2.3, older than the installed {current}; nothing changed", result.stderr)
            self.assertFalse((self.root / "managed-bin/tsk").exists())
        self.assertEqual(self.setup_calls(), [])

    def test_update_from_an_older_or_equal_copy_proceeds(self):
        # v0.10.9 and v1.1.10 sort after v1.2.3 lexically; a non-release string skips the compare.
        self.archive(record_setup=True)
        for current in ("v1.2.3", "v1.2.2", "v0.10.9", "v1.1.10", "v1.10.0-dev"):
            result = self.run_update(TSK_CURRENT_VERSION=current)
            self.assertEqual(result.returncode, 0, (current, result.stderr))
            self.assertIn("Installing tsk v1.2.3...", result.stdout)

    def test_update_failed_herdr_refresh_keeps_the_binary_and_the_nudge(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_update(TSK_HERDR_BOUND="1", TSK_SETUP_FAIL="herdr")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("tsk setup herdr failed; install succeeded.\n    tsk setup: config.toml is not writable", result.stderr)
        # The reason is tsk's own stderr, not something the installer invented.
        self.assertEqual(result.stderr.count("config.toml is not writable"), 1)
        self.assertNotIn("Herdr plugin refreshed", result.stdout)
        self.assertIn("    Herdr plugin:  tsk setup herdr", result.stdout)
        self.assertNotIn("prefix+t", result.stdout)
        self.assertTrue((self.root / "managed-bin/tsk").exists())

    def test_update_prompt_names_each_agent_with_its_own_version(self):
        self.archive(record_setup=True)
        result = self.run_update(TSK_SKILL_STATES="claude\toutdated\t1.1.0\t/h/a\ncodex\toutdated\t-\t/h/b")
        self.assertEqual(result.returncode, 0, result.stderr)
        # Unattended: no prompt, but the refresh still runs and names both.
        self.assertEqual(self.setup_calls(), ["setup claude", "setup codex"])
        self.assertIn("Updated the tsk skill for claude, codex.", result.stdout)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_update_prompt_lists_mixed_versions_per_agent(self):
        self.archive(record_setup=True)
        result = self.run_install_with_answer(
            b"n\n",
            TSK_INSTALL_DIR=str(self.root / "managed-bin"),
            TSK_UPDATE="1",
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_SKILL_STATES="claude\toutdated\t1.1.0\t/h/a\ncodex\toutdated\t-\t/h/b",
            PATH=f"{self.bin}:/usr/bin:/bin",
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("tsk skill installed for claude (v1.1.0), codex (unknown version); update to v1.3.0? [Y/n]", result.stdout)

    def test_update_reports_a_blocked_skill_and_a_failed_refresh(self):
        self.archive(record_setup=True)
        result = self.run_update(
            TSK_SKILL_STATES="claude\toutdated\t1.2.0\t/h/.claude/skills/tsk-cli/SKILL.md\ncodex\toutdated\t1.2.0\t/h/.codex/skills/tsk-cli/SKILL.md\npi\tblocked-symlink\t-\t/h/.pi/skills/tsk-cli/SKILL.md",
            TSK_SETUP_FAIL="codex",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("tsk skill for pi not refreshed: /h/.pi/skills/tsk-cli/SKILL.md is a symlink", result.stderr)
        self.assertIn("tsk setup codex failed; install succeeded.\n    tsk setup: skill dir is a symlink", result.stderr)
        self.assertIn("Updated the tsk skill for claude.", result.stdout)
        self.assertIn("    Agent skills:  tsk setup", result.stdout)
        self.assertTrue((self.root / "managed-bin/tsk").exists())

    def test_herdr_present_without_tty_skips_with_guidance(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_install(TSK_SETUP_LOG=str(self.setup_log))
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("[y/N]", combined)
        self.assertIn("Done. Run tsk in a project directory to open the board.", combined)
        self.assertIn("    Herdr plugin:  tsk setup herdr", combined)
        self.assertNotIn("prefix+t", combined)
        self.assertNotIn("    Agent skills:", combined)
        self.assertFalse(self.setup_log.exists())

    def test_herdr_present_in_ci_skips_without_asking(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_install(TSK_SETUP_LOG=str(self.setup_log), CI="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("[y/N]", combined)
        self.assertIn("Done. Run tsk in a project directory to open the board.", combined)
        self.assertIn("    Herdr plugin:  tsk setup herdr", combined)
        self.assertNotIn("prefix+t", combined)
        self.assertFalse(self.setup_log.exists())

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_herdr_prompt_yes_runs_installed_tsk_setup(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_install_with_answer(b"y\n", TSK_SETUP_LOG=str(self.setup_log))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("[y/N]", result.stdout)
        self.assertIn("Running tsk setup herdr...", result.stdout)
        installed = self.root / "home/.local/bin/tsk"
        self.assertTrue(self.setup_log.exists(), result.stdout)
        self.assertEqual(self.setup_log.read_text().strip(), "setup herdr")
        self.assertTrue(installed.exists())
        self.assertIn(
            "Done. Run tsk in a project directory to open the board, or press prefix+t in Herdr.",
            result.stdout,
        )
        self.assertNotIn("    Herdr plugin:", result.stdout)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_herdr_prompt_no_skips_setup_with_guidance(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_install_with_answer(b"n\n", TSK_SETUP_LOG=str(self.setup_log))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("[y/N]", result.stdout)
        self.assertIn("Done. Run tsk in a project directory to open the board.", result.stdout)
        self.assertIn("    Herdr plugin:  tsk setup herdr", result.stdout)
        self.assertNotIn("prefix+t", result.stdout)
        self.assertFalse(self.setup_log.exists())

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_herdr_setup_failure_keeps_install_and_prints_guidance(self):
        archive = self.assets / "tsk-v1.2.3-x86_64-unknown-linux-musl.tar.gz"
        with tarfile.open(archive, "w:gz") as out:
            data = b"""#!/bin/sh
if [ "${1:-}" = setup ] && [ "${2:-}" = herdr ]; then
    echo setup-failed >&2
    exit 7
fi
echo installed-fixture
"""
            info = tarfile.TarInfo("tsk")
            info.size = len(data)
            info.mode = 0o755
            out.addfile(info, io.BytesIO(data))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.assets / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_install_with_answer(b"y\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("tsk setup herdr failed", result.stdout)
        self.assertTrue((self.root / "home/.local/bin/tsk").exists())
        self.assertIn("Done. Run tsk in a project directory to open the board.", result.stdout)
        self.assertIn("    Herdr plugin:  tsk setup herdr", result.stdout)
        self.assertNotIn("prefix+t", result.stdout)

    def test_agent_skills_absent_stays_silent(self):
        self.archive(record_setup=True)
        result = self.run_install(TSK_SETUP_LOG=str(self.setup_log))
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("    Agent skills:", combined)
        self.assertNotIn("Agents detected:", combined)

    def test_agent_skills_detected_without_tty_nudge(self):
        self.archive(record_setup=True)
        result = self.run_install(
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_DETECT_AGENTS="cursor claude",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("[y/N]", combined)
        self.assertIn("    Agent skills:  tsk setup", combined)
        self.assertFalse(self.setup_log.exists())

    def test_agent_skills_detected_in_ci_nudge(self):
        self.archive(record_setup=True)
        result = self.run_install(
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_DETECT_AGENTS="cursor",
            CI="1",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        combined = result.stdout + result.stderr
        self.assertNotIn("Agents detected:", combined)
        self.assertIn("    Agent skills:  tsk setup", combined)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_agent_skills_prompt_yes_runs_agents_yes(self):
        self.archive(record_setup=True)
        result = self.run_install_with_answer(
            b"y\n",
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_DETECT_AGENTS="cursor",
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("Agents detected: cursor. Install the tsk skill for them?", result.stdout)
        self.assertIn("setup agents --yes", self.setup_log.read_text())
        self.assertNotIn("    Agent skills:  tsk setup", result.stdout)

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_agent_skills_prompt_no_nudge(self):
        self.archive(record_setup=True)
        result = self.run_install_with_answer(
            b"n\n",
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_DETECT_AGENTS="cursor claude",
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("    Agent skills:  tsk setup", result.stdout)
        self.assertFalse(self.setup_log.exists())

    @unittest.skipUnless(os.name == "posix", "PTY prompt requires POSIX")
    def test_herdr_and_skills_yes_answers_both_prompts(self):
        self.archive(record_setup=True)
        self.command("herdr", "#!/bin/sh\nexit 0\n")
        result = self.run_install_with_answer(
            b"y\ny\n",
            TSK_SETUP_LOG=str(self.setup_log),
            TSK_DETECT_AGENTS="cursor",
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        log = self.setup_log.read_text()
        self.assertIn("setup herdr", log)
        self.assertIn("setup agents --yes", log)
        self.assertIn("Running tsk setup herdr...", result.stdout)
        self.assertIn("Running tsk setup agents...", result.stdout)
        self.assertIn("Agents detected: cursor. Install the tsk skill for them?", result.stdout)
        self.assertIn(
            "Done. Run tsk in a project directory to open the board, or press prefix+t in Herdr.",
            result.stdout,
        )
        self.assertNotIn("    Herdr plugin:", result.stdout)
        self.assertNotIn("    Agent skills:", result.stdout)



if __name__ == "__main__":
    unittest.main()
