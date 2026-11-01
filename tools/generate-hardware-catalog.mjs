/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Trusted authoring code only; the engine reads the validated inert JSON.
import { readFile, writeFile } from "node:fs/promises";
import catalog from "../crates/contracts/data/hardware-catalog.ts";
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
