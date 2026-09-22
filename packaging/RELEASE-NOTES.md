<!-- Release notes body. Before publishing, replace this comment and the Breaking/Added/Changed/Fixed skeleton with the matching `## vX.Y.Z` section of CHANGELOG.md, verbatim, dropping any empty subsection. Keep the Install section. -->

### Breaking

### Added

### Changed

### Fixed

## Install

macOS/Linux: `curl -fsSL https://gettsk.sh/install.sh | sh` or `brew install smarzban/tap/tsk`.

Windows 10/11 ARM64 or x64: `irm https://www.gettsk.sh/install.ps1 | iex` in a PowerShell window (Windows PowerShell 5.1 or PowerShell 7).

Details, upgrades and uninstall: https://gettsk.sh/docs/install/

## Maintainer checklist before publishing

Replace this checklist and the skeleton above with the CHANGELOG section for this version. Verify all six archives, both installers, their checksums, and actual platform installation, including a locked Windows self-update. Confirm the release tag/version and public repository access, then publish explicitly. Before advertising Windows, confirm `releases/latest` contains both Windows ZIPs and `install.ps1`, and that Windows PowerShell 5.1 fetches the documented URL directly as plain-text HTTP 200. Only after the assets are public should the generated formula be tested and committed to the tap. Do not advertise Homebrew availability until that step passes.
