// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Firmware delivery packages: the verified program wrapped in the container
//! a printer's firmware consumes (Bambu Lab `.gcode.3mf`). The engine writes
//! and independently re-reads the package after replay verification; the
//! descriptor binds it to the bundle and program it carries. A package that
//! opens is not physical printability.
use super::*;
use crate::ArtifactView;

pub const BAMBU_PACKAGE_MEDIA_TYPE: &str = "application/x-spiling-bambu-gcode-3mf";
/// Representation width of one package: the program plus bounded metadata
/// and thumbnails (deflate never grows the program by more than a few
/// bytes per 64 KiB block).
pub const MAX_DELIVERY_PACKAGE_BYTES: u64 = MAX_MANUFACTURING_PROGRAM_BYTES as u64 + (64 << 20);
/// The one plate member a Bambu package carries.
pub const BAMBU_PLATE_GCODE: &str = "Metadata/plate_1.gcode";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPackageFormat {
    /// OPC/3MF archive with one sliced plate: `Metadata/plate_1.gcode`, its
    /// MD5, plate/slice/model/project metadata and thumbnails.
    BambuGcode3mfV1,
}
impl DeliveryPackageFormat {
    pub fn media_type(self) -> &'static str {
        match self {
            Self::BambuGcode3mfV1 => BAMBU_PACKAGE_MEDIA_TYPE,
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::BambuGcode3mfV1 => ".gcode.3mf",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryPackageInfo {
    pub format: DeliveryPackageFormat,
    pub bundle_hash: SourceHash,
    pub program_hash: SourceHash,
    /// Uppercase hexadecimal MD5 of the embedded program, as the package's
    /// checksum member stores it.
    pub program_md5: String,
    /// Package members in archive order.
    pub members: Vec<String>,
    /// Package checks the independent reader performed, in order.
    pub checks: Vec<String>,
}
const MAX_MEMBERS: usize = 32;
const MAX_CHECKS: usize = 64;
impl DeliveryPackageInfo {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.program_md5.len() != 32
            || !self
                .program_md5
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
        {
            return Err(invalid(
                "package MD5 must be 32 uppercase hexadecimal digits",
            ));
        }
        if self.members.is_empty()
            || self.members.len() > MAX_MEMBERS
            || self.checks.len() > MAX_CHECKS
            || self.members.iter().chain(&self.checks).any(|m| {
                m.is_empty() || m.len() > 256 || !m.bytes().all(|b| (0x20..0x7f).contains(&b))
            })
        {
            return Err(invalid("package member or check list is malformed"));
        }
        match self.format {
            DeliveryPackageFormat::BambuGcode3mfV1
                if self.members.iter().any(|m| m == BAMBU_PLATE_GCODE) => {}
            DeliveryPackageFormat::BambuGcode3mfV1 => {
                return Err(invalid("Bambu package lacks its plate program member"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryPackageDescriptor {
    pub info: DeliveryPackageInfo,
    pub resource: ArtifactView,
}
impl DeliveryPackageDescriptor {
    pub fn validate_for_record(
        &self,
        record: &ManufacturingArtifactRecord,
    ) -> Result<(), ManufacturingError> {
        self.info.validate()?;
        record.validate()?;
        crate::native::validate_artifact_view(&self.resource, MAX_DELIVERY_PACKAGE_BYTES)
            .map_err(|e| invalid(&e))?;
        if self.resource.media_type != self.info.format.media_type()
            || self.info.bundle_hash != record.hash
            || self.info.program_hash != record.program.hash
        {
            return Err(invalid(
                "delivery package descriptor disagrees with the bundle record",
            ));
        }
        Ok(())
    }
}
