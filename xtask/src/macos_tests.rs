// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
fn executable(cpu: u32, minimum: u32, dylib: &str) -> Vec<u8> {
    let size = (24 + dylib.len() + 1).div_ceil(8) * 8;
    let mut bytes = vec![0u8; 32 + 24 + size];
    for (offset, value) in [
        (0, 0xfeedfacf),
        (4, cpu),
        (12, 2),
        (16, 2),
        (20, (24 + size) as u32),
        (32, 0x32),
        (36, 24),
        (40, 1),
        (44, minimum),
        (56, 0xc),
        (60, size as u32),
        (64, 24),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[80..80 + dylib.len()].copy_from_slice(dylib.as_bytes());
    bytes
}
fn inspect_bytes(bytes: &[u8]) -> Result<Audit> {
    inspect(&bytes[..32], &bytes[32..], bytes.len() as u64)
}
#[test]
fn foreign_binaries_dependencies_and_corrupt_loads_do_not_pass_as_mac_apps() {
    assert_eq!(
        inspect_bytes(&executable(
            0x0100000c,
            13 << 16,
            "/usr/lib/libSystem.B.dylib"
        ))
        .unwrap()
        .architecture,
        "arm64"
    );
    for (cpu, min, lib) in [
        (0x01000007, 13 << 16, "/usr/lib/libSystem.B.dylib"),
        (0x0100000c, 14 << 16, "/usr/lib/libSystem.B.dylib"),
        (0x0100000c, 13 << 16, "/opt/homebrew/lib/libz.dylib"),
        (0x0100000c, 13 << 16, "@rpath/libz.dylib"),
        (0x0100000c, 13 << 16, "/usr/lib/../../tmp/z.dylib"),
    ] {
        assert!(inspect_bytes(&executable(cpu, min, lib)).is_err());
    }
    for (offset, value) in [
        (0, 0),
        (20, u32::MAX),
        (36, 7),
        (64, u32::MAX),
        (40, 2),
        (16, 0),
    ] {
        let mut bytes = executable(0x0100000c, 13 << 16, "/usr/lib/libSystem.B.dylib");
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(inspect_bytes(&bytes).is_err(), "mutation at {offset}");
    }
    let mut bytes = executable(0x0100000c, 13 << 16, "/usr/lib/libSystem.B.dylib");
    bytes[80..].fill(65);
    assert!(inspect_bytes(&bytes).is_err());
}
#[test]
fn all_helpers_metadata_permissions_and_bytes_survive_real_archive() {
    let temp = tempfile::tempdir().unwrap();
    let binaries = temp.path().join("bin");
    fs::create_dir(&binaries).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for name in app().binaries {
        fs::write(
            binaries.join(name),
            executable(0x0100000c, 13 << 16, "/usr/lib/libSystem.B.dylib"),
        )
        .unwrap();
    }
    let paths: BTreeMap<String, PathBuf> = app()
        .binaries
        .into_iter()
        .map(|name| {
            let path = binaries.join(&name);
            (name, path)
        })
        .collect();
    let destination = temp.path().join("Spiling.app");
    let receipt = bundle(root, &paths, &destination, "0.1.0", json!({"test":true})).unwrap();
    assert_eq!(receipt["binaries"].as_object().unwrap().len(), 3);
    let plist = fs::read_to_string(destination.join("Contents/Info.plist")).unwrap();
    for text in [
        "org.spiling.workbench",
        "LSMinimumSystemVersion",
        "13.0",
        "CFBundleShortVersionString",
    ] {
        assert!(plist.contains(text));
    }
    assert!(destination.join("Contents/Resources/icon.icns").is_file());
    assert!(
        destination
            .join("Contents/Resources/icon.icns.license")
            .is_file()
    );
    let archive = temp.path().join("dev.tar.gz");
    assert!(
        std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(temp.path())
            .arg("Spiling.app")
            .status()
            .unwrap()
            .success()
    );
    let unpack = temp.path().join("unpack");
    fs::create_dir(&unpack).unwrap();
    assert!(
        std::process::Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(&unpack)
            .status()
            .unwrap()
            .success()
    );
    for name in app().binaries {
        let path = unpack.join("Spiling.app/Contents/MacOS").join(&name);
        assert_eq!(
            fs::read(&path).unwrap(),
            fs::read(binaries.join(name)).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o755);
        }
    }
    assert!(bundle(root, &paths, &destination, "0.1.0", json!({})).is_err());
}
#[test]
fn missing_or_foreign_engine_never_creates_successful_app() {
    let temp = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = temp.path().join("Broken.app");
    let paths: BTreeMap<String, PathBuf> = app()
        .binaries
        .into_iter()
        .map(|name| {
            let path = temp.path().join(&name);
            (name, path)
        })
        .collect();
    fs::write(
        temp.path().join("spiling-workbench"),
        executable(0x0100000c, 13 << 16, "/usr/lib/libSystem.B.dylib"),
    )
    .unwrap();
    assert!(bundle(root, &paths, &output, "0.1.0", json!({})).is_err());
    assert!(!output.exists());
    for name in ["spiling-engine", "spiling-cli"] {
        fs::write(temp.path().join(name), b"ELF").unwrap();
    }
    assert!(bundle(root, &paths, &output, "0.1.0", json!({})).is_err());
    assert!(!output.exists());
}
#[test]
fn cross_native_tools_stay_target_scoped_and_stale_captures_fail() {
    let temp = tempfile::tempdir().unwrap();
    let sdk = temp.path().join("SDK");
    let prefix = temp.path().join("target LLVM");
    let capture = temp.path().join("capture");
    fs::create_dir(&sdk).unwrap();
    fs::write(sdk.join("SDKSettings.json"), r#"{"Version":"15.0"}"#).unwrap();
    fs::create_dir_all(prefix.join("include/llvm-c")).unwrap();
    fs::create_dir(prefix.join("lib")).unwrap();
    fs::write(prefix.join("include/llvm-c/Target.h"), b"headers").unwrap();
    fs::create_dir(&capture).unwrap();
    for query in QUERIES {
        fs::write(
            capture.join(format!("{query}.txt")),
            if query == "version" {
                app().llvm_version
            } else {
                String::new()
            },
        )
        .unwrap();
    }
    fs::write(capture.join("target.txt"), app().target).unwrap();
    let compiler = temp.path().join("clang");
    fs::write(&compiler, b"driver").unwrap();
    let receipt = temp.path().join("toolchain.json");
    let mut config = json!({"target":app().target,"osxcrossRevision":"a".repeat(40),"sdkArchiveSha256":"b".repeat(64),"sdkVersion":"15.0","sdk":sdk,"llvmPrefix":prefix,"llvmConfig":capture,"cc":compiler,"cxx":compiler,"ar":compiler,"ranlib":compiler});
    write_json(&receipt, &config).unwrap();
    let (vars, _) = cross(&receipt).unwrap();
    for name in [
        "CC",
        "CXX",
        "AR",
        "RANLIB",
        "RUSTFLAGS",
        "PKG_CONFIG_ALLOW_CROSS",
    ] {
        assert!(!vars.contains_key(name));
    }
    assert!(vars.contains_key("CC_aarch64_apple_darwin"));
    assert_eq!(vars["MACOSX_DEPLOYMENT_TARGET"], "13.0");
    config["sdkVersion"] = "16.0".into();
    write_json(&receipt, &config).unwrap();
    assert!(cross(&receipt).is_err());
    config["sdkVersion"] = "15.0".into();
    write_json(&receipt, &config).unwrap();
    fs::write(capture.join("target.txt"), "x86_64-unknown-linux-gnu").unwrap();
    assert!(cross(&receipt).is_err());
}
