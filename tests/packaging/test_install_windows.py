"""Offline contracts for the Windows PowerShell installer."""
import hashlib
import os
import platform
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
INSTALLER = ROOT / "site/public/install.ps1"
WINDOWS_TARGETS = ("aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc")


def write_windows_archives(assets, version, executable):
    checksums = []
    for target in WINDOWS_TARGETS:
        archive_name = f"tsk-{version}-{target}.zip"
        archive = assets / archive_name
        with zipfile.ZipFile(archive, "w") as bundle:
            payload = executable[target] if isinstance(executable, dict) else executable
            if isinstance(payload, Path):
                bundle.write(payload, "tsk.exe")
            else:
                bundle.writestr("tsk.exe", payload)
            bundle.writestr("LICENSE", b"license")
            bundle.writestr("README.md", b"readme")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        checksums.append(f"{digest}  {archive_name}\n")
    (assets / "SHA256SUMS").write_text("".join(checksums), encoding="utf-8")


class WindowsInstallerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = INSTALLER.read_text(encoding="utf-8")

    def test_exposes_help_and_environment_configuration(self):
        self.assertIn("unknown option", self.source)
        self.assertIn("TSK_INSTALL_DIR", self.source)
        self.assertIn("TSK_VERSION", self.source)
        self.assertIn("TSK_UPDATE", self.source)
        self.assertIn("TSK_UPDATE_PID", self.source)
        self.assertIn("LOCALAPPDATA", self.source)
        self.assertRegex(self.source, r"(?i)Programs[\\/]tsk[\\/]bin")
        self.assertIn("x86_64-pc-windows-msvc", self.source)
        self.assertIn("aarch64-pc-windows-msvc", self.source)
        self.assertIn("IsWow64Process2", self.source)
        self.assertIn("EntryPointNotFoundException", self.source)
        self.assertIn("GetNativeSystemInfo", self.source)
        self.assertRegex(self.source, r"(?is)IsWow64Process2\(.+out nativeMachine.+return nativeMachine")
        self.assertRegex(self.source, r"(?is)EntryPointNotFoundException.+processorArchitecture == 9.+0x8664.+0")
        self.assertNotIn("PROCESSOR_ARCHITEW6432", self.source)
        self.assertIn("0xaa64", self.source)
        self.assertIn("0x8664", self.source)
        self.assertNotIn("Windows ARM64 is not supported", self.source)
        self.assertRegex(self.source, r"(?is)0xaa64.+aarch64-pc-windows-msvc")
        self.assertRegex(self.source, r"(?is)0x8664.+x86_64-pc-windows-msvc")
        self.assertIn("'^-(?i:nopathupdate)$'", self.source)
        self.assertIn("'^-(?i:help|h|\\?)$'", self.source)
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
        self.assertIn("SendMessageTimeout", self.source)
        self.assertIn("0x001a", self.source)
        self.assertIn("OrdinalIgnoreCase", self.source)
        self.assertIn("installation directory cannot contain a semicolon or newline", self.source)

    def test_path_failure_keeps_successful_binary_install_and_prints_manual_guidance(self):
        self.assertIn("function Update-InstallerPath", self.source)
        self.assertRegex(
            self.source,
            r"(?is)try\s*\{\s*Update-InstallerPath.+catch.+PATH setup failed; install succeeded.+Windows user PATH",
        )

    def test_help_names_public_knobs_and_latest_stable_without_internal_handoff(self):
        help_text = self.source.split("function Show-Help", 1)[1].split("if ($Help)", 1)[0]
        self.assertIn('powershell -c "irm https://www.gettsk.sh/install.ps1 | iex"', help_text)
        self.assertIn("public release tag", help_text)
        self.assertIn("latest stable release", help_text)
        self.assertNotIn("TSK_UPDATE_PID", help_text)
        self.assertNotIn("TSK_UPDATE        ", help_text)
        entrypoint = self.source.rsplit("try {\n    Main", 1)[1]
        self.assertNotIn("exit 1", entrypoint)
        self.assertIn("throw ('tsk install: '", entrypoint)

    def test_body_runs_in_its_own_scope_unless_dot_sourced(self):
        self.assertNotIn("$script:", self.source)
        preamble, body = self.source.split("$__tskInstallerBody = {", 1)
        self.assertNotIn("StrictMode", preamble)
        self.assertNotIn("ErrorActionPreference", preamble)
        # A top-level param() block would overwrite the caller's $Help and $NoPathUpdate under iex.
        self.assertNotRegex(preamble, r"(?im)^\s*param\s*\(")
        self.assertIn("Set-StrictMode -Version 2.0", body)
        self.assertRegex(
            self.source,
            r"(?s)if \(\$__tskInstallerBody\.File -and \$MyInvocation\.InvocationName -eq '\.'\) \{\s*\. \$__tskInstallerBody.+\} else \{\s*try \{\s*& \$__tskInstallerBody @\(if \(\$__tskInstallerBody\.File\) \{ \$args \}\)",
        )

    @unittest.skipUnless(os.name == "nt", "native Windows PowerShell smoke")
    def test_invoke_expression_leaves_the_caller_session_untouched(self):
        help_source = self.source.replace("$Help = $false", "$Help = $true", 1)
        failure_source = self.source.replace(
            "try {\n    Main\n}", "try {\n    throw 'scope-smoke'\n}", 1
        )
        self.assertNotEqual(help_source, self.source)
        self.assertNotEqual(failure_source, self.source)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "help.ps1").write_text(help_source, encoding="utf-8")
            (root / "failure.ps1").write_text(failure_source, encoding="utf-8")
            quote = lambda value: str(value).replace("'", "''")
            harness = f"""
$ErrorActionPreference = 'Continue'
$Help = 'caller help sentinel'
$NoPathUpdate = 'caller path sentinel'
$helpText = [IO.File]::ReadAllText('{quote(root / "help.ps1")}')
$failureText = [IO.File]::ReadAllText('{quote(root / "failure.ps1")}')
$output = @($helpText | Invoke-Expression)
if (-not ($output -join "`n").Contains('irm https://www.gettsk.sh/install.ps1 | iex')) {{ throw 'help did not run' }}
$helpText | Invoke-Expression | Out-Null
# Under iex, $args belongs to the enclosing command, never to the installer.
function Invoke-InstallerFromWrapper {{ $helpText | Invoke-Expression }}
$wrapped = @(Invoke-InstallerFromWrapper -NoPathUpdate unrelated-argument)
if (-not ($wrapped -join "`n").Contains('Usage:')) {{ throw 'enclosing arguments reached the installer' }}
foreach ($name in 'Main', 'Show-Help', 'Fail', 'Invoke-TskDownload') {{
    if (Test-Path "Function:\\$name") {{ throw "function leaked: $name" }}
}}
if ($Help -ne 'caller help sentinel' -or $NoPathUpdate -ne 'caller path sentinel') {{ throw 'caller variables were overwritten' }}
foreach ($name in '__tskInstallerBody', 'TskInstallerState', 'MaxArchiveBytes', 'InstallerArguments') {{
    if (Get-Variable -Name $name -Scope Global -ErrorAction SilentlyContinue) {{ throw "variable leaked: $name" }}
}}
if ($ErrorActionPreference -ne 'Continue') {{ throw 'ErrorActionPreference leaked' }}
$null = $undefinedAfterInstallerRun
$failed = $false
try {{ $failureText | Invoke-Expression }} catch {{
    if ($_.Exception.Message -ne 'tsk install: scope-smoke') {{ throw }}
    $failed = $true
}}
if (-not $failed) {{ throw 'installer failure did not reach the caller' }}
'caller-session-clean'
"""
            result = subprocess.run(
                ["powershell.exe", "-NoLogo", "-NoProfile", "-Command", harness],
                encoding="utf-8",
                errors="replace",
                capture_output=True,
                timeout=60,
            )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("caller-session-clean", result.stdout)

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

    def test_first_install_matches_unix_setup_and_closing_contract(self):
        for text in [
            "Herdr detected. Set up the Herdr plugin now? [y/N] ",
            "Agents detected: {0}. Install the tsk skill for them? [y/N] ",
            "Running tsk setup herdr...",
            "Running tsk setup agents...",
            "Done. Run tsk in a project directory to open the board",
            "Herdr plugin:  tsk setup herdr",
            "Agent skills:  tsk setup",
            "Custom install directory: setup was not run. When you are ready:",
        ]:
            self.assertIn(text, self.source)
        self.assertNotIn("Optional setup:", self.source)
        self.assertRegex(self.source, r"(?is)setup --detected-ids.+setup agents --yes")
        self.assertIn("Test-InstallerInteractive", self.source)
        self.assertIn("Read-InstallerAnswer", self.source)

    def test_update_skill_prompt_matches_unix_contract(self):
        self.assertIn("update to v{1}? [Y/n] ", self.source)
        self.assertRegex(self.source, r"(?is)setup --skill-states.+outdated.+Read-InstallerAnswer")
        self.assertIn("Updated the tsk skill for {0}.", self.source)

    def test_locked_update_defers_selected_integration_writes_until_replacement(self):
        self.assertIn("Prepare-DeferredHerdrSetup $staged", self.source)
        self.assertIn("Prepare-DeferredAgentSkills $staged", self.source)
        self.assertNotIn("Invoke-AgentSkillPostInstall $staged", self.source)
        deferred_herdr = self.source.split("function Prepare-DeferredHerdrSetup", 1)[1].split(
            "function Get-SkillStateRows", 1
        )[0]
        self.assertNotIn("board_prefix", deferred_herdr)
        helper = self.source.split("function Start-UpdateHelper", 1)[1].split(
            "if (-not (Test-Path Function:\\Test-InstallerInteractive))", 1
        )[0]
        self.assertLess(helper.index("MoveFileEx($Source, $Destination"), helper.index("$SkillTargets -ne '-'"))
        self.assertIn("setup $id", helper)

    @unittest.skipUnless(os.name == "nt", "native Windows PowerShell smoke")
    def test_native_machine_selects_matching_archive_including_emulated_powershell(self):
        installer = str(INSTALLER).replace("'", "''")
        for machine, target in [
            ("0xaa64", "aarch64-pc-windows-msvc"),
            ("0x8664", "x86_64-pc-windows-msvc"),
        ]:
            with self.subTest(machine=machine):
                harness = f"""
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Add-Type -TypeDefinition @'
public static class TskNativeInstall {{
    public static ushort GetNativeMachine() {{ return {machine}; }}
}}
'@
function Invoke-TskDownload {{
    param([string] $Uri, [string] $Destination, [long] $MaxBytes)
    throw "requested $Uri"
}}
. '{installer}' -NoPathUpdate
"""
                result = subprocess.run(
                    [
                        "powershell.exe",
                        "-NoLogo",
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        harness,
                    ],
                    env=dict(os.environ, TSK_VERSION="v1.2.3"),
                    encoding="utf-8",
                    errors="replace",
                    capture_output=True,
                    timeout=30,
                )
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(f"tsk-v1.2.3-{target}.zip", result.stderr)

    @unittest.skipUnless(os.name == "nt", "native Windows PowerShell smoke")
    def test_rejects_unknown_native_machine_before_downloading(self):
        installer = str(INSTALLER).replace("'", "''")
        harness = f"""
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Add-Type -TypeDefinition @'
public static class TskNativeInstall {{
    public static ushort GetNativeMachine() {{ return 0x014c; }}
}}
'@
function Invoke-TskDownload {{ throw 'download should not run' }}
. '{installer}' -NoPathUpdate
"""
        result = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", harness],
            env=dict(os.environ, TSK_VERSION="v1.2.3"),
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("unsupported native Windows architecture", result.stderr)
        self.assertNotIn("download should not run", result.stderr)

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
            encoding="utf-8",
            errors="replace",
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
            payloads = {
                target: f"offline-windows-fixture-{target}".encode()
                for target in WINDOWS_TARGETS
            }
            write_windows_archives(assets, version, payloads)
            machine = platform.machine().lower()
            expected_target = (
                "aarch64-pc-windows-msvc"
                if machine in {"arm64", "aarch64"}
                else "x86_64-pc-windows-msvc"
            )
            path_result = root / "path-result.txt"
            bounded_result = root / "bounded-result.txt"
            refresh_log = root / "refresh.log"
            prompt_log = root / "prompts.log"
            fake_tsk = root / "fake-tsk.ps1"
            fake_tsk.write_text(
                "Add-Content -LiteralPath $env:TSK_REFRESH_LOG -Value ($args -join ' ')\n"
                "if (($args -join ' ') -eq 'setup herdr --check') { Write-Output 'bound'; exit 0 }\n"
                "if (($args -join ' ') -eq 'setup --skill-states') { Write-Output \"embedded`t1.3.0`t-`t-\"; Write-Output \"claude`toutdated`t1.2.0`tC:\\skill\"; exit 0 }\n"
                "if (($args -join ' ') -eq 'setup --detected-ids') { Write-Output 'claude pi'; exit 0 }\n"
                "exit 0\n",
                encoding="utf-8",
            )
            (root / "herdr.cmd").write_text("@exit /b 0\r\n", encoding="ascii")

            quote = lambda value: str(value).replace("'", "''")
            harness = f"""
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
function Invoke-TskDownload {{
    param([string] $Uri, [string] $Destination, [long] $MaxBytes)
    $source = Join-Path '{quote(assets)}' ([IO.Path]::GetFileName($Uri))
    if ((Get-Item -LiteralPath $source).Length -gt $MaxBytes) {{ throw 'fixture exceeds download limit' }}
    Copy-Item -LiteralPath $source -Destination $Destination
}}
. '{quote(INSTALLER)}' -NoPathUpdate

$testKeyPath = 'Software\\tsk-tests\\' + [Guid]::NewGuid().ToString('N')
$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($testKeyPath)
try {{
    $key.SetValue('Path', '%TSK_PATH_ROOT%\\existing', [Microsoft.Win32.RegistryValueKind]::ExpandString)
    Add-UserPath 'C:\\Other\\tsk' $key | Out-Null
    Add-UserPath 'C:\\Other\\tsk' $key | Out-Null
    $rawPath = [string]$key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    ($key.GetValueKind('Path').ToString() + '|' + $rawPath) | Set-Content -LiteralPath '{quote(path_result)}' -Encoding ASCII
}} finally {{
    $key.Dispose()
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($testKeyPath, $false)
}}

$script:OversizedPayload = [Text.Encoding]::ASCII.GetBytes('0123456789abcdef')
function Get-HttpsResponse {{
    param([string] $Uri, [Diagnostics.Stopwatch] $Timer, [int] $TimeoutSeconds)
    $response = [PSCustomObject]@{{
        ResponseUri = [Uri]'https://example.test/asset'
        StatusCode = [Net.HttpStatusCode]::OK
        ContentLength = [long]-1
    }}
    $response | Add-Member -MemberType ScriptMethod -Name GetResponseStream -Value {{ [IO.MemoryStream]::new($script:OversizedPayload, $false) }}
    $response | Add-Member -MemberType ScriptMethod -Name Dispose -Value {{ }}
    return $response
}}
$boundedPath = Join-Path '{quote(root)}' 'oversized.bin'
try {{
    Invoke-BoundedHttpsDownload -Uri 'https://example.test/asset' -Destination $boundedPath -MaxBytes 4
    throw 'oversized fixture was accepted'
}} catch {{
    if ($_.Exception.Message -notlike '*byte limit*') {{ throw }}
}}
if (Test-Path -LiteralPath $boundedPath) {{ throw 'partial oversized download was not removed' }}
'bounded-ok' | Set-Content -LiteralPath '{quote(bounded_result)}' -Encoding ASCII

$env:TSK_REFRESH_LOG = '{quote(refresh_log)}'
Refresh-ExistingSetup '{quote(fake_tsk)}' | Out-Null
$refreshCalls = @(Get-Content -LiteralPath '{quote(refresh_log)}')
Remove-Item -LiteralPath '{quote(refresh_log)}'
function Test-InstallerInteractive {{ return $true }}
function Read-InstallerAnswer([string] $Prompt) {{
    Add-Content -LiteralPath '{quote(prompt_log)}' -Value $Prompt
    return 'yes'
}}
Invoke-HerdrPostInstall '{quote(fake_tsk)}' $true $false | Out-Null
Invoke-AgentSkillPostInstall '{quote(fake_tsk)}' $true $false | Out-Null
$refreshCalls | Set-Content -LiteralPath '{quote(refresh_log)}' -Encoding UTF8
"""
            environment = dict(
                os.environ,
                TSK_VERSION=version,
                TSK_INSTALL_DIR=str(destination),
                TSK_PATH_ROOT=str(root / "expanded-root"),
                PATH=str(root) + os.pathsep + os.environ.get("PATH", ""),
            )
            result = subprocess.run(
                ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", harness],
                env=environment,
                encoding="utf-8",
                errors="replace",
                capture_output=True,
                timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            installed = destination / "tsk.exe"
            self.assertTrue(
                installed.exists(),
                f"installer output:\n{result.stdout}\n{result.stderr}\ncreated: {[str(path.relative_to(root)) for path in root.rglob('*')]}",
            )
            self.assertEqual(installed.read_bytes(), payloads[expected_target])
            self.assertIn("Verifying checksum... ok", result.stdout)
            self.assertEqual(
                path_result.read_text(encoding="ascii").strip(),
                "ExpandString|%TSK_PATH_ROOT%\\existing;C:\\Other\\tsk",
            )
            self.assertEqual(bounded_result.read_text(encoding="ascii").strip(), "bounded-ok")
            self.assertEqual(
                refresh_log.read_text(encoding="utf-8-sig", errors="replace").replace("\r\n", "\n").splitlines(),
                [
                    "setup herdr --check",
                    "setup herdr",
                    "setup --skill-states",
                    "setup claude",
                ],
            )
            self.assertEqual(
                prompt_log.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").splitlines(),
                [
                    "Herdr detected. Set up the Herdr plugin now? [y/N] ",
                    "Agents detected: claude, pi. Install the tsk skill for them? [y/N] ",
                ],
            )

            binary_value = os.environ.get("TSK_TEST_BINARY")
            if not binary_value:
                return
            update_binary = Path(binary_value).resolve()
            self.assertTrue(update_binary.is_file(), update_binary)
            update_payload = update_binary.read_bytes()
            write_windows_archives(
                assets,
                version,
                {target: update_payload for target in WINDOWS_TARGETS},
            )

            holder_ready = root / "holder-ready"
            holder_script = (
                "$stream=[IO.File]::Open('"
                + quote(installed)
                + "',[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read); "
                + "[IO.File]::WriteAllText('"
                + quote(holder_ready)
                + "','ready'); while ($true) { Start-Sleep -Seconds 1 }"
            )
            holder = subprocess.Popen(
                ["powershell.exe", "-NoLogo", "-NoProfile", "-Command", holder_script],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                for _ in range(100):
                    if holder_ready.exists():
                        break
                    time.sleep(0.1)
                self.assertTrue(holder_ready.exists(), "lock holder did not become ready")
                update_harness = f"""
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
function Invoke-TskDownload {{
    param([string] $Uri, [string] $Destination, [long] $MaxBytes)
    $source = Join-Path '{quote(assets)}' ([IO.Path]::GetFileName($Uri))
    if ((Get-Item -LiteralPath $source).Length -gt $MaxBytes) {{ throw 'fixture exceeds download limit' }}
    Copy-Item -LiteralPath $source -Destination $Destination
}}
function Test-InstallerInteractive {{ return $true }}
function Read-InstallerAnswer([string] $Prompt) {{ return 'yes' }}
$env:TSK_UPDATE_PID = $PID
. '{quote(INSTALLER)}' -NoPathUpdate
"""
                profile = root / "isolated-profile"
                appdata = profile / "AppData" / "Roaming"
                profile.mkdir()
                appdata.mkdir(parents=True)
                update_environment = dict(
                    environment,
                    HOME=str(profile),
                    USERPROFILE=str(profile),
                    APPDATA=str(appdata),
                    TSK_UPDATE="1",
                    TSK_CURRENT_VERSION="v1.2.2",
                    TSK_UPDATE_HELPER_RETRY_SECONDS="1",
                )
                update = subprocess.run(
                    ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", update_harness],
                    env=update_environment,
                    encoding="utf-8",
                    errors="replace",
                    capture_output=True,
                    timeout=60,
                )
                self.assertEqual(update.returncode, 0, update.stderr)
                self.assertIn("Update staged", update.stdout)
                self.assertIn("Herdr setup selected; it will finish after tsk exits.", update.stdout)
                self.assertIn("Done. Run tsk in a project directory to open the board.", update.stdout)
                self.assertNotIn("prefix+t", update.stdout)
                error_log = destination / ".tsk-update-error.log"
                for _ in range(600):
                    if (
                        error_log.exists()
                        and error_log.stat().st_size > 0
                        and not list(destination.glob(".tsk-*.exe"))
                        and not list(destination.glob(".tsk-update-*.ps1"))
                    ):
                        break
                    time.sleep(0.1)
                self.assertTrue(error_log.exists(), "detached replacement timeout was not reported")
                self.assertIn("run tsk update again", error_log.read_text(encoding="utf-8-sig"))
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
            write_windows_archives(assets, version, binary)
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
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
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
                    encoding="utf-8",
                    errors="replace",
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
                encoding="utf-8",
                errors="replace",
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
                encoding="utf-8",
                errors="replace",
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
