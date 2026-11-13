/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
// Native X11 opening benchmark. No synthetic scene, partial-open shortcut or source mutation.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { availableParallelism, cpus, totalmem } from "node:os";

const root = resolve(import.meta.dirname, "..");
const args = process.argv.slice(2);
const options = { samples: 3, warmups: 1, timeout: 120_000 };
for (let i = 0; i < args.length; i++) {
  const key = args[i];
  assert.ok(
    [
      "--project",
      "--source",
      "--interaction",
      "--user-data",
      "--output",
      "--bin-dir",
      "--samples",
      "--warmups",
      "--timeout",
    ].includes(key),
    `Unknown argument ${key}`,
  );
  assert.ok(args[i + 1] && !args[i + 1].startsWith("--"), `${key} requires a value`);
  const value = args[++i];
  options[key.slice(2)] = ["samples", "warmups", "timeout"].includes(key.slice(2))
    ? Number(value)
    : key === "--interaction" ? value : resolve(root, value);
}
assert.equal(
  process.platform,
  "linux",
  "This real-window automation adapter currently requires Linux X11",
);
assert.ok(process.env.DISPLAY, "An accessible X11 DISPLAY is required");
assert.ok(
  Boolean(options.project) !== Boolean(options.source) && options["user-data"] && options.output,
  "usage: pnpm bench:open (--project FILE.spil | --source MODEL) --user-data EXISTING_XDG_DATA --output NEW_DIRECTORY [--bin-dir DIR] [--samples 3] [--warmups 1]",
);
assert.ok(Number.isInteger(options.samples) && options.samples > 0 && options.samples <= 30);
assert.ok(Number.isInteger(options.warmups) && options.warmups >= 0 && options.warmups <= 10);
assert.ok(
  Number.isInteger(options.timeout) && options.timeout >= 1000 && options.timeout <= 600_000,
);
const bin =
  options["bin-dir"] ?? resolve(root, process.env.CARGO_TARGET_DIR ?? "target", "profiling");
const preference = await readFile(
  join(options["user-data"], "org.spiling.workbench", "app.ron"),
  "utf8",
);
assert.match(
  preference,
  /accepted_osl3:true/,
  "Use isolated native preferences with the license already accepted; the benchmark does not accept it for you",
);
const digest = async (path) =>
  createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
const input = options.source ?? options.project;
const importing = Boolean(options.source);
const failureInteraction = options.interaction === "failure";
assert.ok(!options.interaction || failureInteraction && importing,"Only --source --interaction failure is supported");
const identity = {
  preferences_sha256: createHash("sha256").update(preference).digest("hex"),
  ...(importing ? { source_sha256: await digest(input) } : { project_sha256: await digest(input) }),
  workbench_sha256: await digest(join(bin, "spiling-workbench")),
  engine_sha256: await digest(join(bin, "spiling-engine")),
};
const readOptional = async (path) =>
  readFile(path, "utf8").then(
    (s) => s.trim(),
    () => null,
  );
const cgroup = (await readOptional("/proc/self/cgroup"))?.match(/^0::(.+)$/m)?.[1];
const resource_limits = Object.fromEntries(
  await Promise.all(
    ["cpu.max", "cpuset.cpus.effective", "memory.max", "memory.swap.max", "pids.max"].map(
      async (name) => [
        name,
        cgroup ? await readOptional(join("/sys/fs/cgroup", cgroup, name)) : null,
      ],
    ),
  ),
);
const environment = {
  available_parallelism: availableParallelism(),
  cpu_model: cpus()[0]?.model,
  total_memory_bytes: totalmem(),
  display: process.env.DISPLAY,
  cpu_affinity: (await readFile("/proc/self/status", "utf8")).match(
    /^Cpus_allowed_list:\s*(.+)$/m,
  )?.[1],
  resource_limits,
  rayon_threads: process.env.RAYON_NUM_THREADS ?? null,
  wgpu_backend: process.env.WGPU_BACKEND ?? null,
  wgpu_adapter_name: process.env.WGPU_ADAPTER_NAME ?? null,
  vulkan_driver_files: process.env.VK_DRIVER_FILES ?? null,
};
await mkdir(options.output); // Exclusive evidence directory: never replace a previous run.
const invoke = promisify(execFile);
for (let index = -options.warmups; index < options.samples; index++) {
  const prefix = index < 0 ? `warmup-${index + options.warmups}` : `sample-${index}`;
  const sampleData = join(options.output, `${prefix}.data`);
  const samplePreferences = join(sampleData, "org.spiling.workbench");
  await mkdir(samplePreferences, { recursive: true });
  await writeFile(join(samplePreferences, "app.ron"), preference, { flag: "wx" });
  const stages = [];
  let interaction=Promise.resolve();
  let inspected=false,selected=false,located=false;
  const controls=new Map();
  const click=(row)=>invoke("python3",[join(root,"tools/native-window.py"),"click",String(applicationPid),String(row.x),String(row.y)],{timeout:10_000});
  let stdout = "",
    stderr = "",
    pending = "",
    bytes = 0,
    stopping = false;
  let applicationPid, enginePid, boundError, processMemory;
  const started = performance.now();
  const child = spawn(join(bin, "spiling-workbench"), importing ? ["--import", input] : ["--read-only", "--open", input], {
    cwd: root,
    detached: true,
    env: { ...process.env, XDG_DATA_HOME: sampleData, SPILING_OPEN_TRACE: "1", ...(failureInteraction ? {SPILING_IMPORT_INTERACTION_TRACE:"1"}:{}) },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const kill = (reason) => {
    boundError ??= reason;
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {}
  };
  const interrupted = () => kill("native benchmark interrupted");
  process.once("SIGINT", interrupted);
  process.once("SIGTERM", interrupted);
  const timer = setTimeout(() => {
    void invoke("python3",[join(root,"tools/native-window.py"),"capture",String(applicationPid),join(options.output,`${prefix}.timeout.png`)],{timeout:5_000}).catch(()=>{}).finally(()=>kill("native opening/cleanup timed out"));
  }, options.timeout);
  const stop = async () => {
    try {
      const peak = async (pid) => {
        const status = await readFile(`/proc/${pid}/status`, "utf8");
        const bytes = Number(status.match(/^VmHWM:\s*(\d+) kB$/m)?.[1]) * 1024;
        assert.ok(Number.isSafeInteger(bytes) && bytes > 0, "Missing peak process RSS");
        return bytes;
      };
      processMemory = {
        workbench_peak_rss_bytes: await peak(applicationPid),
        engine_peak_rss_bytes: await peak(enginePid),
      };
      await invoke(
        "python3",
        [
          join(root, "tools/native-window.py"),
          "capture",
          String(applicationPid),
          join(options.output, `${prefix}.png`),
        ],
        { timeout: 10_000 },
      );
      await invoke(
        "python3",
        [join(root, "tools/native-window.py"), "close", String(applicationPid)],
        { timeout: 10_000 },
      );
    } catch (error) {
      kill(error.message);
    }
  };
  child.stdout.setEncoding("utf8").on("data", (data) => {
    bytes += Buffer.byteLength(data);
    if (bytes > 4 * 1024 * 1024) {
      kill("native output exceeded 4 MiB");
      return;
    }
    stdout += data;
  });
  child.stderr.setEncoding("utf8").on("data", (data) => {
    bytes += Buffer.byteLength(data);
    if (bytes > 4 * 1024 * 1024) {
      kill("native output exceeded 4 MiB");
      return;
    }
    stderr += data;
    pending += data;
    let end;
    while ((end = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, end);
      pending = pending.slice(end + 1);
      const marker = line.indexOf("SPILING_OPEN_TRACE ");
      if (marker < 0) continue;
      let row;
      try {
        row = JSON.parse(line.slice(marker + "SPILING_OPEN_TRACE ".length));
      } catch {
        kill("Malformed native trace record");
        return;
      }
      if (
        !row ||
        typeof row !== "object" ||
        row.schema !== 1 ||
        !["engine", "workbench"].includes(row.process) ||
        !Number.isSafeInteger(row.pid) ||
        row.pid <= 0 ||
        !Number.isSafeInteger(row.at_us) ||
        row.at_us < 0 ||
        !Number.isSafeInteger(row.elapsed_us) ||
        row.elapsed_us < 0 ||
        typeof row.stage !== "string"
      ) {
        kill("Invalid native trace record");
        return;
      }
      stages.push(row);
      if (row.process === "workbench") applicationPid = row.pid;
      if (row.process === "engine") enginePid = row.pid;
      if (failureInteraction && ["import_failure_issue","import_failure_inspect","import_failure_locate"].includes(row.stage)) {
        if (![row.x,row.y].every(Number.isFinite)) {kill("Invalid native control location");return;}
        controls.set(row.stage,row);
        if (row.stage === "import_failure_inspect" && !inspected) {inspected=true;interaction=interaction.then(()=>click(row));}
        if (row.stage === "import_failure_locate" && !selected) {selected=true;interaction=interaction.then(()=>click(controls.get("import_failure_issue")));}
        interaction.catch((e)=>kill(e.message));
      }
      if (failureInteraction && row.stage === "import_failure_selected" && !located) {
        located=true;
        interaction=interaction.then(()=>click(controls.get("import_failure_locate")));
        interaction.catch((e)=>kill(e.message));
      }
      if (failureInteraction && row.stage === "import_failure_located" && !stopping) {
        stopping=true;
        void interaction.then(stop).catch((e)=>kill(e.message));
      }
      if (row.stage === "discard_confirmation_frame" && importing && stopping) {
        void invoke("python3", [join(root, "tools/native-window.py"), "discard", String(applicationPid)], { timeout: 10_000 }).catch((e) => kill(e.message));
      }
      if (row.stage === "source_frame_readback" && !failureInteraction && !stopping) {
        stopping = true;
        void stop();
      }
    }
  });
  let status;
  try {
    status = await new Promise((done, fail) => {
      child.once("error", fail);
      child.once("close", (code, signal) => done({ code, signal }));
    });
  } finally {
    clearTimeout(timer);
    process.removeListener("SIGINT", interrupted);
    process.removeListener("SIGTERM", interrupted);
  }
  await writeFile(join(options.output, `${prefix}.stderr.txt`), stderr, { flag: "wx" });
  await writeFile(join(options.output, `${prefix}.stdout.txt`), stdout, { flag: "wx" });
  assert.ok(!boundError, boundError);
  assert.deepEqual(status, { code: 0, signal: null }, "Native window must close successfully");
  assert.ok(applicationPid && enginePid && applicationPid !== enginePid);
  assert.throws(
    () => process.kill(enginePid, 0),
    "Supervised engine must have exited before the window process returns",
  );
  const milestone = (name) => {
    const rows = stages.filter((r) => r.stage === name);
    assert.equal(rows.length, 1, `Expected one ${name} milestone`);
    return rows[0].at_us;
  };
  const request = milestone(importing ? "import_requested" : "open_requested");
  if (failureInteraction) {
    assert.ok(milestone("import_failed")>request && !stages.some((r)=>r.stage==="import_published"));
    assert.ok(milestone("source_drawn")<=milestone("import_failure_located"));
    assert.ok(milestone("import_failure_selected")<milestone("import_failure_located"));
  } else assert.ok(milestone("source_drawn") <= milestone("source_frame_readback"), "Readback must follow actual source drawing");
  const result = {
    benchmark: failureInteraction ? "native-import-failure-v1" : importing ? "native-import-v1" : "native-open-v1",
    sample: index,
    samples: options.samples,
    warmups: options.warmups,
    software_only: true,
    cache_state: "filesystem uncontrolled; separate fresh process each sample",
    ...identity,
    environment,
    process_memory: processMemory,
    graphics: stderr.split("\n").find((line) => line.startsWith("Spiling native graphics:")),
    kernel: stages.find((r) => r.process === "engine")?.kernel,
    engine_build: stderr.match(/Spiling native host: connected pid=\d+ build=(\S+)/)?.[1],
    first_frame_readback_us: milestone("first_frame_readback"),
    ...(failureInteraction ? {import_to_failure_us:milestone("import_failed")-request,import_to_diagnostic_interaction_us:milestone("import_failure_located")-request} : importing ? {
      import_to_published_us: milestone("import_published") - request,
      import_to_source_readback_us: milestone("source_frame_readback") - request,
    } : {
      open_to_published_us: milestone("open_published") - request,
      open_to_source_readback_us: milestone("source_frame_readback") - request,
    }),
    ...(failureInteraction ? {} : {launch_to_source_readback_us:milestone("source_frame_readback")}),
    process_including_cleanup_ms: performance.now() - started,
    stages,
  };
  assert.ok(
    result.graphics && result.engine_build && result.kernel,
    "Missing observed graphics/producer identity",
  );
  assert.equal(
    await digest(input),
    importing ? identity.source_sha256 : identity.project_sha256,
    "Read-only opening changed the project file",
  );
  await writeFile(join(options.output, `${prefix}.json`), JSON.stringify(result, null, 2), {
    flag: "wx",
  });
  if (index >= 0) console.log(JSON.stringify(result));
}
