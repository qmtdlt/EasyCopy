#!/usr/bin/env python3
"""Import only the CI certificate into an ephemeral keychain, or clean it up."""
import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--cleanup", action="store_true")
args = parser.parse_args()
temporary = Path(os.environ["RUNNER_TEMP"])
keychain = temporary / "easycopy-signing.keychain-db"
original = temporary / "easycopy-original-keychains.json"
p12 = temporary / "easycopy-signing.p12"


def run(*command):
    # Never echo commands: passwords are passed as subprocess arguments.
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(f"Signing setup command failed: {command[0]} {command[1]}")
    return result.stdout


if args.cleanup:
    try:
        if original.exists():
            run("security", "list-keychains", "-d", "user", "-s", *json.loads(original.read_text()))
        if keychain.exists():
            run("security", "delete-keychain", str(keychain))
    finally:
        p12.unlink(missing_ok=True)
        original.unlink(missing_ok=True)
else:
    required = ["EASYCOPY_APPLE_CERTIFICATES_P12", "EASYCOPY_APPLE_CERTIFICATES_PASSWORD", "EASYCOPY_APPLE_ID", "EASYCOPY_APPLE_APP_PASSWORD", "EASYCOPY_APPLE_TEAM_ID", "EASYCOPY_CODESIGN_IDENTITY"]
    missing = [name for name in required if not os.environ.get(name)]
    if missing:
        raise RuntimeError("Missing required signing credentials: " + ", ".join(missing))
    import shlex
    original.write_text(json.dumps(shlex.split(run("security", "list-keychains", "-d", "user").decode())))
    password = secrets.token_urlsafe(40)
    p12.write_bytes(base64.b64decode(os.environ["EASYCOPY_APPLE_CERTIFICATES_P12"], validate=True))
    p12.chmod(0o600)
    try:
        run("security", "create-keychain", "-p", password, str(keychain))
        run("security", "set-keychain-settings", "-lut", "21600", str(keychain))
        run("security", "unlock-keychain", "-p", password, str(keychain))
        run("security", "import", str(p12), "-k", str(keychain), "-P", os.environ["EASYCOPY_APPLE_CERTIFICATES_PASSWORD"], "-T", "/usr/bin/codesign")
        run("security", "set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, str(keychain))
        run("security", "list-keychains", "-d", "user", "-s", str(keychain), *json.loads(original.read_text()))
        identities = run("security", "find-identity", "-v", "-p", "codesigning", str(keychain)).decode()
        if os.environ["EASYCOPY_CODESIGN_IDENTITY"] not in identities:
            raise RuntimeError("Configured signing identity is not valid in imported keychain")
        run("xcrun", "notarytool", "store-credentials", "easycopy-notary", "--keychain", str(keychain), "--apple-id", os.environ["EASYCOPY_APPLE_ID"], "--team-id", os.environ["EASYCOPY_APPLE_TEAM_ID"], "--password", os.environ["EASYCOPY_APPLE_APP_PASSWORD"])
    finally:
        p12.unlink(missing_ok=True)
