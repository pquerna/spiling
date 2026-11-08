/*
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import assert from "node:assert/strict";
import { decodeReplayDisplay } from "../packages/protocol/src/replay.ts";
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

function faceCoordinates(frame, point) {
  const offset = point.slice(0, 2).map((v, axis) => v - frame.world_origin_mm[axis]);
  return [
    offset.reduce((sum, v, axis) => sum + v * frame.horizontal_direction[axis], 0),
    offset.reduce((sum, v, axis) => sum - v * frame.outward_normal[axis], 0),
  ];
}

function checkPlan(plan, request, stockBounds, baseFraction = 1, normal = [0, -1, 0]) {
  assert.deepEqual(plan.material_frames, [], "functional construction acquired optical frames");
  const frame = plan.functional_material_frame;
  assert.ok(frame, "missing captured functional frame");
  for (const key of ["occurrence_id", "definition_id", "source_hash", "face_id"])
    assert.deepEqual(frame[key], request[key]);
  const depthAxis = normal[0] === 0 ? 1 : 0;
  const horizontalAxis = 1 - depthAxis;
  frame.outward_normal.forEach((v, axis) => near(v, normal[axis], "selected face normal"));
  frame.horizontal_direction.forEach((v, axis) =>
    near(v, Number(axis === horizontalAxis), "canonical positive face U direction"),
  );
  assert.deepEqual(frame.stock_bounds_mm, {
    min: stockBounds.min.map((v, axis) => v + pose.translation_mm[axis]),
    max: stockBounds.max.map((v, axis) => v + pose.translation_mm[axis]),
  });
  frame.world_origin_mm.forEach((v, axis) =>
    near(v, frame.origin_mm[axis] + pose.translation_mm[axis], "placed frame"),
  );
  near(
    frame.width_mm,
    stockBounds.max[horizontalAxis] - stockBounds.min[horizontalAxis],
    "full face width",
  );
  const bottom = stockBounds.min[2] + pose.translation_mm[2];
  const top = stockBounds.max[2] + pose.translation_mm[2];
  const selectedU = request.face_patch?.horizontal_mm ?? [0, frame.width_mm];
  const sourceZ = request.face_patch?.source_z_mm ?? [stockBounds.min[2], stockBounds.max[2]];
  const regionZ = sourceZ.map((z) => z + pose.translation_mm[2]);
  assert.deepEqual(frame.selected_u_mm, selectedU);
  assert.deepEqual(frame.region_z_mm, regionZ);
  assert.deepEqual(frame.source_z_mm, [stockBounds.min[2], stockBounds.max[2]]);
  const middle = [
    regionZ[0] + request.join.end_collar_height_mm,
    regionZ[1] - request.join.end_collar_height_mm,
  ];
  const events = [...regionZ, ...middle];
  frame.key_middle_z_mm.forEach((v, i) => near(v, middle[i], "collar transition"));
  assert.ok(
    frame.key_centers_u_mm.includes((selectedU[0] + selectedU[1]) / 2),
    "missing centered key",
  );
  assert.ok(frame.key_centers_u_mm.length >= 1 && frame.key_centers_u_mm.length <= 16);
  for (let i = 0; i < frame.key_centers_u_mm.length; i++) {
    const center = frame.key_centers_u_mm[i];
    assert.ok(
      center - request.join.head_width_mm / 2 >=
        selectedU[0] + request.join.edge_margin_mm - epsilon,
    );
    assert.ok(
      center + request.join.head_width_mm / 2 <=
        selectedU[1] - request.join.edge_margin_mm + epsilon,
    );
    if (i) near(center - frame.key_centers_u_mm[i - 1], request.join.pitch_mm, "key pitch");
  }
  const pure = request.minimum_pure_depth_mm;
  const end = pure + request.join.reach_into_region_mm + request.join.reach_into_base_mm;
  const neckEnd = end - request.join.head_depth_mm;
  const stockDepth = stockBounds.max[depthAxis] - stockBounds.min[depthAxis];
  const world = (u, depth) =>
    frame.world_origin_mm
      .slice(0, 2)
      .map((v, axis) => v + u * frame.horizontal_direction[axis] - depth * normal[axis]);
  const local = (point) => faceCoordinates(frame, point);
  const depth = (point) => local(point)[1];
  assert.equal(request.minimum_solid_base_root_depth_mm, 0.8);
  assert.ok(frame.solid_base_zones.length >= 1);
  const root = frame.solid_base_zones[0];
  assert.equal(root.kind, "root");
  root.inward_depth_mm.forEach((v, i) =>
    near(v, [pure, end + request.minimum_solid_base_root_depth_mm][i], "solid root depth"),
  );
  assert.deepEqual(root.physical_z_mm, regionZ);
  assert.ok(root.inward_depth_mm[1] < stockDepth, "root leaves no inherited Base interior");
  if (!request.face_patch) assert.equal(frame.solid_base_zones.length, 1);
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
  let volume = 0;
  for (const [[lower, upper], pair] of ordered) {
    const active = lower >= regionZ[0] - epsilon && upper <= regionZ[1] + epsilon;
    assert.deepEqual(Object.keys(pair).sort(), active ? [...roles].sort() : ["functional_base"]);
    near(
      Object.values(pair).reduce((sum, domain) => sum + area(domain), 0),
      frame.width_mm * stockDepth,
      "complete stock partition",
    );
    volume += Object.values(pair).reduce((sum, domain) => sum + area(domain), 0) * (upper - lower);
    const vertices = Object.values(pair).flatMap((domain) =>
      domain.polygons.flatMap((polygon) => [polygon.exterior_mm, ...polygon.holes_mm].flat()),
    );
    const cuts = [
      [0, frame.width_mm, ...selectedU, ...vertices.map((p) => local(p)[0])],
      [0, stockDepth, pure, ...vertices.map(depth)],
    ].map((values) => [...new Set(values)].sort((a, b) => a - b));
    for (let x = 1; x < cuts[0].length; x++) {
      const u = (cuts[0][x - 1] + cuts[0][x]) / 2;
      for (let y = 1; y < cuts[1].length; y++) {
        const d = (cuts[1][y - 1] + cuts[1][y]) / 2;
        const owners = Object.values(pair).filter((domain) => contains(domain, world(u, d)));
        assert.equal(owners.length, 1, "stock cell has a gap or overlapping material owners");
        if (d < pure) {
          const selected = active && u > selectedU[0] && u < selectedU[1];
          assert.equal(
            owners[0].reference.role,
            selected ? "functional_region" : "functional_base",
            "pure depth must apply only inside selected U and active Z",
          );
        }
      }
    }
    assert.ok(
      !events.some((z) => lower < z - epsilon && upper > z + epsilon),
      "domain crosses a physical phase boundary",
    );
    if (!active) {
      near(area(pair.functional_base), frame.width_mm * stockDepth, "outside active Z is all Base");
      phases.add(upper <= regionZ[0] + epsilon ? "below" : "above");
      continue;
    }
    const phase =
      upper <= middle[0] + epsilon ? "bottom" : lower >= middle[1] - epsilon ? "top" : "middle";
    phases.add(phase);
    const head =
      phase === "middle" ? request.join.head_width_mm : request.join.collar_head_width_mm;
    const regionArea =
      (selectedU[1] - selectedU[0]) * pure +
      frame.key_centers_u_mm.length *
        (request.join.stem_width_mm * (neckEnd - pure) + head * request.join.head_depth_mm);
    near(area(pair.functional_region), regionArea, "pure slab and key footprint");
    near(
      area(pair.functional_region) + area(pair.functional_base),
      frame.width_mm * stockDepth,
      "complete stock partition",
    );
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
        if (local(point)[0] > selectedU[0] + epsilon && local(point)[0] < selectedU[1] - epsilon)
          assert.ok(depth(point) >= pure - epsilon, "base boundary enters selected pure depth");
  }
  assert.deepEqual([...phases].sort(), [
    ...(regionZ[1] < top ? ["above"] : []),
    ...(regionZ[0] > bottom ? ["below"] : []),
    "bottom",
    "middle",
    "top",
  ]);
  near(volume, frame.width_mm * stockDepth * (top - bottom), "complete stock volume");
  for (let i = 1; i < ordered.length; i++)
    near(ordered[i][0][0], ordered[i - 1][0][1], "continuous domains");
  assert.ok(plan.layers.length > 0);
  let rootAndInheritedLayers = 0;
  for (const layer of plan.layers) {
    assert.ok(layer.paths.length > 0);
    assert.ok(
      !events.some((z) => layer.lower_z_mm < z - epsilon && layer.z_mm > z + epsilon),
      "bead crosses collar event",
    );
    const active = layer.lower_z_mm >= regionZ[0] - epsilon && layer.z_mm <= regionZ[1] + epsilon;
    const regions = layer.region_coverage.process_regions;
    for (const region of regions) {
      assert.ok(region.area_mm2 > 0, "empty functional process region");
      assert.equal(region.material_domain.construction_index, 0);
      if (region.material_domain.role === "functional_region") {
        near(region.fraction, 1, "functional pure region remains solid");
        assert.equal(region.functional_solid_zone, null);
      } else {
        assert.equal(region.material_domain.role, "functional_base");
        if (region.functional_solid_zone === null) {
          near(region.fraction, baseFraction, "Base inherits the captured infill fraction");
        } else {
          const zone = frame.solid_base_zones[region.functional_solid_zone];
          assert.ok(zone, "unknown functional solid zone");
          near(region.fraction, 1, "protected Base zone remains solid");
          assert.ok(layer.lower_z_mm >= zone.physical_z_mm[0] - epsilon);
          assert.ok(layer.z_mm <= zone.physical_z_mm[1] + epsilon);
        }
      }
    }
    if (
      regions.some((region) => region.functional_solid_zone === 0 && region.fraction === 1) &&
      regions.some(
        (region) =>
          region.material_domain.role === "functional_base" &&
          region.functional_solid_zone === null &&
          region.fraction === baseFraction,
      )
    )
      rootAndInheritedLayers++;
    assert.deepEqual(
      [...new Set(layer.paths.map((p) => p.material_domain?.role))].sort(),
      active ? [...roles].sort() : ["functional_base"],
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
      if (active && path.material_domain.role === "functional_base") {
        for (let i = 1; i < path.points_mm.length; i++) {
          const width =
            path.width.kind === "constant" ? path.width.width_mm : path.width.widths_mm[i - 1];
          assert.ok(width > 0);
          for (const point of [path.points_mm[i - 1], path.points_mm[i]]) {
            const u = local(point)[0];
            if (u <= selectedU[0] + epsilon || u >= selectedU[1] - epsilon) continue;
            assert.ok(
              depth(point) - width / 2 >= pure - epsilon,
              "base bead footprint enters selected protected pure depth",
            );
          }
        }
      }
    }
  }
  if (baseFraction < 1) {
    assert.ok(rootAndInheritedLayers > 0, "missing solid root beside inherited sparse Base");
    assert.ok(
      plan.layers.some((layer) =>
        layer.paths.some(
          (path) => path.kind === "sparse_fill" && path.material_domain.role === "functional_base",
        ),
      ),
      "sparse Base has no sparse-fill paths",
    );
  }
  return {
    layers: plan.layers.length,
    domains: plan.material_domains.length,
    phases: [...phases],
    keys: frame.key_centers_u_mm.length,
    selected_u_mm: frame.selected_u_mm,
    region_z_mm: frame.region_z_mm,
    stock_volume_mm3: volume,
    stock_depth_mm: stockDepth,
    base_infill_fraction: baseFraction,
    solid_base_zones: frame.solid_base_zones,
    root_and_inherited_layers: rootAndInheritedLayers,
  };
}

function checkVerified(report, intent, modelTools = [0, 1]) {
  assert.equal(report.manufacturing.verification.verified, true);
  assert.equal(report.manufacturing.software_only, true);
  assert.equal(report.manufacturing.not_machine_ready, true);
  assert.deepEqual(report.manufacturing.intent, intent);
  assert.equal(report.manufacturing.verified_artifact_hash, report.manufacturing.artifact.hash);
  const routing = report.manufacturing.verification.routing;
  assert.ok(routing?.selection_count > 0);
  assert.equal(routing.tools.length, 2);
  routing.tools.forEach((tool, i) => {
    assert.equal(tool.tool, i);
    assert.equal(tool.material_id, intent.printer.workflow.job.tools[i].material.id);
    assert.ok(tool.waste_deposition_segments > 0);
    assert.ok(tool.waste_volume_mm3 > 0 && tool.waste_filament_length_mm > 0);
    if (modelTools.includes(i)) {
      assert.ok(tool.model_deposition_segments > 0);
      assert.ok(tool.model_volume_mm3 > 0 && tool.model_filament_length_mm > 0);
    } else {
      assert.equal(
        tool.model_deposition_segments,
        0,
        "unused loaded tool deposited model material",
      );
      assert.equal(tool.model_volume_mm3, 0);
      assert.equal(tool.model_filament_length_mm, 0);
    }
  });
}

async function checkExport(directory, report, intent) {
  assert.deepEqual((await readdir(directory)).sort(), ["manifest.json", ...artifacts]);
  assert.deepEqual(report.manufacturing_export.artifact, report.manufacturing.artifact);
  assert.deepEqual(report.manufacturing_export.verification, report.manufacturing.verification);
  assert.deepEqual(
    await json(join(directory, "verification.json")),
    report.manufacturing.verification,
  );
  const provenance = await json(join(directory, "provenance.json"));
  assert.deepEqual(provenance.intent, intent);
  assert.deepEqual(provenance.inspection.workflow, intent.printer.workflow);
  assert.deepEqual(provenance.inspection.surface_color, []);
  for (const [key, value] of Object.entries(producers)) {
    assert.equal(provenance[key], value);
    assert.equal(provenance.inspection[key], value);
  }
  return json(join(directory, "plan.json"));
}

async function checkReplay(file, directory, report, intent, plan) {
  const replay = report.manufacturing_replay;
  assert.deepEqual(replay.artifact, report.manufacturing.artifact);
  assert.equal(replay.verification.verified, true);
  assert.equal(replay.replay_display.info.bundle_hash, report.manufacturing.artifact.hash);
  assert.equal(
    replay.replay_display.info.program_hash,
    hash(await readFile(join(directory, "program.gcode"))),
  );
  const bytes = await readFile(file);
  const decoded = await decodeReplayDisplay(Uint8Array.from(bytes).buffer, replay.replay_display);
  assert.equal(decoded.layerCount, plan.layers.length);
  const job = intent.printer.workflow.job;
  const frame = plan.functional_material_frame;
  const layers = plan.layers.map(() => [0, 1].map(() => ({ epochs: 0, waste: 0, model: 0 })));
  const totals = [0, 0];
  const filament = [0, 1].map(() => ({ model: 0, waste: 0 }));
  for (let event = 0; event < decoded.eventCount; event++) {
    const kind = decoded.kind(event);
    if (![1, 2, 3, 4, 5, 8, 14, 15].includes(kind)) continue;
    const tool = decoded.activeTool(event);
    const layer = decoded.layer(event);
    assert.ok(tool === 0 || tool === 1);
    assert.ok(layer < layers.length);
    if (kind !== 14) {
      assert.ok(event > 0 && decoded.activeTool(event - 1) === tool);
      const delta = decoded.extrusion(event) - decoded.extrusion(event - 1);
      assert.ok(delta > 0, "actual deposition has no positive filament movement");
      filament[tool][kind === 15 ? "waste" : "model"] += delta;
    }
    if (kind === 14) layers[layer][tool].epochs++;
    else if (kind === 15) layers[layer][tool].waste++;
    else {
      layers[layer][tool].model++;
      totals[tool]++;
      const association = decoded.association(event);
      assert.ok(association, "actual model stroke lost its captured physical interval");
      near(association.physicalZMm[0], plan.layers[layer].lower_z_mm, "actual bead lower interval");
      near(association.physicalZMm[1], plan.layers[layer].z_mm, "actual bead upper interval");
      if (frame && job.tools[tool].material.id === job.functional_construction.material_id) {
        assert.ok(association.physicalZMm[0] >= frame.region_z_mm[0] - epsilon);
        assert.ok(association.physicalZMm[1] <= frame.region_z_mm[1] + epsilon);
        for (const point of [decoded.position(event - 1), decoded.position(event)]) {
          const u = faceCoordinates(frame, point)[0];
          assert.ok(
            u >= frame.selected_u_mm[0] - epsilon && u <= frame.selected_u_mm[1] + epsilon,
            "actual functional material escaped selected U",
          );
        }
      } else if (!frame) {
        const assignments = job.assignments.filter(
          (row) => row.material.material_id === job.tools[tool].material.id,
        );
        assert.ok(
          assignments.some((row) =>
            [decoded.position(event - 1), decoded.position(event)].every((point) =>
              point.every(
                (value, axis) =>
                  value >= row.work_envelope_mm.min[axis] - epsilon &&
                  value <= row.work_envelope_mm.max[axis] + epsilon,
              ),
            ),
          ),
          "actual uniform model tool disagrees with occurrence reservation",
        );
      }
    }
  }
  for (let layer = 0; layer < layers.length; layer++) {
    for (const tool of [0, 1]) {
      assert.equal(layers[layer][tool].epochs, 1, "each loaded tool requires one epoch per layer");
      assert.equal(
        layers[layer][tool].waste,
        1,
        "each loaded tool requires waste even without model work",
      );
      const isBase =
        frame && job.tools[tool].material.id === job.functional_construction.base_material_id;
      const active =
        frame &&
        plan.layers[layer].lower_z_mm >= frame.region_z_mm[0] - epsilon &&
        plan.layers[layer].z_mm <= frame.region_z_mm[1] + epsilon;
      const expectedModel = frame
        ? isBase || active
        : job.assignments.some((row) => row.material.material_id === job.tools[tool].material.id);
      assert.equal(
        layers[layer][tool].model > 0,
        Boolean(expectedModel),
        "actual per-layer model tool disagrees with active functional Z or uniform assignment",
      );
    }
  }
  for (const tool of [0, 1]) {
    const accounting = report.manufacturing.verification.routing.tools[tool];
    assert.equal(totals[tool], accounting.model_deposition_segments);
    const filamentArea = Math.PI * (job.tools[tool].filament_diameter_mm / 2) ** 2;
    for (const kind of ["model", "waste"]) {
      near(
        filament[tool][kind],
        accounting[`${kind}_filament_length_mm`],
        `actual ${kind} filament`,
      );
      near(
        filament[tool][kind] * filamentArea,
        accounting[`${kind}_volume_mm3`],
        `actual ${kind} volume`,
      );
    }
  }
  return bytes;
}

function checkUniformPlan(plan, count) {
  assert.equal(plan.functional_material_frame, null, "uniform job retained functional geometry");
  assert.deepEqual(plan.material_frames, [], "uniform job acquired optical geometry");
  assert.deepEqual(plan.material_domains, [], "uniform job retained construction domains");
  assert.ok(plan.layers.length > 0);
  near(plan.layers[0].lower_z_mm, 0, "uniform stock bottom");
  near(plan.layers.at(-1).z_mm, 8, "uniform stock top");
  for (const layer of plan.layers) {
    near(layer.region_coverage.source_section_area_mm2, count * 200, "uniform source stock area");
    near(layer.region_coverage.section_area_mm2, count * 200, "uniform retained stock area");
    near(layer.region_coverage.surface_added_area_mm2, 0, "uniform added material");
    assert.ok(layer.paths.length > 0);
    for (const path of layer.paths) assert.equal(path.material_domain, null);
  }
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
  const unconfiguredProject = join(output, "before-face.spil");
  await copyFile(project, unconfiguredProject);
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
  assert.equal(job.tools[1].material.polymer_family, "PLA", "mixed-family smoke requires TPU/PLA");
  job.functional_construction = {
    occurrence_id: 1,
    definition_id: selected.reference.definition_id,
    source_hash: selected.provenance.source_hash,
    face_id: selected.face.face_id,
    material_id: job.tools[0].material.id,
    base_material_id: job.tools[1].material.id,
    interface_policy: "mechanical_capture",
    face_patch: null,
    minimum_pure_depth_mm: 5,
    minimum_solid_base_root_depth_mm: 0.8,
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
  const recoveryCases = [];
  async function compileCase(
    name,
    seed,
    capturedIntent,
    modelTools,
    extraArgs = [],
    normal = [0, -1, 0],
  ) {
    const caseProject = join(output, `${name}.spil`);
    const caseIntentFile = join(output, `${name}.intent.json`);
    const caseExport = join(output, `${name}-export`);
    const replayFile = join(output, `${name}.splr`);
    await copyFile(seed, caseProject);
    await save(`${name}.intent.json`, capturedIntent);
    const report = await invoke(name, [
      "edit",
      caseProject,
      ...extraArgs,
      "--intent",
      caseIntentFile,
      "--preflight",
      "--compile",
      "--verify",
      "--save",
      "--manufacturing-out",
      caseExport,
    ]);
    checkVerified(report, capturedIntent, modelTools);
    const applied = report.operations.find((op) => op.operation === "intent");
    assert.ok(applied, "missing full-intent transaction");
    assert.equal(applied.manufacturing.artifact, null, "intent edit retained an active artifact");
    assert.equal(report.manufacturing.preflight.disposition, "admitted");
    assert.equal(
      report.operations.find((op) => op.operation === "preflight").preflight_freshness,
      "current",
    );
    const casePlan = await checkExport(caseExport, report, capturedIntent);
    const replayReport = await invoke(`${name}-current-replay`, [
      "inspect",
      caseProject,
      "--verify",
      "--replay-out",
      replayFile,
    ]);
    checkVerified(replayReport, capturedIntent, modelTools);
    assert.notEqual(replayReport.hello.session_id, report.hello.session_id);
    assert.equal(replayReport.manufacturing.artifact.hash, report.manufacturing.artifact.hash);
    await checkReplay(replayFile, caseExport, replayReport, capturedIntent, casePlan);
    if (capturedIntent.printer.workflow.job.functional_construction) {
      evidence[name] = checkPlan(
        casePlan,
        capturedIntent.printer.workflow.job.functional_construction,
        fixtureStock,
        capturedIntent.recipe.infill_fraction,
        normal,
      );
    } else {
      checkUniformPlan(casePlan, capturedIntent.printer.workflow.job.assignments.length);
      evidence[name] = { routing: report.manufacturing.verification.routing };
    }
    recoveryCases.push({
      name,
      project: caseProject,
      intentFile: caseIntentFile,
      intent: capturedIntent,
      modelTools,
      exported: caseExport,
      replayFile,
      report,
      normal,
    });
    return report;
  }
  const fixtureStock = { min: [0, 0, 0], max: [20, 10, 8] };
  for (const base of [0, 1]) {
    const uniform = structuredClone(intent);
    uniform.printer.workflow.job.functional_construction = null;
    uniform.printer.workflow.job.assignments[0].material = {
      kind: "uniform",
      material_id: job.tools[base].material.id,
    };
    await compileCase(`setup-before-face-base-${base}`, unconfiguredProject, uniform, [base]);
  }
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
  changed.printer.workflow.job.functional_construction.minimum_solid_base_root_depth_mm = 1.2;
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

  const patchIntent = structuredClone(intent);
  patchIntent.printer.workflow.job.functional_construction.face_patch = {
    horizontal_mm: [2, 18],
    source_z_mm: [1.2, 6.8],
  };
  const patched = await compileCase("bounded-face-patch", project, patchIntent, [0, 1]);
  assert.notEqual(
    patched.manufacturing.artifact.hash,
    baselineHash,
    "patch edit retained the full-face artifact identity",
  );

  const negativeX = inspected.face_inspections.find((row) => {
    const plane = row.face.geometry.analytic;
    return (
      plane?.kind === "plane" &&
      plane.normal[0] === -1 &&
      plane.normal[1] === 0 &&
      plane.normal[2] === 0
    );
  });
  assert.ok(negativeX, "native CLI inspection has no -X planar face for 20 mm-deep stock");
  assert.equal(negativeX.provenance.source_hash, sourceHash);
  const sparseBase = structuredClone(intent);
  sparseBase.printer.workflow.job.functional_construction.face_id = negativeX.face.face_id;
  sparseBase.recipe.infill_fraction = 0.2;
  const solidBase = structuredClone(sparseBase);
  solidBase.recipe.infill_fraction = 1;
  const sparseReport = await compileCase(
    "full-height-negative-x-base-20",
    unconfiguredProject,
    sparseBase,
    [0, 1],
    [],
    [-1, 0, 0],
  );
  const solidReport = await compileCase(
    "full-height-negative-x-base-100",
    unconfiguredProject,
    solidBase,
    [0, 1],
    [],
    [-1, 0, 0],
  );
  const sparseEntry = recoveryCases.find(
    (entry) => entry.name === "full-height-negative-x-base-20",
  );
  const solidEntry = recoveryCases.find(
    (entry) => entry.name === "full-height-negative-x-base-100",
  );
  const sparsePlan = await json(join(sparseEntry.exported, "plan.json"));
  const solidPlan = await json(join(solidEntry.exported, "plan.json"));
  assert.equal(evidence[sparseEntry.name].stock_depth_mm, 20);
  assert.deepEqual(sparsePlan.functional_material_frame, solidPlan.functional_material_frame);
  assert.deepEqual(
    sparsePlan.material_domains,
    solidPlan.material_domains,
    "density changed geometry",
  );
  assert.deepEqual(
    sparsePlan.layers.map((layer) => [layer.lower_z_mm, layer.z_mm]),
    solidPlan.layers.map((layer) => [layer.lower_z_mm, layer.z_mm]),
    "density changed physical layer intervals",
  );
  assert.notEqual(
    sparseReport.manufacturing.artifact.hash,
    solidReport.manufacturing.artifact.hash,
  );
  const baseTool = job.tools.findIndex(
    (tool) => tool.material.id === job.functional_construction.base_material_id,
  );
  assert.ok(baseTool >= 0);
  const sparseAccounting = sparseReport.manufacturing.verification.routing.tools[baseTool];
  const solidAccounting = solidReport.manufacturing.verification.routing.tools[baseTool];
  assert.ok(
    sparseAccounting.model_volume_mm3 < solidAccounting.model_volume_mm3 - epsilon,
    "Base 20% must use less actual MODEL volume than identical Base 100% geometry",
  );
  assert.ok(
    sparseAccounting.model_filament_length_mm < solidAccounting.model_filament_length_mm - epsilon,
    "Base 20% must use less actual MODEL filament than Base 100%",
  );
  near(sparseAccounting.waste_volume_mm3, solidAccounting.waste_volume_mm3, "separate Base waste");
  near(
    sparseAccounting.waste_filament_length_mm,
    solidAccounting.waste_filament_length_mm,
    "separate Base waste filament",
  );
  evidence.sparse_base_comparison = {
    base_tool: baseTool,
    base_20: sparseAccounting,
    base_100: solidAccounting,
    same_geometry: true,
    lower_actual_base_model_deposition: true,
  };

  const sparsePad = structuredClone(sparseBase);
  sparsePad.printer.workflow.job.functional_construction.face_patch = {
    horizontal_mm: [1, 9],
    source_z_mm: [1.13, 6.87],
  };
  sparsePad.recipe.top_skin_axis = "x";
  await compileCase(
    "localized-negative-x-base-20",
    unconfiguredProject,
    sparsePad,
    [0, 1],
    [],
    [-1, 0, 0],
  );
  const raisedPad = structuredClone(sparsePad);
  raisedPad.printer.workflow.job.functional_construction.face_patch = {
    horizontal_mm: [0, 10],
    source_z_mm: [2.4, 8],
  };
  await compileCase(
    "raised-negative-x-base-20",
    unconfiguredProject,
    raisedPad,
    [0, 1],
    [],
    [-1, 0, 0],
  );

  for (const base of [0, 1]) {
    const uniform = structuredClone(intent);
    uniform.printer.workflow.job.functional_construction = null;
    uniform.printer.workflow.job.assignments[0].material = {
      kind: "uniform",
      material_id: job.tools[base].material.id,
    };
    const removed = await compileCase(`remove-to-uniform-base-${base}`, project, uniform, [base]);
    assert.notEqual(
      removed.manufacturing.artifact.hash,
      baselineHash,
      "Remove retained a functional manufacturing artifact",
    );
  }

  const separate = structuredClone(intent);
  const separateJob = separate.printer.workflow.job;
  separateJob.functional_construction = null;
  separateJob.assignments[0].material = { kind: "uniform", material_id: job.tools[0].material.id };
  const second = structuredClone(separateJob.assignments[0]);
  second.occurrence_id = 2;
  second.material.material_id = job.tools[1].material.id;
  second.work_envelope_mm.min[0] += 100;
  second.work_envelope_mm.max[0] += 100;
  separateJob.assignments.push(second);
  const secondPose = { ...pose, translation_mm: [120, 20, 0] };
  const disjoint = await compileCase(
    "two-disjoint-uniform-families",
    unconfiguredProject,
    separate,
    [0, 1],
    ["--add", selected.reference.definition_id, JSON.stringify(secondPose)],
  );
  assert.equal(disjoint.scene.occurrence_count, 2);
  assert.deepEqual(disjoint.occurrences.map((row) => row.occurrence_id).sort(), [1, 2]);
  assert.ok(
    disjoint.occurrences.every((row) => row.definition_id === selected.reference.definition_id),
  );
  assert.deepEqual(disjoint.occurrences.find((row) => row.occurrence_id === 1).pose, pose);
  assert.deepEqual(disjoint.occurrences.find((row) => row.occurrence_id === 2).pose, secondPose);

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

  for (const entry of recoveryCases) {
    await unlink(entry.intentFile);
    const recoveredExport = join(output, `${entry.name}-source-free-export`);
    const replayFile = join(output, `${entry.name}-source-free.splr`);
    const recovered = await invoke(`${entry.name}-source-free-replay`, [
      "inspect",
      entry.project,
      "--preflight",
      "--verify",
      "--manufacturing-out",
      recoveredExport,
      "--replay-out",
      replayFile,
    ]);
    assert.notEqual(recovered.hello.session_id, entry.report.hello.session_id);
    checkVerified(recovered, entry.intent, entry.modelTools);
    assert.equal(recovered.manufacturing.preflight.disposition, "admitted");
    assert.equal(recovered.manufacturing.preflight_freshness, "current");
    assert.equal(recovered.manufacturing.artifact.hash, entry.report.manufacturing.artifact.hash);
    const recoveredPlan = await checkExport(recoveredExport, recovered, entry.intent);
    const replayBytes = await checkReplay(
      replayFile,
      recoveredExport,
      recovered,
      entry.intent,
      recoveredPlan,
    );
    assert.deepEqual(
      replayBytes,
      await readFile(entry.replayFile),
      `${entry.name}: fresh source-free replay display changed`,
    );
    if (entry.intent.printer.workflow.job.functional_construction) {
      checkPlan(
        recoveredPlan,
        entry.intent.printer.workflow.job.functional_construction,
        fixtureStock,
        entry.intent.recipe.infill_fraction,
        entry.normal,
      );
    } else {
      checkUniformPlan(recoveredPlan, entry.intent.printer.workflow.job.assignments.length);
    }
    for (const artifact of artifacts) {
      assert.deepEqual(
        await readFile(join(recoveredExport, artifact)),
        await readFile(join(entry.exported, artifact)),
        `${entry.name}: source-free recovery changed immutable ${artifact}`,
      );
    }
    evidence[entry.name].source_free_fresh_replay = true;
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
