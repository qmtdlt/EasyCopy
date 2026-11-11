#!/usr/bin/env python3
"""Verify the final DMG, checksum, mounted app, architecture and Gatekeeper."""
import argparse
import os
from macos_release import TARGETS, checksum, package_path, verify_dmg

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--arch", choices=TARGETS, required=True)
args = parser.parse_args()
dmg = package_path(args.arch)
expected = f"{checksum(dmg)}  {dmg.name}\n"
if dmg.with_suffix(".dmg.sha256").read_text() != expected:
    raise ValueError("DMG checksum mismatch")
verify_dmg(dmg, args.arch, os.environ["EASYCOPY_APPLE_TEAM_ID"])
print("PASS: signed notarized DMG, payload, architecture, checksum and Gatekeeper")
