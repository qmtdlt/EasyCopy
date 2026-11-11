"""Shared strict checks for EasyCopy Developer ID distribution."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {"arm64": "aarch64-apple-darwin", "x64": "x86_64-apple-darwin"}


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def version():
    value = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    for filename in ("apps/desktop/package.json", "apps/desktop/package-lock.json", "apps/desktop/src-tauri/tauri.conf.json"):
        config = json.loads((ROOT / filename).read_text())
        if config["version"] != value:
            raise ValueError(f"Version mismatch: {filename}")
    if not re.fullmatch(r"\d+\.\d+\.\d+", value):
        raise ValueError("Expected stable three-part desktop version")
    if os.environ.get("GITHUB_REF_TYPE") == "tag" and os.environ.get("GITHUB_REF_NAME") != "desktop-v" + value:
        raise ValueError("Desktop tag must match source version")
    return value


def package_path(arch):
    return ROOT / "dist" / "macos" / arch / f"EasyCopy_{version()}_macos_{arch}.dmg"


def signature(path, team, runtime=False):
    run("codesign", "--verify", "--strict", "--verbose=2", path)
    details = run("codesign", "-dvvv", path, capture_output=True, text=True).stderr
    if f"TeamIdentifier={team}" not in details.splitlines():
        raise ValueError(f"Wrong signing team: {path}")
    if not re.search(r"^Authority=Developer ID Application: .+", details, re.M):
        raise ValueError(f"Missing Developer ID Application signature: {path}")
    if not re.search(r"^Timestamp=.+", details, re.M):
        raise ValueError(f"Missing secure timestamp: {path}")
    if runtime and not re.search(r"flags=.*\bruntime\b", details):
        raise ValueError(f"Missing hardened runtime: {path}")


def verify_app(app, arch, team):
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    expected = {"CFBundleIdentifier": "com.easycopy.desktop", "CFBundleShortVersionString": version(), "LSMinimumSystemVersion": "12.0"}
    for key, value in expected.items():
        if info.get(key) != value:
            raise ValueError(f"Unexpected {key}: {info.get(key)}")
    if not info.get("NSLocalNetworkUsageDescription"):
        raise ValueError("Local network usage description was lost")
    machine = "arm64" if arch == "arm64" else "x86_64"
    binary = app / "Contents/MacOS" / info["CFBundleExecutable"]
    actual = run("lipo", "-archs", binary, capture_output=True, text=True).stdout.strip()
    if actual != machine:
        raise ValueError(f"Expected {machine}, found {actual}")
    run("codesign", "--verify", "--deep", "--strict", "--verbose=2", app)
    signature(app, team, runtime=True)
    # Verify every bundled Mach-O, including any future dylibs or helpers.
    for path in app.rglob("*"):
        if path.is_file() and not path.is_symlink():
            kind = run("file", "-b", path, capture_output=True, text=True).stdout
            if "Mach-O" in kind:
                signature(path, team, runtime="executable" in kind)


def verify_dmg(dmg, arch, team):
    signature(dmg, team)
    run("hdiutil", "verify", dmg)
    run("xcrun", "stapler", "validate", dmg)
    run("spctl", "--assess", "--type", "open", "--context", "context:primary-signature", "--verbose=2", dmg)
    with tempfile.TemporaryDirectory(prefix="easycopy-dmg-check-") as temporary:
        mount = Path(temporary) / "mounted"
        mount.mkdir()
        run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, dmg)
        try:
            if {p.name for p in mount.iterdir()} - {"EasyCopy.app", "Applications", ".Trashes", ".fseventsd", ".DS_Store", ".VolumeIcon.icns"}:
                raise ValueError("Unexpected DMG payload")
            if not (mount / "Applications").is_symlink() or os.readlink(mount / "Applications") != "/Applications":
                raise ValueError("Missing Applications shortcut")
            app = mount / "EasyCopy.app"
            verify_app(app, arch, team)
            run("spctl", "--assess", "--type", "execute", "--verbose=2", app)
        finally:
            run("hdiutil", "detach", mount)


def checksum(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()
