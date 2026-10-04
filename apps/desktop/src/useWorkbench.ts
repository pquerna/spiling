/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { decodeTriangle, PROTOCOL_VERSION, type Hello } from "@spiling/protocol";
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
  });
  const mounted = useRef(false);
  const generation = useRef(0);
  const inCommand = useRef(false);
  const polling = useRef(false);
  const viewport = useRef<DiagnosticViewport | null>(null);

  const disposeViewport = useCallback(async () => {
    const previous = viewport.current;
    viewport.current = null;
    await previous?.dispose();
  }, []);

  const execute = useCallback(
    async (action: Action) => {
      if (!native.available() || inCommand.current || !mounted.current) return;
      inCommand.current = true;
      const token = ++generation.current;
      const current = () => mounted.current && generation.current === token;
      setState((previous) => ({ ...previous, busy: true, message: "" }));
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
            viewport.current = null;
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
          if (hello.protocol_version !== PROTOCOL_VERSION) {
            throw new Error(
              `Upgrade required: client protocol ${PROTOCOL_VERSION}, engine protocol ${hello.protocol_version}.`,
            );
          }
          setState((previous) => ({ ...previous, hello }));
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
        const binary = await native.triangle();
        const received = performance.now();
        if (!current()) return;
        const triangle = decodeTriangle(binary);
        const decoded = performance.now();
        await activeViewport.showTriangle(triangle);
        const rendered = performance.now();
        if (!current()) return;
        setState((previous) => ({
          ...previous,
          phase: "running",
          message: "",
          transfer: {
            bytes: triangle.byteLength,
            requestMs: received - began,
            decodeMs: decoded - received,
            renderMs: rendered - decoded,
            totalMs: rendered - began,
          },
        }));
      } catch (error) {
        if (!current()) return;
        let message = errorText(error);
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
            await disposeViewport();
            if (!active || !mounted.current || generation.current !== stoppedGeneration) return;
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
          }
        } catch (error) {
          if (active && mounted.current && token === generation.current) {
            const failedGeneration = ++generation.current;
            await disposeViewport().catch(() => undefined);
            if (active && mounted.current && generation.current === failedGeneration)
              setState((previous) => ({
                ...previous,
                phase: "interrupted",
                gpu: null,
                transfer: null,
                message: `Engine supervision failed: ${errorText(error)}`,
              }));
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

  return { state, execute };
}
