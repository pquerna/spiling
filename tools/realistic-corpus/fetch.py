#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Fetch the pinned realistic STEP corpus into a cache directory and check hashes.

The parts are third-party GPL-3.0 files; they are fetched, never committed.
Usage: fetch.py DESTINATION
"""
import hashlib, json, os, sys, urllib.parse, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
dest = os.path.join(sys.argv[1], "step")
os.makedirs(dest, exist_ok=True)
manifest = json.load(open(os.path.join(HERE, "manifest.json")))
for part in manifest["parts"]:
    path = os.path.join(dest, part["file"])
    if not os.path.exists(path):
        url = (f"https://raw.githubusercontent.com/{manifest['repository']}/"
               f"{manifest['commit']}/{urllib.parse.quote(part['path'])}")
        with urllib.request.urlopen(url, timeout=120) as response, open(path, "wb") as out:
            out.write(response.read())
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    if digest != part["sha256"]:
        sys.exit(f"hash mismatch: {part['file']}")
print(f"{len(manifest['parts'])} parts verified in {dest}")
