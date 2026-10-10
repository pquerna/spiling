/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, open, readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { MAX_PROFILE_JSON_BYTES } from "../packages/protocol/src/generated.ts";

// Trusted authoring code only. Type checking is not a sandbox; the native engine
// never imports this module or executes a supplied TypeScript specification.
async function main() {
  const [sourceArgument, outputArgument, ...extra] = process.argv.slice(2);
  if (!sourceArgument || !outputArgument || extra.length)
    throw new Error("Usage: pnpm manufacturing:intent TRUSTED_SOURCE.ts NEW_INTENT.json");
  const source = resolve(sourceArgument);
  const output = resolve(outputArgument);
  if (!/\.(?:ts|mts)$/.test(source))
    throw new Error("Expected a TypeScript source module (.ts or .mts)");
  const sourceStat = await lstat(source);
  if (!sourceStat.isFile() || sourceStat.size > 1_048_576)
    throw new Error("Trusted source must be a regular file of at most 1 MiB");

  const require = createRequire(new URL("../packages/protocol/package.json", import.meta.url));
  const compilerPackage = require.resolve("typescript/package.json");
  const compiler = resolve(
    dirname(compilerPackage),
    JSON.parse(await readFile(compilerPackage, "utf8")).bin.tsc,
  );
  await new Promise((accept, reject) => {
    const child = spawn(
      process.execPath,
      [
        compiler,
        "--noEmit",
        "--strict",
        "--module",
        "nodenext",
        "--moduleResolution",
        "nodenext",
        "--target",
        "es2024",
        "--allowImportingTsExtensions",
        "--erasableSyntaxOnly",
        "--skipLibCheck",
        source,
      ],
      { stdio: "inherit", windowsHide: true },
    );
    child.once("error", reject);
    child.once("exit", (code, signal) =>
      code === 0
        ? accept()
        : reject(new Error(`TypeScript authoring check failed (${code ?? signal})`)),
    );
  });

  const intent = (await import(pathToFileURL(source).href)).default;
  if (!intent || typeof intent !== "object" || !intent.printer || !intent.recipe)
    throw new Error(
      "Source must default-export a ManufacturingIntent; use generated Rust-defined types",
    );
  const text =
    JSON.stringify(
      intent,
      (_key, value) => {
        if (
          (typeof value === "number" && !Number.isFinite(value)) ||
          ["undefined", "function", "symbol", "bigint"].includes(typeof value)
        )
          throw new Error("Intent must contain only finite, explicit JSON data");
        return value;
      },
      2,
    ) + "\n";
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length > MAX_PROFILE_JSON_BYTES)
    throw new Error("Compiled intent exceeds the native profile JSON limit");
  const file = await open(output, "wx");
  try {
    await file.writeFile(bytes);
    await file.sync();
  } finally {
    await file.close();
  }
  // Rust performs semantic/capability validation when --intent is submitted.
  // This is not a physical printer profile certification or a project save.
  console.log(
    JSON.stringify({
      intent_json: output,
      bytes: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      software_only: true,
      machine_ready: false,
    }),
  );
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
