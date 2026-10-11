// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut prost = tonic_prost_build::Config::new();
    prost.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    // Keep bounded protobuf control oneofs inline rather than adding per-call boxes.
    for oneof in [
        ".spiling.engine.ArtifactSummary.kind",
        ".spiling.engine.ManufacturingRequest.command",
    ] {
        prost.type_attribute(
            oneof,
            "#[expect(clippy::large_enum_variant, reason = \"Bounded control oneofs stay inline without an extra allocation\")]",
        );
    }
    tonic_prost_build::configure().compile_with_config(
        prost,
        &[
            std::path::PathBuf::from("proto/spiling/engine.proto"),
            std::path::PathBuf::from("proto/spiling/native_common.proto"),
            std::path::PathBuf::from("proto/spiling/native_geometry.proto"),
            std::path::PathBuf::from("proto/spiling/native_project.proto"),
            std::path::PathBuf::from("proto/spiling/native_manufacturing.proto"),
            std::path::PathBuf::from("proto/vendor/google/longrunning/operations.proto"),
            std::path::PathBuf::from("proto/vendor/google/bytestream/bytestream.proto"),
        ],
        &[
            std::path::PathBuf::from("proto/vendor"),
            std::path::PathBuf::from("proto"),
            protoc_bin_vendored::include_path()?,
        ],
    )?;
    println!("cargo:rerun-if-changed=proto");
    Ok(())
}
