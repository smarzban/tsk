#!/usr/bin/env python3
"""Prepare release artifacts and a tap formula locally. Never publishes anything.

Requires Python 3.11+. Usage: check-version TAG | package TAG TARGET BINARY OUT |
assemble TAG OUT. Assemble requires all six archives, so partial builds cannot ship.
"""
import argparse
import hashlib
import os
from pathlib import Path
import re
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
REPO = "https://github.com/smarzban/tsk"
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-musl",
)
WINDOWS_TARGETS = (
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
)
RELEASE_TARGETS = TARGETS + WINDOWS_TARGETS


def version(tag):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ValueError("expected a stable tag such as v1.2.3")
    return tag[1:]


def read_versions(root):
    """Every place the release version is written. All must agree with the tag."""
    root = Path(root)
    with (root / "Cargo.toml").open("rb") as source:
        crate = tomllib.load(source)["package"]["version"]
    with (root / "Cargo.lock").open("rb") as source:
        locked = [p["version"] for p in tomllib.load(source).get("package", []) if p.get("name") == "tsk-tui"]
    if len(locked) != 1:
        raise ValueError("Cargo.lock must contain exactly one tsk-tui package entry")
    with (root / "herdr-plugin.toml").open("rb") as source:
        plugin = tomllib.load(source)["version"]
    site = re.search(r"^export const VERSION = '([^']+)';$", (root / "site" / "src" / "version.mjs").read_text(), re.M)
    if site is None:
        raise ValueError("site/src/version.mjs has no export const VERSION = '...' line")
    return {
        "Cargo.toml": crate,
        "Cargo.lock": locked[0],
        "herdr-plugin.toml": plugin,
        "site/src/version.mjs": site.group(1),
    }


def check_version(tag, root=ROOT):
    wanted = version(tag)
    mismatched = {path: found for path, found in read_versions(root).items() if found != wanted}
    if mismatched:
        detail = ", ".join(f"{path} has {found}" for path, found in mismatched.items())
        raise ValueError(f"tag {tag} does not match: {detail}")


def package(tag, target, binary, out):
    version(tag)
    if target not in RELEASE_TARGETS:
        raise ValueError(f"unsupported target: {target}")
    binary, out = Path(binary), Path(out)
    if not binary.is_file() or not binary.stat().st_size:
        raise ValueError("binary must be a nonempty executable file")
    if target not in WINDOWS_TARGETS and not os.access(binary, os.X_OK):
        raise ValueError("binary must be a nonempty executable file")
    out.mkdir(parents=True, exist_ok=True)
    suffix = ".zip" if target in WINDOWS_TARGETS else ".tar.gz"
    archive = out / f"tsk-{tag}-{target}{suffix}"
    if archive.exists():
        raise ValueError(f"refusing to replace {archive}")
    members = [(binary, "tsk.exe" if target in WINDOWS_TARGETS else "tsk"), (ROOT / "LICENSE", "LICENSE"), (ROOT / "README.md", "README.md")]
    if target in WINDOWS_TARGETS:
        with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) as bundle:
            for path, name in members:
                info = zipfile.ZipInfo(name)
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = (0o755 if name == "tsk.exe" else 0o644) << 16
                bundle.writestr(info, path.read_bytes())
    else:
        with tarfile.open(archive, "x:gz") as bundle:
            for path, name in members:
                info = bundle.gettarinfo(str(path), arcname=name)
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mode = 0o755 if name == "tsk" else 0o644
                with path.open("rb") as data:
                    bundle.addfile(info, data)
    return archive


def assemble(tag, out):
    version(tag)  # validates the tag shape; the formula carries the version in its URLs
    out = Path(out)
    unix_archives = [out / f"tsk-{tag}-{target}.tar.gz" for target in TARGETS]
    windows_archives = [out / f"tsk-{tag}-{target}.zip" for target in WINDOWS_TARGETS]
    archives = unix_archives + windows_archives
    if any(not path.is_file() for path in archives):
        raise ValueError("all six platform archives are required")
    outputs = [out / name for name in ("install.sh", "install.ps1", "SHA256SUMS", "tsk.rb")]
    for path in outputs:
        if os.path.lexists(path):
            raise ValueError(f"refusing to replace {path}")
    digests = {}
    for target, archive in zip(TARGETS, unix_archives):
        with tarfile.open(archive) as bundle:
            members = bundle.getmembers()
            if [m.name for m in members] != ["tsk", "LICENSE", "README.md"] or any(not m.isfile() for m in members):
                raise ValueError(f"unexpected archive contents: {archive}")
            if not members[0].size or members[0].mode != 0o755:
                raise ValueError(f"invalid executable in {archive}")
        digests[target] = hashlib.sha256(archive.read_bytes()).hexdigest()
    for target, archive in zip(WINDOWS_TARGETS, windows_archives):
        with zipfile.ZipFile(archive) as bundle:
            members = bundle.infolist()
            if [member.filename for member in members] != ["tsk.exe", "LICENSE", "README.md"] or any(member.is_dir() for member in members):
                raise ValueError(f"unexpected archive contents: {archive}")
            if not members[0].file_size:
                raise ValueError(f"invalid executable in {archive}")
        digests[target] = hashlib.sha256(archive.read_bytes()).hexdigest()
    installers = {name: (ROOT / "site/public" / name).read_bytes() for name in ("install.sh", "install.ps1")}
    sums = [f"{digests[target]}  {archive.name}\n" for target, archive in zip(RELEASE_TARGETS, archives)]
    sums.extend(f"{hashlib.sha256(content).hexdigest()}  {name}\n" for name, content in installers.items())
    lines = [
        "# Generated by herdr-tsk scripts/release.py. Commit to homebrew-tap/Formula/tsk.rb.",
        "class Tsk < Formula",
        '  desc "Terminal task board for you and your agents"',
        '  homepage "https://gettsk.sh"',
        # No `version` line: brew audit --strict flags it as redundant with the
        # version scanned from the pinned URLs (tsk-vX.Y.Z-<target>.tar.gz).
        '  license "MIT"',
    ]
    for os_name, suffix in [("macos", "apple-darwin"), ("linux", "unknown-linux-musl")]:
        lines.extend(["", f"  on_{os_name} do"])
        for cpu, arch in [("arm", "aarch64"), ("intel", "x86_64")]:
            target = f"{arch}-{suffix}"
            lines.extend([
                f"    on_{cpu} do",
                f'      url "{REPO}/releases/download/{tag}/tsk-{tag}-{target}.tar.gz"',
                f'      sha256 "{digests[target]}"',
                "    end",
            ])
        lines.append("  end")
    lines.extend([
        "", "  def install", '    bin.install "tsk"', "  end", "",
        "  def caveats", '    <<~EOS', "      Homebrew installs are noninteractive. To register the shared tsk binary and shortcuts in Herdr, run:", "        tsk setup herdr", "      To add the tsk skill to your coding agents, run:", "        tsk setup agents", "    EOS", "  end", "",
        "  test do", '    ENV["TSK_STATE_DIR"] = (testpath/"state").to_s',
        '    assert_match "usage: tsk", shell_output("#{bin}/tsk --help")',
        '    system bin/"tsk", "add", "--desk", "-t", "Homebrew smoke"',
        '    assert_match "Homebrew smoke", shell_output("#{bin}/tsk list --desk")',
        "  end", "end", "",
    ])
    for path, content in zip(outputs, [installers["install.sh"], installers["install.ps1"], "".join(sums).encode("utf-8"), "\n".join(lines).encode("utf-8")]):
        # Exclusive creation also refuses a file/link introduced after the preflight.
        with path.open("xb") as destination:
            destination.write(content)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check-version")
    check.add_argument("tag")
    pack = commands.add_parser("package")
    for name in ["tag", "target", "binary", "out"]:
        pack.add_argument(name)
    gather = commands.add_parser("assemble")
    gather.add_argument("tag")
    gather.add_argument("out")
    args = parser.parse_args()
    try:
        if args.command == "check-version":
            check_version(args.tag)
        elif args.command == "package":
            package(args.tag, args.target, args.binary, args.out)
        else:
            assemble(args.tag, args.out)
    except (ValueError, OSError, tarfile.TarError, zipfile.BadZipFile, zipfile.LargeZipFile) as error:
        parser.exit(1, f"release: {error}\n")


if __name__ == "__main__":
    main()
