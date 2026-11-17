#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Fetch a pinned realistic STEP corpus into a cache directory and check hashes.

The parts are third-party GPL-3.0 files; they are fetched, never committed.
A part names its own repository and commit, else the manifest's.
Usage: fetch.py DESTINATION [--manifest MANIFEST] (default: manifest.json beside this script)
"""
import hashlib, json, os, sys, urllib.parse, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
manifest_path = os.path.join(HERE, "manifest.json")
if "--manifest" in args:
    i = args.index("--manifest")
    manifest_path = args[i + 1]
    del args[i:i + 2]
dest = os.path.join(args[0], "step")
os.makedirs(dest, exist_ok=True)
manifest = json.load(open(manifest_path))
for part in manifest["parts"]:
    path = os.path.join(dest, part["file"])
    if not os.path.exists(path):
        repository = part.get("repository") or manifest["repository"]
        commit = part.get("commit") or manifest["commit"]
        url = (f"https://raw.githubusercontent.com/{repository}/"
               f"{commit}/{urllib.parse.quote(part['path'])}")
        with urllib.request.urlopen(url, timeout=120) as response, open(path, "wb") as out:
            out.write(response.read())
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    if digest != part["sha256"]:
        sys.exit(f"hash mismatch: {part['file']}")
print(f"{len(manifest['parts'])} parts verified in {dest}")
