/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
export function referenceValues(ini, values) {
  for (const [key, value] of Object.entries(values)) {
    if (!/^[a-z_]+$/.test(key) || /[\r\n]/.test(String(value)))
      throw Error("invalid reference setting");
    const pattern = new RegExp(`^${key} = .*`, "gm");
    if ([...ini.matchAll(pattern)].length !== 1)
      throw Error(`reference setting ${key} is absent or ambiguous`);
    ini = ini.replace(pattern, `${key} = ${value}`);
  }
  return ini;
}

// Pinned reference releases preserve these explicitly supplied values verbatim.
// Comparing all supplied fields prevents a successful slice from concealing an
// ignored setting or a later override in the complete resolved configuration.
export function verifyReferenceSettings(requested, resolved) {
  const parse = (ini) => {
    const values = new Map();
    for (const line of ini.split(/\r?\n/)) {
      if (!line.trim() || line.startsWith("#")) continue;
      const match = /^([a-z0-9_]+) = (.*)$/.exec(line);
      if (!match || values.has(match[1])) throw Error("malformed or duplicate reference settings");
      values.set(match[1], match[2]);
    }
    return values;
  };
  const expected = parse(requested),
    actual = parse(resolved);
  if (expected.size === 0) throw Error("missing requested reference settings");
  for (const [key, value] of expected)
    if (actual.get(key) !== value) throw Error(`reference setting ${key} was not retained`);
  return { matched: true, requestedFieldCount: expected.size, resolvedFieldCount: actual.size };
}
