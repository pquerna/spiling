/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */

// Independent test observer. No Spiling plan, generated contracts, production
// parser, or slicer role comment supplies commanded geometry or extrusion.
const MAX_BYTES = 16 * 1024 * 1024;
const MAX_LINES = 400_000;
const E_SCALE = 100_000_000n;
const numeric = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)$/;
function e8(text) {
  if (!numeric.test(text)) throw new Error("invalid extrusion decimal");
  const negative = text.startsWith("-");
  const [whole, fraction = ""] = text.replace(/^[+-]/, "").split(".");
  if (fraction.length > 8) throw new Error("extrusion decimal exceeds observer precision");
  return (negative ? -1n : 1n) * (BigInt(whole || "0") * E_SCALE + BigInt(fraction.padEnd(8, "0")));
}
function finite(text) {
  if (!numeric.test(text)) throw new Error("invalid numeric field");
  const value = Number(text);
  if (!Number.isFinite(value) || Math.abs(value) > 1e9)
    throw new Error("numeric field outside observer bounds");
  return value;
}
const masks = {
  G0: "XYZEF",
  G1: "XYZEF",
  G21: "",
  G90: "",
  G91: "",
  G92: "E",
  G28: "XYZ",
  M82: "",
  M83: "",
  M84: "XYZES",
  M104: "ST",
  M109: "SRT",
  M140: "S",
  M190: "SR",
  M106: "SP",
  M107: "P",
  M149: "C",
  M200: "D",
  M220: "S",
  M221: "S",
  M400: "",
  M420: "S",
  M73: "PR",
  M201: "XYZE",
  M203: "XYZE",
  M204: "PSTR",
  M205: "XYZESTBJ",
};

export function decodeGcode(program, { filamentDiameterMm = 1.75 } = {}) {
  if (Buffer.byteLength(program) > MAX_BYTES) throw new Error("G-code exceeds observer byte limit");
  const lines = program.split(/\r?\n/);
  if (lines.length > MAX_LINES) throw new Error("G-code exceeds observer line limit");
  if (!(filamentDiameterMm > 0 && filamentDiameterMm < 10))
    throw new Error("invalid filament diameter");
  const filamentArea = Math.PI * (filamentDiameterMm / 2) ** 2;
  let xyz = [null, null, null],
    absolute = null,
    absoluteE = null,
    millimetres = false;
  let e = 0n,
    debt = 0n,
    speedScale = 1,
    flowScale = 1,
    feed = null,
    fan = 0;
  let hint = null,
    totalRetracted = 0n,
    totalRecovered = 0n,
    stationaryFilament = 0;
  const strokes = [],
    travels = [],
    controls = [],
    commands = {};
  for (let index = 0; index < lines.length; index++) {
    const raw = lines[index];
    if (raw.length > 4096) throw new Error(`line ${index + 1}: observer line limit`);
    if (raw.startsWith(";TYPE:")) hint = raw.slice(6).trim(); // diagnostic only
    const code = raw.split(";", 1)[0].trim();
    if (!code) continue;
    try {
      const header = /^(G\d+|M\d+|T\d+)(?=\s|$)/.exec(code);
      if (!header) throw new Error("unsupported command syntax");
      const op = header[1],
        mask = masks[op];
      if (mask === undefined) throw new Error(`unsupported command ${op}`);
      const fields = {},
        strings = {};
      let rest = code.slice(header[0].length).trim();
      while (rest) {
        const match = /^([A-Z])([+-]?(?:\d+(?:\.\d*)?|\.\d+))?(?=\s|$)/.exec(rest);
        if (!match) throw new Error("unsupported field syntax");
        const [, key, value] = match;
        if (!mask.includes(key) || key in fields)
          throw new Error(`unexpected/duplicate field ${key}`);
        if (value === undefined && !(op === "G28" || op === "M149"))
          throw new Error("missing field value");
        fields[key] = value === undefined ? null : finite(value);
        strings[key] = value;
        rest = rest.slice(match[0].length).trim();
      }
      commands[op] = (commands[op] ?? 0) + 1;
      const need = (key) => {
        if (!(key in fields) || fields[key] === null) throw new Error(`missing ${key}`);
        return fields[key];
      };
      if (op === "G21") millimetres = true;
      else if (op === "G90" || op === "G91") {
        absolute = op === "G90";
        // Marlin G90/G91 clears M83's relative-E override. Require an explicit
        // M82/M83 before the next extrusion, rather than guessing a dialect.
        absoluteE = null;
      } else if (op === "M82" || op === "M83") absoluteE = op === "M82";
      else if (op === "G28") {
        if (debt) throw new Error("homing with unrecovered extrusion debt");
        xyz = xyz.map((v, axis) =>
          Object.keys(fields).length === 0 || "XYZ"[axis] in fields ? null : v,
        );
      } else if (op === "G92") {
        if (!millimetres) throw new Error("coordinate reset before explicit millimetres");
        if ("E" in fields) e = e8(strings.E); // coordinate reset never cancels debt
        for (let axis = 0; axis < 3; axis++)
          if ("XYZ"[axis] in fields) xyz[axis] = fields["XYZ"[axis]];
      } else if (op === "M200" && need("D") !== 0)
        throw new Error("volumetric E is outside observer scope");
      else if (op === "M220" || op === "M221") {
        const scale = need("S") / 100;
        if (!(scale > 0 && scale <= 10)) throw new Error("invalid speed/flow override");
        if (op === "M220") speedScale = scale;
        else flowScale = scale;
      } else if (op === "M106") {
        if ("P" in fields && fields.P !== 0)
          throw new Error("multiple fans are outside observer scope");
        fan = "S" in fields ? fields.S : 255;
        if (fan < 0 || fan > 255) throw new Error("invalid fan PWM");
      } else if (op === "M107") {
        if ("P" in fields && fields.P !== 0)
          throw new Error("multiple fans are outside observer scope");
        fan = 0;
      } else if (op === "M149" && !("C" in fields)) throw new Error("non-Celsius temperature mode");
      else if (["M104", "M109", "M140", "M190"].includes(op)) {
        if ("T" in fields && fields.T !== 0)
          throw new Error("multiple tools are outside observer scope");
        if ("S" in fields === "R" in fields)
          throw new Error("temperature requires exactly one S/R field");
      } else if (op === "G0" || op === "G1") {
        if (!millimetres || absolute === null)
          throw new Error("motion before explicit units/positioning mode");
        if ("F" in fields) feed = fields.F;
        if (!(feed > 0)) throw new Error("motion before positive feed");
        const from = [...xyz];
        for (let axis = 0; axis < 3; axis++) {
          const key = "XYZ"[axis];
          if (!(key in fields)) continue;
          if (!absolute && xyz[axis] === null)
            throw new Error("relative motion from unknown position");
          xyz[axis] = absolute ? fields[key] : xyz[axis] + fields[key];
        }
        let delta = 0n;
        if ("E" in fields) {
          if (absoluteE === null) throw new Error("extrusion without explicit E mode");
          const next = e8(strings.E);
          delta = absoluteE ? next - e : next;
          e = absoluteE ? next : e + next;
        }
        const known = from.every((v) => v !== null) && xyz.every((v) => v !== null);
        const length = known ? Math.hypot(...xyz.map((v, axis) => v - from[axis])) : null;
        const moving = length === null || length > 1e-12;
        if (delta < 0n) {
          if (moving) throw new Error("moving retraction is outside observer scope");
          debt -= delta;
          totalRetracted -= delta;
        }
        let material = delta > 0n ? delta : 0n;
        if (material && debt) {
          const recovered = material < debt ? material : debt;
          if (moving) throw new Error("moving recovery is outside observer scope");
          material -= recovered;
          debt -= recovered;
          totalRecovered += recovered;
        }
        const filament = (Number(material) / Number(E_SCALE)) * flowScale;
        const record = {
          line: index + 1,
          from,
          to: [...xyz],
          feedMmMin: feed * speedScale,
          fanPwm: fan,
        };
        if (material && moving) {
          if (!known) throw new Error("deposition from unknown position");
          if (Math.abs(from[2] - xyz[2]) > 1e-8)
            throw new Error("nonplanar deposition outside observer scope");
          strokes.push({
            ...record,
            lengthMm: length,
            filamentMm: filament,
            volumeMm3: filament * filamentArea,
            roleHint: hint,
          });
        } else if (material) stationaryFilament += filament;
        else if (moving) travels.push(record);
      }
      if (op !== "G0" && op !== "G1") controls.push({ line: index + 1, op, fields });
    } catch (error) {
      throw new Error(`line ${index + 1}: ${error.message}`, { cause: error });
    }
  }
  if (debt) throw new Error("program ends with unrecovered extrusion debt");
  return {
    strokes,
    travels,
    controls,
    commands,
    stationaryFilamentMm: stationaryFilament,
    retractedFilamentMm: Number(totalRetracted) / Number(E_SCALE),
    recoveredFilamentMm: Number(totalRecovered) / Number(E_SCALE),
  };
}
