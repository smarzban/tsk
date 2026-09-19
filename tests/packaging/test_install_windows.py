"""Offline contracts for the Windows PowerShell installer."""
import hashlib
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import time
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
INSTALLER = ROOT / "site/public/install.ps1"


class WindowsInstallerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = INSTALLER.read_text()

    def test_exposes_help_and_environment_configuration(self):
        self.assertRegex(self.source, r"(?i)\[switch\]\s*\$Help")
        self.assertIn("TSK_INSTALL_DIR", self.source)
        self.assertIn("TSK_VERSION", self.source)
        self.assertIn("TSK_UPDATE", self.source)
        self.assertIn("TSK_UPDATE_PID", self.source)
        self.assertIn("LOCALAPPDATA", self.source)
        self.assertRegex(self.source, r"(?i)Programs[\\/]tsk[\\/]bin")
        self.assertIn("x86_64-pc-windows-msvc", self.source)
        self.assertIn("GetNativeSystemInfo", self.source)
        self.assertNotIn("PROCESSOR_ARCHITEW6432", self.source)
        self.assertIn("Windows ARM64 is not supported", self.source)
        self.assertRegex(self.source, r"(?i)\[switch\]\s*\$NoPathUpdate")
        self.assertIn("-NoPathUpdate", self.source)

    def test_download_is_bounded_pinned_and_checksum_verified_before_extract(self):
        checksum = self.source.index("$actual = Get-Sha256 $archive")
        extract = self.source.index("Expand-Archive")
        publish = self.source.index("Move-Atomic $staged")
        self.assertLess(checksum, extract)
        self.assertLess(extract, publish)
        self.assertIn("SHA256SUMS", self.source)
        self.assertIn("tsk-$version-$target.zip", self.source)
        self.assertIn("releases/download/$version", self.source)
        self.assertNotIn("Invoke-Expression", self.source)
        for token in [
            "ContentLength",
            "MaximumAutomaticRedirections",
            "ReadWriteTimeout",
            "Diagnostics.Stopwatch",
            "Elapsed.TotalSeconds",
            "DefaultWebProxy",
            "DefaultNetworkCredentials",
            "MaxArchiveBytes",
            "MaxExpandedBytes",
        ]:
            self.assertIn(token, self.source)
        self.assertRegex(self.source, r"(?is)while\s*\(.+Read\(.+written.+MaxBytes")
        self.assertRegex(self.source, r"(?is)Entries.+Length.+MaxExpandedBytes.+Expand-Archive")
        self.assertIn("AllowAutoRedirect = $false", self.source)
        self.assertRegex(self.source, r"(?is)Headers\['Location'\].+Scheme.+https")
        self.assertRegex(self.source, r"(?is)ResponseUri.+Scheme.+https")

    def test_path_update_preserves_raw_user_registry_value_and_kind(self):
        self.assertIn("DoNotExpandEnvironmentNames", self.source)
        self.assertIn("GetValueKind", self.source)
        self.assertRegex(self.source, r"(?is)SetValue\(.+\$updated.+\$kind")
        self.assertNotRegex(self.source, r"SetEnvironmentVariable\([^\n]+['\"]Machine['\"]")
        self.assertIn("ExpandEnvironmentVariables", self.source)
        self.assertIn("OrdinalIgnoreCase", self.source)
        self.assertIn("installation directory cannot contain a semicolon or newline", self.source)

    def test_refuses_downgrades_and_reparse_paths(self):
        self.assertIn("TSK_CURRENT_VERSION", self.source)
        self.assertRegex(self.source, r"(?is)releaseVersion\s+-lt\s+\$currentVersion.+nothing changed")
        self.assertIn("Assert-NoReparsePath $installDir", self.source)
        self.assertIn("[IO.FileAttributes]::ReparsePoint", self.source)

    def test_update_helper_uses_stock_windows_powershell_from_any_host(self):
        self.assertIn("GetSystemDirectory", self.source)
        self.assertIn("WindowsPowerShell\\v1.0\\powershell.exe", self.source)
        self.assertNotIn("$env:SystemRoot", self.source)
        self.assertNotIn("Join-Path $PSHOME 'powershell.exe'", self.source)
        self.assertIn("Get-LatestReleaseTag", self.source)
        self.assertNotIn("BaseResponse.ResponseUri", self.source)

    def test_update_refreshes_only_existing_herdr_and_skills(self):
        self.assertIn("setup herdr --check", self.source)
        self.assertIn("setup --skill-states", self.source)
        self.assertIn("-eq 'outdated'", self.source)
        self.assertIn(".tsk-update-error.log", self.source)
        self.assertIn("run tsk setup herdr", self.source)
        self.assertNotRegex(self.source, r"(?is)Refresh-ExistingSetup.+setup agents")

    @unittest.skipUnless(
        os.name == "nt" and platform.machine().lower() in {"arm64", "aarch64"},
        "native Windows ARM64 smoke",
    )
    def test_rejects_native_arm64_before_downloading(self):
        result = subprocess.run(
            [
                "powershell.exe",
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(INSTALLER),
                "-NoPathUpdate",
            ],
            env=dict(os.environ, TSK_VERSION="v1.2.3"),
            text=True,
            capture_output=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Windows ARM64 is not supported", result.stderr)
        self.assertNotIn("Downloading", result.stdout)

    @unittest.skipUnless(os.name == "nt", "native Windows PowerShell smoke")
    def test_refuses_downgrade_before_downloading(self):
        environment = dict(
            os.environ,
            TSK_VERSION="v1.2.3",
            TSK_UPDATE="1",
            TSK_CURRENT_VERSION="v1.2.4",
        )
        result = subprocess.run(
            [
                "powershell.exe",
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(INSTALLER),
                "-NoPathUpdate",
            ],
            env=environment,
            text=True,
            capture_output=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("older than installed", result.stderr)
        self.assertNotIn("Downloading", result.stdout)

    @unittest.skipUnless(os.name == "nt", "native Windows PowerShell smoke")
    def test_installs_verified_zip_offline_with_real_powershell(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets = root / "assets"
            assets.mkdir()
            destination = root / "Windows ü path" / "bin"
            version = "v1.2.3"
            archive_name = f"tsk-{version}-x86_64-pc-windows-msvc.zip"
            archive = assets / archive_name
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("tsk.exe", b"offline-windows-fixture")
                bundle.writestr("LICENSE", b"license")
                bundle.writestr("README.md", b"readme")
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            (assets / "SHA256SUMS").write_text(f"{digest}  {archive_name}\n")

            quote = lambda value: str(value).replace("'", "''")
            harness = f"""
function Invoke-TskDownload {{
    param([string] $Uri, [string] $Destination, [long] $MaxBytes)
    $source = Join-Path '{quote(assets)}' ([IO.Path]::GetFileName($Uri))
    if ((Get-Item -LiteralPath $source).Length -gt $MaxBytes) {{ throw 'fixture exceeds download limit' }}
    Copy-Item -LiteralPath $source -Destination $Destination
}}
. '{quote(INSTALLER)}' -NoPathUpdate
"""
            environment = dict(os.environ, TSK_VERSION=version, TSK_INSTALL_DIR=str(destination))
            result = subprocess.run(
                ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", harness],
                env=environment,
                text=True,
                capture_output=True,
                timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            installed = destination / "tsk.exe"
            self.assertTrue(
                installed.exists(),
                f"installer output:\n{result.stdout}\n{result.stderr}\ncreated: {[str(path.relative_to(root)) for path in root.rglob('*')]}",
            )
            self.assertEqual(installed.read_bytes(), b"offline-windows-fixture")
            self.assertIn("Verifying checksum... ok", result.stdout)

            holder_script = (
                "$stream=[IO.File]::Open('"
                + quote(installed)
                + "',[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read); "
                + "Start-Sleep -Seconds 8"
            )
            holder = subprocess.Popen(
                ["powershell.exe", "-NoLogo", "-NoProfile", "-Command", holder_script],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                time.sleep(1)
                update_harness = f"""
function Invoke-TskDownload {{
    param([string] $Uri, [string] $Destination, [long] $MaxBytes)
    $source = Join-Path '{quote(assets)}' ([IO.Path]::GetFileName($Uri))
    if ((Get-Item -LiteralPath $source).Length -gt $MaxBytes) {{ throw 'fixture exceeds download limit' }}
    Copy-Item -LiteralPath $source -Destination $Destination
}}
$env:TSK_UPDATE_PID = $PID
. '{quote(INSTALLER)}' -NoPathUpdate
"""
                update_environment = dict(
                    environment,
                    TSK_UPDATE="1",
                    TSK_CURRENT_VERSION="v1.2.2",
                    TSK_UPDATE_HELPER_RETRY_SECONDS="1",
                )
                update = subprocess.run(
                    ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", update_harness],
                    env=update_environment,
                    text=True,
                    capture_output=True,
                    timeout=60,
                )
                self.assertEqual(update.returncode, 0, update.stderr)
                self.assertIn("Update staged", update.stdout)
                error_log = destination / ".tsk-update-error.log"
                for _ in range(50):
                    if error_log.exists():
                        break
                    time.sleep(0.1)
                self.assertTrue(error_log.exists(), "detached replacement timeout was not reported")
                self.assertIn("run tsk update again", error_log.read_text())
                self.assertFalse(list(destination.glob(".tsk-*.exe")))
                self.assertFalse(list(destination.glob(".tsk-update-*.ps1")))
            finally:
                holder.terminate()
                holder.wait(timeout=10)

    @unittest.skipUnless(os.name == "nt", "native PowerShell 7 helper smoke")
    def test_pwsh_default_latest_and_helper_replacement_succeed(self):
        pwsh = shutil.which("pwsh.exe") or shutil.which("pwsh")
        binary_value = os.environ.get("TSK_TEST_BINARY")
        if not pwsh or not binary_value:
            self.skipTest("PowerShell 7 and TSK_TEST_BINARY are required")
        binary = Path(binary_value).resolve()
        self.assertTrue(binary.is_file(), binary)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets = root / "assets"
            destination = root / "bin"
            home = root / "home"
            assets.mkdir()
            destination.mkdir()
            home.mkdir()
            version = "v1.2.3"
            archive_name = f"tsk-{version}-x86_64-pc-windows-msvc.zip"
            archive = assets / archive_name
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.write(binary, "tsk.exe")
                bundle.writestr("LICENSE", b"license")
                bundle.writestr("README.md", b"readme")
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            (assets / "SHA256SUMS").write_text(f"{digest}  {archive_name}\n")
            installed = destination / "tsk.exe"
            installed.write_bytes(b"old-binary")

            quote = lambda value: str(value).replace("'", "''")
            holder_script = (
                "$stream=[IO.File]::Open('"
                + quote(installed)
                + "',[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read); "
                + "Start-Sleep -Seconds 30"
            )
            holder = subprocess.Popen(
                ["powershell.exe", "-NoLogo", "-NoProfile", "-Command", holder_script],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                time.sleep(1)
                harness = f"""
function Get-LatestReleaseTag {{ param([string] $Uri) '{version}' }}
function Invoke-TskDownload {{
    param([string] $Uri, [string] $Destination, [long] $MaxBytes)
    $source = Join-Path '{quote(assets)}' ([IO.Path]::GetFileName($Uri))
    if ((Get-Item -LiteralPath $source).Length -gt $MaxBytes) {{ throw 'fixture exceeds download limit' }}
    Copy-Item -LiteralPath $source -Destination $Destination
}}
$env:TSK_UPDATE_PID = $PID
. '{quote(INSTALLER)}' -NoPathUpdate
"""
                environment = dict(
                    os.environ,
                    TSK_INSTALL_DIR=str(destination),
                    TSK_UPDATE="1",
                    TSK_CURRENT_VERSION="v1.2.2",
                    TSK_UPDATE_HELPER_RETRY_SECONDS="15",
                    TSK_STATE_DIR=str(root / "state"),
                    HOME=str(home),
                    USERPROFILE=str(home),
                    APPDATA=str(home / "AppData" / "Roaming"),
                    LOCALAPPDATA=str(home / "AppData" / "Local"),
                    XDG_CONFIG_HOME=str(home / ".config"),
                    XDG_STATE_HOME=str(home / ".local" / "state"),
                    HERDR_SOCKET_PATH=str(root / "no-herdr.sock"),
                )
                environment.pop("TSK_VERSION", None)
                update = subprocess.run(
                    [pwsh, "-NoLogo", "-NoProfile", "-Command", harness],
                    env=environment,
                    text=True,
                    capture_output=True,
                    timeout=60,
                )
                self.assertEqual(update.returncode, 0, update.stderr)
                self.assertIn("Update staged", update.stdout)
            finally:
                holder.terminate()
                holder.wait(timeout=10)

            expected_binary_digest = hashlib.sha256(binary.read_bytes()).digest()
            for _ in range(150):
                if not list(destination.glob(".tsk-*.exe")) and not list(destination.glob(".tsk-update-*.ps1")):
                    break
                time.sleep(0.1)
            self.assertEqual(hashlib.sha256(installed.read_bytes()).digest(), expected_binary_digest)
            self.assertFalse(list(destination.glob(".tsk-*.exe")))
            self.assertFalse(list(destination.glob(".tsk-update-*.ps1")))
            self.assertFalse((destination / ".tsk-update-error.log").exists())

    @unittest.skipUnless(os.name == "nt", "native Windows PowerShell smoke")
    def test_rejects_junction_install_directory_before_downloading(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            real = root / "real"
            junction = root / "junction"
            real.mkdir()
            made = subprocess.run(
                ["cmd.exe", "/d", "/c", "mklink", "/J", str(junction), str(real)],
                text=True,
                capture_output=True,
                timeout=10,
            )
            if made.returncode != 0:
                self.skipTest("could not create an unprivileged junction: " + made.stderr)
            result = subprocess.run(
                [
                    "powershell.exe",
                    "-NoLogo",
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(INSTALLER),
                    "-NoPathUpdate",
                ],
                env=dict(os.environ, TSK_VERSION="v1.2.3", TSK_INSTALL_DIR=str(junction / "bin")),
                text=True,
                capture_output=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("reparse point", result.stderr)
            self.assertNotIn("Downloading", result.stdout)

    def test_locked_self_update_stages_detached_atomic_replacement(self):
        for token in ["MoveFileEx", "MOVEFILE_REPLACE_EXISTING", "Start-Process", "Get-Process", "TSK_UPDATE_PID"]:
            self.assertIn(token, self.source)
        self.assertRegex(self.source, r"(?is)Get-Process.+Start-Sleep.+MoveFileEx")
        self.assertIn("Find-OtherRunningCopies", self.source)
        self.assertIn("close every other running tsk board and retry", self.source)
        self.assertRegex(self.source, r"(?i)-EncodedCommand")
        self.assertRegex(self.source, r"(?is)TSK_UPDATE.+LastMoveError.+(?:5|32)")


if __name__ == "__main__":
    unittest.main()
