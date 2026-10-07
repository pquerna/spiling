/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { decodeTriangle, type Hello, type OperationView } from "@spiling/protocol";
import {
  createDiagnosticViewport,
  WebGPUUnavailableError,
  type DiagnosticViewport,
} from "@spiling/viewport";
import { native, type RuntimeInfo } from "./native.ts";

export type Phase =
  | "desktop-required"
  | "probing"
  | "starting"
  | "running"
  | "stopped"
  | "interrupted"
  | "unsupported"
  | "error";
export interface Transfer {
  bytes: number;
  requestMs: number;
  decodeMs: number;
  renderMs: number;
  totalMs: number;
}
export interface WorkbenchState {
  phase: Phase;
  busy: boolean;
  message: string;
  gpu: string | null;
  runtime: RuntimeInfo | null;
  hello: Hello | null;
  transfer: Transfer | null;
  operation: OperationView | null;
}
type Action = "start" | "restart" | "stop" | "interrupt" | "transfer";
const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

export function useWorkbench(canvas: RefObject<HTMLCanvasElement | null>) {
  const [state, setState] = useState<WorkbenchState>({
    phase: native.available() ? "probing" : "desktop-required",
    busy: false,
    message: "",
    gpu: null,
    runtime: null,
    hello: null,
    transfer: null,
    operation: null,
  });
  const activeOperation = useRef<string | null>(null);
  const mounted = useRef(false);
  const generation = useRef(0);
  const inCommand = useRef(false);
  const polling = useRef(false);
  const viewport = useRef<DiagnosticViewport | null>(null);
  const viewportDisposal = useRef<Promise<void> | null>(null);

  const disposeViewport = useCallback(() => {
    const previous = viewport.current;
    viewport.current = null;
    if (previous) {
      const pending = (viewportDisposal.current ?? Promise.resolve())
        .then(() => previous.dispose())
        .finally(() => {
          if (viewportDisposal.current === pending) viewportDisposal.current = null;
        });
      viewportDisposal.current = pending;
    }
    return viewportDisposal.current ?? Promise.resolve();
  }, []);

  const execute = useCallback(
    async (action: Action) => {
      if (!native.available() || inCommand.current || !mounted.current) return;
      inCommand.current = true;
      const token = ++generation.current;
      const current = () => mounted.current && generation.current === token;
      setState((previous) => ({ ...previous, busy: true, message: "" }));
      let engineEstablished = action === "transfer";
      try {
        if (action === "start" || action === "restart") {
          await disposeViewport();
          if (!current()) return;
          setState((previous) => ({
            ...previous,
            phase: "probing",
            gpu: null,
            transfer: null,
            hello: null,
          }));
          if (!canvas.current) throw new Error("Viewport canvas is missing.");
          let next: DiagnosticViewport | undefined;
          next = await createDiagnosticViewport(canvas.current, (message) => {
            if (
              !mounted.current ||
              (next ? viewport.current !== next : generation.current !== token)
            )
              return;
            const lostGeneration = ++generation.current;
            void disposeViewport().catch(() => undefined);
            setState((previous) => ({
              ...previous,
              phase: "unsupported",
              gpu: null,
              transfer: null,
              message,
            }));
            // Device loss tears down the renderer and the engine, not just its badge.
            void native.stop().catch((error: unknown) => {
              if (mounted.current && generation.current === lostGeneration) {
                setState((previous) => ({
                  ...previous,
                  message: `${message} Engine cleanup failed: ${errorText(error)}`,
                }));
              }
            });
          });
          if (!current()) {
            await next.dispose();
            return;
          }
          viewport.current = next;
          setState((previous) => ({ ...previous, gpu: next.adapter, phase: "starting" }));
          const runtime = await native.runtime();
          if (!current()) return;
          setState((previous) => ({ ...previous, runtime }));
          // Never launch a sidecar until BOTH the WebGPU probe and renderer init succeed.
          const hello = await (action === "restart" ? native.restart() : native.start());
          if (!current()) return;
          engineEstablished = true;
          setState((previous) => ({ ...previous, hello, operation: null }));
        }
        if (action === "stop" || action === "interrupt") {
          await (action === "stop" ? native.stop() : native.interrupt());
          if (!current()) return;
          await disposeViewport();
          if (!current()) return;
          setState((previous) => ({
            ...previous,
            phase: action === "stop" ? "stopped" : "interrupted",
            gpu: null,
            transfer: null,
            message:
              action === "stop"
                ? "Engine shut down gracefully. Start a new session when ready."
                : "Diagnostic interruption requested. The engine process was terminated; restart opens a fresh session.",
          }));
          return;
        }
        const activeViewport = viewport.current;
        if (!activeViewport)
          throw new Error("A ready WebGPU viewport is required before requesting display data.");
        const began = performance.now();
        let operation = await native.runDiagnostic(crypto.randomUUID());
        activeOperation.current = operation.name;
        let displayed = 0;
        while (current()) {
          setState((previous) => ({ ...previous, operation }));
          if (operation.outputs.length > displayed) {
            // Pull one complete output at a time; fetch again only after GPU consumption.
            const artifact = operation.outputs[operation.outputs.length - 1];
            if (!artifact) throw new Error("Published diagnostic artifact missing.");
            const binary = await native.readArtifact(artifact.name);
            const received = performance.now();
            if (!current()) return;
            const triangle = decodeTriangle(binary);
            const decoded = performance.now();
            await activeViewport.showTriangle(triangle);
            const rendered = performance.now();
            if (!current()) return;
            displayed = operation.outputs.length;
            setState((previous) => ({
              ...previous,
              phase: "running",
              transfer: {
                bytes: triangle.byteLength,
                requestMs: received - began,
                decodeMs: decoded - received,
                renderMs: rendered - decoded,
                totalMs: rendered - began,
              },
            }));
          }
          if (operation.done) {
            if (operation.error_code !== null && operation.state !== "cancelled")
              throw new Error(operation.error_message ?? "Diagnostic operation failed.");
            if (current())
              setState((previous) => ({
                ...previous,
                phase: "running",
                message:
                  operation.state === "cancelled"
                    ? "Diagnostic cancelled. Completed partial output remains inspectable."
                    : "",
              }));
            break;
          }
          await new Promise((resolve) => setTimeout(resolve, 100));
          if (!current()) return;
          operation = await native.getOperation(operation.name);
        }
      } catch (error) {
        if (!current()) return;
        let message = errorText(error);
        if (engineEstablished && viewport.current && !(error instanceof WebGPUUnavailableError)) {
          // Domain/RPC failure does not own the engine. OS status polling detects a dead child.
          setState((previous) => ({
            ...previous,
            phase: "running",
            message: `Diagnostic failed: ${message}`,
          }));
          return;
        }
        try {
          await native.stop();
        } catch (cleanupError) {
          message += ` Engine cleanup failed: ${errorText(cleanupError)}`;
        }
        await disposeViewport().catch((cleanupError: unknown) => {
          message += ` GPU cleanup failed: ${errorText(cleanupError)}`;
        });
        if (!current()) return;
        setState((previous) => ({
          ...previous,
          phase: error instanceof WebGPUUnavailableError ? "unsupported" : "error",
          gpu: null,
          transfer: null,
          message,
        }));
      } finally {
        activeOperation.current = null;
        inCommand.current = false;
        if (mounted.current) setState((previous) => ({ ...previous, busy: false }));
      }
    },
    [canvas, disposeViewport],
  );

  useEffect(() => {
    mounted.current = true;
    void execute("start");
    return () => {
      mounted.current = false;
      ++generation.current;
      void disposeViewport().catch(() => undefined);
      if (native.available()) void native.stop().catch(() => undefined);
    };
  }, [execute, disposeViewport]);

  useEffect(() => {
    if (state.phase !== "running" || state.busy) return;
    let active = true;
    let timer: number;
    const token = generation.current;
    const poll = async () => {
      if (!active) return;
      if (!polling.current && !inCommand.current) {
        polling.current = true;
        try {
          const status = await native.status();
          if (!active || !mounted.current || token !== generation.current) return;
          if (status.state !== "running") {
            const stoppedGeneration = ++generation.current;
            setState((previous) => ({
              ...previous,
              phase: status.state,
              gpu: null,
              transfer: null,
              message:
                status.message ||
                (status.state === "interrupted"
                  ? "The engine exited unexpectedly. Restart to create a new session."
                  : "The engine is stopped."),
            }));
            await disposeViewport().catch((error: unknown) => {
              if (mounted.current && generation.current === stoppedGeneration)
                setState((previous) => ({
                  ...previous,
                  message: `${previous.message} GPU cleanup failed: ${errorText(error)}`,
                }));
            });
          }
        } catch (error) {
          if (active && mounted.current && token === generation.current) {
            const failedGeneration = ++generation.current;
            setState((previous) => ({
              ...previous,
              phase: "interrupted",
              gpu: null,
              transfer: null,
              message: `Engine supervision failed: ${errorText(error)}`,
            }));
            await disposeViewport().catch((cleanupError: unknown) => {
              if (mounted.current && generation.current === failedGeneration)
                setState((previous) => ({
                  ...previous,
                  message: `${previous.message} GPU cleanup failed: ${errorText(cleanupError)}`,
                }));
            });
          }
        } finally {
          polling.current = false;
        }
      }
      if (active)
        timer = window.setTimeout(() => {
          void poll();
        }, 750);
    };
    timer = window.setTimeout(() => {
      void poll();
    }, 750);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [state.phase, state.busy, disposeViewport]);

  const cancelOperation = useCallback(async () => {
    const name = activeOperation.current;
    if (!name) return;
    try {
      await native.cancelOperation(name);
    } catch (error) {
      if (mounted.current && activeOperation.current === name)
        setState((previous) => ({ ...previous, message: errorText(error) }));
    }
  }, []);
  return { state, execute, cancelOperation };
}
