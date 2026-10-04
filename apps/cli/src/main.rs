// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::PROTOCOL_VERSION;
use spiling_engine_client::EngineClient;
use std::{env, path::PathBuf, process::ExitCode, time::Instant};

const USAGE: &str =
    "usage: spiling-cli [--engine PATH] [--protocol-version N] diagnose | triangle [--output FILE]";

struct Options {
    command: String,
    engine: PathBuf,
    protocol_version: u16,
    output: Option<PathBuf>,
}

fn options() -> Result<Option<Options>, Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let mut command = None;
    let mut engine = None;
    let mut protocol_version = PROTOCOL_VERSION;
    let mut output = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some("--engine") => {
                engine = Some(PathBuf::from(
                    args.next().ok_or("--engine requires a path")?,
                ))
            }
            Some("--protocol-version") => {
                protocol_version = args
                    .next()
                    .ok_or("--protocol-version requires a u16")?
                    .to_str()
                    .ok_or("invalid protocol version")?
                    .parse()?;
            }
            Some("--output") => {
                output = Some(PathBuf::from(
                    args.next().ok_or("--output requires a file")?,
                ))
            }
            Some("diagnose" | "triangle") if command.is_none() => {
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
        protocol_version,
        output,
    }))
}

async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut client = EngineClient::spawn(&options.engine, options.protocol_version).await?;
    let handshake_us = start.elapsed().as_micros();
    let operation: Result<serde_json::Value, Box<dyn std::error::Error>> = async {
        let transfer = Instant::now();
        if options.command == "diagnose" {
            client.ping().await?;
            Ok(serde_json::json!({
                "command": "diagnose", "engine": options.engine, "hello": client.hello(),
                "ping": "pong", "handshake_us": handshake_us,
                "ping_us": transfer.elapsed().as_micros()
            }))
        } else {
            let payload = client.triangle().await?;
            let transfer_us = transfer.elapsed().as_micros();
            if let Some(path) = &options.output {
                std::fs::write(path, &payload)?;
            }
            Ok(serde_json::json!({
                "command": "triangle", "synthetic": true, "bytes": payload.len(),
                "transfer_us": transfer_us, "handshake_us": handshake_us,
                "output": options.output, "pid": client.hello().pid
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

#[tokio::main]
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
