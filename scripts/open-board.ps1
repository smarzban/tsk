# Open or focus one Tasks board in the invoking workspace, across all its tabs.
# Boards in other workspaces are independent views of the same task store.
$ErrorActionPreference = 'Stop'

$herdrBin = if ($env:HERDR_BIN_PATH) { $env:HERDR_BIN_PATH } else { 'herdr' }
$workspaceId = $env:HERDR_WORKSPACE_ID
$targetPane = $env:HERDR_PANE_ID
$originTab = $env:HERDR_TAB_ID
if (-not $workspaceId -or -not $targetPane -or -not $originTab) {
    [Console]::Error.WriteLine('tsk: cannot open board without a Herdr workspace, tab and pane')
    exit 1
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$pluginBin = if ($env:TSK_BIN) { $env:TSK_BIN } else { Join-Path $scriptDir '..\target\release\tsk.exe' }
if (-not (Test-Path $pluginBin)) {
    [Console]::Error.WriteLine("tsk: board launcher binary is unavailable: $pluginBin")
    exit 1
}

# Never search globally or treat a failed lookup as an empty workspace.
$panes = & $herdrBin pane list --workspace $workspaceId 2>$null
if ($LASTEXITCODE -ne 0) {
    [Console]::Error.WriteLine("tsk: could not list panes in workspace $workspaceId")
    exit 1
}

$paneId = ($panes | & $pluginBin --find-board-pane) 2>$null
if ($paneId) {
    $tabId = ($panes | & $pluginBin --find-board-tab) 2>$null
    if (-not $tabId) {
        [Console]::Error.WriteLine('tsk: could not locate the existing board tab')
        exit 1
    }
    # Herdr 0.9.0 plugin pane focus updates server focus but does not navigate
    # attached clients. Activate the tab through the public navigation route first.
    & $herdrBin tab focus $tabId
    if ($LASTEXITCODE -ne 0) { exit 1 }
    # Focusing the existing pane preserves the board's current view and edits.
    & $herdrBin plugin pane focus $paneId
    if ($LASTEXITCODE -eq 0) { exit 0 }
}

# Split placement requires a target pane, not a workspace argument. Anchor it to
# the invoking pane so a focus change cannot redirect creation to another workspace.
# Plugin open does not navigate attached clients either. Return to the invoking
# tab before creation, including when a stale board was found in another tab.
& $herdrBin tab focus $originTab
if ($LASTEXITCODE -ne 0) { exit 1 }
# New panes still receive the host's invocation context.
& $herdrBin plugin pane open --plugin herdr-tsk --entrypoint board --placement split --target-pane $targetPane --focus
exit $LASTEXITCODE
