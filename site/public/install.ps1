# Install a published tsk release on Windows 10/11 ARM64 or x64.
# Compatible with Windows PowerShell 5.1. No task data is changed.
[CmdletBinding()]
param(
    [switch] $Help,
    [switch] $NoPathUpdate
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
$MaxArchiveBytes = 100MB
$MaxChecksumBytes = 1MB
$MaxExpandedBytes = 250MB

function Show-Help {
    @'
Usage:
  irm https://www.gettsk.sh/install.ps1 | iex
  powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1 [-NoPathUpdate] [-Help]

Options:
  -NoPathUpdate     Install tsk without changing the user PATH
  -Help             Show this help without network access

Environment:
  TSK_VERSION       Install a specific public release tag such as v1.2.3 (default: latest stable release)
  TSK_INSTALL_DIR   Install directory (default: %LOCALAPPDATA%\Programs\tsk\bin)
'@ | Write-Output
}

if ($Help) {
    Show-Help
    exit 0
}

function Fail([string] $Message) {
    throw $Message
}

function New-HttpsRequest([string] $Uri, [int] $TimeoutMilliseconds) {
    $parsed = [Uri]$Uri
    if (-not $parsed.IsAbsoluteUri -or $parsed.Scheme -ne 'https') {
        Fail "refusing non-HTTPS download URL: $Uri"
    }
    $request = [Net.HttpWebRequest]::CreateHttp($parsed)
    $request.AllowAutoRedirect = $false
    $request.MaximumAutomaticRedirections = 5
    $request.Timeout = $TimeoutMilliseconds
    $request.ReadWriteTimeout = $TimeoutMilliseconds
    $request.UserAgent = 'tsk-installer'
    $request.Proxy = [Net.WebRequest]::DefaultWebProxy
    if ($null -ne $request.Proxy) {
        $request.Proxy.Credentials = [Net.CredentialCache]::DefaultNetworkCredentials
    }
    return $request
}

function Get-HttpsResponse([string] $Uri, [Diagnostics.Stopwatch] $Timer, [int] $TimeoutSeconds) {
    $current = [Uri]$Uri
    for ($redirects = 0; $redirects -le 5; $redirects++) {
        $remaining = ($TimeoutSeconds * 1000) - $Timer.ElapsedMilliseconds
        if ($remaining -le 0) { Fail "download exceeded the $TimeoutSeconds second limit" }
        $requestTimeout = [Math]::Min(30000, [int]$remaining)
        $request = New-HttpsRequest $current.AbsoluteUri $requestTimeout
        try {
            $response = [Net.HttpWebResponse]$request.GetResponse()
        } catch {
            $exception = $_.Exception
            if ($exception -is [Net.WebException] -and $null -ne $exception.Response) { $exception.Response.Dispose() }
            throw
        }
        $status = [int]$response.StatusCode
        if ($status -lt 300 -or $status -ge 400) { return $response }
        if ($redirects -eq 5) {
            $response.Dispose()
            Fail 'download exceeded the 5 redirect limit'
        }
        $location = $response.Headers['Location']
        $response.Dispose()
        if (-not $location) { Fail 'download redirect had no Location header' }
        $current = [Uri]::new($current, $location)
        if ($current.Scheme -ne 'https') { Fail 'download redirected away from HTTPS' }
    }
}

function Invoke-BoundedHttpsDownload([string] $Uri, [string] $Destination, [long] $MaxBytes) {
    $response = $null
    $inputStream = $null
    $outputStream = $null
    $complete = $false
    $timeoutSeconds = 120
    $timer = [Diagnostics.Stopwatch]::StartNew()
    try {
        $response = Get-HttpsResponse $Uri $timer $timeoutSeconds
        if ($response.ResponseUri.Scheme -ne 'https') { Fail 'download redirected away from HTTPS' }
        if ($response.StatusCode -ne [Net.HttpStatusCode]::OK) { Fail "download failed with HTTP status $([int]$response.StatusCode)" }
        if ($response.ContentLength -gt $MaxBytes) { Fail "download exceeds the $MaxBytes byte limit" }
        $inputStream = $response.GetResponseStream()
        $outputStream = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        $buffer = New-Object byte[] 65536
        [long]$written = 0
        while (($count = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            if ($timer.Elapsed.TotalSeconds -gt $timeoutSeconds) { Fail "download exceeded the $timeoutSeconds second limit" }
            $written += $count
            if ($written -gt $MaxBytes) { Fail "download exceeds the $MaxBytes byte limit" }
            $outputStream.Write($buffer, 0, $count)
        }
        $outputStream.Flush()
        $complete = $true
    } finally {
        if ($null -ne $outputStream) { $outputStream.Dispose() }
        if ($null -ne $inputStream) { $inputStream.Dispose() }
        if ($null -ne $response) { $response.Dispose() }
        if (-not $complete) { Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue }
    }
}

# Native smoke tests replace only this wrapper to supply local, already-built assets.
if (-not (Test-Path Function:\Invoke-TskDownload)) {
    function Invoke-TskDownload([string] $Uri, [string] $Destination, [long] $MaxBytes) {
        Invoke-BoundedHttpsDownload -Uri $Uri -Destination $Destination -MaxBytes $MaxBytes
    }
}

if (-not (Test-Path Function:\Get-LatestReleaseTag)) {
    function Get-LatestReleaseTag([string] $Uri) {
        $response = $null
        $timer = [Diagnostics.Stopwatch]::StartNew()
        try {
            $response = Get-HttpsResponse $Uri $timer 120
            if ($response.ResponseUri.Scheme -ne 'https') { Fail 'latest-release lookup redirected away from HTTPS' }
            if ($response.StatusCode -ne [Net.HttpStatusCode]::OK) { Fail "latest-release lookup failed with HTTP status $([int]$response.StatusCode)" }
            return $response.ResponseUri.AbsolutePath.TrimEnd('/').Split('/')[-1]
        } finally {
            if ($null -ne $response) { $response.Dispose() }
        }
    }
}

function Assert-NoReparsePath([string] $Path) {
    $current = [IO.Path]::GetFullPath($Path)
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                Fail "$current is a reparse point; choose a direct installation path"
            }
        }
        $parent = [IO.Directory]::GetParent($current)
        if ($null -eq $parent) { break }
        $current = $parent.FullName
    }
}

function Add-NativeMoveType {
    if (-not ('TskNativeInstall' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
public static class TskNativeInstall {
    [StructLayout(LayoutKind.Sequential)]
    private struct SYSTEM_INFO {
        internal ushort processorArchitecture;
        internal ushort reserved;
        internal uint pageSize;
        internal IntPtr minimumApplicationAddress;
        internal IntPtr maximumApplicationAddress;
        internal UIntPtr activeProcessorMask;
        internal uint numberOfProcessors;
        internal uint processorType;
        internal uint allocationGranularity;
        internal ushort processorLevel;
        internal ushort processorRevision;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool MoveFileEx(string existingName, string newName, int flags);

    [DllImport("kernel32.dll")]
    private static extern IntPtr GetCurrentProcess();

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsWow64Process2(IntPtr process, out ushort processMachine, out ushort nativeMachine);

    [DllImport("kernel32.dll")]
    private static extern void GetNativeSystemInfo(out SYSTEM_INFO systemInfo);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetSystemDirectory(StringBuilder buffer, uint size);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);

    public static ushort GetNativeMachine() {
        try {
            ushort processMachine;
            ushort nativeMachine;
            if (!IsWow64Process2(GetCurrentProcess(), out processMachine, out nativeMachine)) {
                throw new Win32Exception(Marshal.GetLastWin32Error());
            }
            return nativeMachine;
        } catch (EntryPointNotFoundException) {
            // ARM64 Windows first shipped with IsWow64Process2. This fallback keeps
            // earlier x86-64 Windows 10 releases usable while refusing unknown hosts.
            SYSTEM_INFO info;
            GetNativeSystemInfo(out info);
            return info.processorArchitecture == 9 ? (ushort)0x8664 : (ushort)0;
        }
    }

    public static string GetSystemPowerShell() {
        var buffer = new StringBuilder(32768);
        uint length = GetSystemDirectory(buffer, (uint)buffer.Capacity);
        if (length == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
        if (length >= (uint)buffer.Capacity) throw new PathTooLongException("Windows system directory path is too long");
        return Path.Combine(buffer.ToString(), @"WindowsPowerShell\v1.0\powershell.exe");
    }

    public static void BroadcastEnvironmentChange() {
        UIntPtr result;
        SendMessageTimeout(new IntPtr(0xffff), 0x001a, UIntPtr.Zero, "Environment", 0x0002, 5000, out result);
    }
}
'@ | Out-Null
    }
}

function Get-NativeMachine {
    Add-NativeMoveType
    return [TskNativeInstall]::GetNativeMachine()
}

function Get-SystemPowerShellPath {
    Add-NativeMoveType
    return [TskNativeInstall]::GetSystemPowerShell()
}

function Notify-EnvironmentChanged {
    Add-NativeMoveType
    [TskNativeInstall]::BroadcastEnvironmentChange()
}

function Move-Atomic([string] $Source, [string] $Destination) {
    Add-NativeMoveType
    $MOVEFILE_REPLACE_EXISTING = 0x1
    $MOVEFILE_WRITE_THROUGH = 0x8
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        if ([TskNativeInstall]::MoveFileEx($Source, $Destination, ($MOVEFILE_REPLACE_EXISTING -bor $MOVEFILE_WRITE_THROUGH))) {
            $script:LastMoveError = 0
            return $true
        }
        $script:LastMoveError = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
        if (($script:LastMoveError -ne 5 -and $script:LastMoveError -ne 32) -or $attempt -eq 19) {
            return $false
        }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Quote-Single([string] $Value) {
    return "'" + $Value.Replace("'", "''") + "'"
}

function Get-Sha256([string] $Path) {
    $stream = [IO.File]::OpenRead($Path)
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try {
        return (($algorithm.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) -join '')
    } finally {
        $algorithm.Dispose()
        $stream.Dispose()
    }
}

function Find-OtherRunningCopies([string] $Destination, [int] $UpdatePid) {
    $matches = @()
    foreach ($process in @(Get-Process -Name 'tsk' -ErrorAction SilentlyContinue)) {
        if ($process.Id -eq $UpdatePid) { continue }
        try {
            if ([String]::Equals($process.Path, $Destination, [StringComparison]::OrdinalIgnoreCase)) {
                $matches += $process.Id
            }
        } catch { }
    }
    return $matches
}

function Start-UpdateHelper([string] $Source, [string] $Destination, [int] $UpdatePid, [string] $InstallDirectory, [bool] $SetupHerdr, [string] $SkillTargets) {
    $retrySeconds = 30
    $configuredRetry = 0
    if ($env:TSK_UPDATE_HELPER_RETRY_SECONDS -and [int]::TryParse($env:TSK_UPDATE_HELPER_RETRY_SECONDS, [ref]$configuredRetry) -and $configuredRetry -gt 0) {
        $retrySeconds = $configuredRetry
    }
    $helper = Join-Path $InstallDirectory ('.tsk-update-{0}.ps1' -f [Guid]::NewGuid().ToString('N'))
    @'
param(
    [Parameter(Mandatory=$true)][int] $UpdatePid,
    [Parameter(Mandatory=$true)][string] $Source,
    [Parameter(Mandatory=$true)][string] $Destination,
    [Parameter(Mandatory=$true)][int] $RetrySeconds,
    [Parameter(Mandatory=$true)][bool] $SetupHerdr,
    [Parameter(Mandatory=$true)][string] $SkillTargets
)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class TskUpdateNative {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool MoveFileEx(string existingName, string newName, int flags);
}
"@
while (Get-Process -Id $UpdatePid -ErrorAction SilentlyContinue) {
    Start-Sleep -Milliseconds 200
}
$deadline = [DateTime]::UtcNow.AddSeconds($RetrySeconds)
do {
    if ([TskUpdateNative]::MoveFileEx($Source, $Destination, (0x1 -bor 0x8))) {
        $refreshErrors = @()
        try {
            if ($SetupHerdr) {
                $setupOutput = @(& $Destination setup herdr 2>&1)
                if ($LASTEXITCODE -ne 0) { $refreshErrors += 'run tsk setup herdr: ' + ($setupOutput -join ' ') }
            }
            if ($SkillTargets -ne '-') {
                foreach ($id in @($SkillTargets -split ',' | Where-Object { $_ })) {
                    $setupOutput = @(& $Destination setup $id 2>&1)
                    if ($LASTEXITCODE -ne 0) { $refreshErrors += 'run tsk setup ' + $id + ': ' + ($setupOutput -join ' ') }
                }
            }
        } catch {
            $refreshErrors += $_.Exception.Message
        }
        $errorLog = Join-Path (Split-Path -Parent $Destination) '.tsk-update-error.log'
        if ($refreshErrors.Count -gt 0) {
            $refreshErrors | Set-Content -LiteralPath $errorLog -Encoding UTF8
        } else {
            Remove-Item -LiteralPath $errorLog -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force -ErrorAction SilentlyContinue
        exit 0
    }
    Start-Sleep -Milliseconds 200
} while ([DateTime]::UtcNow -lt $deadline)
$errorLog = Join-Path (Split-Path -Parent $Destination) '.tsk-update-error.log'
("binary replacement timed out; close every running tsk board and run tsk update again. Staged file was " + $Source) | Set-Content -LiteralPath $errorLog -Encoding UTF8
Remove-Item -LiteralPath $Source -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force -ErrorAction SilentlyContinue
exit 1
'@ | Set-Content -LiteralPath $helper -Encoding UTF8

    $setupHerdrLiteral = if ($SetupHerdr) { '$true' } else { '$false' }
    $command = '& {0} -UpdatePid {1} -Source {2} -Destination {3} -RetrySeconds {4} -SetupHerdr {5} -SkillTargets {6}' -f (Quote-Single $helper), $UpdatePid, (Quote-Single $Source), (Quote-Single $Destination), $retrySeconds, $setupHerdrLiteral, (Quote-Single $SkillTargets)
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
    $windowsPowerShell = Get-SystemPowerShellPath
    if (-not (Test-Path -LiteralPath $windowsPowerShell -PathType Leaf)) {
        Fail 'stock Windows PowerShell is required to finish a locked update'
    }
    Start-Process -FilePath $windowsPowerShell -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encoded) -WindowStyle Hidden | Out-Null
}

if (-not (Test-Path Function:\Test-InstallerInteractive)) {
    function Test-InstallerInteractive {
        if ($env:CI) { return $false }
        try { return -not [Console]::IsInputRedirected } catch { return $false }
    }
}

if (-not (Test-Path Function:\Read-InstallerAnswer)) {
    function Read-InstallerAnswer([string] $Prompt) {
        [Console]::Error.Write($Prompt)
        $answer = [Console]::ReadLine()
        if ($null -eq $answer) { return '' }
        return $answer
    }
}

function Write-SetupFailure([string] $Command, [object[]] $Output) {
    [Console]::Error.WriteLine("")
    [Console]::Error.WriteLine("$Command failed; install succeeded.")
    foreach ($line in $Output) {
        if ($null -ne $line -and "$line".Length -gt 0) {
            [Console]::Error.WriteLine("    $line")
        }
    }
}

function Invoke-HerdrPostInstall([string] $Executable, [bool] $PostInstallSetup, [bool] $UpdateMode) {
    $script:HerdrWrap = 'board'
    if ($null -eq (Get-Command herdr -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1)) { return }
    if (-not $PostInstallSetup) {
        $script:HerdrWrap = 'board_setup'
        return
    }

    if ($UpdateMode) {
        $bound = @(& $Executable setup herdr --check 2>$null)
        if ($LASTEXITCODE -eq 0 -and ($bound -join '') -eq 'bound') {
            $setupOutput = @(& $Executable setup herdr 2>&1)
            if ($LASTEXITCODE -eq 0) {
                Write-Output ''
                Write-Output 'Herdr plugin refreshed.'
                return
            }
            Write-SetupFailure 'tsk setup herdr' $setupOutput
            $script:HerdrWrap = 'board_setup'
            return
        }
    }

    if (-not (Test-InstallerInteractive)) {
        $script:HerdrWrap = 'board_setup'
        return
    }
    [Console]::Error.WriteLine('')
    $answer = Read-InstallerAnswer 'Herdr detected. Set up the Herdr plugin now? [y/N] '
    if ($answer -notmatch '^(?i:y|yes)$') {
        $script:HerdrWrap = 'board_setup'
        return
    }

    Write-Output ''
    Write-Output 'Running tsk setup herdr...'
    Write-Output ''
    & $Executable setup herdr
    if ($LASTEXITCODE -ne 0) {
        Write-SetupFailure 'tsk setup herdr' @()
        $script:HerdrWrap = 'board_setup'
    } else {
        $script:HerdrWrap = 'board_prefix'
    }
}

function Prepare-DeferredHerdrSetup([string] $Executable) {
    $script:HerdrWrap = 'board'
    if ($null -eq (Get-Command herdr -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1)) { return $false }

    $bound = @(& $Executable setup herdr --check 2>$null)
    if ($LASTEXITCODE -eq 0 -and ($bound -join '') -eq 'bound') {
        [Console]::Out.WriteLine('')
        [Console]::Out.WriteLine('Herdr plugin refresh staged; it will finish after tsk exits.')
        return $true
    }
    if (-not (Test-InstallerInteractive)) {
        $script:HerdrWrap = 'board_setup'
        return $false
    }
    [Console]::Error.WriteLine('')
    $answer = Read-InstallerAnswer 'Herdr detected. Set up the Herdr plugin now? [y/N] '
    if ($answer -notmatch '^(?i:y|yes)$') {
        $script:HerdrWrap = 'board_setup'
        return $false
    }
    # Do not promise prefix+t until the detached helper has actually completed setup.
    $script:HerdrWrap = 'board'
    [Console]::Out.WriteLine('')
    [Console]::Out.WriteLine('Herdr setup selected; it will finish after tsk exits.')
    return $true
}

function Get-SkillStateRows([string] $Executable) {
    $rows = @(& $Executable setup --skill-states 2>$null)
    if ($LASTEXITCODE -ne 0) { return @() }
    return $rows
}

function Invoke-AgentSkillPostInstall([string] $Executable, [bool] $PostInstallSetup, [bool] $UpdateMode) {
    $script:SkillsWrap = ''
    if (-not $PostInstallSetup) { return }

    $detectedIds = @()
    if ($UpdateMode) {
        $states = @(Get-SkillStateRows $Executable)
        $embedded = $null
        $outdated = @()
        $outdatedLabels = @()
        $missing = @()
        $installed = @()
        foreach ($line in $states) {
            $fields = "$line" -split "`t"
            if ($fields.Count -lt 2) { continue }
            if ($fields[0] -eq 'embedded') {
                $embedded = $fields[1]
            } elseif ($fields[1] -eq 'outdated') {
                $outdated += $fields[0]
                $installed += $fields[0]
                $installedVersion = if ($fields.Count -ge 3 -and $fields[2] -ne '-') { 'v' + $fields[2] } else { 'unknown version' }
                $outdatedLabels += ($fields[0] + ' (' + $installedVersion + ')')
            } elseif ($fields[1] -eq 'current') {
                $installed += $fields[0]
            } elseif ($fields[1] -eq 'missing') {
                $missing += $fields[0]
            } elseif ($fields[1] -eq 'blocked-symlink' -and $fields.Count -ge 4) {
                [Console]::Error.WriteLine("tsk skill for $($fields[0]) not refreshed: $($fields[3]) is a symlink")
            }
        }

        if ($embedded -and $outdated.Count -gt 0) {
            $answer = 'y'
            if (Test-InstallerInteractive) {
                [Console]::Error.WriteLine('')
                $answer = Read-InstallerAnswer (('tsk skill installed for {0}; update to v{1}? [Y/n] ' -f ($outdatedLabels -join ', '), $embedded))
            }
            if ($answer -match '^(?i:n|no)$') {
                $script:SkillsWrap = 'nudge'
                return
            }
            $updated = @()
            foreach ($id in $outdated) {
                $setupOutput = @(& $Executable setup $id 2>&1)
                if ($LASTEXITCODE -eq 0) {
                    $updated += $id
                } else {
                    Write-SetupFailure "tsk setup $id" $setupOutput
                    $script:SkillsWrap = 'nudge'
                }
            }
            if ($updated.Count -gt 0) {
                Write-Output ''
                Write-Output ('Updated the tsk skill for {0}.' -f ($updated -join ', '))
            }
            return
        }
        if ($embedded -and $installed.Count -gt 0) { return }
        if ($embedded) {
            if ($missing.Count -eq 0) { return }
            $detectedIds = $missing
        }
    }

    if ($detectedIds.Count -eq 0) {
        $detectedOutput = @(& $Executable setup --detected-ids 2>$null)
        if ($LASTEXITCODE -ne 0) { return }
        $detectedIds = @((($detectedOutput -join ' ') -split '\s+') | Where-Object { $_ })
    }
    if ($detectedIds.Count -eq 0) { return }
    if (-not (Test-InstallerInteractive)) {
        $script:SkillsWrap = 'nudge'
        return
    }

    [Console]::Error.WriteLine('')
    $answer = Read-InstallerAnswer (('Agents detected: {0}. Install the tsk skill for them? [y/N] ' -f ($detectedIds -join ', ')))
    if ($answer -notmatch '^(?i:y|yes)$') {
        $script:SkillsWrap = 'nudge'
        return
    }
    Write-Output ''
    Write-Output 'Running tsk setup agents...'
    Write-Output ''
    & $Executable setup agents --yes
    if ($LASTEXITCODE -ne 0) {
        Write-SetupFailure 'tsk setup agents --yes' @()
        $script:SkillsWrap = 'nudge'
    }
}

function Prepare-DeferredAgentSkills([string] $Executable) {
    $script:SkillsWrap = ''
    $states = @(Get-SkillStateRows $Executable)
    $embedded = $null
    $outdated = @()
    $outdatedLabels = @()
    $missing = @()
    $installed = @()
    foreach ($line in $states) {
        $fields = "$line" -split "`t"
        if ($fields.Count -lt 2) { continue }
        if ($fields[0] -eq 'embedded') {
            $embedded = $fields[1]
        } elseif ($fields[1] -eq 'outdated') {
            $outdated += $fields[0]
            $installed += $fields[0]
            $installedVersion = if ($fields.Count -ge 3 -and $fields[2] -ne '-') { 'v' + $fields[2] } else { 'unknown version' }
            $outdatedLabels += ($fields[0] + ' (' + $installedVersion + ')')
        } elseif ($fields[1] -eq 'current') {
            $installed += $fields[0]
        } elseif ($fields[1] -eq 'missing') {
            $missing += $fields[0]
        } elseif ($fields[1] -eq 'blocked-symlink' -and $fields.Count -ge 4) {
            [Console]::Error.WriteLine("tsk skill for $($fields[0]) not refreshed: $($fields[3]) is a symlink")
        }
    }

    if ($embedded -and $outdated.Count -gt 0) {
        $answer = 'y'
        if (Test-InstallerInteractive) {
            [Console]::Error.WriteLine('')
            $answer = Read-InstallerAnswer (('tsk skill installed for {0}; update to v{1}? [Y/n] ' -f ($outdatedLabels -join ', '), $embedded))
        }
        if ($answer -match '^(?i:n|no)$') {
            $script:SkillsWrap = 'nudge'
            return '-'
        }
        [Console]::Out.WriteLine('')
        [Console]::Out.WriteLine(('Agent skill update staged for {0}; it will finish after tsk exits.' -f ($outdated -join ', ')))
        return ($outdated -join ',')
    }
    if ($embedded -and $installed.Count -gt 0) { return '-' }

    $detectedIds = @()
    if ($embedded) {
        if ($missing.Count -eq 0) { return '-' }
        $detectedIds = $missing
    } else {
        $detectedOutput = @(& $Executable setup --detected-ids 2>$null)
        if ($LASTEXITCODE -ne 0) { return '-' }
        $detectedIds = @((($detectedOutput -join ' ') -split '\s+') | Where-Object { $_ })
    }
    if ($detectedIds.Count -eq 0) { return '-' }
    if (-not (Test-InstallerInteractive)) {
        $script:SkillsWrap = 'nudge'
        return '-'
    }

    [Console]::Error.WriteLine('')
    $answer = Read-InstallerAnswer (('Agents detected: {0}. Install the tsk skill for them? [y/N] ' -f ($detectedIds -join ', ')))
    if ($answer -notmatch '^(?i:y|yes)$') {
        $script:SkillsWrap = 'nudge'
        return '-'
    }
    [Console]::Out.WriteLine('')
    [Console]::Out.WriteLine(('Agent skill setup staged for {0}; it will finish after tsk exits.' -f ($detectedIds -join ', ')))
    return ($detectedIds -join ',')
}

function Write-InstallClosing([string] $Executable, [bool] $PostInstallSetup) {
    Write-Output ''
    if ($script:HerdrWrap -eq 'board_prefix') {
        Write-Output 'Done. Run tsk in a project directory to open the board, or press prefix+t in Herdr.'
    } else {
        Write-Output 'Done. Run tsk in a project directory to open the board.'
    }

    $herdrRow = $null
    $agentRow = $null
    if ($PostInstallSetup) {
        if ($script:HerdrWrap -eq 'board_setup') { $herdrRow = '    Herdr plugin:  tsk setup herdr' }
        if ($script:SkillsWrap -eq 'nudge') { $agentRow = '    Agent skills:  tsk setup' }
    } else {
        Write-Output ''
        Write-Output 'Custom install directory: setup was not run. When you are ready:'
        if ($script:HerdrWrap -eq 'board_setup') { $herdrRow = "    Herdr plugin:  $Executable setup herdr" }
        $agentRow = "    Agent skills:  $Executable setup"
    }
    if ($herdrRow -or $agentRow) {
        Write-Output ''
        if ($herdrRow) { Write-Output $herdrRow }
        if ($agentRow) { Write-Output $agentRow }
    }
}

function Refresh-ExistingSetup([string] $Executable) {
    try {
        Invoke-HerdrPostInstall $Executable $true $true
        Invoke-AgentSkillPostInstall $Executable $true $true
    } catch {
        [Console]::Error.WriteLine('tsk setup refresh failed; the binary update succeeded: ' + $_.Exception.Message)
    }
}

function Add-UserPath([string] $InstallDirectory, [Microsoft.Win32.RegistryKey] $EnvironmentKey = $null) {
    $ownsKey = $null -eq $EnvironmentKey
    $key = $EnvironmentKey
    if ($ownsKey) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        if ($null -eq $key) {
            $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
        }
    }
    $changed = $false
    try {
        $hasPath = @($key.GetValueNames()) -contains 'Path'
        $current = if ($hasPath) {
            [string]$key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        } else {
            ''
        }
        $kind = if ($hasPath) { $key.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
        if ($kind -ne [Microsoft.Win32.RegistryValueKind]::String -and $kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
            Fail 'the user Path registry value is not a string'
        }

        $present = $false
        foreach ($entry in @($current -split ';' | Where-Object { $_ })) {
            $expandedEntry = [Environment]::ExpandEnvironmentVariables($entry)
            if ([String]::Equals($expandedEntry.TrimEnd('\'), $InstallDirectory.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)) {
                $present = $true
                break
            }
        }
        if (-not $present) {
            $separator = if (-not $current -or $current.EndsWith(';')) { '' } else { ';' }
            $updated = $current + $separator + $InstallDirectory
            $key.SetValue('Path', $updated, $kind)
            $changed = $true
            Write-Output "Added $InstallDirectory to your user PATH. Open a new terminal to use tsk."
        }
    } finally {
        if ($ownsKey) { $key.Dispose() }
    }
    if ($changed -and $ownsKey) { Notify-EnvironmentChanged }

    $processEntries = @($env:Path -split ';')
    if (-not ($processEntries | Where-Object { [String]::Equals($_.TrimEnd('\'), $InstallDirectory.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase) })) {
        $env:Path = $InstallDirectory + ';' + $env:Path
    }
}

if (-not (Test-Path Function:\Update-InstallerPath)) {
    function Update-InstallerPath([string] $InstallDirectory) {
        Add-UserPath $InstallDirectory
    }
}

function Main {
    if ($env:OS -ne 'Windows_NT' -or [Environment]::OSVersion.Version -lt [Version]'10.0' -or -not [Environment]::Is64BitOperatingSystem -or -not [Environment]::Is64BitProcess) {
        Fail 'Windows 10/11 ARM64 or x86-64 and a 64-bit PowerShell process are required'
    }
    $nativeMachine = Get-NativeMachine
    if ($nativeMachine -eq 0xaa64) {
        $target = 'aarch64-pc-windows-msvc'
    } elseif ($nativeMachine -eq 0x8664) {
        $target = 'x86_64-pc-windows-msvc'
    } else {
        Fail ('unsupported native Windows architecture: 0x{0:x4}' -f $nativeMachine)
    }
    if ($PSVersionTable.PSVersion -lt [Version]'5.1') {
        Fail 'Windows PowerShell 5.1 or newer is required'
    }

    $repo = 'https://github.com/smarzban/tsk'
    $version = $env:TSK_VERSION
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    if (-not $version) {
        try {
            $version = Get-LatestReleaseTag "$repo/releases/latest"
        } catch {
            Fail 'could not resolve latest stable release'
        }
    }
    if ($version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') {
        Fail 'TSK_VERSION must be a public release tag such as v1.2.3'
    }
    if ($env:TSK_UPDATE -and $env:TSK_CURRENT_VERSION -match '^v[0-9]+\.[0-9]+\.[0-9]+$') {
        $currentVersion = [Version]($env:TSK_CURRENT_VERSION.Substring(1))
        $releaseVersion = [Version]($version.Substring(1))
        if ($releaseVersion -lt $currentVersion) {
            Fail "latest stable release is $version, older than installed $($env:TSK_CURRENT_VERSION); nothing changed"
        }
    }

    $installDir = $env:TSK_INSTALL_DIR
    if (-not $installDir) {
        if (-not $env:LOCALAPPDATA) { Fail 'LOCALAPPDATA is required' }
        $installDir = Join-Path $env:LOCALAPPDATA 'Programs\tsk\bin'
    }
    if (-not [IO.Path]::IsPathRooted($installDir)) {
        Fail 'TSK_INSTALL_DIR must be an absolute path'
    }
    if ($installDir -match "[;`r`n]") {
        Fail 'installation directory cannot contain a semicolon or newline (PATH separators)'
    }
    $installDir = [IO.Path]::GetFullPath($installDir)
    Assert-NoReparsePath $installDir
    $destination = Join-Path $installDir 'tsk.exe'
    if (Test-Path -LiteralPath $destination) {
        $item = Get-Item -LiteralPath $destination -Force
        if ($item.PSIsContainer) { Fail 'destination is a directory' }
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { Fail 'destination is a reparse point; use its package manager or a different TSK_INSTALL_DIR' }
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ('tsk-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    $staged = $null
    $keepStaged = $false
    try {
        $archiveName = "tsk-$version-$target.zip"
        $base = "$repo/releases/download/$version"
        $archive = Join-Path $work $archiveName
        $sums = Join-Path $work 'SHA256SUMS'
        Write-Output "Downloading tsk $version for $target..."
        try {
            Invoke-TskDownload -Uri "$base/$archiveName" -Destination $archive -MaxBytes $MaxArchiveBytes
        } catch {
            Fail "could not download $archiveName"
        }
        try {
            Invoke-TskDownload -Uri "$base/SHA256SUMS" -Destination $sums -MaxBytes $MaxChecksumBytes
        } catch {
            Fail 'could not download checksums'
        }
        if ((Get-Item -LiteralPath $archive).Length -gt $MaxArchiveBytes) { Fail 'release archive exceeds the compressed size limit' }
        if ((Get-Item -LiteralPath $sums).Length -gt $MaxChecksumBytes) { Fail 'checksum file exceeds the size limit' }

        $pattern = '^([0-9a-fA-F]{64})  ' + [Regex]::Escape($archiveName) + '$'
        $matches = @(Get-Content -LiteralPath $sums | Where-Object { $_ -match $pattern })
        if ($matches.Count -ne 1) { Fail 'missing, malformed, or duplicate checksum' }
        [void]($matches[0] -match $pattern)
        $expected = $Matches[1]
        $actual = Get-Sha256 $archive
        if (-not [String]::Equals($actual, $expected, [StringComparison]::OrdinalIgnoreCase)) {
            Fail 'checksum mismatch; existing installation unchanged'
        }
        Write-Output 'Verifying checksum... ok'
        Write-Output ''
        if ($env:TSK_UPDATE -and $env:TSK_CURRENT_VERSION) {
            Write-Output "Current version $($env:TSK_CURRENT_VERSION)"
        }
        Write-Output "Installing tsk $version..."
        Write-Output ''

        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            $names = @($zip.Entries | ForEach-Object { $_.FullName })
            if ($names.Count -ne 3 -or $names[0] -ne 'tsk.exe' -or $names[1] -ne 'LICENSE' -or $names[2] -ne 'README.md' -or $zip.Entries[0].Length -eq 0) {
                Fail 'release archive has unexpected contents'
            }
            [long]$expandedBytes = 0
            foreach ($entry in $zip.Entries) {
                if ($entry.Length -gt $MaxExpandedBytes) { Fail 'release archive entry exceeds the expanded size limit' }
                $expandedBytes += $entry.Length
                if ($expandedBytes -gt $MaxExpandedBytes) { Fail 'release archive exceeds the expanded size limit' }
            }
        } finally {
            $zip.Dispose()
        }
        $expanded = Join-Path $work 'expanded'
        Expand-Archive -LiteralPath $archive -DestinationPath $expanded
        $executable = Join-Path $expanded 'tsk.exe'
        if (-not (Test-Path -LiteralPath $executable -PathType Leaf) -or (Get-Item -LiteralPath $executable).Length -eq 0) {
            Fail 'release executable is missing or empty'
        }

        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        Assert-NoReparsePath $installDir
        $staged = Join-Path $installDir ('.tsk-{0}.exe' -f [Guid]::NewGuid().ToString('N'))
        Copy-Item -LiteralPath $executable -Destination $staged
        if (-not (Move-Atomic $staged $destination)) {
            $isUpdate = -not [String]::IsNullOrEmpty($env:TSK_UPDATE)
            $updatePid = 0
            $validPid = [int]::TryParse($env:TSK_UPDATE_PID, [ref]$updatePid) -and $updatePid -gt 0
            if ($isUpdate -and $validPid -and ($script:LastMoveError -eq 5 -or $script:LastMoveError -eq 32)) {
                $otherCopies = @(Find-OtherRunningCopies -Destination $destination -UpdatePid $updatePid)
                if ($otherCopies.Count -gt 0) {
                    Fail ('close every other running tsk board and retry; active process ids: ' + ($otherCopies -join ', '))
                }
                $setupHerdr = Prepare-DeferredHerdrSetup $staged
                $skillTargets = Prepare-DeferredAgentSkills $staged
                Start-UpdateHelper -Source $staged -Destination $destination -UpdatePid $updatePid -InstallDirectory $installDir -SetupHerdr $setupHerdr -SkillTargets $skillTargets
                $keepStaged = $true
                Write-Output "Update staged. tsk $version will replace the running executable when process $updatePid exits."
                Write-Output "If deferred setup fails, details are written to $(Join-Path $installDir '.tsk-update-error.log')."
                Write-InstallClosing $destination $true
            } else {
                Fail "could not replace $destination (Windows error $script:LastMoveError); existing installation unchanged"
            }
        } else {
            $staged = $null
            Write-Output 'Installed:'
            Write-Output ''
            Write-Output "    tsk $version to $destination"
        }
        if (-not $NoPathUpdate) {
            try {
                Update-InstallerPath $installDir
            } catch {
                [Console]::Error.WriteLine('PATH setup failed; install succeeded: ' + $_.Exception.Message)
                [Console]::Error.WriteLine("Add $installDir to your Windows user PATH manually.")
            }
        }
        if (-not $keepStaged) {
            $updateMode = -not [String]::IsNullOrEmpty($env:TSK_UPDATE)
            $customInstallDirectory = -not [String]::IsNullOrEmpty($env:TSK_INSTALL_DIR)
            $postInstallSetup = -not $customInstallDirectory -or $updateMode
            Invoke-HerdrPostInstall $destination $postInstallSetup $updateMode
            Invoke-AgentSkillPostInstall $destination $postInstallSetup $updateMode
            Write-InstallClosing $destination $postInstallSetup
        }
    } finally {
        if ($staged -and -not $keepStaged) { Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue }
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

try {
    Main
} catch {
    [Console]::Error.WriteLine('tsk install: ' + $_.Exception.Message)
    exit 1
}
