// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
mod macos;
mod packaging;
mod process;
use process::Runner;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const BINARIES: [&str; 3] = ["spiling-engine", "spiling-workbench", "spiling-cli"];

struct Context {
    root: PathBuf,
    target: PathBuf,
    runner: Runner,
}
impl Context {
    fn new() -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("missing repository root")?
            .to_owned();
        let runner = Runner::new()?;
        let mut metadata = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        metadata.current_dir(&root).args([
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ]);
        let value: Value = serde_json::from_str(&runner.capture(&mut metadata)?)?;
        let target = PathBuf::from(
            value["target_directory"]
                .as_str()
                .ok_or("Cargo omitted target directory")?,
        );
        Ok(Self {
            root,
            target,
            runner,
        })
    }
    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.root);
        for key in ["CARGO_BUILD_JOBS", "RUST_TEST_THREADS", "RAYON_NUM_THREADS"] {
            if env::var_os(key).is_none() {
                command.env(key, "1");
            }
        }
        if cfg!(target_os = "macos") {
            command.env(
                "MACOSX_DEPLOYMENT_TARGET",
                macos::app().minimum_system_version,
            );
        }
        command
    }
    fn cargo(&self, args: &[&str]) -> Command {
        let mut command = self.command(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command.args(args);
        command
    }
    fn capture(&self, program: &str, args: &[&str]) -> Result<String> {
        self.runner.capture(self.command(program).args(args))
    }
    fn host(&self) -> Result<String> {
        let version = self.capture("rustc", &["-vV"])?;
        Ok(version
            .lines()
            .find_map(|l| l.strip_prefix("host: "))
            .ok_or("rustc omitted host")?
            .into())
    }
    fn metadata(&self) -> Result<Value> {
        Ok(serde_json::from_str(&self.runner.capture(
            &mut self.cargo(&["metadata", "--locked", "--format-version", "1"]),
        )?)?)
    }
    fn build(
        &self,
        release: bool,
        target: Option<&str>,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, PathBuf>> {
        let mut command = self.cargo(&[
            "build",
            "--locked",
            "--message-format=json-render-diagnostics",
        ]);
        for name in BINARIES {
            command.args(["-p", name]);
        }
        if release {
            command.arg("--release");
        }
        if let Some(target) = target {
            command.args(["--target", target]);
        }
        command.envs(extra_env);
        built_executables(&self.runner.capture(&mut command)?)
    }
    fn contracts(&self, check: bool) -> Result<()> {
        let mut command = self.cargo(&[
            "run",
            "--locked",
            "-p",
            "spiling-contracts",
            "--bin",
            "generate",
            "--",
        ]);
        if check {
            command.arg("--check");
        }
        self.runner.run(&mut command)
    }
    fn native_tests(&self, args: &[&str], engine: Option<&Path>) -> Result<()> {
        let mut command = self.cargo(args);
        command.env(
            "RUST_MIN_STACK",
            env::var("RUST_MIN_STACK").unwrap_or_else(|_| "8388608".into()),
        );
        if let Some(engine) = engine {
            command.env("SPILING_TEST_ENGINE", engine);
        }
        self.runner.run(&mut command)
    }
    fn check(&self) -> Result<()> {
        self.contracts(true)?;
        let metadata: Value = serde_json::from_str(&self.runner.capture(&mut self.cargo(&[
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ]))?)?;
        let mut format = self.cargo(&["fmt"]);
        // Explicit workspace packages: do not format excluded upstream snapshots.
        for pkg in metadata["packages"].as_array().ok_or("missing packages")? {
            format.args(["-p", pkg["name"].as_str().ok_or("missing package name")?]);
        }
        self.runner.run(format.args(["--", "--check"]))?;
        self.runner.run(&mut self.cargo(&[
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]))
    }
    fn tests(&self) -> Result<()> {
        let binaries = self.build(false, None, &BTreeMap::new())?;
        let suites: [(&str, &[&str], bool); 3] = [
            (
                "Rust workspace",
                &["test", "--locked", "--workspace", "--no-fail-fast"],
                false,
            ),
            (
                "saved-project client",
                &[
                    "test",
                    "--locked",
                    "-p",
                    "spiling-engine-client",
                    "--lib",
                    "tests::real::manufacturing_bundle_survives_save_restart_and_source_removal",
                    "--",
                    "--ignored",
                    "--exact",
                    "--test-threads=1",
                ],
                true,
            ),
            (
                "saved-project CLI",
                &[
                    "test",
                    "--locked",
                    "-p",
                    "spiling-cli",
                    "--test",
                    "project",
                    "manufacturing_fresh_process_replay_export_invalidation_and_undo",
                    "--",
                    "--ignored",
                    "--exact",
                    "--test-threads=1",
                ],
                true,
            ),
        ];
        let mut failures = Vec::new();
        if let Err(error) = self.contracts(true) {
            if self.runner.interrupted() {
                return Err(error);
            }
            failures.push(format!("generated contracts: {error}"));
        }
        for (name, args, engine) in suites {
            if let Err(error) =
                self.native_tests(args, engine.then(|| binaries["spiling-engine"].as_path()))
            {
                if self.runner.interrupted() {
                    return Err(error);
                }
                failures.push(format!("{name}: {error}"));
            }
        }
        if !failures.is_empty() {
            return Err(format!("Failed native suites:\n{}", failures.join("\n")).into());
        }
        Ok(())
    }
}
fn main() {
    if let Err(error) = execute() {
        eprintln!("{error}");
        std::process::exit(
            error
                .downcast_ref::<process::ChildFailure>()
                .map_or(1, |e| e.0),
        );
    }
}
fn execute() -> Result<()> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("help");
    if matches!(command, "help" | "--help" | "-h") {
        println!(
            "cargo xtask bootstrap|build|dev|check|test|test-native|gallery|contracts [--check]|package [--profile dev|release]|provenance|macos ...\nNative tasks require Rust and platform C/C++ tooling. Optional TypeScript/research tools live in package.json."
        );
        return Ok(());
    }
    let context = Context::new()?;
    let rest = &args[1..];
    match command {
        "bootstrap" => {
            require_empty(rest)?;
            context
                .runner
                .run(&mut context.cargo(&["fetch", "--locked"]))?;
            context.build(false, None, &BTreeMap::new()).map(|_| ())
        }
        "build" => {
            require_empty(rest)?;
            context.build(false, None, &BTreeMap::new()).map(|_| ())
        }
        "dev" => {
            let binaries = context.build(false, None, &BTreeMap::new())?;
            context
                .runner
                .run(context.command(&binaries["spiling-workbench"]).args(rest))
        }
        "check" => {
            require_empty(rest)?;
            context.check()
        }
        "test" => {
            require_empty(rest)?;
            context.tests()
        }
        "contracts" => {
            if !rest.is_empty() && rest != ["--check"] {
                return Err("usage: cargo xtask contracts [--check]".into());
            }
            context.contracts(!rest.is_empty())
        }
        "test-native" => {
            require_empty(rest)?;
            let binaries = context.build(false, None, &BTreeMap::new())?;
            context.native_tests(
                &["test", "--locked", "-p", "spiling-workbench", "--lib"],
                None,
            )?;
            context.native_tests(
                &[
                    "test",
                    "--locked",
                    "-p",
                    "spiling-workbench",
                    "--lib",
                    "--",
                    "--ignored",
                    "--test-threads=1",
                ],
                Some(&binaries["spiling-engine"]),
            )
        }
        "gallery" => context.runner.run(
            context
                .cargo(&[
                    "run",
                    "--locked",
                    "-p",
                    "spiling-workbench",
                    "--bin",
                    "gallery",
                    "--",
                ])
                .args(rest),
        ),
        "package" => packaging::package(&context, rest),
        "provenance" => {
            require_empty(rest)?;
            let path = packaging::provenance(&context)?;
            println!("{}", path.display());
            Ok(())
        }
        "macos" => macos::execute(&context, rest),
        _ => Err(format!("unknown task: {command}; run cargo xtask --help").into()),
    }
}
fn require_empty(args: &[OsString]) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err("unexpected task arguments".into())
    }
}
fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}

fn built_executables(messages: &str) -> Result<BTreeMap<String, PathBuf>> {
    let mut binaries = BTreeMap::new();
    for line in messages.lines().filter(|line| !line.is_empty()) {
        let message: Value = serde_json::from_str(line)?;
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let Some(name) = message["target"]["name"]
            .as_str()
            .filter(|name| BINARIES.contains(name))
        else {
            continue;
        };
        if !message["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        {
            continue;
        }
        if let Some(path) = message["executable"].as_str() {
            binaries.insert(name.to_owned(), PathBuf::from(path));
        }
    }
    if BINARIES.iter().any(|name| !binaries.contains_key(*name)) {
        return Err("Cargo did not report all three native executable artifacts".into());
    }
    Ok(binaries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_cargo_reported_executables_are_used_including_fresh_custom_target_paths() {
        let mut lines = Vec::new();
        for name in BINARIES {
            lines.push(serde_json::json!({"reason":"compiler-artifact","target":{"name":name,"kind":["bin"]},"executable":format!("/custom target/host/debug/{name}"),"fresh":true}).to_string());
        }
        let artifacts = built_executables(&lines.join("\n")).unwrap();
        assert_eq!(
            artifacts["spiling-engine"],
            Path::new("/custom target/host/debug/spiling-engine")
        );
        assert!(built_executables(&lines[..2].join("\n")).is_err());
        let libraries = lines.join("\n").replace("[\"bin\"]", "[\"lib\"]");
        assert!(built_executables(&libraries).is_err());
    }
}
