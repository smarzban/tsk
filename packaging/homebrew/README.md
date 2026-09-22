# smarzban Homebrew tap

Homebrew installs checksum-pinned release binaries from `smarzban/homebrew-tap`.

Install with:

```sh
brew install smarzban/tap/tsk
```

Or add the tap once, then use the short name:

```sh
brew tap smarzban/tap
brew install tsk
```

Updates and removal:

```sh
brew update
brew upgrade tsk
brew uninstall tsk
```

The formula installs versioned, checksummed GitHub release binaries, not builds
from main. Homebrew installs are noninteractive, so they do not offer the curl
installer's Herdr setup prompt; the formula caveat tells you to run
`tsk setup herdr` to register that same binary and bundled plugin assets with
Herdr 0.9+ (including its supported `0.9.0-preview.*` Windows builds), adding prefix+t / prefix+a with conflict confirmation, and
`tsk setup agents` to install the tsk skill for your coding agents.
Homebrew installation itself does not relink plugins or remove task data.

## Maintenance

Copy this README into the tap repository and add the generated `tsk.rb` from the
reviewed release bundle under `Formula/`. Update the formula for
every release, after its assets are public. Keep the old formula until then.

On each supported platform, tap the checkout and run:

```sh
brew tap smarzban/tap /absolute/path/to/homebrew-tap
brew install smarzban/tap/tsk
brew test smarzban/tap/tsk
brew audit --strict smarzban/tap/tsk
```

Run these in a clean test environment, not over someone's existing installation.
The formula test captures a task in Homebrew's temporary test directory, not `~/.tsk`.
Include these checks in the tap's own CI before publishing formula updates.
