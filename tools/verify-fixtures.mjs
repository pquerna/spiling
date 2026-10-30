/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const manifests = process.argv.slice(2);
if (manifests.length === 0) {
  manifests.push(
    "fixtures/geometry/manifest.json",
    "fixtures/manufacturing/native/manifest.json",
    "fixtures/manufacturing/bridges/manifest.json",
    "fixtures/geometry/retained-sources/manifest.json",
    "fixtures/geometry/certificates/source-provenance.json",
  );
}
let count = 0;
for (const name of manifests) {
  const manifestPath = resolve(root, name);
  const base = dirname(manifestPath);
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  assert.ok(
    Array.isArray(manifest.fixtures) && manifest.fixtures.length > 0,
    `${name}: empty fixtures`,
  );
  const seen = new Set();
  for (const fixture of manifest.fixtures) {
    const file = fixture.path ?? fixture.file;
    assert.ok(
      typeof file === "string" && file.length > 0 && !isAbsolute(file),
      `${name}: invalid path`,
    );
    const path = resolve(base, file);
    const local = relative(base, path);
    assert.ok(
      local !== ".." && !local.startsWith("../") && !local.startsWith("..\\"),
      `${name}: escaping path`,
    );
    assert.ok(!seen.has(path), `${name}: duplicate ${file}`);
    seen.add(path);
    assert.match(fixture.sha256, /^[0-9a-f]{64}$/, `${name}: invalid SHA-256 for ${file}`);
    const bytes = await readFile(path);
    assert.equal(
      createHash("sha256").update(bytes).digest("hex"),
      fixture.sha256,
      `${name}: changed ${file}`,
    );
    if (fixture.byte_count !== undefined)
      assert.equal(bytes.length, fixture.byte_count, `${name}: size ${file}`);
    const license = await readFile(`${path}.license`, "utf8");
    assert.match(license, /^SPDX-License-Identifier: OSL-3\.0\s*$/m, `${name}: license ${file}`);
    assert.match(license, /^SPDX-FileCopyrightText: .+/m, `${name}: copyright ${file}`);
    count++;
  }
}
console.log(
  `Verified ${count} immutable fixture hashes and OSL sidecars; no geometry or manufacturing qualification.`,
);
