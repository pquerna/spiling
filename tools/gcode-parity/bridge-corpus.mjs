/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Original analytic STEP author. Frozen bytes are read by the runtime tests;
// this author is provenance, never a substitute geometry service.
import { writeFile, mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { sha256 } from "./corpus.mjs";
export function prism(name, polygon, depth = 6) {
  let next = 18;
  const entries = [];
  const add = (body) => {
    const id = next++;
    entries.push(`#${id} = ${body};`);
    return id;
  };
  const number = (v) => Number(v).toFixed(9);
  const tuple = (p) => `(${p.map(number).join(", ")})`;
  const n = polygon.length;
  const points = [...polygon.map(([x, z]) => [x, 0, z]), ...polygon.map(([x, z]) => [x, depth, z])];
  const pointIds = points.map((p) => add(`CARTESIAN_POINT('', ${tuple(p)})`));
  const vertices = pointIds.map((id) => add(`VERTEX_POINT('', #${id})`));
  const edges = new Map();
  const edge = (a, b) => {
    const key = [a, b].sort((x, y) => x - y).join("/");
    let value = edges.get(key);
    if (!value) {
      const delta = points[b].map((v, i) => v - points[a][i]);
      const length = Math.hypot(...delta);
      const dir = add(`DIRECTION('', ${tuple(delta.map((v) => v / length))})`);
      const vector = add(`VECTOR('', #${dir}, ${number(length)})`);
      const line = add(`LINE('', #${pointIds[a]}, #${vector})`);
      value = { a, b, id: add(`EDGE_CURVE('', #${vertices[a]}, #${vertices[b]}, #${line}, .T.)`) };
      edges.set(key, value);
    }
    return add(`ORIENTED_EDGE('', *, *, #${value.id}, ${value.a === a ? ".T." : ".F."})`);
  };
  const face = (indices) => {
    const oriented = indices.map((a, i) => edge(a, indices[(i + 1) % indices.length]));
    const loop = add(`EDGE_LOOP('', (${oriented.map((id) => "#" + id).join(", ")}))`);
    const bound = add(`FACE_OUTER_BOUND('', #${loop}, .T.)`);
    const a = points[indices[0]],
      u = points[indices[1]].map((v, i) => v - a[i]),
      v = points[indices[2]].map((x, i) => x - a[i]);
    const cross = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    const norm = Math.hypot(...cross);
    if (!(norm > 0)) throw Error("degenerate plane");
    const normal = add(`DIRECTION('', ${tuple(cross.map((x) => x / norm))})`),
      direction = add(`DIRECTION('', ${tuple(u.map((x) => x / Math.hypot(...u)))})`);
    const placement = add(
      `AXIS2_PLACEMENT_3D('', #${pointIds[indices[0]]}, #${normal}, #${direction})`,
    );
    const plane = add(`PLANE('', #${placement})`);
    return add(`ADVANCED_FACE('', (#${bound}), #${plane}, .T.)`);
  };
  const faces = [
    face(Array.from({ length: n }, (_, i) => i)),
    face(Array.from({ length: n }, (_, i) => 2 * n - 1 - i)),
  ];
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    faces.push(face([i, i + n, j + n, j]));
  }
  return `ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('Original Spiling analytic bridge coupon'),'2;1');\nFILE_NAME('${name}', '2026-01-01T00:00:00', ('Spiling contributors'), ('Spiling'), 'Spiling bounded analytic prism author', 'Spiling original bridge corpus', 'OSL-3.0');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1 = APPLICATION_PROTOCOL_DEFINITION('international standard', 'automotive_design', 2000, #2);\n#2 = APPLICATION_CONTEXT('core data for automotive mechanical design processes');\n#3 = SHAPE_DEFINITION_REPRESENTATION(#4, #10);\n#4 = PRODUCT_DEFINITION_SHAPE('','', #5);\n#5 = PRODUCT_DEFINITION('design','', #6, #9);\n#6 = PRODUCT_DEFINITION_FORMATION('','', #7);\n#7 = PRODUCT('','','', (#8));\n#8 = PRODUCT_CONTEXT('', #2, 'mechanical');\n#9 = PRODUCT_DEFINITION_CONTEXT('part definition', #2, 'design');\n#10 = SHAPE_REPRESENTATION('', (#16), #11);\n#11 = (GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#15)) GLOBAL_UNIT_ASSIGNED_CONTEXT((#12,#13,#14)) REPRESENTATION_CONTEXT('Context #1','3D Context with UNIT and UNCERTAINTY'));\n#12 = (LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\n#13 = (NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.));\n#14 = (NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT());\n#15 = UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.0E-6), #12, 'distance_accuracy_value','confusion accuracy');\n#16 = MANIFOLD_SOLID_BREP('', #17);\n#17 = CLOSED_SHELL('', (${faces.map((id) => "#" + id).join(", ")}));\n${entries.join("\n")}\nENDSEC;\nEND-ISO-10303-21;\n`;
}

export const bridgeSources = [2, 6, 10]
  .map((span) => ({
    id: `span-${span}`,
    spanMm: span,
    widthMm: span + 4,
    polygon: [
      [0, 0],
      [2, 0],
      [2, 3],
      [2 + span, 3],
      [2 + span, 0],
      [4 + span, 0],
      [4 + span, 4],
      [0, 4],
    ],
  }))
  .concat([
    {
      id: "one-ended",
      spanMm: 8,
      widthMm: 10,
      polygon: [
        [0, 0],
        [2, 0],
        [2, 3],
        [10, 3],
        [10, 4],
        [0, 4],
      ],
    },
  ]);
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const destination = process.argv[2];
  if (!destination) throw Error("supply a NEW output directory");
  await mkdir(destination); // never replace frozen files in place
  const fixtures = [];
  const license =
    "SPDX-FileCopyrightText: 2026 Spiling contributors\nSPDX-License-Identifier: OSL-3.0\n";
  for (const source of bridgeSources) {
    const path = `${source.id}.step`,
      bytes = prism(path, source.polygon);
    await writeFile(resolve(destination, path), bytes, { flag: "wx" });
    await writeFile(resolve(destination, path + ".license"), license, { flag: "wx" });
    fixtures.push({
      path,
      sha256: sha256(bytes),
      byte_count: Buffer.byteLength(bytes),
      xz_vertices_mm: source.polygon,
      depth_mm: 6,
      roof_z_mm: [3, 4],
      provenance:
        "Original Spiling analytic closed planar prism; tools/gcode-parity/bridge-corpus.mjs",
    });
  }
  await writeFile(
    resolve(destination, "manifest.json"),
    JSON.stringify(
      {
        "SPDX-License-Identifier": "OSL-3.0",
        fixtures,
      },
      null,
      2,
    ) + "\n",
    { flag: "wx" },
  );
  await writeFile(resolve(destination, "manifest.json.license"), license, { flag: "wx" });
}
