/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Trusted authoring code only; the engine reads the validated inert JSON.
import { readFile, writeFile } from "node:fs/promises";
import catalog from "../crates/contracts/data/hardware-catalog.ts";
import references from "../crates/contracts/data/printer-references.ts";
import machines from "../crates/contracts/data/machine-descriptions.ts";
const args = process.argv.slice(2);
if (args.length > 1 || (args.length && args[0] !== "--check"))
  throw new Error(
    "Usage: node --experimental-strip-types tools/generate-hardware-catalog.mjs [--check]",
  );
const output = new URL("../crates/contracts/data/hardware-catalog.json", import.meta.url);
if (
  !Array.isArray(catalog.models) ||
  !Array.isArray(catalog.setups) ||
  !Array.isArray(catalog.plates)
)
  throw new Error("Hardware author must export a catalog object directly");
const text = JSON.stringify(catalog, null, 2) + "\n";
if (Buffer.byteLength(text) > 48 * 1024) throw new Error("Hardware catalog exceeds control budget");
if (args[0] === "--check") {
  if ((await readFile(output, "utf8")) !== text)
    throw new Error("Hardware catalog drift; run pnpm contracts");
} else await writeFile(output, text);

if (
  !Array.isArray(references) ||
  references.length > 128 ||
  new Set(references.map((r) => r.id)).size !== references.length ||
  references.some((r) => !r.id || r.item.kind !== "printer")
)
  throw new Error("Invalid authored printer references");
const referenceOutput = new URL(
  "../crates/contracts/data/printer-references.json",
  import.meta.url,
);
const referenceText = JSON.stringify(references, null, 2) + "\n";
if (Buffer.byteLength(referenceText) > 48 * 1024)
  throw new Error("Printer references exceed source budget");
if (args[0] === "--check") {
  if ((await readFile(referenceOutput, "utf8")) !== referenceText)
    throw new Error("Printer reference drift; run pnpm contracts");
} else await writeFile(referenceOutput, referenceText);

if (!Array.isArray(machines.layers) || !Array.isArray(machines.setups))
  throw new Error("Machine author must export layers and setups directly");
const machineOutput = new URL("../crates/contracts/data/machine-descriptions.json", import.meta.url);
const machineText = JSON.stringify(machines, null, 2) + "\n";
if (args[0] === "--check") {
  if ((await readFile(machineOutput, "utf8")) !== machineText)
    throw new Error("Machine description drift; run pnpm contracts");
} else await writeFile(machineOutput, machineText);
