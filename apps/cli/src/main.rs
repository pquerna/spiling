// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{metadata, operation_view, rpc::RunDiagnosticRequest};
use spiling_engine_client::EngineClient;
use std::{env, path::PathBuf, process::ExitCode, time::Instant};
use uuid::Uuid;

const USAGE: &str = "usage: spiling-cli [--engine PATH] [--store PATH] diagnose | triangle [--output FILE] | job [--request-id UUID] [--chunks N] [--delay-ms N] [--chunk-bytes N] [--input-revision TEXT]";

struct Options {
    command: String,
    engine: PathBuf,
    store: Option<PathBuf>,
    request: RunDiagnosticRequest,
    output: Option<PathBuf>,
}

fn options() -> Result<Option<Options>, Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let mut command = None;
    let mut engine = None;
    let mut store = None;
    let mut request = RunDiagnosticRequest {
        parent: "diagnostics/default".into(),
        request_id: Uuid::new_v4().to_string(),
        chunk_count: 4,
        delay_ms: 250,
        chunk_bytes: 64,
        input_revision: "diagnostic".into(),
    };
    let mut output = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some("--engine") => {
                engine = Some(PathBuf::from(
                    args.next().ok_or("--engine requires a path")?,
                ))
            }
            Some("--store") => {
                store = Some(PathBuf::from(args.next().ok_or("--store requires a path")?))
            }
            Some(
                flag @ ("--request-id" | "--chunks" | "--delay-ms" | "--chunk-bytes"
                | "--input-revision"),
            ) => {
                let value = args.next().ok_or("missing job option value")?;
                let value = value.to_str().ok_or("job options must be UTF-8")?;
                match flag {
                    "--request-id" => request.request_id = value.into(),
                    "--chunks" => request.chunk_count = value.parse()?,
                    "--delay-ms" => request.delay_ms = value.parse()?,
                    "--chunk-bytes" => request.chunk_bytes = value.parse()?,
                    "--input-revision" => request.input_revision = value.into(),
                    _ => unreachable!(),
                }
            }
            Some("--output") => {
                output = Some(PathBuf::from(
                    args.next().ok_or("--output requires a file")?,
                ))
            }
            Some("diagnose" | "triangle" | "job") if command.is_none() => {
                command = arg.to_str().map(str::to_owned)
            }
            _ => return Err(format!("unrecognized argument {:?}; {USAGE}", arg).into()),
        }
    }
    let command = command.ok_or(USAGE)?;
    if command != "triangle" && output.is_some() {
        return Err("--output is only valid with triangle".into());
    }
    let engine = match engine {
        Some(path) => path,
        None => {
            env::current_exe()?.with_file_name(format!("spiling-engine{}", env::consts::EXE_SUFFIX))
        }
    };
    Ok(Some(Options {
        command,
        engine,
        store,
        request,
        output,
    }))
}

async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut client = if let Some(store) = &options.store {
        EngineClient::spawn_in(&options.engine, store).await?
    } else {
        EngineClient::spawn(&options.engine).await?
    };
    let handshake_us = start.elapsed().as_micros();
    let operation: Result<serde_json::Value, Box<dyn std::error::Error>> = async {
        let transfer = Instant::now();
        if options.command == "diagnose" {
            client.ping().await?;
            Ok(serde_json::json!({
                "command": "diagnose", "engine": options.engine.to_string_lossy(), "hello": client.hello(),
                "ping": "pong", "handshake_us": handshake_us,
                "ping_us": transfer.elapsed().as_micros()
            }))
        } else if options.command == "job" {
            let rpc = client.rpc();
            let accepted = rpc.run_diagnostic(options.request).await?;
            let mut watch = rpc.watch_operation(accepted.name).await?;
            let mut fetched = 0; let mut bytes = 0; let mut partial_updates = 0;
            while let Some(op) = watch.message().await? {
                let view = operation_view(&op).map_err(std::io::Error::other)?;
                eprintln!("{}", serde_json::json!({"event":"operation_progress", "operation":view}));
                let meta = metadata(&op).map_err(std::io::Error::other)?;
                for artifact in meta.outputs.iter().skip(fetched) { bytes += rpc.read_artifact(artifact).await?.len(); }
                fetched = meta.outputs.len();
                if !op.done && fetched>0 { partial_updates+=1; }
                if op.done {
                    if let Some(message) = &view.error_message { return Err(message.clone().into()); }
                    return Ok(serde_json::json!({"command":"job", "operation":view, "bytes":bytes, "partial_updates":partial_updates}));
                }
            }
            Err("operation watch ended without terminal result".into())
        } else {
            let payload = client.triangle().await?;
            let transfer_us = transfer.elapsed().as_micros();
            if let Some(path) = &options.output {
                std::fs::write(path, &payload)?;
            }
            Ok(serde_json::json!({
                "command": "triangle", "synthetic": true, "bytes": payload.len(),
                "transfer_us": transfer_us, "handshake_us": handshake_us,
                "output": options.output.as_ref().map(|path| path.to_string_lossy()), "pid": client.hello().pid
            }))
        }
    }
    .await;
    match operation {
        Ok(report) => {
            client.shutdown().await?;
            println!("{report}");
            Ok(())
        }
        Err(error) => {
            let _ = client.terminate().await;
            Err(error)
        }
    }
}

#[tokio::main(worker_threads = 2)]
async fn main() -> ExitCode {
    let result = match options() {
        Ok(Some(options)) => run(options).await,
        Ok(None) => {
            println!("{USAGE}");
            Ok(())
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"level":"error", "event":"cli_failure", "message":error.to_string()})
            );
            ExitCode::FAILURE
        }
    }
}
