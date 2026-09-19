# Install a published tsk release on Windows 10/11 x64.
# Compatible with Windows PowerShell 5.1. No task data is changed.
[CmdletBinding()]
param(
    [switch] $Help
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

function Show-Help {
    @'
Usage: powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1 [-Help]

Environment:
  TSK_VERSION       Stable release tag such as v1.2.3 (default: latest)
  TSK_INSTALL_DIR   Install directory (default: %LOCALAPPDATA%\Programs\tsk\bin)
  TSK_UPDATE        Set to 1 by `tsk update`
  TSK_UPDATE_PID    Running tsk process to wait for when an update is locked
'@ | Write-Output
}

if ($Help) {
    Show-Help
    exit 0
}

function Fail([string] $Message) {
    throw $Message
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
using System.Runtime.InteropServices;
public static class TskNativeInstall {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool MoveFileEx(string existingName, string newName, int flags);
}
'@ | Out-Null
    }
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

function Start-UpdateHelper([string] $Source, [string] $Destination, [int] $UpdatePid, [string] $InstallDirectory) {
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
    [Parameter(Mandatory=$true)][int] $RetrySeconds
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
            $bound = @(& $Destination setup herdr --check 2>$null)
            if ($LASTEXITCODE -eq 0 -and ($bound -join '') -eq 'bound') {
                $setupOutput = @(& $Destination setup herdr 2>&1)
                if ($LASTEXITCODE -ne 0) { $refreshErrors += 'run tsk setup herdr: ' + ($setupOutput -join ' ') }
            }
            $states = @(& $Destination setup --skill-states 2>$null)
            foreach ($line in $states) {
                $fields = $line -split "`t"
                if ($fields.Count -ge 2 -and $fields[1] -eq 'outdated') {
                    $setupOutput = @(& $Destination setup $fields[0] 2>&1)
                    if ($LASTEXITCODE -ne 0) { $refreshErrors += 'run tsk setup ' + $fields[0] + ': ' + ($setupOutput -join ' ') }
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

    $command = '& {0} -UpdatePid {1} -Source {2} -Destination {3} -RetrySeconds {4}' -f (Quote-Single $helper), $UpdatePid, (Quote-Single $Source), (Quote-Single $Destination), $retrySeconds
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
    Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encoded) -WindowStyle Hidden | Out-Null
}

function Refresh-ExistingSetup([string] $Executable) {
    try {
        $bound = @(& $Executable setup herdr --check 2>$null)
        if ($LASTEXITCODE -eq 0 -and ($bound -join '') -eq 'bound') {
            $setupOutput = @(& $Executable setup herdr 2>&1)
            if ($LASTEXITCODE -eq 0) {
                Write-Output 'Herdr plugin refreshed.'
            } else {
                [Console]::Error.WriteLine('Herdr refresh failed; run tsk setup herdr: ' + ($setupOutput -join ' '))
            }
        }
        $states = @(& $Executable setup --skill-states 2>$null)
        $updated = @()
        foreach ($line in $states) {
            $fields = $line -split "`t"
            if ($fields.Count -ge 2 -and $fields[1] -eq 'outdated') {
                $setupOutput = @(& $Executable setup $fields[0] 2>&1)
                if ($LASTEXITCODE -eq 0) {
                    $updated += $fields[0]
                } else {
                    [Console]::Error.WriteLine('Skill refresh failed for ' + $fields[0] + '; run tsk setup ' + $fields[0] + ': ' + ($setupOutput -join ' '))
                }
            }
        }
        if ($updated.Count -gt 0) { Write-Output ('Updated the tsk skill for ' + ($updated -join ', ') + '.') }
    } catch {
        [Console]::Error.WriteLine('tsk setup refresh failed; the binary update succeeded: ' + $_.Exception.Message)
    }
}

function Add-UserPath([string] $InstallDirectory) {
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @()
    if ($current) {
        $entries = @($current -split ';' | Where-Object { $_ })
    }
    $present = $false
    foreach ($entry in $entries) {
        if ([String]::Equals($entry.TrimEnd('\'), $InstallDirectory.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)) {
            $present = $true
            break
        }
    }
    if (-not $present) {
        $updated = if ($current) { $current.TrimEnd(';') + ';' + $InstallDirectory } else { $InstallDirectory }
        [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
        Write-Output "Added $InstallDirectory to your user PATH. Open a new terminal to use tsk."
    }
    $processEntries = @($env:Path -split ';')
    if (-not ($processEntries | Where-Object { [String]::Equals($_.TrimEnd('\'), $InstallDirectory.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase) })) {
        $env:Path = $InstallDirectory + ';' + $env:Path
    }
}

function Main {
    if ($env:OS -ne 'Windows_NT' -or [Environment]::OSVersion.Version -lt [Version]'10.0' -or -not [Environment]::Is64BitOperatingSystem -or -not [Environment]::Is64BitProcess) {
        Fail 'Windows 10/11 x86-64 and a 64-bit PowerShell process are required'
    }
    $nativeArchitecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if (-not [String]::Equals($nativeArchitecture, 'AMD64', [StringComparison]::OrdinalIgnoreCase)) {
        Fail 'Windows ARM64 is not supported; an x86-64 (AMD64) host is required'
    }
    if ($PSVersionTable.PSVersion -lt [Version]'5.1') {
        Fail 'Windows PowerShell 5.1 or newer is required'
    }

    $target = 'x86_64-pc-windows-msvc'
    $repo = 'https://github.com/smarzban/tsk'
    $version = $env:TSK_VERSION
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    if (-not $version) {
        $latest = Invoke-WebRequest -Uri "$repo/releases/latest" -UseBasicParsing
        $version = $latest.BaseResponse.ResponseUri.AbsolutePath.TrimEnd('/').Split('/')[-1]
    }
    if ($version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') {
        Fail 'TSK_VERSION must be a stable release tag such as v1.2.3'
    }
    if ($env:TSK_UPDATE -and $env:TSK_CURRENT_VERSION -match '^v[0-9]+\.[0-9]+\.[0-9]+$') {
        $currentVersion = [Version]($env:TSK_CURRENT_VERSION.Substring(1))
        $releaseVersion = [Version]($version.Substring(1))
        if ($releaseVersion -lt $currentVersion) {
            Fail "latest published release is $version, older than installed $($env:TSK_CURRENT_VERSION); nothing changed"
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
        Invoke-WebRequest -Uri "$base/$archiveName" -OutFile $archive -UseBasicParsing
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing

        $pattern = '^([0-9a-fA-F]{64})  ' + [Regex]::Escape($archiveName) + '$'
        $matches = @(Get-Content -LiteralPath $sums | Where-Object { $_ -match $pattern })
        if ($matches.Count -ne 1) { Fail 'missing, malformed, or duplicate checksum' }
        [void]($matches[0] -match $pattern)
        $expected = $Matches[1]
        $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
        if (-not [String]::Equals($actual, $expected, [StringComparison]::OrdinalIgnoreCase)) {
            Fail 'checksum mismatch; existing installation unchanged'
        }
        Write-Output 'Verifying checksum... ok'

        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            $names = @($zip.Entries | ForEach-Object { $_.FullName })
            if ($names.Count -ne 3 -or $names[0] -ne 'tsk.exe' -or $names[1] -ne 'LICENSE' -or $names[2] -ne 'README.md' -or $zip.Entries[0].Length -eq 0) {
                Fail 'release archive has unexpected contents'
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
                Start-UpdateHelper -Source $staged -Destination $destination -UpdatePid $updatePid -InstallDirectory $installDir
                $keepStaged = $true
                Write-Output "Update staged. tsk $version will replace the running executable when process $updatePid exits."
                Write-Output "If integration refresh fails, details are written to $(Join-Path $installDir '.tsk-update-error.log')."
            } else {
                Fail "could not replace $destination (Windows error $script:LastMoveError); existing installation unchanged"
            }
        } else {
            $staged = $null
            Write-Output "Installed tsk $version to $destination"
            if ($env:TSK_UPDATE) { Refresh-ExistingSetup $destination }
        }
        Add-UserPath $installDir
        if (-not $env:TSK_UPDATE) {
            Write-Output "Optional setup: $destination setup herdr"
            Write-Output "Agent skills:  $destination setup"
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
