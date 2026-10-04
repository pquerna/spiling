/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useEffect, useRef, useState } from "react";
import licenseDocument from "../../../LICENSE.md?raw";
import { PROTOCOL_VERSION } from "@spiling/protocol";
import { useWorkbench, type Phase } from "./useWorkbench.ts";

const licenseText = licenseDocument;
const assentKey = "spiling:license-assent:OSL-3.0:b0-1";
const phaseDescriptions: Record<Phase, string> = {
  "desktop-required": "Desktop runtime required",
  probing: "Checking WebGPU",
  starting: "Connecting to native engine",
  running: "Engine connected",
  stopped: "Session stopped",
  interrupted: "Engine interrupted",
  unsupported: "WebGPU unavailable",
  error: "Session could not start",
};

function LicenseDialog({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  return (
    <dialog ref={dialog} onCancel={onClose} onClose={onClose} className="license-dialog">
      <div className="section-heading">
        <div>
          <span className="eyebrow">Offline license</span>
          <h2>Open Software License 3.0</h2>
        </div>
        <button onClick={onClose} aria-label="Close license">
          Close
        </button>
      </div>
      <pre className="license-copy">{licenseText}</pre>
    </dialog>
  );
}

function Workbench() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const { state, execute } = useWorkbench(canvas);
  const running = state.phase === "running";
  const blocked = state.phase === "desktop-required";
  const unavailable = !running;
  const greeting = state.hello;
  return (
    <main className="workbench">
      <aside className="outline panel">
        <div className="panel-title">
          SESSION OUTLINE <span>01</span>
        </div>
        <div className="outline-item">
          <span className="shape-icon">△</span>
          <div>
            <strong>Diagnostic triangle</strong>
            <small>Engine-delivered display artifact</small>
          </div>
        </div>
        <dl className="compact-facts">
          <div>
            <dt>Artifact</dt>
            <dd>SPLT / schema 1</dd>
          </div>
          <div>
            <dt>Topology</dt>
            <dd>3 vertices · 1 triangle</dd>
          </div>
          <div>
            <dt>Coordinates</dt>
            <dd>Diagnostic, unitless</dd>
          </div>
        </dl>
        <div className="scope-note">
          <span className="eyebrow">B0 · Packaged bootstrap</span>
          <h3>A connection, not a CAD model.</h3>
          <p>
            This synthetic triangle exercises the native pipe, bounded binary decoder, and WebGPU
            upload. It is not a BREP artifact or a manufacturing plan.
          </p>
        </div>
        <div className="kernel-note">
          <span className="eyebrow">Kernel direction</span>
          <strong>
            {greeting?.kernel || "Monstertruck"} <span className="quiet">/ configured default</span>
          </strong>
          <p>
            The CAD kernel is not linked in B0. No import, modeling, slicing, or machine execution
            is available.
          </p>
        </div>
      </aside>

      <section className="viewport-panel panel" aria-label="Diagnostic viewport">
        <div className="viewport-toolbar">
          <div>
            <span className="eyebrow">DISPLAY / DIAGNOSTIC</span>
            <h1>Native transfer inspection</h1>
          </div>
          <span className={`gpu-tag ${state.gpu ? "ready" : ""}`}>WEBGPU ONLY</span>
        </div>
        <div className={`canvas-stage ${unavailable ? "inactive" : ""}`}>
          <canvas
            ref={canvas}
            data-testid="viewport"
            aria-label="WebGPU rendering of the engine diagnostic triangle"
          />
          {unavailable && (
            <div className="viewport-empty" role="status">
              <span className="empty-symbol">{state.phase === "interrupted" ? "!" : "△"}</span>
              <h2>{phaseDescriptions[state.phase]}</h2>
              <p>
                {blocked
                  ? "Open Spiling in its Tauri / CEF desktop shell. A web browser can inspect the interface, but cannot launch the native engine."
                  : state.phase === "unsupported"
                    ? "A working WebGPU device is required. The workbench never falls back to WebGL."
                    : state.phase === "probing"
                      ? "Requesting a GPU adapter and initializing the renderer before launching the engine."
                      : state.phase === "starting"
                        ? "Negotiating protocol identity and requesting the real engine payload."
                        : "No active display artifact. Start or restart the engine to establish a fresh session."}
              </p>
            </div>
          )}
          <div className="axis-key" aria-hidden="true">
            <span className="axis-x">X</span>
            <span className="axis-y">Y</span>
            <span>FRONT / XY</span>
          </div>
          <div className="artifact-caption">SYNTHETIC DIAGNOSTIC · NOT MANUFACTURING GEOMETRY</div>
        </div>
        <div className="transfer-strip" data-testid="transfer-metrics">
          <div>
            <span>Payload</span>
            <strong>{state.transfer ? `${state.transfer.bytes} B` : "—"}</strong>
          </div>
          <div>
            <span>Engine → UI</span>
            <strong>{state.transfer ? `${state.transfer.requestMs.toFixed(2)} ms` : "—"}</strong>
          </div>
          <div>
            <span>Decode</span>
            <strong>{state.transfer ? `${state.transfer.decodeMs.toFixed(2)} ms` : "—"}</strong>
          </div>
          <div>
            <span>First render</span>
            <strong>{state.transfer ? `${state.transfer.renderMs.toFixed(2)} ms` : "—"}</strong>
          </div>
        </div>
        <p className="measurement-note">
          Wall-clock request + IPC, validation, and first-render intervals. These are not GPU
          timestamp measurements or a zero-copy claim.
        </p>
      </section>

      <aside className="inspector panel">
        <div className="panel-title">
          ENGINE SUPERVISION <span>LIVE</span>
        </div>
        <div className={`status-card status-${state.phase}`}>
          <span className="status-dot" />
          <div>
            <strong data-testid="engine-status">{state.phase}</strong>
            <small>{phaseDescriptions[state.phase]}</small>
          </div>
        </div>
        {state.message && (
          <div className="diagnostic-message" role="alert">
            {state.message}
          </div>
        )}
        <div className="engine-actions">
          <button
            className="primary"
            data-testid="restart-engine"
            disabled={blocked || state.busy}
            onClick={() => {
              void execute(running ? "restart" : "start");
            }}
          >
            {state.busy ? "Working…" : running ? "Restart engine" : "Start engine"}
          </button>
          <button
            data-testid="stop-engine"
            disabled={!running || state.busy}
            onClick={() => {
              void execute("stop");
            }}
          >
            Graceful stop
          </button>
        </div>
        <dl className="session-facts">
          <div>
            <dt>{running ? "Engine PID" : "Last engine PID"}</dt>
            <dd data-testid="engine-pid">{greeting?.pid ?? "—"}</dd>
          </div>
          <div>
            <dt>Protocol</dt>
            <dd>
              {greeting?.protocol_version ?? PROTOCOL_VERSION}{" "}
              <span className="quiet">/ client {PROTOCOL_VERSION}</span>
            </dd>
          </div>
          <div>
            <dt>Engine build</dt>
            <dd>{greeting?.engine_build ?? "No handshake"}</dd>
          </div>
          <div>
            <dt>Workbench build</dt>
            <dd>{state.runtime?.app_build ?? "0.1.0"}</dd>
          </div>
          <div>
            <dt>Desktop runtime</dt>
            <dd>
              {state.runtime
                ? `${state.runtime.runtime} · API ${state.runtime.cef_api_version}`
                : "Not connected"}
            </dd>
          </div>
          <div>
            <dt>Geometry capabilities</dt>
            <dd>
              {greeting
                ? greeting.geometry_capabilities.length
                  ? greeting.geometry_capabilities.join(", ")
                  : "None — B0 diagnostic only"
                : "Not negotiated"}
            </dd>
          </div>
          <div>
            <dt>WebGPU adapter</dt>
            <dd>{state.gpu ?? "No active device"}</dd>
          </div>
        </dl>
        <div className="diagnostic-tools">
          <span className="eyebrow">Failure inspection</span>
          <p>
            Intentionally terminate the sidecar to inspect interruption and recovery. Status polling
            also detects an external process crash.
          </p>
          <button
            className="danger"
            data-testid="interrupt-engine"
            disabled={!running || state.busy}
            onClick={() => {
              void execute("interrupt");
            }}
          >
            Interrupt engine
          </button>
          <button
            className="subtle"
            disabled={!running || state.busy}
            onClick={() => {
              void execute("transfer");
            }}
          >
            Measure another transfer
          </button>
        </div>
      </aside>
    </main>
  );
}

export function App() {
  const [accepted, setAccepted] = useState(() => {
    try {
      return localStorage.getItem(assentKey) === "accepted";
    } catch {
      return false;
    }
  });
  const [checked, setChecked] = useState(false);
  const [showLicense, setShowLicense] = useState(false);
  const [storageNotice, setStorageNotice] = useState("");
  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark">S</span>
          <div>
            <strong>SPILING</strong>
            <span>Manufacturing workbench</span>
          </div>
        </div>
        <div className="milestone">
          <span className="milestone-dot" />
          B0 <span className="quiet">/ packaged bootstrap</span>
        </div>
      </header>
      {accepted ? (
        <Workbench />
      ) : (
        <main className="license-gate" data-testid="license-gate">
          <section className="license-welcome">
            <span className="eyebrow">A NATIVE WORKBENCH, BUILT IN THE OPEN</span>
            <h1>
              Inspect the path
              <br />
              from engine to display.
            </h1>
            <p>
              Spiling B0 establishes the desktop runtime, a supervised Rust engine, and a real
              WebGPU diagnostic viewport. Manufacturing authoring is not available in this
              milestone.
            </p>
            <div className="welcome-detail">
              <span>01</span>
              <div>
                <strong>Explicit license assent</strong>
                <small>
                  Review the full license before entering. No engine launches until you accept and
                  WebGPU initialization succeeds.
                </small>
              </div>
            </div>
          </section>
          <section className="license-review panel">
            <div className="section-heading">
              <div>
                <span className="eyebrow">OSL-3.0 / AVAILABLE OFFLINE</span>
                <h2>Open Software License 3.0</h2>
              </div>
            </div>
            <pre className="license-copy">{licenseText}</pre>
            <label className="assent-control">
              <input
                type="checkbox"
                data-testid="license-assent"
                checked={checked}
                onChange={(event) => setChecked(event.target.checked)}
              />
              <span>I have read and expressly accept the Open Software License version 3.0.</span>
            </label>
            <p className="assent-note">
              Your versioned acknowledgement is stored on this device only. No telemetry or network
              request is made.
            </p>
            <button
              className="primary enter-workbench"
              data-testid="accept-license"
              disabled={!checked}
              onClick={() => {
                if (!checked) return;
                try {
                  localStorage.setItem(assentKey, "accepted");
                } catch {
                  setStorageNotice(
                    "License acknowledgement applies to this session only because local storage is unavailable.",
                  );
                }
                setAccepted(true);
              }}
            >
              Accept license & enter workbench <span aria-hidden="true">→</span>
            </button>
          </section>
        </main>
      )}
      {storageNotice && (
        <p className="storage-notice" role="status">
          {storageNotice}
        </p>
      )}
      <footer className="app-footer">
        <span>
          © 2026 Spiling contributors <span className="footer-divider">/</span> Licensed under the
          Open Software License version 3.0
        </span>
        <div>
          <button className="text-button" onClick={() => setShowLicense(true)}>
            Read OSL-3.0
          </button>
          {accepted && (
            <button
              className="text-button"
              onClick={() => {
                try {
                  localStorage.removeItem(assentKey);
                } catch {
                  /* In-memory assent is still revoked. */
                }
                setChecked(false);
                setAccepted(false);
              }}
            >
              Review assent
            </button>
          )}
        </div>
      </footer>
      {showLicense && <LicenseDialog onClose={() => setShowLicense(false)} />}
    </div>
  );
}
