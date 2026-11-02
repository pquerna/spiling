/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, rm, writeFile, mkdir, readdir, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  sync,
  checkSnapshot,
  fetchBytes,
  validateCatalog,
  changes,
  buildSnapshot,
  collectCatalogProvenance,
} from "./material-catalogs.mjs";

const revision = "a".repeat(40);
const notice = Buffer.from(
  `MIT License\n${"Copyright upstream\n".repeat(4)}Permission is hereby granted, free of charge, to any person obtaining a copy.\n`,
);
const encode = (value) => Buffer.from(`${JSON.stringify(value)}\n`);
function catalog(
  source,
  entries = [
    {
      id: "maker-pla-blue",
      brand: "Maker",
      material: "PLA",
      name: "Blue",
      hex: "#008ea6",
      searchText: "maker pla blue",
    },
  ],
) {
  return {
    version: 1,
    source,
    sourceUrl:
      source === "hueforge"
        ? "https://shop.thehueforge.com/pages/affiliates"
        : "https://github.com/OpenPrintTag/openprinttag-database",
    lastSynced: "2026-11-01T04:00:00.000Z",
    entryCount: entries.length,
    entries,
  };
}
function files() {
  const hf = catalog("hueforge");
  hf.entries[0].td = 1.1;
  return new Map([
    ["filament-library-hueforge.json", encode(hf)],
    ["filament-library-openprinttag.json", encode(catalog("openprinttag"))],
    ["LICENSE-prusa-fdm-mixer.txt", notice],
    ["LICENSE-OpenPrintTag.txt", notice],
  ]);
}
function fetcher(input) {
  return async (url) => {
    if (url.endsWith("commits/main")) return new Response(JSON.stringify({ sha: revision }));
    assert.match(url, new RegExp(`/(${revision}|2451ae7adc115cfc40cf1892bddbb0b4a2f1b1a3)/`));
    const file = url.endsWith("LICENSE")
      ? url.includes("OpenPrintTag")
        ? "LICENSE-OpenPrintTag.txt"
        : "LICENSE-prusa-fdm-mixer.txt"
      : url.split("/").at(-1);
    return new Response(input.get(file));
  };
}
async function temporary(t) {
  const path = await mkdtemp(join(tmpdir(), "spiling-catalog-"));
  t.after(() => rm(path, { recursive: true, force: true }));
  return join(path, "catalogs");
}
test("coherent pinned publication retains exact source bytes and separate identities", async (t) => {
  const path = await temporary(t);
  const upstream = files();
  assert.equal((await sync({ path, fetcher: fetcher(upstream) })).updated, true);
  for (const [file, bytes] of upstream) assert.deepEqual(await readFile(join(path, file)), bytes);
  const stored = await checkSnapshot(path);
  const manifest = JSON.parse(stored.get("manifest.json"));
  assert.equal(manifest.revision, revision);
  assert.deepEqual(
    manifest.catalogs.map((s) => [s.source, s.entry_count, s.td_count]),
    [
      ["hueforge", 1, 1],
      ["openprinttag", 1, 0],
    ],
  );
  // Equal IDs and colors in different sources stay separate; missing TD stays missing.
  assert.equal(
    Object.hasOwn(
      validateCatalog(stored.get("filament-library-openprinttag.json"), "openprinttag").entries[0],
      "td",
    ),
    false,
  );
});
test("timestamp-only upstream refresh preserves accepted bytes and revision", async (t) => {
  const path = await temporary(t);
  const upstream = files();
  await sync({ path, revision, fetcher: fetcher(upstream) });
  const before = await checkSnapshot(path);
  for (const file of ["filament-library-hueforge.json", "filament-library-openprinttag.json"]) {
    const value = JSON.parse(upstream.get(file));
    value.lastSynced = "2026-11-02T04:00:00.000Z";
    upstream.set(file, encode(value));
  }
  assert.equal((await sync({ path, revision, fetcher: fetcher(upstream) })).updated, false);
  assert.deepEqual(await checkSnapshot(path), before);
});
test("partial download, malformed input and disappearing rows never replace the snapshot", async (t) => {
  const path = await temporary(t);
  const upstream = files();
  const extra = { ...catalog("hueforge").entries[0], id: "other-pla-blue", brand: "Other" };
  let value = JSON.parse(upstream.get("filament-library-hueforge.json"));
  value.entries.push(extra);
  value.entryCount++;
  upstream.set("filament-library-hueforge.json", encode(value));
  await sync({ path, revision, fetcher: fetcher(upstream) });
  const before = await checkSnapshot(path);
  const bad = async (url) =>
    url.includes("openprinttag.json")
      ? new Response("offline", { status: 503 })
      : fetcher(upstream)(url);
  await assert.rejects(sync({ path, revision, fetcher: bad }), /HTTP 503/);
  assert.deepEqual(await checkSnapshot(path), before);
  upstream.set("filament-library-hueforge.json", Buffer.from("<html>bad gateway</html>"));
  await assert.rejects(sync({ path, revision, fetcher: fetcher(upstream) }));
  assert.deepEqual(await checkSnapshot(path), before);
  upstream.set("filament-library-hueforge.json", files().get("filament-library-hueforge.json"));
  await assert.rejects(sync({ path, revision, fetcher: fetcher(upstream) }), /disappeared.*Other/);
  assert.deepEqual(await checkSnapshot(path), before);
  assert.equal(
    (await sync({ path, revision, allowRemovals: true, fetcher: fetcher(upstream) })).updated,
    true,
  );
  assert.equal(
    JSON.parse((await checkSnapshot(path)).get("manifest.json")).catalogs[0].entry_count,
    1,
  );
});
test("duplicate IDs, invalid TD/RGB and upstream schema drift refuse", () => {
  const original = catalog("hueforge");
  for (const mutate of [
    (c) => {
      c.entries.push(c.entries[0]);
      c.entryCount++;
    },
    (c) => {
      c.entries[0].td = 0;
    },
    (c) => {
      c.entries[0].td = "1.1";
    },
    (c) => {
      c.entries[0].hex = "#008ea6ff";
    },
    (c) => {
      c.entries[0].diameter = 1.75;
    },
    (c) => {
      c.entryCount = 4;
    },
    (c) => {
      c.sourceUrl = "https://example.com";
    },
  ]) {
    const value = structuredClone(original);
    mutate(value);
    assert.throws(() => validateCatalog(encode(value), "hueforge"));
  }
});
test("source value changes remain changes rather than duplicate or missing inventory", () => {
  const old = buildSnapshot(revision, files());
  const newer = files();
  const value = JSON.parse(newer.get("filament-library-hueforge.json"));
  value.entries[0].td = 4.2;
  newer.set("filament-library-hueforge.json", encode(value));
  assert.deepEqual(changes(old, buildSnapshot(revision, newer))[0], {
    source: "hueforge",
    added: [],
    removed: [],
    changed: ["maker-pla-blue"],
    missing_brands: [],
  });
});
test("dirty imported bytes and unexpected files are preserved for explicit reconciliation", async (t) => {
  const path = await temporary(t);
  const upstream = files();
  await sync({ path, revision, fetcher: fetcher(upstream) });
  await writeFile(join(path, "operator-notes.txt"), "keep me");
  await assert.rejects(
    sync({ path, revision, fetcher: fetcher(upstream) }),
    /Unexpected snapshot contents/,
  );
  assert.equal(await readFile(join(path, "operator-notes.txt"), "utf8"), "keep me");
  await rm(join(path, "operator-notes.txt"));
  await writeFile(join(path, "filament-library-hueforge.json"), "{}");
  await assert.rejects(sync({ path, revision, fetcher: fetcher(upstream) }));
  assert.equal(await readFile(join(path, "filament-library-hueforge.json"), "utf8"), "{}");
});
test("concurrent writer and interrupted publication are never overwritten", async (t) => {
  const path = await temporary(t);
  await writeFile(`${path}.lock`, "another writer");
  await assert.rejects(sync({ path, revision, fetcher: fetcher(files()) }), /EEXIST/);
  assert.equal(await readFile(`${path}.lock`, "utf8"), "another writer");
  await rm(`${path}.lock`);
  await mkdir(`${path}.previous`);
  await assert.rejects(sync({ path, revision, fetcher: fetcher(files()) }), /reconcile/);
  assert.deepEqual(await readdir(`${path}.previous`), []);
});
test("network responses enforce byte admission while streaming", async () => {
  await assert.rejects(
    fetchBytes("https://example.test", async () => new Response(new Uint8Array(33)), 32),
    /exceeds 32 bytes/,
  );
});

test("snapshot manifests refuse symlinks and oversized input before parsing", async (t) => {
  const path = await temporary(t);
  await sync({ path, revision, fetcher: fetcher(files()) });
  const original = await readFile(join(path, "manifest.json"));
  await rm(join(path, "manifest.json"));
  await writeFile(`${path}.external-manifest`, original);
  await symlink(`${path}.external-manifest`, join(path, "manifest.json"));
  await assert.rejects(checkSnapshot(path), /Invalid snapshot manifest/);
  await rm(join(path, "manifest.json"));
  await writeFile(join(path, "manifest.json"), Buffer.alloc(65537));
  await assert.rejects(checkSnapshot(path), /64 KiB/);
});
test("source provenance collects verified exact notices and both distinct data components", async (t) => {
  const path = await temporary(t);
  await sync({ path, revision, fetcher: fetcher(files()) });
  const output = `${path}.notices`;
  const components = await collectCatalogProvenance(output, path);
  assert.equal(components.length, 2);
  const manifest = JSON.parse(await readFile(join(path, "manifest.json")));
  for (const [index, catalog] of manifest.catalogs.entries()) {
    assert.equal(components[index].revision, revision);
    assert.equal(components[index].sha256, catalog.sha256);
    assert.equal(components[index].bytes, catalog.bytes);
  }
  for (const file of ["manifest.json", ...manifest.notices.map((item) => item.file)]) {
    assert.deepEqual(
      await readFile(join(output, `prusa-material-catalogs-${revision}`, file)),
      await readFile(join(path, file)),
    );
  }
  await writeFile(join(path, "filament-library-hueforge.json"), "{}");
  await assert.rejects(collectCatalogProvenance(output, path));
});
