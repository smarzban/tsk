#!/bin/sh
# Install a published tsk release, never a build of main. No sudo or task-data changes.
set -eu

fail() { printf 'tsk install: %s\n' "$*" >&2; exit 1; }
if [ "$#" -eq 1 ] && [ "$1" = --help ]; then
    printf '%s\n' 'Usage: sh install.sh' 'TSK_VERSION=vX.Y.Z pins a public release tag (default: latest stable release).' 'TSK_INSTALL_DIR=/absolute/path overrides ~/.local/bin.'
    exit 0
fi
[ "$#" -eq 0 ] || fail 'unexpected arguments; use --help'
for command in curl tar uname awk grep sed mktemp chmod mv mkdir; do
    command -v "$command" >/dev/null 2>&1 || fail "missing required command: $command"
done
if command -v sha256sum >/dev/null 2>&1; then
    checksum() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
    checksum() { shasum -a 256 "$1" | awk '{print $1}'; }
else
    fail 'sha256sum or shasum is required'
fi

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-musl ;;
    Linux/x86_64) target=x86_64-unknown-linux-musl ;;
    *) fail 'supported platforms: macOS and Linux, ARM64 or x86-64' ;;
esac

repo=https://github.com/smarzban/tsk
version=${TSK_VERSION:-}
if [ -z "$version" ]; then
    latest=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o /dev/null -w '%{url_effective}' "$repo/releases/latest") || fail 'could not resolve latest stable release'
    case "$latest" in
        "$repo/releases/tag/"*) version=${latest##*/} ;;
        *) fail 'unexpected latest-release redirect' ;;
    esac
fi
printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail 'TSK_VERSION must be a public release tag such as v1.2.3'

install_dir=${TSK_INSTALL_DIR:-${HOME:?HOME is required}/.local/bin}
case "$install_dir" in /*) ;; *) fail 'TSK_INSTALL_DIR must be an absolute path' ;; esac
case "$install_dir" in
    *:*|*'
'*) fail 'installation directory cannot contain a colon or newline (PATH separators)' ;;
esac
[ ! -L "$install_dir/tsk" ] || fail 'destination is a symlink; use its package manager or a different TSK_INSTALL_DIR'
[ ! -d "$install_dir/tsk" ] || fail 'destination is a directory'
work=$(mktemp -d "${TMPDIR:-/tmp}/tsk-install.XXXXXX")
staged=
cleanup() { rm -rf "$work"; if [ -n "$staged" ]; then rm -f "$staged"; fi; }
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
archive="tsk-$version-$target.tar.gz"
base="$repo/releases/download/$version"
printf 'Downloading tsk %s for %s...\n' "$version" "$target"
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o "$work/$archive" "$base/$archive" || fail "could not download $archive"
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS" || fail 'could not download checksums'
expected=$(awk -v name="$archive" '$2 == name {print $1}' "$work/SHA256SUMS")
printf '%s\n' "$expected" | grep -Eq '^[0-9a-f]{64}$' || fail 'missing or malformed checksum'
[ "$(printf '%s\n' "$expected" | awk 'END {print NR}')" = 1 ] || fail 'duplicate checksum'
actual=$(checksum "$work/$archive")
[ "$actual" = "$expected" ] || fail 'checksum mismatch; existing installation unchanged'
printf 'Verifying checksum... ok\n\n'
# `tsk update` names the copy it is replacing; a first install has nothing to compare.
# An update never moves backwards: `releases/latest` can lag a copy built from a newer tag.
if [ -n "${TSK_UPDATE:-}" ] && [ -n "${TSK_CURRENT_VERSION:-}" ]; then
    printf 'Current version %s\n' "$TSK_CURRENT_VERSION"
    if printf '%s\n' "$TSK_CURRENT_VERSION" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$'; then
        newest=$(printf '%s\n%s\n' "${TSK_CURRENT_VERSION#v}" "${version#v}" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
        if [ "$newest" = "${TSK_CURRENT_VERSION#v}" ] && [ "${version#v}" != "${TSK_CURRENT_VERSION#v}" ]; then
            fail "latest stable release is $version, older than the installed $TSK_CURRENT_VERSION; nothing changed"
        fi
    fi
fi
printf 'Installing tsk %s...\n\n' "$version"
# Extract only the executable to stdout, never archive paths into the filesystem.
tar -xOzf "$work/$archive" tsk > "$work/tsk" || fail 'release archive does not contain tsk'
[ -s "$work/tsk" ] || fail 'release executable is empty'
mkdir -p "$install_dir"
staged=$(mktemp "$install_dir/.tsk.XXXXXX")
cat "$work/tsk" > "$staged"
chmod 755 "$staged"
mv -f "$staged" "$install_dir/tsk"
staged=
printf 'Installed:\n\n    tsk %s to %s/tsk\n' "$version" "$install_dir"

# Only append to safe regular startup files, never source/evaluate user config.
# Bash reads .bashrc for interactive shells and the first available login file.
# Zsh reads .zshrc for both login and non-login interactive shells.
append_path_to() {
    rc=$1
    case "$rc" in /*) ;; *) return 1 ;; esac
    [ ! -L "$rc" ] || return 1
    if [ -e "$rc" ]; then
        [ -f "$rc" ] && [ -r "$rc" ] && [ -w "$rc" ] || return 1
        if grep -F -x "$path_line" "$rc" >/dev/null; then return 0; fi
    fi
    mkdir -p "${rc%/*}" || return 1
    (umask 077; printf '\n# tsk PATH\n%s\n' "$path_line" >> "$rc") || return 1
    printf 'Configured PATH in %s\n' "$rc"
}
add_path_to() {
    if append_path_to "$1"; then return 0; fi
    printf 'Skipped PATH setup in %s (not a writable regular file or write failed).\n' "$1" >&2
    return 1
}
configure_path() {
    case "${SHELL##*/}" in
        zsh)
            zsh_dir=${ZDOTDIR-${HOME:-}}
            case "$zsh_dir" in /*) ;; *) return 1 ;; esac
            add_path_to "$zsh_dir/.zshrc"
            ;;
        bash)
            case "${HOME:-}" in /*) ;; *) return 1 ;; esac
            login_rc=$HOME/.profile
            for candidate in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
                if [ -e "$candidate" ] || [ -L "$candidate" ]; then login_rc=$candidate; break; fi
            done
            path_complete=1
            add_path_to "$HOME/.bashrc" || path_complete=0
            add_path_to "$login_rc" || path_complete=0
            [ "$path_complete" = 1 ]
            ;;
        *) return 1 ;;
    esac
}
case ":${PATH:-}:" in
    *":$install_dir:"*) ;;
    *)
        printf '\n'
        # Single-quote the literal path, including embedded quotes, so neither the
        # printed export nor the startup line can execute path metacharacters.
        quoted_dir="'$(printf '%s' "$install_dir" | sed "s/'/'\\\\''/g")'"
        path_export="export PATH=$quoted_dir:\"\$PATH\""
        path_line="case \":\${PATH:-}:\" in *:$quoted_dir:*) ;; *) $path_export ;; esac"
        SHELL=${SHELL:-}
        if configure_path; then
            printf 'Reopen your terminal, or run this in the current shell:\n  %s\n' "$path_export"
        else
            printf 'Could not update all shell startup files; any successful edits were kept. Please configure PATH manually for the remaining files. Automatic setup supports Bash and Zsh.\n' >&2
            printf 'For Bash, Zsh or sh, run:\n  %s\n' "$path_export"
            printf 'For other shells, add %s to PATH using your shell configuration.\n' "$install_dir"
        fi
        ;;
esac

# Offer Herdr plugin registration when the host binary is already on PATH.
# Curl|sh often has a non-TTY stdin; prefer /dev/tty so an interactive terminal can still answer.
# CI and headless installs skip the ask so they never hang on a prompt.
# Probe /dev/tty in a child shell: a failed `exec <>/dev/tty` in this shell would exit under set -e.
# herdr_wrap selects the install-completed closing lines (not mid-stream coaching):
#   board         — Herdr absent
#   board_prefix  — setup ran successfully (prefix+t is available)
#   board_setup   — declined, CI/no-TTY skip, or setup failed (nudge tsk setup herdr)
herdr_wrap=board
# skills_wrap: empty | nudge — close with an `Agent skills:  tsk setup` row when agents were
# detected but the batch ask was declined, skipped (CI/no-TTY), or agents --yes failed.
skills_wrap=
tsk_bin=$install_dir/tsk
# An overridden destination can be a shared directory. Do not execute a newly
# published path there: the user can run `tsk setup` after choosing the directory.
# `tsk update` also passes TSK_INSTALL_DIR, but only to overwrite the copy it runs from:
# that directory is already trusted, so the update path keeps post-install setup and
# refreshes what is installed instead of nudging.
update_mode=
if [ -n "${TSK_UPDATE:-}" ]; then
    update_mode=1
fi
post_install_setup=1
if [ -n "${TSK_INSTALL_DIR:-}" ] && [ -z "$update_mode" ]; then
    post_install_setup=0
fi

# Update path: a registration that already binds both plugin commands (on any key) is
# refreshed without asking, so its manifest and launchers follow the new binary. An
# unbound Herdr falls through to the first-install ask.
refresh_herdr() {
    [ "$("$tsk_bin" setup herdr --check 2>/dev/null)" = bound ] || return 1
    if setup_error=$("$tsk_bin" setup herdr </dev/null 2>&1 >/dev/null); then
        # The user's own keys stay; the closing line must not claim prefix+t.
        printf '\nHerdr plugin refreshed.\n'
        herdr_wrap=board
    else
        printf '\ntsk setup herdr failed; install succeeded.\n' >&2
        [ -z "$setup_error" ] || printf '%s\n' "$setup_error" | sed 's/^/    /' >&2
        herdr_wrap=board_setup
    fi
    return 0
}

maybe_setup_herdr() {
    command -v herdr >/dev/null 2>&1 || return 0
    [ "$post_install_setup" = 1 ] || { herdr_wrap=board_setup; return 0; }
    [ -x "$tsk_bin" ] || return 0
    if [ -n "$update_mode" ] && refresh_herdr; then
        return 0
    fi
    if [ -n "${CI:-}" ]; then
        herdr_wrap=board_setup
        return 0
    fi
    answer=
    setup_stdin=
    if [ -t 0 ]; then
        printf '\n' >&2
        printf 'Herdr detected. Set up the Herdr plugin now? [y/N] ' >&2
        read -r answer || true
    elif sh -c 'exec <>/dev/tty' 2>/dev/null; then
        printf '\n' >&2
        printf 'Herdr detected. Set up the Herdr plugin now? [y/N] ' >&2
        read -r answer </dev/tty || true
        setup_stdin=/dev/tty
    else
        herdr_wrap=board_setup
        return 0
    fi
    case $answer in
        y|Y|yes|YES)
            printf '\nRunning tsk setup herdr...\n\n'
            setup_status=0
            if [ -n "$setup_stdin" ]; then
                "$tsk_bin" setup herdr <"$setup_stdin" || setup_status=$?
            else
                "$tsk_bin" setup herdr || setup_status=$?
            fi
            if [ "$setup_status" -ne 0 ]; then
                printf '\ntsk setup herdr failed; install succeeded.\n' >&2
                herdr_wrap=board_setup
            else
                herdr_wrap=board_prefix
            fi
            ;;
        *)
            herdr_wrap=board_setup
            ;;
    esac
}

# Update path. Outdated skills are refreshed (asked on a TTY, default yes; unattended
# otherwise) and named afterwards. Missing skills get the first-install ask only when no
# skill is installed at all: an update never adds an agent the user did not opt into.
# Returns 1 when nothing about the update path applies (no agents detected).
# Returns 3 when the installed binary predates the probe (no `embedded` line): the caller
# keeps the plain nudge rather than staying silent about a skill it could not inspect.
refresh_agent_skills() {
    states=$("$tsk_bin" setup --skill-states 2>/dev/null) || states=
    embedded=$(printf '%s\n' "$states" | awk -F'\t' '$1 == "embedded" {print $2; exit}')
    [ -n "$embedded" ] || return 3
    outdated=$(printf '%s\n' "$states" | awk -F'\t' '$2 == "outdated" {printf "%s%s", sep, $1; sep=" "}')
    outdated_list=$(printf '%s\n' "$states" | awk -F'\t' '$2 == "outdated" {printf "%s%s (%s)", sep, $1, ($3 == "-" ? "unknown version" : "v" $3); sep=", "}')
    missing=$(printf '%s\n' "$states" | awk -F'\t' '$2 == "missing" {printf "%s%s", sep, $1; sep=" "}')
    installed=$(printf '%s\n' "$states" | awk -F'\t' '$2 == "current" || $2 == "outdated" {printf "%s%s", sep, $1; sep=" "}')
    printf '%s\n' "$states" | awk -F'\t' '$2 == "blocked-symlink" {printf "tsk skill for %s not refreshed: %s is a symlink\n", $1, $4}' >&2
    [ -n "$outdated$missing$installed" ] || return 1

    if [ -n "$outdated" ]; then
        answer=y
        if [ -z "${CI:-}" ]; then
            if [ -t 0 ]; then
                printf '\n' >&2
                printf 'tsk skill installed for %s; update to v%s? [Y/n] ' "$outdated_list" "$embedded" >&2
                read -r answer || true
            elif sh -c 'exec <>/dev/tty' 2>/dev/null; then
                printf '\n' >&2
                printf 'tsk skill installed for %s; update to v%s? [Y/n] ' "$outdated_list" "$embedded" >&2
                read -r answer </dev/tty || true
            fi
        fi
        case ${answer:-y} in
            n|N|no|NO)
                skills_wrap=nudge
                ;;
            *)
                updated=
                for id in $outdated; do
                    if setup_error=$("$tsk_bin" setup "$id" </dev/null 2>&1 >/dev/null); then
                        updated="$updated${updated:+ }$id"
                    else
                        printf '\ntsk setup %s failed; install succeeded.\n' "$id" >&2
                        [ -z "$setup_error" ] || printf '%s\n' "$setup_error" | sed 's/^/    /' >&2
                        skills_wrap=nudge
                    fi
                done
                if [ -n "$updated" ]; then
                    printf '\nUpdated the tsk skill for %s.\n' "$(printf '%s' "$updated" | sed 's/ /, /g')"
                fi
                ;;
        esac
        return 0
    fi
    if [ -n "$installed" ]; then
        # Every installed skill is current, and an update never adds agents. Quiet.
        return 0
    fi
    # Nothing installed anywhere: the first-install ask applies to the missing agents.
    detected_ids=$missing
    return 2
}

maybe_setup_agent_skills() {
    [ "$post_install_setup" = 1 ] || return 0
    [ -x "$tsk_bin" ] || return 0
    detected_ids=
    legacy_detection=1
    if [ -n "$update_mode" ]; then
        refresh_status=0
        refresh_agent_skills || refresh_status=$?
        case $refresh_status in
            0|1) return 0 ;;
            2) legacy_detection= ;;
            # 3: the installed release predates the probe. Detect the way a first install
            # does, so a machine without agents stays quiet and one with agents gets the ask.
        esac
    fi
    if [ -n "$legacy_detection" ]; then
        detected_ids=$("$tsk_bin" setup --detected-ids 2>/dev/null) || detected_ids=
        detected_ids=$(printf '%s' "$detected_ids" | tr -s '[:space:]' ' ' | sed 's/^ *//;s/ *$//')
    fi
    [ -n "$detected_ids" ] || return 0

    if [ -n "${CI:-}" ]; then
        skills_wrap=nudge
        return 0
    fi

    answer=
    setup_stdin=
    id_list=$(printf '%s' "$detected_ids" | sed 's/ /, /g')
    if [ -t 0 ]; then
        printf '\n' >&2
        printf 'Agents detected: %s. Install the tsk skill for them? [y/N] ' "$id_list" >&2
        read -r answer || true
    elif sh -c 'exec <>/dev/tty' 2>/dev/null; then
        printf '\n' >&2
        printf 'Agents detected: %s. Install the tsk skill for them? [y/N] ' "$id_list" >&2
        read -r answer </dev/tty || true
        setup_stdin=/dev/tty
    else
        skills_wrap=nudge
        return 0
    fi
    case $answer in
        y|Y|yes|YES)
            printf '\nRunning tsk setup agents...\n\n'
            setup_status=0
            if [ -n "$setup_stdin" ]; then
                "$tsk_bin" setup agents --yes <"$setup_stdin" || setup_status=$?
            else
                "$tsk_bin" setup agents --yes || setup_status=$?
            fi
            if [ "$setup_status" -ne 0 ]; then
                printf '\ntsk setup agents --yes failed; install succeeded.\n' >&2
                skills_wrap=nudge
            fi
            ;;
        *)
            skills_wrap=nudge
            ;;
    esac
}

maybe_setup_herdr
maybe_setup_agent_skills

printf '\nDone. Run tsk in a project directory to open the board'
if [ "$herdr_wrap" = board_prefix ]; then
    printf ', or press prefix+t in Herdr.\n'
else
    printf '.\n'
fi
herdr_row=
agent_row=
if [ "$post_install_setup" = 1 ]; then
    if [ "$herdr_wrap" = board_setup ]; then
        herdr_row='    Herdr plugin:  tsk setup herdr'
    fi
    if [ "$skills_wrap" = nudge ]; then
        agent_row='    Agent skills:  tsk setup'
    fi
else
    # Custom TSK_INSTALL_DIR: the installer never ran setup, so say why and name the
    # full binary path, which may not be on PATH. The agent row is unconditional here:
    # detection would have required executing the published binary.
    printf '\nCustom install directory: setup was not run. When you are ready:\n'
    if [ "$herdr_wrap" = board_setup ]; then
        herdr_row="    Herdr plugin:  $tsk_bin setup herdr"
    fi
    agent_row="    Agent skills:  $tsk_bin setup"
fi
if [ -n "$herdr_row" ] || [ -n "$agent_row" ]; then
    printf '\n'
    if [ -n "$herdr_row" ]; then
        printf '%s\n' "$herdr_row"
    fi
    if [ -n "$agent_row" ]; then
        printf '%s\n' "$agent_row"
    fi
fi
