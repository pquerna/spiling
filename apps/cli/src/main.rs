// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod geometry;
mod output;
mod project;
use spiling_contracts::{PROTOCOL_VERSION, geometry::PlaneMm};
use spiling_engine_client::EngineClient;
use std::{env, ffi::OsString, io::Write, path::PathBuf, process::ExitCode, time::Instant};
type Error = Box<dyn std::error::Error>;
const USAGE: &str = "usage: spiling-cli [--engine PATH] [--protocol-version N] diagnose | triangle [--output FILE]\n       spiling-cli geometry (--scene SCENE_JSON | --source PATH...) [--section ox,oy,oz:nx,ny,nz] [--out DIRECTORY] [--engine PATH]\n       spiling-cli project (create|edit|save|open|inspect|recover) PATH [--import STEP] [--add DEFINITION_ID POSE_JSON] [--pose OCCURRENCE_ID POSE_JSON] [--remove ID] [--undo] [--redo] [--intent JSON_FILE] [--compile] [--verify] [--manufacturing-out NEW_DIRECTORY] [--save] [--save-as PATH] [--read-only] [--inspect-face OCCURRENCE_ID FACE_ID] [--section PLANE] [--out NEW_DIRECTORY] [--engine PATH]\nProject operations execute in flag order. Manufacturing output is SOFTWARE ONLY, NOT MACHINE READY; --intent reads bounded inert printer/recipe JSON, never executes TypeScript. Inspect/read-only permits --verify and --manufacturing-out, not --intent or --compile.";

struct Options {
    command: Command,
    engine: PathBuf,
    protocol_version: u16,
}
enum Command {
    Diagnose,
    Triangle {
        output: Option<PathBuf>,
    },
    Geometry {
        input: geometry::SceneInput,
        plane: Option<PlaneMm>,
        output: Option<PathBuf>,
    },
    Project {
        workflow: project::Workflow,
        plane: Option<PlaneMm>,
        output: Option<PathBuf>,
        manufacturing_output: Option<PathBuf>,
    },
}
fn set_once<T>(target: &mut Option<T>, value: T, name: &str) -> Result<(), Error> {
    if target.replace(value).is_some() {
        return Err(format!("duplicate {name}").into());
    }
    Ok(())
}
fn parse_options(args: impl IntoIterator<Item = OsString>) -> Result<Option<Options>, Error> {
    let mut args = args.into_iter();
    let mut command = None;
    let mut engine = None;
    let mut version = None;
    let mut triangle_output = None;
    let mut output = None;
    let mut recipe = None;
    let mut sources = Vec::new();
    let mut plane = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some("--engine") => set_once(
                &mut engine,
                PathBuf::from(args.next().ok_or("--engine requires a path")?),
                "--engine",
            )?,
            Some("--protocol-version") => set_once(
                &mut version,
                args.next()
                    .ok_or("--protocol-version requires a u16")?
                    .to_str()
                    .ok_or("invalid protocol version")?
                    .parse::<u16>()?,
                "--protocol-version",
            )?,
            Some("--output") => set_once(
                &mut triangle_output,
                PathBuf::from(args.next().ok_or("--output requires a path")?),
                "--output",
            )?,
            Some("--out") => set_once(
                &mut output,
                PathBuf::from(args.next().ok_or("--out requires a path")?),
                "--out",
            )?,
            Some("--scene") => set_once(
                &mut recipe,
                PathBuf::from(args.next().ok_or("--scene requires a path")?),
                "--scene",
            )?,
            Some("--source") => sources.push(PathBuf::from(
                args.next().ok_or("--source requires a native path")?,
            )),
            Some("--section") => set_once(
                &mut plane,
                geometry::parse_plane(
                    args.next()
                        .ok_or("--section requires a plane")?
                        .to_str()
                        .ok_or("section must be UTF-8")?,
                )?,
                "--section",
            )?,
            Some("project") if command.is_none() => {
                if triangle_output.is_some()
                    || output.is_some()
                    || recipe.is_some()
                    || !sources.is_empty()
                    || plane.is_some()
                {
                    return Err("geometry/triangle flags cannot precede project".into());
                }
                return project::parse(args, engine, version);
            }
            Some("diagnose" | "triangle" | "geometry") if command.is_none() => {
                command = arg.to_str().map(str::to_owned)
            }
            _ => return Err(format!("unrecognized argument {arg:?}; {USAGE}").into()),
        }
    }
    let command = command.ok_or(USAGE)?;
    if command != "geometry"
        && (output.is_some() || recipe.is_some() || !sources.is_empty() || plane.is_some())
    {
        return Err("--scene, --source, --section and --out are only valid with geometry".into());
    }
    if command != "triangle" && triangle_output.is_some() {
        return Err("--output is only valid with triangle".into());
    }
    let command = match command.as_str() {
        "diagnose" => Command::Diagnose,
        "triangle" => Command::Triangle {
            output: triangle_output,
        },
        "geometry" => Command::Geometry {
            input: geometry::SceneInput::load(recipe.as_deref(), &sources)?,
            plane,
            output,
        },
        _ => unreachable!(),
    };
    let engine = engine.unwrap_or(
        env::current_exe()?.with_file_name(format!("spiling-engine{}", env::consts::EXE_SUFFIX)),
    );
    Ok(Some(Options {
        command,
        engine,
        protocol_version: version.unwrap_or(PROTOCOL_VERSION),
    }))
}

async fn run(options: Options) -> Result<(), Error> {
    // Input validation and exclusive output admission occur before spawning.
    let mut output = match &options.command {
        Command::Geometry {
            output: Some(path), ..
        }
        | Command::Project {
            output: Some(path), ..
        } => Some(output::OutputDirectory::create(path)?),
        _ => None,
    };
    let mut manufacturing_output = match &options.command {
        Command::Project {
            manufacturing_output: Some(path),
            ..
        } => Some(output::OutputDirectory::create(path)?),
        _ => None,
    };
    let start = Instant::now();
    let mut client = EngineClient::spawn(&options.engine, options.protocol_version).await?;
    let handshake_us = start.elapsed().as_micros();
    let operation: Result<serde_json::Value, Error> = async {
        let transfer = Instant::now();
        let mut report = match options.command {
            Command::Diagnose => {
                client.ping().await?;
                serde_json::json!({"command": "diagnose", "engine": options.engine.to_string_lossy(), "hello": client.hello(), "ping": "pong", "ping_us": transfer.elapsed().as_micros()})
            },
            Command::Triangle { output } => {
                let payload = client.triangle().await?;
                let transfer_us = transfer.elapsed().as_micros();
                if let Some(path) = &output { std::fs::write(path, &payload)?; }
                serde_json::json!({"command": "triangle", "synthetic": true, "bytes": payload.len(), "transfer_us": transfer_us, "output": output.as_ref().map(|path| path.to_string_lossy()), "pid": client.hello().pid})
            },
            Command::Geometry { input, plane, output: output_path } => {
                let mut report = geometry::run(&mut client, input, plane, output.as_mut()).await?;
                report["transfer_and_jobs_us"] = serde_json::json!(transfer.elapsed().as_micros());
                report["output"] = serde_json::json!(output_path.as_ref().map(|path| path.to_string_lossy()));
                report
            },
            Command::Project { workflow, plane, output: output_path, manufacturing_output: manufacturing_path } => {
                let mut report = project::run(&mut client, workflow, plane, output.as_mut(), manufacturing_output.as_mut()).await?;
                report["transfer_and_jobs_us"] = serde_json::json!(transfer.elapsed().as_micros());
                report["output"] = serde_json::json!(output_path.as_ref().map(|path| path.to_string_lossy()));
                report["manufacturing_output"] = serde_json::json!(manufacturing_path.as_ref().map(|path| path.to_string_lossy()));
                report
            },
        };
        report["handshake_us"] = serde_json::json!(handshake_us);
        Ok(report)
    }.await;
    let report = match operation {
        Ok(report) => report,
        Err(error) => {
            let _ = client.terminate().await;
            return Err(error);
        }
    };
    // A completion manifest must never certify a failed shutdown. Child cleanup
    // remains explicit for every failure after spawn, including output failure.
    if let Err(error) = client.shutdown().await {
        let _ = client.terminate().await;
        return Err(error.into());
    }
    if let Some(output) = &mut output {
        output.write("manifest.json", &serde_json::to_vec_pretty(&report)?)?;
    }
    if let Some(output) = &mut manufacturing_output {
        output.write("manifest.json", &serde_json::to_vec_pretty(&report)?)?;
    }
    // Observe stdout errors rather than panicking with an abandoned transaction.
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &report)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    if let Some(output) = &mut output {
        output.commit();
    }
    if let Some(output) = &mut manufacturing_output {
        output.commit();
    }
    Ok(())
}

fn failure_report(error: &(dyn std::error::Error + 'static)) -> serde_json::Value {
    use spiling_engine_client::ClientError;
    if let Some(failure) = error.downcast_ref::<project::SaveFailure>() {
        let mut report = failure_report(failure.error.as_ref());
        report["project"] = serde_json::json!(failure.project);
        report["operation"] = serde_json::json!("save");
        return report;
    }
    if let Some(failure) = error.downcast_ref::<project::ManufacturingFailure>() {
        let mut report = failure_report(failure.error.as_ref());
        report["project"] = serde_json::json!(failure.project);
        report["manufacturing"] = failure.manufacturing.clone();
        return report;
    }
    match error.downcast_ref::<ClientError>() {
        Some(ClientError::Project(error)) => {
            serde_json::json!({"level":"error","event":"cli_failure","domain":"project","error":error,"message":error.to_string()})
        }
        Some(ClientError::Geometry(error)) => {
            serde_json::json!({"level":"error","event":"cli_failure","domain":"geometry","error":error,"message":error.to_string()})
        }
        Some(ClientError::Manufacturing(error)) => {
            serde_json::json!({"level":"error","event":"cli_failure","domain":"manufacturing","error":error,"message":error.to_string(),"software_only":true,"not_machine_ready":true})
        }
        Some(error) => {
            serde_json::json!({"level":"error","event":"cli_failure","domain":"transport","fatal":error.is_fatal(),"message":error.to_string()})
        }
        None => {
            serde_json::json!({"level":"error","event":"cli_failure","domain":"input_or_output","message":error.to_string()})
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let result = match parse_options(env::args_os().skip(1)) {
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
            eprintln!("{}", failure_report(error.as_ref()));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Option<Options>, Error> {
        parse_options(args.iter().map(OsString::from))
    }
    #[test]
    fn geometry_arguments_fail_before_engine_start() {
        for args in [
            vec!["geometry"],
            vec![
                "geometry",
                "--source",
                "part.step",
                "--scene",
                "recipe.json",
            ],
            vec![
                "geometry",
                "--source",
                "part.step",
                "--section",
                "0,0,0:0,0,0",
            ],
            vec!["geometry", "--source", "part.step", "--output", "x"],
            vec!["diagnose", "--out", "x"],
            vec![
                "geometry",
                "--source",
                "part.step",
                "--engine",
                "x",
                "--engine",
                "y",
            ],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
    #[test]
    fn repeated_sources_and_explicit_diagnostics_remain_distinct() {
        let options = parse(&[
            "geometry",
            "--source",
            "one.step",
            "--source",
            "two.step",
            "--section",
            "0,0,4:0,0,1",
        ])
        .unwrap()
        .unwrap();
        assert!(matches!(options.command, Command::Geometry { .. }));
        assert_eq!(options.protocol_version, PROTOCOL_VERSION);
        assert!(matches!(
            parse(&["triangle", "--output", "triangle.bin"])
                .unwrap()
                .unwrap()
                .command,
            Command::Triangle { .. }
        ));
    }
    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_native_source_arguments() {
        use std::os::unix::ffi::OsStringExt;
        let source = OsString::from_vec(b"part-\xff.step".to_vec());
        assert!(
            parse_options([
                OsString::from("geometry"),
                OsString::from("--source"),
                source
            ])
            .is_ok()
        );
    }
}
