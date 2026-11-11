#!/usr/bin/env python3
"""Export the already-tested image and render the self-contained release bundle."""
import argparse
import gzip
import hashlib
import pathlib
import re
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("version")
parser.add_argument("output", type=pathlib.Path)
args = parser.parse_args()
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?|validation-[a-f0-9]+-[0-9]+-[0-9]+", args.version):
    parser.error("invalid release or validation version")
root = pathlib.Path(__file__).resolve().parents[1]
args.output.mkdir(parents=True, exist_ok=True)
if any(args.output.iterdir()):
    parser.error("output directory must be empty")
image = f"ghcr.io/qmtdlt/easycopy-relay:{args.version}"
archive = args.output / f"easycopy-relay-linux-amd64-{args.version}.tar.gz"
with archive.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as compressed:
    process = subprocess.Popen(["docker", "image", "save", image], stdout=subprocess.PIPE)
    try:
        shutil.copyfileobj(process.stdout, compressed)
    finally:
        process.stdout.close()
    if process.wait() != 0:
        raise SystemExit("docker image save failed")
for source, destination in [("compose.yaml.in", "compose.yaml"), ("部署说明.md.in", "README.md")]:
    content = (root / "deploy/release" / source).read_text()
    (args.output / destination).write_text(content.replace("@VERSION@", args.version))
shutil.copyfile(root / "deploy/compose.snap.yaml", args.output / "compose.snap.yaml")
with (args.output / "SHA256SUMS").open("w") as checksums:
    for path in sorted(args.output.iterdir()):
        if path.name != "SHA256SUMS":
            with path.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            checksums.write(f"{digest}  {path.name}\n")
print(f"Exported Linux amd64 release bundle: {args.output}")
