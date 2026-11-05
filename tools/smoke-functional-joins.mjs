/*
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  stat,
  unlink,
  writeFile,
} from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Requires freshly built CLI/engine binaries. Run sequentially inside an external
// whole-process memory/task bound; worker counts are not a memory bound. No build.
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = join(target, "debug", `spiling-cli${suffix}`);
const engine = join(target, "debug", `spiling-engine${suffix}`);
const artifacts = ["plan.json", "program.gcode", "provenance.json", "verification.json"];
const pose = { translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] };
const epsilon = 1e-6;
const json = async (path) => JSON.parse(await readFile(path, "utf8"));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "functional-joins-"));
console.error(`Evidence: ${output}`);
const evidence = { software_only: true, not_machine_ready: true, commands: [], refusals: [] };
const started = Date.now();

// Read the producer declarations, rather than accepting an obsolete hardcoded
// Hello substring or silently accepting whatever binary happens to be installed.
const contracts = await readFile(join(root, "crates/contracts/src/lib.rs"), "utf8");
const buildDeclaration = contracts.match(
  /pub const ENGINE_BUILD_ID: &str = concat!\(([\s\S]*?)\);/,
);
assert.ok(buildDeclaration, "ENGINE_BUILD_ID declaration not found");
const buildSuffix = buildDeclaration[1].match(/"(;[^"\n]+)"/)?.[1];
assert.ok(buildSuffix, "ENGINE_BUILD_ID capability suffix not found");
const cargo = await readFile(join(root, "Cargo.toml"), "utf8");
const packageVersion = cargo.match(/\[workspace\.package\][\s\S]*?\bversion\s*=\s*"([^"]+)"/)?.[1];
assert.ok(packageVersion, "workspace package version not found");
evidence.engine_build = packageVersion + buildSuffix;
const manufacturing = await readFile(join(root, "crates/manufacturing/src/lib.rs"), "utf8");
const producers = Object.fromEntries(
  ["planner", "emitter", "verifier"].map((kind) => {
    const value = manufacturing.match(
      new RegExp(`pub const ${kind.toUpperCase()}_VERSION: &str = "([^"]+)"`),
    )?.[1];
    assert.ok(value, `${kind} producer declaration not found`);
    assert.ok(buildSuffix.split(";").includes(kind + value), `${kind} and Hello disagree`);
    return [`${kind}_version`, value];
  }),
);
const constructionContracts = await readFile(
  join(root, "crates/contracts/src/manufacturing/construction.rs"),
  "utf8",
);
const maxDomains = Number(
  constructionContracts.match(/pub const MAX_MATERIAL_DOMAINS: usize = (\d+);/)?.[1],
);
assert.ok(maxDomains > 0, "material-domain bound not found");
evidence.environment = {
  node: process.version,
  platform: process.platform,
  arch: process.arch,
  cli,
  engine,
  target,
  rayon_num_threads: process.env.RAYON_NUM_THREADS ?? "1",
};

async function save(name, value) {
  await writeFile(join(output, name), JSON.stringify(value, null, 2) + "\n", { flag: "wx" });
}

async function invoke(name, args, refusal = null) {
  assert.ok(Date.now() - started < 20 * 60_000, "smoke exceeded twenty-minute command budget");
  const command = ["project", ...args, "--engine", engine];
  const entry = { name, args: command };
  evidence.commands.push(entry);
  let stdout = "",
    stderr = "",
    bytes = 0,
    aborted;
  await new Promise((done, fail) => {
    const child = spawn(cli, command, {
      cwd: root,
      env: {
        ...process.env,
        RAYON_NUM_THREADS: process.env.RAYON_NUM_THREADS ?? "1",
        RUST_TEST_THREADS: process.env.RUST_TEST_THREADS ?? "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const abort = (message) => {
      aborted ??= new Error(message);
      // The engine's inherited owner-liveness pipe closes with the CLI.
      child.kill("SIGKILL");
    };
    const timer = setTimeout(() => abort(`${name}: CLI exceeded five minutes`), 300_000);
    for (const [stream, key] of [
      [child.stdout, "stdout"],
      [child.stderr, "stderr"],
    ]) {
      stream.setEncoding("utf8").on("data", (chunk) => {
        bytes += Buffer.byteLength(chunk);
        if (bytes > 16 * 1024 * 1024) abort(`${name}: log budget exceeded`);
        else if (key === "stdout") stdout += chunk;
        else stderr += chunk;
      });
    }
    child.once("error", (error) => {
      clearTimeout(timer);
      fail(error);
    });
    child.once("close", (code, signal) => {
      clearTimeout(timer);
      entry.status = code;
      entry.signal = signal;
      if (aborted) fail(aborted);
      else done();
    });
  }).finally(async () => {
    await writeFile(join(output, `${name}.stdout.txt`), stdout, { flag: "wx" });
    await writeFile(join(output, `${name}.stderr.txt`), stderr, { flag: "wx" });
    await save(`${name}.command.json`, entry);
  });
  if (refusal) {
    assert.notEqual(entry.status, 0, `${name}: invalid request succeeded`);
    assert.equal(entry.signal, null, `${name}: crash is not a typed refusal`);
    const report = stderr
      .trim()
      .split("\n")
      .map((line) => {
        try {
          return JSON.parse(line);
        } catch {
          return null;
        }
      })
      .find((row) => row?.event === "cli_failure" && row.domain === "manufacturing");
    assert.ok(report?.error, `${name}: missing typed manufacturing refusal`);
    assert.equal(report.error.code, refusal.code);
    assert.match(report.error.message, refusal.message);
    assert.equal(report.software_only, true);
    assert.equal(report.not_machine_ready, true);
    assert.ok(report.project && report.manufacturing, `${name}: missing reconciled status`);
    evidence.refusals.push({ name, domain: report.domain, error: report.error });
    return report;
  }
  assert.equal(entry.status, 0, `${name}: ${stderr}`);
  const report = JSON.parse(stdout);
  assert.equal(report.hello.engine_build, evidence.engine_build, "binary/source producer mismatch");
  assert.equal(report.hello.kernel, "solidcore");
  assert.ok(report.hello.pid > 0);
  assert.match(report.hello.session_id, /^[0-9a-f-]{36}$/);
  return report;
}

function near(actual, expected, message) {
  assert.ok(Math.abs(actual - expected) <= epsilon, `${message}: ${actual} != ${expected}`);
}

function inRing([x, y], ring) {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [ax, ay] = ring[i],
      [bx, by] = ring[j];
    if (ay > y !== by > y && x < ((bx - ax) * (y - ay)) / (by - ay) + ax) inside = !inside;
  }
  return inside;
}

function contains(domain, point) {
  return domain.polygons.some(
    (p) => inRing(point, p.exterior_mm) && p.holes_mm.every((hole) => !inRing(point, hole)),
  );
}

function area(domain) {
  const ringArea = (ring) =>
    Math.abs(
      ring.reduce((sum, p, i) => {
        const q = ring[(i + 1) % ring.length];
        return sum + p[0] * q[1] - q[0] * p[1];
      }, 0),
    ) / 2;
  return domain.polygons.reduce(
    (sum, p) => sum + ringArea(p.exterior_mm) - p.holes_mm.reduce((a, h) => a + ringArea(h), 0),
    0,
  );
}

function checkPlan(plan, request, stockBounds) {
  assert.deepEqual(plan.material_frames, [], "functional construction acquired optical frames");
  const frame = plan.functional_material_frame;
  assert.ok(frame, "missing captured functional frame");
  for (const key of ["occurrence_id", "definition_id", "source_hash", "face_id"])
    assert.deepEqual(frame[key], request[key]);
  assert.deepEqual(frame.outward_normal, [0, -1, 0]);
  assert.deepEqual(frame.horizontal_direction, [1, 0, 0]);
  assert.deepEqual(frame.stock_bounds_mm, {
    min: stockBounds.min.map((v, axis) => v + pose.translation_mm[axis]),
    max: stockBounds.max.map((v, axis) => v + pose.translation_mm[axis]),
  });
  frame.world_origin_mm.forEach((v, axis) =>
    near(v, frame.origin_mm[axis] + pose.translation_mm[axis], "placed frame"),
  );
  near(frame.width_mm, stockBounds.max[0] - stockBounds.min[0], "full face width");
  const bottom = stockBounds.min[2] + pose.translation_mm[2];
  const top = stockBounds.max[2] + pose.translation_mm[2];
  const middle = [
    bottom + request.join.end_collar_height_mm,
    top - request.join.end_collar_height_mm,
  ];
  frame.key_middle_z_mm.forEach((v, i) => near(v, middle[i], "collar transition"));
  assert.ok(frame.key_centers_u_mm.includes(frame.width_mm / 2), "missing centered key");
  assert.ok(frame.key_centers_u_mm.length >= 1 && frame.key_centers_u_mm.length <= 16);
  for (let i = 0; i < frame.key_centers_u_mm.length; i++) {
    const center = frame.key_centers_u_mm[i];
    assert.ok(center - request.join.head_width_mm / 2 >= request.join.edge_margin_mm - epsilon);
    assert.ok(
      center + request.join.head_width_mm / 2 <=
        frame.width_mm - request.join.edge_margin_mm + epsilon,
    );
    if (i) near(center - frame.key_centers_u_mm[i - 1], request.join.pitch_mm, "key pitch");
  }
  const pure = request.minimum_pure_depth_mm;
  const end = pure + request.join.reach_into_region_mm + request.join.reach_into_base_mm;
  const neckEnd = end - request.join.head_depth_mm;
  const stockDepth = stockBounds.max[1] - stockBounds.min[1];
  const world = (u, depth) => [frame.world_origin_mm[0] + u, frame.world_origin_mm[1] + depth];
  const depth = (point) => point[1] - frame.world_origin_mm[1];
  const roles = ["functional_region", "functional_base"];
  assert.ok(plan.material_domains.length >= 6 && plan.material_domains.length <= maxDomains);
  const intervals = new Map();
  for (const domain of plan.material_domains) {
    assert.ok(roles.includes(domain.reference.role), "unexpected optical material role");
    assert.equal(domain.reference.construction_index, 0);
    assert.ok(domain.physical_z_mm[0] < domain.physical_z_mm[1]);
    const key = JSON.stringify(domain.physical_z_mm);
    const pair = intervals.get(key) ?? {};
    assert.equal(pair[domain.reference.role], undefined, "duplicate role interval");
    pair[domain.reference.role] = domain;
    intervals.set(key, pair);
  }
  const ordered = [...intervals]
    .map(([key, pair]) => [JSON.parse(key), pair])
    .sort((a, b) => a[0][0] - b[0][0]);
  near(ordered[0][0][0], bottom, "domain start");
  near(ordered.at(-1)[0][1], top, "domain end");
  const phases = new Set();
  for (const [[lower, upper], pair] of ordered) {
    assert.deepEqual(Object.keys(pair).sort(), [...roles].sort());
    assert.ok(
      !middle.some((z) => lower < z - epsilon && upper > z + epsilon),
      "domain crosses a physical phase boundary",
    );
    const phase =
      upper <= middle[0] + epsilon ? "bottom" : lower >= middle[1] - epsilon ? "top" : "middle";
    phases.add(phase);
    const head =
      phase === "middle" ? request.join.head_width_mm : request.join.collar_head_width_mm;
    const regionArea =
      frame.width_mm * pure +
      frame.key_centers_u_mm.length *
        (request.join.stem_width_mm * (neckEnd - pure) + head * request.join.head_depth_mm);
    near(area(pair.functional_region), regionArea, "pure slab and key footprint");
    near(
      area(pair.functional_region) + area(pair.functional_base),
      frame.width_mm * stockDepth,
      "complete stock partition",
    );
    // Every vertex-defined horizontal strip in the protected pure slab belongs
    // to region only. This is an assertion over captured geometry, not a planner.
    const cuts = [
      0,
      frame.width_mm,
      ...Object.values(pair).flatMap((d) =>
        d.polygons.flatMap((p) =>
          [p.exterior_mm, ...p.holes_mm].flatMap((ring) =>
            ring.map((point) => point[0] - frame.world_origin_mm[0]),
          ),
        ),
      ),
    ];
    const sorted = [...new Set(cuts)].sort((a, b) => a - b);
    for (let i = 1; i < sorted.length; i++) {
      const u = (sorted[i - 1] + sorted[i]) / 2;
      if (u <= 0 || u >= frame.width_mm) continue;
      for (const d of [pure * 0.01, pure / 2, pure * 0.99]) {
        assert.ok(contains(pair.functional_region, world(u, d)), "pure-depth region gap");
        assert.equal(contains(pair.functional_base, world(u, d)), false, "base enters pure depth");
      }
    }
    for (const center of frame.key_centers_u_mm) {
      for (const sign of [-1, 1]) {
        const shoulder = world(
          center + (sign * (request.join.collar_head_width_mm + request.join.head_width_mm)) / 4,
          (neckEnd + end) / 2,
        );
        assert.equal(
          contains(pair.functional_region, shoulder),
          phase === "middle",
          "head/collar phase footprint",
        );
        assert.equal(
          contains(pair.functional_base, shoulder),
          phase !== "middle",
          "complementary retainer footprint",
        );
      }
    }
    for (const p of pair.functional_base.polygons)
      for (const point of p.exterior_mm)
        assert.ok(depth(point) >= pure - epsilon, "base boundary enters pure depth");
  }
  assert.deepEqual([...phases].sort(), ["bottom", "middle", "top"]);
  for (let i = 1; i < ordered.length; i++)
    near(ordered[i][0][0], ordered[i - 1][0][1], "continuous domains");
  assert.ok(plan.layers.length > 0);
  for (const layer of plan.layers) {
    assert.ok(layer.paths.length > 0);
    assert.ok(
      !middle.some((z) => layer.lower_z_mm < z - epsilon && layer.z_mm > z + epsilon),
      "bead crosses collar event",
    );
    assert.deepEqual(
      [...new Set(layer.paths.map((p) => p.material_domain?.role))].sort(),
      [...roles].sort(),
    );
    for (const path of layer.paths) {
      assert.equal(path.material_domain.construction_index, 0);
      const domain = plan.material_domains.find(
        (d) =>
          d.reference.role === path.material_domain.role &&
          path.physical_z_mm[0] >= d.physical_z_mm[0] - epsilon &&
          path.physical_z_mm[1] <= d.physical_z_mm[1] + epsilon,
      );
      assert.ok(domain, "path has no phase-specific role domain");
      near(path.physical_z_mm[0], layer.lower_z_mm, "fine bead lower interval");
      near(path.physical_z_mm[1], layer.z_mm, "fine bead upper interval");
      if (path.material_domain.role === "functional_base") {
        for (let i = 1; i < path.points_mm.length; i++) {
          const width =
            path.width.kind === "constant" ? path.width.width_mm : path.width.widths_mm[i - 1];
          assert.ok(width > 0);
          for (const point of [path.points_mm[i - 1], path.points_mm[i]])
            assert.ok(
              depth(point) - width / 2 >= pure - epsilon,
              "base bead footprint enters protected pure depth",
            );
        }
      }
    }
  }
  return {
    layers: plan.layers.length,
    domains: plan.material_domains.length,
    phases: [...phases],
    keys: frame.key_centers_u_mm.length,
  };
}

function checkVerified(report, intent) {
  assert.equal(report.manufacturing.verification.verified, true);
  assert.equal(report.manufacturing.software_only, true);
  assert.equal(report.manufacturing.not_machine_ready, true);
  assert.deepEqual(report.manufacturing.intent, intent);
  assert.equal(report.manufacturing.verified_artifact_hash, report.manufacturing.artifact.hash);
  const routing = report.manufacturing.verification.routing;
  assert.ok(routing?.selection_count > 0);
  routing.tools.forEach((tool, i) => {
    assert.equal(tool.tool, i);
    assert.equal(tool.material_id, intent.printer.workflow.job.tools[i].material.id);
    assert.ok(tool.model_deposition_segments > 0 && tool.waste_deposition_segments > 0);
  });
}

function checkStaleHistory(report, labels, baselineHash) {
  assert.equal(report.operations[0].operation, "edit");
  const operations = report.operations.slice(1);
  assert.deepEqual(
    operations.map((op) => op.operation),
    labels,
  );
  assert.equal(operations[0].preflight_freshness, "current");
  assert.equal(operations[1].manufacturing.artifact, null, "intent edit retained active artifact");
  assert.equal(
    operations[2].manufacturing.artifact.hash,
    baselineHash,
    "one undo failed to restore the full capture",
  );
  for (const operation of operations.slice(1)) {
    assert.equal(
      operation.preflight_freshness,
      "stale",
      "historical preflight presented as current",
    );
    assert.ok(operation.project.revision > operations[0].project.revision);
  }
  assert.equal(report.manufacturing.preflight_freshness, "stale");
}

try {
  await Promise.all([stat(cli), stat(engine)]);
  const sourceFixture = join(root, "fixtures/geometry/box-mm.step");
  const source = join(output, "box-mm.step");
  const project = join(output, "functional.spil");
  const sourceHash = hash(await readFile(sourceFixture));
  evidence.source_sha256 = sourceHash;
  await copyFile(sourceFixture, source);
  const created = await invoke("01-create", [
    "create",
    project,
    "--import",
    source,
    "--pose",
    "1",
    JSON.stringify(pose),
  ]);
  assert.equal(created.definitions.length, 1);
  const definition = created.definitions[0];
  const inspected = await invoke("02-inspect-native-faces", [
    "inspect",
    project,
    ...definition.faces.flatMap((face) => ["--inspect-face", "1", face.face_id]),
  ]);
  const selected = inspected.face_inspections.find((row) => {
    const plane = row.face.geometry.analytic;
    return (
      plane?.kind === "plane" &&
      plane.normal[0] === 0 &&
      plane.normal[1] === -1 &&
      plane.normal[2] === 0
    );
  });
  assert.ok(selected, "native CLI inspection has no -Y planar face");
  assert.equal(selected.provenance.source_hash, sourceHash);
  evidence.selected_face = selected;
  const intent = await json(join(root, "fixtures/manufacturing/fixed-dual.intent.json"));
  const job = intent.printer.workflow.job;
  job.assignments = job.assignments.slice(0, 1);
  job.assignments[0].material = { kind: "uniform", material_id: job.tools[1].material.id };
  const bounds = definition.record.bounds_mm;
  job.assignments[0].work_envelope_mm = {
    min: bounds.min.map((v, axis) => v + pose.translation_mm[axis] - 1),
    max: bounds.max.map((v, axis) => v + pose.translation_mm[axis] + 1),
  };
  job.surface_constructions = [];
  job.tools[0].material.id = "software-tpu";
  job.tools[0].material.polymer_family = "TPU";
  job.tools[0].material.declaration_hash = hash(Buffer.from("software TPU declaration"));
  job.tools[0].filament_diameter_mm = 2.85;
  job.tools[0].nozzle_temperature_c = 220;
  job.functional_construction = {
    occurrence_id: 1,
    definition_id: selected.reference.definition_id,
    source_hash: selected.provenance.source_hash,
    face_id: selected.face.face_id,
    material_id: job.tools[0].material.id,
    base_material_id: job.tools[1].material.id,
    interface_policy: "mechanical_capture",
    minimum_pure_depth_mm: 5,
    join: {
      reach_into_region_mm: 1.6,
      reach_into_base_mm: 1.6,
      pitch_mm: 6.4,
      stem_width_mm: 1.6,
      head_width_mm: 4.8,
      head_depth_mm: 1.2,
      edge_margin_mm: 1.2,
      end_collar_height_mm: 1.2,
      collar_head_width_mm: 3.2,
    },
  };
  intent.recipe.infill_fraction = 1;
  intent.recipe.perimeter_count = 1;
  const intentFile = join(output, "functional.intent.json");
  await save("functional.intent.json", intent);
  const exported = join(output, "baseline");
  const compiled = await invoke("03-compile-verify-export", [
    "edit",
    project,
    "--intent",
    intentFile,
    "--preflight",
    "--compile",
    "--verify",
    "--save",
    "--manufacturing-out",
    exported,
  ]);
  checkVerified(compiled, intent);
  assert.equal(compiled.manufacturing.preflight.disposition, "admitted");
  const preflightOperation = compiled.operations.find((op) => op.operation === "preflight");
  assert.equal(preflightOperation.preflight_freshness, "current");
  assert.ok(
    compiled.project.revision > preflightOperation.project.revision,
    "compile did not publish a project transaction",
  );
  assert.equal(compiled.manufacturing.preflight_freshness, "stale");
  assert.deepEqual((await readdir(exported)).sort(), ["manifest.json", ...artifacts]);
  const plan = await json(join(exported, "plan.json"));
  const fixtureStock = { min: [0, 0, 0], max: [20, 10, 8] };
  for (let axis = 0; axis < 3; axis++) {
    assert.ok(bounds.min[axis] <= fixtureStock.min[axis]);
    assert.ok(bounds.max[axis] >= fixtureStock.max[axis]);
  }
  evidence.construction = checkPlan(plan, job.functional_construction, fixtureStock);
  const provenance = await json(join(exported, "provenance.json"));
  assert.deepEqual(provenance.intent, intent);
  assert.deepEqual(provenance.inspection.workflow, intent.printer.workflow);
  assert.deepEqual(
    provenance.inspection.surface_color,
    [],
    "functional construction acquired optical assessment",
  );
  for (const [key, value] of Object.entries(producers)) {
    assert.equal(provenance[key], value);
    assert.equal(provenance.inspection[key], value);
  }
  assert.deepEqual(
    await json(join(exported, "verification.json")),
    compiled.manufacturing.verification,
  );
  const baselineHash = compiled.manufacturing.artifact.hash;
  const changed = structuredClone(intent);
  changed.printer.workflow.job.functional_construction.minimum_pure_depth_mm = 4.5;
  const changedFile = join(output, "changed.intent.json");
  await save("changed.intent.json", changed);
  const undone = await invoke("04-undo-captured-intent", [
    "edit",
    project,
    "--preflight",
    "--intent",
    changedFile,
    "--undo",
    "--verify",
    "--save",
  ]);
  checkVerified(undone, intent);
  checkStaleHistory(undone, ["preflight", "intent", "undo", "verify", "save"], baselineHash);
  const redoProject = join(output, "redo.spil");
  await copyFile(project, redoProject);
  const redone = await invoke("05-redo-captured-intent", [
    "edit",
    redoProject,
    "--preflight",
    "--intent",
    changedFile,
    "--undo",
    "--redo",
  ]);
  checkStaleHistory(redone, ["preflight", "intent", "undo", "redo", "save"], baselineHash);
  assert.deepEqual(redone.manufacturing.intent, changed, "redo lost part of the captured intent");
  assert.equal(redone.manufacturing.artifact, null, "redo retained stale manufacturing artifact");
  evidence.undo_redo = {
    full_intent_restored: true,
    full_edited_intent_redone: true,
    stale_preflight_labelled: true,
  };

  // Only these disposable copies are removed. Repository fixtures and Core-owned
  // immutable storage are never modified. Neither input path exists on reopen.
  await unlink(source);
  await unlink(intentFile);
  await unlink(changedFile);
  const recoveredExport = join(output, "source-and-intents-absent");
  const reopened = await invoke("06-source-free-fresh-replay", [
    "inspect",
    project,
    "--preflight",
    "--verify",
    "--manufacturing-out",
    recoveredExport,
  ]);
  assert.notEqual(reopened.hello.session_id, compiled.hello.session_id);
  checkVerified(reopened, intent);
  assert.equal(reopened.manufacturing.preflight.disposition, "admitted");
  assert.equal(reopened.manufacturing.preflight_freshness, "current");
  assert.equal(reopened.manufacturing.artifact.hash, baselineHash);
  assert.deepEqual((await readdir(recoveredExport)).sort(), ["manifest.json", ...artifacts]);
  evidence.artifacts = {};
  for (const name of artifacts) {
    const before = await readFile(join(exported, name));
    const after = await readFile(join(recoveredExport, name));
    assert.deepEqual(after, before, `fresh source-free export changed immutable ${name}`);
    evidence.artifacts[name] = { bytes: before.length, sha256: hash(before) };
  }

  for (const [name, alter, refusal] of [
    [
      "insufficient-depth",
      (value) => {
        value.printer.workflow.job.functional_construction.minimum_pure_depth_mm = 9;
      },
      {
        code: "unsupported_geometry",
        message: /pure depth.*(?:do not fit|stock)|functional join.*depth/i,
      },
    ],
    [
      "wrong-source",
      (value) => {
        value.printer.workflow.job.functional_construction.source_hash = "0".repeat(64);
      },
      {
        code: "unsupported_geometry",
        message:
          /Functional material construction does not bind the immutable native source and face/,
      },
    ],
    [
      "mixed-polymer-family",
      (value) => {
        value.printer.workflow.job.functional_construction.interface_policy =
          "declared_same_family";
      },
      { code: "invalid_specification", message: /declared common polymer family/ },
    ],
  ]) {
    const invalid = structuredClone(intent);
    alter(invalid);
    const clone = join(output, `${name}.spil`);
    const invalidFile = join(output, `${name}.intent.json`);
    const invalidExport = join(output, `${name}-export`);
    await copyFile(project, clone);
    await save(`${name}.intent.json`, invalid);
    const rejected = await invoke(
      `07-reject-${name}`,
      [
        "edit",
        clone,
        "--intent",
        invalidFile,
        "--preflight",
        "--compile",
        "--verify",
        "--manufacturing-out",
        invalidExport,
      ],
      refusal,
    );
    if (name === "mixed-polymer-family") {
      assert.deepEqual(
        rejected.manufacturing.intent,
        intent,
        "invalid polymer intent was committed",
      );
      assert.equal(rejected.manufacturing.artifact.hash, baselineHash);
    } else {
      assert.deepEqual(rejected.manufacturing.intent, invalid);
      assert.equal(rejected.manufacturing.artifact, null);
    }
    await assert.rejects(stat(invalidExport), { code: "ENOENT" });
  }
  assert.equal(hash(await readFile(sourceFixture)), sourceHash, "immutable fixture changed");
  await save("result.json", evidence);
  console.log(JSON.stringify({ output, ...evidence }, null, 2));
} catch (error) {
  await save("failure.json", { message: error.message, stack: error.stack, evidence });
  console.error(`Functional join smoke failed; exact commands and output: ${output}`);
  throw error;
}
