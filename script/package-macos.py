#!/usr/bin/env python3
"""Build, sign, notarize and verify a DMG. Never falls back to unsigned output."""
import argparse
import json
import os
from pathlib import Path
import platform
import tempfile

from macos_release import ROOT, TARGETS, checksum, package_path, run, verify_app, verify_dmg, version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=TARGETS, required=True)
    parser.add_argument("--notary-profile", required=True)
    parser.add_argument("--keychain", type=Path)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("macOS is required")
    team = os.environ.get("EASYCOPY_APPLE_TEAM_ID", "")
    identity = os.environ.get("EASYCOPY_CODESIGN_IDENTITY", "")
    if not team or not identity or identity == "-":
        parser.error("EASYCOPY_APPLE_TEAM_ID and EASYCOPY_CODESIGN_IDENTITY are required")
    version()
    target = TARGETS[args.arch]
    env = os.environ.copy()
    # Tauri signs nested code inside-out; notarization is explicitly handled below.
    for key in ("APPLE_ID", "APPLE_PASSWORD", "APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_PATH"):
        env.pop(key, None)
    env["APPLE_SIGNING_IDENTITY"] = identity
    env["MACOSX_DEPLOYMENT_TARGET"] = "12.0"
    run("npm", "run", "tauri", "--", "build", "--target", target, "--bundles", "app", "--config", "src-tauri/tauri.macos.conf.json", "--", "--locked", cwd=ROOT / "apps/desktop", env=env)
    app = ROOT / "target" / target / "release/bundle/macos/EasyCopy.app"
    verify_app(app, args.arch, team)
    destination = package_path(args.arch)
    destination.parent.mkdir(parents=True, exist_ok=True)
    # A failed retry must not leave an older distributable at the requested path.
    destination.unlink(missing_ok=True)
    destination.with_suffix(".dmg.sha256").unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix="easycopy-package-") as temporary:
        work = Path(temporary)
        payload = work / "payload"
        payload.mkdir()
        run("ditto", app, payload / "EasyCopy.app")
        (payload / "Applications").symlink_to("/Applications")
        dmg = work / destination.name
        run("hdiutil", "create", "-volname", "EasyCopy", "-srcfolder", payload, "-format", "UDZO", "-fs", "HFS+", dmg)
        run("codesign", "--force", "--timestamp", "--sign", identity, dmg)
        credentials = ["--keychain-profile", args.notary_profile]
        if args.keychain:
            credentials += ["--keychain", str(args.keychain)]
        result = run("xcrun", "notarytool", "submit", dmg, *credentials, "--wait", "--timeout", "30m", "--output-format", "json", capture_output=True, text=True)
        submission = json.loads(result.stdout)
        print("Notarization:", submission.get("id"), submission.get("status"), flush=True)
        if submission.get("status") != "Accepted":
            raise RuntimeError("Apple did not accept this build; no distributable produced")
        run("xcrun", "stapler", "staple", dmg)
        verify_dmg(dmg, args.arch, team)
        run("ditto", dmg, destination)
    destination.with_suffix(".dmg.sha256").write_text(f"{checksum(destination)}  {destination.name}\n")
    print(f"Verified signed and notarized DMG: {destination}")


if __name__ == "__main__":
    main()
