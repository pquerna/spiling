/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { createHash } from "node:crypto";
import {
  lstat,
  readdir,
  mkdir,
  mkdtemp,
  readFile,
  rename,
  rm,
  writeFile,
  open,
} from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const directory = join(root, "data", "material-catalogs");
const repository = "https://github.com/prusa3d/prusa-fdm-mixer";
const raw = "https://raw.githubusercontent.com/prusa3d/prusa-fdm-mixer/";
const maximumBytes = 8 * 1024 * 1024;
const maximumRows = 32768;
const sources = [
  {
    source: "hueforge",
    file: "filament-library-hueforge.json",
    url: "https://shop.thehueforge.com/pages/affiliates",
  },
  {
    source: "openprinttag",
    file: "filament-library-openprinttag.json",
    url: "https://github.com/OpenPrintTag/openprinttag-database",
  },
];
// This is the notice owner's revision, not the unrecorded source revision used
// by Prusa's aggregate. Never infer the latter from lastSynced or this pin.
const notice = {
  repository: "https://github.com/OpenPrintTag/openprinttag-database",
  revision: "2451ae7adc115cfc40cf1892bddbb0b4a2f1b1a3",
  path: "LICENSE",
  file: "LICENSE-OpenPrintTag.txt",
};
export const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const json = (value) => Buffer.from(`${JSON.stringify(value, null, 2)}\n`);
function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}
function text(value, bound) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= bound &&
    value.trim() === value &&
    !/[\u0000-\u001f\u007f]/u.test(value)
  );
}
function keys(value, required, optional = []) {
  requireValue(value && typeof value === "object" && !Array.isArray(value), "Expected an object");
  requireValue(
    required.every((key) => Object.hasOwn(value, key)),
    "Missing catalog field",
  );
  requireValue(
    Object.keys(value).every((key) => required.includes(key) || optional.includes(key)),
    "Unknown catalog field; review upstream schema",
  );
}
export function validateCatalog(bytes, source) {
  requireValue(bytes.byteLength <= maximumBytes, "Catalog exceeds 8 MiB");
  const value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  keys(value, ["version", "source", "sourceUrl", "lastSynced", "entryCount", "entries"]);
  const spec = sources.find((item) => item.source === source);
  requireValue(
    spec && value.version === 1 && value.source === source && value.sourceUrl === spec.url,
    "Catalog schema/source differs",
  );
  requireValue(
    typeof value.lastSynced === "string" &&
      /^\d{4}-\d\d-\d\dT.*Z$/u.test(value.lastSynced) &&
      Number.isFinite(Date.parse(value.lastSynced)),
    "Invalid upstream sync timestamp",
  );
  requireValue(
    Array.isArray(value.entries) &&
      value.entries.length > 0 &&
      value.entries.length <= maximumRows &&
      value.entries.length === value.entryCount,
    "Catalog row count differs or exceeds bounds",
  );
  const seen = new Set();
  for (const row of value.entries) {
    keys(row, ["id", "brand", "material", "name", "hex", "searchText"], ["td", "finish"]);
    requireValue(
      ["id", "brand", "material", "name"].every((key) => text(row[key], 256)) &&
        text(row.searchText, 1024),
      "Invalid catalog identity/text",
    );
    requireValue(!seen.has(row.id), `Duplicate ${source} entry ID: ${row.id}`);
    seen.add(row.id);
    requireValue(
      typeof row.hex === "string" && /^#[0-9a-fA-F]{6}$/u.test(row.hex),
      `Invalid RGB for ${source}/${row.id}`,
    );
    if (Object.hasOwn(row, "finish")) requireValue(text(row.finish, 128), "Invalid finish");
    if (Object.hasOwn(row, "td"))
      requireValue(
        typeof row.td === "number" && Number.isFinite(row.td) && row.td > 0,
        `Invalid declared HueForge TD for ${source}/${row.id}`,
      );
  }
  return value;
}
function counts(entries, key) {
  const result = new Map();
  for (const row of entries) result.set(row[key], (result.get(row[key]) ?? 0) + 1);
  return Object.fromEntries([...result].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
}
function record(spec, bytes) {
  const value = validateCatalog(bytes, spec.source);
  return {
    source: spec.source,
    file: spec.file,
    upstream_path: `data/${spec.file}`,
    source_url: spec.url,
    bytes: bytes.length,
    sha256: hash(bytes),
    entry_count: value.entryCount,
    last_synced: value.lastSynced,
    td_count: value.entries.filter((row) => Object.hasOwn(row, "td")).length,
    brands: counts(value.entries, "brand"),
    materials: counts(value.entries, "material"),
  };
}
export function buildSnapshot(revision, files) {
  requireValue(/^[0-9a-f]{40}$/u.test(revision), "Use an exact 40-character upstream commit");
  for (const file of ["LICENSE-prusa-fdm-mixer.txt", notice.file]) {
    const bytes = files.get(file);
    requireValue(
      bytes &&
        bytes.length > 100 &&
        bytes.length < 16384 &&
        bytes.toString().includes("Permission is hereby granted, free of charge"),
      `Missing or changed MIT notice: ${file}`,
    );
  }
  const manifest = {
    schema: 1,
    repository,
    revision,
    interpretation:
      "Separate reference catalogs. HEX is declared sRGB; optional td is HueForge-style transmission distance in mm, not an attenuation length or measured/calibrated evidence. IDs are source-local; no cross-source identity merge or inventory/profile inference.",
    catalogs: sources.map((spec) => record(spec, files.get(spec.file))),
    notices: [
      {
        file: "LICENSE-prusa-fdm-mixer.txt",
        repository,
        revision,
        upstream_path: "LICENSE",
        sha256: hash(files.get("LICENSE-prusa-fdm-mixer.txt")),
      },
      {
        file: notice.file,
        repository: notice.repository,
        revision: notice.revision,
        upstream_path: notice.path,
        sha256: hash(files.get(notice.file)),
      },
    ],
  };
  return new Map([...files, ["manifest.json", json(manifest)]]);
}
export async function checkSnapshot(path = directory) {
  const stat = await lstat(path);
  requireValue(
    stat.isDirectory() && !stat.isSymbolicLink(),
    "Catalog snapshot must be an ordinary directory",
  );
  const names = [
    ...sources.map((item) => item.file),
    "LICENSE-prusa-fdm-mixer.txt",
    notice.file,
    "manifest.json",
  ].sort();
  requireValue(
    JSON.stringify((await readdir(path)).sort()) === JSON.stringify(names),
    "Unexpected snapshot contents; preserve and reconcile before refreshing",
  );
  const manifestInfo = await lstat(join(path, "manifest.json"));
  requireValue(
    manifestInfo.isFile() && !manifestInfo.isSymbolicLink() && manifestInfo.size <= 65536,
    "Invalid snapshot manifest or exceeds 64 KiB",
  );
  const manifestBytes = await readFile(join(path, "manifest.json"));
  const manifest = JSON.parse(manifestBytes);
  const files = new Map();
  for (const file of [
    ...sources.map((item) => item.file),
    "LICENSE-prusa-fdm-mixer.txt",
    notice.file,
  ]) {
    const info = await lstat(join(path, file));
    requireValue(
      info.isFile() && !info.isSymbolicLink() && info.size <= maximumBytes,
      `Invalid snapshot file: ${file}`,
    );
    files.set(file, await readFile(join(path, file)));
  }
  const expected = buildSnapshot(manifest.revision, files);
  requireValue(
    expected.get("manifest.json").equals(manifestBytes),
    "Snapshot hash/count/provenance manifest differs; do not hand-edit imported files",
  );
  return expected;
}
export async function collectCatalogProvenance(output, path = directory) {
  const files = await checkSnapshot(path);
  const manifest = JSON.parse(files.get("manifest.json"));
  const target = join(output, `prusa-material-catalogs-${manifest.revision}`);
  await mkdir(target, { recursive: true });
  for (const file of ["manifest.json", ...manifest.notices.map((item) => item.file)]) {
    await writeFile(join(target, file), files.get(file));
  }
  return manifest.catalogs.map((item) => ({
    name: `prusa-material-catalog-${item.source}`,
    source: manifest.repository,
    revision: manifest.revision,
    license: "MIT",
    path: `data/material-catalogs/${item.file}`,
    sha256: item.sha256,
    bytes: item.bytes,
    modifications:
      "None; exact upstream aggregate bytes. Separate declared reference data, not calibrated evidence or inventory.",
  }));
}
export function changes(previous, candidate) {
  return sources.map((spec) => {
    const before = previous ? validateCatalog(previous.get(spec.file), spec.source).entries : [];
    const after = validateCatalog(candidate.get(spec.file), spec.source).entries;
    const oldRows = new Map(before.map((row) => [row.id, row]));
    const newRows = new Map(after.map((row) => [row.id, row]));
    const newBrands = new Set(after.map((row) => row.brand));
    return {
      source: spec.source,
      added: after.filter((row) => !oldRows.has(row.id)).map((row) => row.id),
      removed: before.filter((row) => !newRows.has(row.id)).map((row) => row.id),
      changed: after
        .filter(
          (row) =>
            oldRows.has(row.id) && JSON.stringify(row) !== JSON.stringify(oldRows.get(row.id)),
        )
        .map((row) => row.id),
      missing_brands: [...new Set(before.map((row) => row.brand))].filter(
        (brand) => !newBrands.has(brand),
      ),
    };
  });
}
export function sameContent(previous, candidate) {
  if (!previous) return false;
  return (
    sources.every((spec) => {
      const a = validateCatalog(previous.get(spec.file), spec.source);
      const b = validateCatalog(candidate.get(spec.file), spec.source);
      // Preserve accepted exact bytes/revision when only upstream timestamps change.
      return (
        JSON.stringify({ ...a, lastSynced: null }) === JSON.stringify({ ...b, lastSynced: null })
      );
    }) &&
    ["LICENSE-prusa-fdm-mixer.txt", notice.file].every((file) =>
      previous.get(file).equals(candidate.get(file)),
    )
  );
}
export async function fetchBytes(url, fetcher = fetch, limit = maximumBytes) {
  const response = await fetcher(url, {
    signal: AbortSignal.timeout(30000),
    headers: { "User-Agent": "spiling-material-catalog-sync" },
  });
  requireValue(response.ok, `${url}: HTTP ${response.status}`);
  requireValue(response.body, `${url}: empty response`);
  const reader = response.body.getReader();
  const chunks = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      requireValue(length <= limit, `${url}: exceeds ${limit} bytes`);
      chunks.push(value);
    }
  } finally {
    // Do not hide the download/admission error if cancellation also fails.
    await reader.cancel().catch(() => {});
  }
  return Buffer.concat(chunks, length);
}
export async function download(revision, fetcher = fetch) {
  if (revision === undefined) {
    const commit = JSON.parse(
      await fetchBytes(
        "https://api.github.com/repos/prusa3d/prusa-fdm-mixer/commits/main",
        fetcher,
        1024 * 1024,
      ),
    );
    revision = commit.sha;
  }
  requireValue(
    typeof revision === "string" && /^[0-9a-f]{40}$/u.test(revision),
    "Upstream revision must be an exact commit",
  );
  const files = new Map();
  // Fetch everything at one commit. Nothing reaches the source tree until all
  // downloads, notices, schema and disappearance checks have passed.
  for (const spec of sources)
    files.set(spec.file, await fetchBytes(`${raw}${revision}/data/${spec.file}`, fetcher));
  files.set(
    "LICENSE-prusa-fdm-mixer.txt",
    await fetchBytes(`${raw}${revision}/LICENSE`, fetcher, 16384),
  );
  files.set(
    notice.file,
    await fetchBytes(
      `https://raw.githubusercontent.com/OpenPrintTag/openprinttag-database/${notice.revision}/${notice.path}`,
      fetcher,
      16384,
    ),
  );
  return buildSnapshot(revision, files);
}
export async function sync({
  path = directory,
  revision,
  allowRemovals = false,
  fetcher = fetch,
} = {}) {
  await mkdir(dirname(path), { recursive: true });
  const lockPath = `${path}.lock`;
  const lock = await open(lockPath, "wx");
  let stage;
  const backup = `${path}.previous`;
  try {
    requireValue(
      !(await lstat(backup).catch((error) => {
        if (error.code !== "ENOENT") throw error;
      })),
      `Retained snapshot at ${backup}; reconcile before refreshing`,
    );
    const existing = await lstat(path).catch((error) => {
      if (error.code !== "ENOENT") throw error;
    });
    const previous = existing ? await checkSnapshot(path) : undefined;
    const candidate = await download(revision, fetcher);
    const delta = changes(previous, candidate);
    if (!allowRemovals && delta.some((item) => item.removed.length || item.missing_brands.length)) {
      throw new Error(
        `Catalog entries disappeared; review before --allow-removals: ${JSON.stringify(delta.map((item) => ({ source: item.source, removed_count: item.removed.length, removed_sample: item.removed.slice(0, 32), missing_brands: item.missing_brands })))}`,
      );
    }
    if (sameContent(previous, candidate)) return { updated: false, changes: delta };
    stage = await mkdtemp(`${path}.staging-`);
    for (const [file, bytes] of candidate)
      await writeFile(join(stage, file), bytes, { flag: "wx" });
    await checkSnapshot(stage);
    if (previous) {
      await rename(path, backup);
    }
    try {
      await rename(stage, path);
      stage = undefined;
    } catch (error) {
      if (previous) await rename(backup, path);
      throw error;
    }
    if (previous) await rm(backup, { recursive: true });
    return { updated: true, changes: delta };
  } finally {
    if (stage) await rm(stage, { recursive: true, force: true });
    await lock.close();
    await rm(lockPath);
  }
}
async function main() {
  const [mode, ...args] = process.argv.slice(2);
  if (mode === "--check" && !args.length) {
    const files = await checkSnapshot();
    const manifest = JSON.parse(files.get("manifest.json"));
    console.log(
      JSON.stringify(
        {
          revision: manifest.revision,
          checked: manifest.catalogs.map(({ source, entry_count, td_count, sha256 }) => ({
            source,
            entry_count,
            td_count,
            sha256,
          })),
        },
        null,
        2,
      ),
    );
  } else if (mode === "--sync") {
    let revision;
    let allowRemovals = false;
    for (let i = 0; i < args.length; i++) {
      if (args[i] === "--revision" && revision === undefined) revision = args[++i];
      else if (args[i] === "--allow-removals" && !allowRemovals) allowRemovals = true;
      else throw new Error("Usage: materials:sync [--revision COMMIT] [--allow-removals]");
    }
    requireValue(
      !args.includes("--revision") || revision !== undefined,
      "--revision needs an exact commit",
    );
    const result = await sync({ revision, allowRemovals });
    console.log(
      JSON.stringify(
        {
          updated: result.updated,
          changes: result.changes.map((item) => ({
            source: item.source,
            added: item.added.length,
            removed: item.removed.length,
            changed: item.changed.length,
            missing_brands: item.missing_brands,
          })),
        },
        null,
        2,
      ),
    );
  } else
    throw new Error(
      "Usage: materials:check | materials:sync [--revision COMMIT] [--allow-removals]",
    );
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
