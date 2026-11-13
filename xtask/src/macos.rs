// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::{Context, Result, read_json, write_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub name: String,
    pub identifier: String,
    pub executable: String,
    pub binaries: Vec<String>,
    pub minimum_system_version: String,
    pub target: String,
    pub llvm_version: String,
    pub llvm_bundle_version: String,
}
pub fn app() -> App {
    serde_json::from_str(include_str!("../../tools/macos/app.json"))
        .expect("checked-in app manifest")
}
const QUERIES: [&str; 8] = [
    "libs",
    "libnames",
    "system-libs",
    "libdir",
    "includedir",
    "version",
    "cflags",
    "cxxflags",
];
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    architecture: String,
    minimum_system_version: String,
    dylibs: Vec<String>,
    rpaths: Vec<String>,
    pub sha256: String,
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("truncated Mach-O integer")?
            .try_into()?,
    ))
}
fn version(text: &str) -> Result<u32> {
    let parts: Vec<u32> = text
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    if parts.is_empty()
        || parts.len() > 3
        || parts[0] > 65535
        || parts.iter().skip(1).any(|p| *p > 255)
    {
        return Err("invalid macOS version".into());
    }
    Ok((parts[0] << 16)
        | (parts.get(1).copied().unwrap_or(0) << 8)
        | parts.get(2).copied().unwrap_or(0))
}
fn text_at(bytes: &[u8], start: usize, end: usize) -> Result<String> {
    let text = bytes.get(start..end).ok_or("invalid Mach-O string range")?;
    let zero = text
        .iter()
        .position(|b| *b == 0)
        .ok_or("unterminated Mach-O string")?;
    Ok(std::str::from_utf8(&text[..zero])?.to_owned())
}
fn inspect(header: &[u8], commands: &[u8], file_size: u64) -> Result<Audit> {
    if header.len() != 32 || u32_at(header, 0)? != 0xfeedfacf {
        return Err("expected thin little-endian 64-bit Mach-O executable".into());
    }
    if u32_at(header, 4)? != 0x0100000c || u32_at(header, 8)? & 0xffffff != 0 {
        return Err("expected ARM64 Mach-O, excluding Intel/arm64e".into());
    }
    if u32_at(header, 12)? != 2 {
        return Err("Mach-O is not an executable".into());
    }
    let count = u32_at(header, 16)? as usize;
    let size = u32_at(header, 20)? as usize;
    if size != commands.len()
        || file_size
            .checked_sub(32)
            .is_none_or(|remaining| size as u64 > remaining)
        || count > size / 8
    {
        return Err("invalid Mach-O load-command bounds".into());
    }
    let mut offset = 0;
    let mut dylibs = Vec::new();
    let mut rpaths = Vec::new();
    let mut versions = Vec::new();
    for _ in 0..count {
        let kind = u32_at(commands, offset)?;
        let bytes = u32_at(commands, offset + 4)? as usize;
        let end = offset
            .checked_add(bytes)
            .ok_or("Mach-O load size overflow")?;
        if bytes < 8 || !bytes.is_multiple_of(8) || end > size {
            return Err("invalid Mach-O load-command size".into());
        }
        match kind {
            0xc | 0x80000018 | 0x8000001f | 0x20 | 0x80000023 => {
                if bytes < 24 {
                    return Err("truncated Mach-O dylib command".into());
                }
                let start = u32_at(commands, offset + 8)? as usize;
                if start < 24 || start >= bytes {
                    return Err("invalid dylib string offset".into());
                }
                let name = text_at(commands, offset + start, end)?;
                if !(name.starts_with("/System/Library/Frameworks/")
                    || name.starts_with("/usr/lib/"))
                    || name.split('/').any(|s| s == "..")
                {
                    return Err(
                        format!("non-system dylib requires explicit bundling: {name}").into(),
                    );
                }
                dylibs.push(name);
            }
            0x8000001c => {
                if bytes < 16 {
                    return Err("truncated rpath command".into());
                }
                let start = u32_at(commands, offset + 8)? as usize;
                if start < 12 || start >= bytes {
                    return Err("invalid rpath string offset".into());
                }
                let path = text_at(commands, offset + start, end)?;
                let mut components = path.split('/');
                if !matches!(components.next(), Some("@executable_path" | "@loader_path"))
                    || components.any(|c| c.is_empty() || c.starts_with('.'))
                {
                    return Err(format!("developer/external rpath: {path}").into());
                }
                rpaths.push(path);
            }
            0x32 => {
                if bytes < 24
                    || u32_at(commands, offset + 8)? != 1
                    || 24u64 + 8 * u64::from(u32_at(commands, offset + 20)?) > bytes as u64
                {
                    return Err("invalid macOS build-version command".into());
                }
                versions.push(u32_at(commands, offset + 12)?);
            }
            0x24 => {
                if bytes < 16 {
                    return Err("truncated macOS minimum-version command".into());
                }
                versions.push(u32_at(commands, offset + 8)?);
            }
            _ => {}
        }
        offset = end;
    }
    if offset != size || versions.len() != 1 {
        return Err("missing, duplicate or inconsistent Mach-O deployment metadata".into());
    }
    let actual = versions[0];
    let minimum_system_version =
        format!("{}.{}.{}", actual >> 16, (actual >> 8) & 255, actual & 255);
    if actual > version(&app().minimum_system_version)? {
        return Err(format!(
            "binary requires macOS {minimum_system_version}; manifest promises {}",
            app().minimum_system_version
        )
        .into());
    }
    Ok(Audit {
        architecture: "arm64".into(),
        minimum_system_version,
        dylibs,
        rpaths,
        sha256: String::new(),
    })
}
pub fn audit(path: &Path) -> Result<Audit> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;
    let size = file.metadata()?.len();
    let command_size = u32_at(&header, 20)? as usize;
    if u32_at(&header, 0)? != 0xfeedfacf
        || size.checked_sub(32).is_none_or(|n| command_size as u64 > n)
    {
        return Err("invalid Mach-O header/load bounds".into());
    }
    let mut commands = vec![0; command_size];
    file.read_exact(&mut commands)?;
    let mut result = inspect(&header, &commands, size)?;
    result.sha256 = crate::packaging::hash(path)?;
    Ok(result)
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
pub fn metadata(root: &Path, contents: &Path, version: &str) -> Result<()> {
    if version.split('.').count() != 3
        || version
            .split('.')
            .any(|v| v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("bundle version must have three numeric components".into());
    }
    let app = app();
    let resources = contents.join("Resources");
    fs::create_dir_all(&resources)?;
    let mut plist = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>",
    );
    for (key, value) in [
        ("CFBundleExecutable", app.executable.as_str()),
        ("CFBundleIdentifier", &app.identifier),
        ("CFBundleName", &app.name),
        ("CFBundleDisplayName", &app.name),
        ("CFBundleShortVersionString", version),
        ("CFBundleVersion", "1"),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleIconFile", "icon.icns"),
        ("LSMinimumSystemVersion", &app.minimum_system_version),
    ] {
        plist.push_str(&format!(
            "<key>{key}</key><string>{}</string>\n",
            xml(value)
        ));
    }
    plist.push_str("<key>NSHighResolutionCapable</key><true/></dict></plist>\n");
    fs::write(contents.join("Info.plist"), plist)?;
    fs::copy(
        root.join("apps/workbench/icons/icon.icns"),
        resources.join("icon.icns"),
    )?;
    Ok(())
}
pub fn bundle(
    root: &Path,
    binaries: &BTreeMap<String, PathBuf>,
    destination: &Path,
    version: &str,
    receipt: Value,
) -> Result<Value> {
    if destination.extension().is_none_or(|e| e != "app") {
        return Err("destination must be a new .app directory".into());
    }
    let mut audits = BTreeMap::new();
    for name in app().binaries {
        audits.insert(
            name.clone(),
            audit(binaries.get(&name).ok_or("missing executable artifact")?)?,
        );
    }
    fs::create_dir(destination)?;
    let result = (|| {
        let contents = destination.join("Contents");
        let output = contents.join("MacOS");
        fs::create_dir_all(&output)?;
        metadata(root, &contents, version)?;
        for (name, audit) in &audits {
            let target = output.join(name);
            fs::copy(
                binaries.get(name).ok_or("missing executable artifact")?,
                &target,
            )?;
            crate::packaging::executable(&target)?;
            if crate::packaging::hash(&target)? != audit.sha256 {
                return Err("binary changed while assembling bundle".into());
            }
        }
        let resources = contents.join("Resources");
        for name in ["LICENSE.md", "NOTICE.md"] {
            fs::copy(root.join(name), resources.join(name))?;
        }
        fs::copy(
            root.join("apps/workbench/icons/icon.icns.license"),
            resources.join("icon.icns.license"),
        )?;
        let result = json!({"purpose":"local development only; unsigned, not a release distribution","provenance":receipt,"target":app().target,"deploymentTarget":app().minimum_system_version,"binaries":audits});
        write_json(&resources.join("development-build.json"), &result)?;
        Ok(result)
    })();
    if result.is_err() {
        fs::remove_dir_all(destination)?;
    }
    result
}
fn existing(path: &Path, directory: bool) -> Result<()> {
    if !path.is_absolute()
        || if directory {
            !path.is_dir()
        } else {
            !path.is_file()
        }
    {
        return Err(format!(
            "expected existing absolute toolchain {}: {}",
            if directory { "directory" } else { "file" },
            path.display()
        )
        .into());
    }
    Ok(())
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn checked_text(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_owned())
}
fn cross(path: &Path) -> Result<(BTreeMap<String, String>, Value)> {
    let config = read_json(path)?;
    let value = |key: &str| -> Result<String> {
        Ok(config[key]
            .as_str()
            .ok_or_else(|| format!("toolchain omitted {key}"))?
            .to_owned())
    };
    if value("target")? != app().target
        || !hex(&value("osxcrossRevision")?, 40)
        || !hex(&value("sdkArchiveSha256")?, 64)
    {
        return Err("toolchain must record ARM64 target, exact OSXCross revision and private SDK archive SHA-256".into());
    }
    let sdk = value("sdk")?;
    existing(Path::new(&sdk), true)?;
    let settings = Path::new(&sdk).join("SDKSettings.json");
    if read_json(&settings)?["Version"] != value("sdkVersion")? {
        return Err("SDK version differs from toolchain receipt".into());
    }
    let mut vars = BTreeMap::from([
        ("SDKROOT".into(), sdk),
        (
            "MACOSX_DEPLOYMENT_TARGET".into(),
            app().minimum_system_version,
        ),
    ]);
    for (var, key) in [
        ("CC", "cc"),
        ("CXX", "cxx"),
        ("AR", "ar"),
        ("RANLIB", "ranlib"),
    ] {
        let path = value(key)?;
        existing(Path::new(&path), false)?;
        vars.insert(format!("{var}_aarch64_apple_darwin"), path);
    }
    vars.insert(
        "CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER".into(),
        value("cc")?,
    );
    let prefix = value("llvmPrefix")?;
    let capture = value("llvmConfig")?;
    existing(Path::new(&prefix), true)?;
    existing(Path::new(&capture), true)?;
    if checked_text(&Path::new(&capture).join("target.txt"))? != app().target
        || checked_text(&Path::new(&capture).join("version.txt"))? != app().llvm_version
    {
        return Err("target LLVM capture target/version mismatch".into());
    }
    for query in QUERIES {
        checked_text(&Path::new(&capture).join(format!("{query}.txt")))?;
    }
    existing(&Path::new(&prefix).join("include/llvm-c/Target.h"), false)?;
    existing(&Path::new(&prefix).join("lib"), true)?;
    vars.insert("TRACEL_LLVM_TARGET_PREFIX".into(), prefix);
    vars.insert("TRACEL_LLVM_TARGET_CONFIG".into(), capture);
    let receipt = json!({"config":config,"configSha256":crate::packaging::hash(path)?,"sdkSettingsSha256":crate::packaging::hash(&settings)?});
    Ok((vars, receipt))
}
pub fn execute(context: &Context, args: &[OsString]) -> Result<()> {
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("doctor");
    if command == "audit" {
        if args.len() != 2 {
            return Err("usage: cargo xtask macos audit MACH_O_EXECUTABLE".into());
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&audit(&PathBuf::from(&args[1]))?)?
        );
        return Ok(());
    }
    let mut options = BTreeMap::new();
    let flags = if args.is_empty() { args } else { &args[1..] };
    for pair in flags.chunks(2) {
        if pair.len() != 2 {
            return Err("macos options require a value".into());
        }
        let key = pair[0].to_str().ok_or("invalid option")?;
        if !["--toolchain", "--output", "--prefix"].contains(&key)
            || options.insert(key, PathBuf::from(&pair[1])).is_some()
        {
            return Err("unknown or duplicate macos option".into());
        }
    }
    if command == "check-core" {
        if !options.is_empty() {
            return Err("check-core takes no options".into());
        }
        return context.runner.run(&mut context.cargo(&[
            "check",
            "--locked",
            "--target",
            &app().target,
            "-p",
            "spiling-model-import",
            "-p",
            "spiling-contracts",
        ]));
    }
    if command == "capture-llvm" {
        return capture(context, &options);
    }
    if !["doctor", "build", "bundle"].contains(&command) {
        return Err("usage: cargo xtask macos doctor|check-core|build|bundle|capture-llvm".into());
    }
    if options.contains_key("--prefix") || (command != "bundle" && options.contains_key("--output"))
    {
        return Err("option does not apply to this command".into());
    }
    let installed = context.capture("rustup", &["target", "list", "--installed"])?;
    if !installed.split_whitespace().any(|s| s == app().target) {
        return Err(format!(
            "install pinned Rust target: rustup target add {}",
            app().target
        )
        .into());
    }
    let (vars, toolchain) = if cfg!(target_os = "linux") {
        let path=options.get("--toolchain").cloned().or_else(||env::var_os("SPILING_MACOS_TOOLCHAIN").map(PathBuf::from)).ok_or("Linux cross-build needs --toolchain PRIVATE_TOOLCHAIN.json (Apple SDK, OSXCross and target LLVM); see tools/macos/README.md")?;
        let (vars, mut receipt) = cross(&path)?;
        let cc = &vars["CC_aarch64_apple_darwin"];
        let machine = context
            .runner
            .capture(context.command(cc).arg("-dumpmachine").envs(&vars))?;
        if !machine.starts_with("arm64-apple-darwin")
            && !machine.starts_with("aarch64-apple-darwin")
        {
            return Err("cross compiler is not an ARM64 Apple driver".into());
        }
        receipt["compiler"] = context
            .runner
            .capture(context.command(cc).arg("--version").envs(&vars))?
            .into();
        (vars, receipt)
    } else if cfg!(target_os = "macos") && context.host()? == app().target {
        if options.contains_key("--toolchain") {
            return Err("native Mac builds use xcrun, not a Linux toolchain receipt".into());
        }
        let sdk = context.capture("xcrun", &["--sdk", "macosx", "--show-sdk-path"])?;
        existing(Path::new(&sdk), true)?;
        (
            BTreeMap::from([
                ("SDKROOT".into(), sdk),
                (
                    "MACOSX_DEPLOYMENT_TARGET".into(),
                    app().minimum_system_version,
                ),
            ]),
            json!({"sdkVersion":context.capture("xcrun",&["--sdk","macosx","--show-sdk-version"])?}),
        )
    } else {
        return Err(
            "macOS tooling supports Linux cross-builds or native Apple Silicon Macs".into(),
        );
    };
    let receipt = json!({"host":context.host()?,"rust":context.capture("rustc",&["-vV"])?,"toolchain":toolchain,"source":context.capture("git",&["rev-parse","HEAD"])?,"dirty":!context.capture("git",&["status","--porcelain"])?.is_empty(),"cargoLockSha256":crate::packaging::hash(&context.root.join("Cargo.lock"))?,"profile":"dev"});
    if command == "doctor" {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }
    let binaries = context.build(false, Some(&app().target), &vars)?;
    for name in app().binaries {
        audit(binaries.get(&name).ok_or("missing executable artifact")?)?;
    }
    if command == "bundle" {
        let output = options
            .get("--output")
            .cloned()
            .unwrap_or_else(|| context.root.join("dist/Spiling-dev.app"));
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let version = crate::packaging::workspace_version(context)?;
        let receipt = bundle(&context.root, &binaries, &output, &version, receipt)?;
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    }
    Ok(())
}
fn capture(context: &Context, options: &BTreeMap<&str, PathBuf>) -> Result<()> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err("capture target LLVM queries on an ARM64 Mac, then copy its LLVM prefix and capture to Linux".into());
    }
    if options.contains_key("--toolchain") {
        return Err("capture-llvm does not use a Linux toolchain".into());
    }
    let output = options
        .get("--output")
        .ok_or("capture-llvm requires --output NEW_DIRECTORY")?;
    let prefix = options
        .get("--prefix")
        .cloned()
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_default())
                .join("Library/Application Support/tracel")
                .join(format!("tracel-llvm-{}", app().llvm_bundle_version))
        })
        .canonicalize()?;
    let config = prefix.join("bin/llvm-config");
    let mut values = BTreeMap::new();
    for query in QUERIES {
        values.insert(
            query,
            context
                .runner
                .capture(
                    context
                        .command(&config)
                        .args(["--link-static", &format!("--{query}")]),
                )?
                .replace(
                    prefix.to_str().ok_or("LLVM prefix must be UTF-8")?,
                    "${LLVM_PREFIX}",
                ),
        );
    }
    if values["version"] != app().llvm_version
        || values["libdir"] != "${LLVM_PREFIX}/lib"
        || values["includedir"] != "${LLVM_PREFIX}/include"
    {
        return Err("LLVM capture must use the matching self-contained Tracel bundle".into());
    }
    fs::create_dir(output)?;
    let result = (|| {
        for (name, text) in values {
            fs::write(output.join(format!("{name}.txt")), format!("{text}\n"))?;
        }
        fs::write(output.join("target.txt"), format!("{}\n", app().target))?;
        write_json(
            &output.join("capture.json"),
            &json!({"target":app().target,"llvmVersion":app().llvm_version,"llvmConfigSha256":crate::packaging::hash(&config)?,"capturedOn":context.capture("sw_vers",&[])?}),
        )
    })();
    if result.is_err() {
        fs::remove_dir_all(output)?;
    }
    result
}

#[cfg(test)]
#[path = "macos_tests.rs"]
mod tests;
