// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Fit-table admission mirrors `SkinCommandFitDomain::allocated_bytes`, plus
//! each layer's domain-vector capacity. All nested seeds borrow the plan's byte
//! allowance; only the established point limit resets for each domain.
use super::*;
use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use std::{fmt, marker::PhantomData, mem::size_of};

const BYTE_LIMIT: &str = "skin fitting metadata byte limit exceeded";

fn reserve_slot<T, E: Error>(
    values: &mut Vec<T>,
    limit: usize,
    budget: &mut SegmentBudget,
) -> Result<(), E> {
    if values.len() == values.capacity() {
        let headroom = limit
            .saturating_sub(values.capacity())
            .min(budget.remaining_skin_fit_bytes / size_of::<T>());
        if headroom == 0 {
            return Err(E::custom(BYTE_LIMIT));
        }
        let additional = values.capacity().max(4).min(headroom);
        budget
            .charge_skin_fit_bytes(additional * size_of::<T>())
            .map_err(E::custom)?;
        values
            .try_reserve_exact(additional)
            .map_err(|_| E::custom("skin fitting allocation refused"))?;
    }
    Ok(())
}

// Only the repeated retained vectors use a generic seed. A slot (including its
// inline Vec/String headers) is charged before its nested payload is decoded.
trait FitElement: Sized {
    const LIMIT: usize;
    fn admit<E: Error>(_: &mut SegmentBudget, _: &mut u32) -> Result<(), E> {
        Ok(())
    }
    fn decode<'de, D: serde::Deserializer<'de>>(
        d: D,
        budget: &mut SegmentBudget,
        points: &mut u32,
    ) -> Result<Self, D::Error>;
}
struct FitVecSeed<'a, T> {
    budget: &'a mut SegmentBudget,
    points: &'a mut u32,
    marker: PhantomData<T>,
}
impl<'a, T> FitVecSeed<'a, T> {
    fn new(budget: &'a mut SegmentBudget, points: &'a mut u32) -> Self {
        Self {
            budget,
            points,
            marker: PhantomData,
        }
    }
}
impl<'de, T: FitElement> DeserializeSeed<'de> for FitVecSeed<'_, T> {
    type Value = Vec<T>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}
impl<'de, T: FitElement> Visitor<'de> for FitVecSeed<'_, T> {
    type Value = Vec<T>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a bounded skin fitting array within the cumulative byte allowance")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while values.len() < T::LIMIT {
            let Some(value) = seq.next_element_seed(FitSlotSeed {
                values: &mut values,
                budget: &mut *self.budget,
                points: &mut *self.points,
            })?
            else {
                return Ok(values);
            };
            values.push(value);
        }
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(A::Error::custom("skin fitting array limit exceeded"));
        }
        Ok(values)
    }
}
struct FitSlotSeed<'a, T> {
    values: &'a mut Vec<T>,
    budget: &'a mut SegmentBudget,
    points: &'a mut u32,
}
impl<'de, T: FitElement> DeserializeSeed<'de> for FitSlotSeed<'_, T> {
    type Value = T;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
        T::admit::<D::Error>(self.budget, self.points)?;
        reserve_slot::<T, D::Error>(self.values, T::LIMIT, self.budget)?;
        T::decode(d, self.budget, self.points)
    }
}
impl FitElement for [f64; 2] {
    const LIMIT: usize = MAX_DEPOSITION_SEGMENTS as usize;
    fn admit<E: Error>(_: &mut SegmentBudget, points: &mut u32) -> Result<(), E> {
        *points = points
            .checked_sub(1)
            .ok_or_else(|| E::custom("skin command fitting domain point limit exceeded"))?;
        Ok(())
    }
    fn decode<'de, D: serde::Deserializer<'de>>(
        d: D,
        _: &mut SegmentBudget,
        _: &mut u32,
    ) -> Result<Self, D::Error> {
        Self::deserialize(d)
    }
}
impl FitElement for Vec<[f64; 2]> {
    const LIMIT: usize = MAX_PROCESS_REGIONS;
    fn decode<'de, D: serde::Deserializer<'de>>(
        d: D,
        budget: &mut SegmentBudget,
        points: &mut u32,
    ) -> Result<Self, D::Error> {
        FitVecSeed::<[f64; 2]>::new(budget, points).deserialize(d)
    }
}
impl FitElement for SkinCommandFit {
    const LIMIT: usize = MAX_DEPOSITION_SEGMENTS as usize;
    fn admit<E: Error>(budget: &mut SegmentBudget, _: &mut u32) -> Result<(), E> {
        budget.remaining_skin_fits = budget
            .remaining_skin_fits
            .checked_sub(1)
            .ok_or_else(|| E::custom("cumulative skin fitting witness limit exceeded"))?;
        Ok(())
    }
    fn decode<'de, D: serde::Deserializer<'de>>(
        d: D,
        _: &mut SegmentBudget,
        _: &mut u32,
    ) -> Result<Self, D::Error> {
        Self::deserialize(d)
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum PolygonField {
    Outer,
    Holes,
}
struct PolygonSeed<'a> {
    budget: &'a mut SegmentBudget,
    points: &'a mut u32,
}
impl FitElement for DiaphragmPolygonMm {
    const LIMIT: usize = MAX_PROCESS_REGIONS;
    fn decode<'de, D: serde::Deserializer<'de>>(
        d: D,
        budget: &mut SegmentBudget,
        points: &mut u32,
    ) -> Result<Self, D::Error> {
        d.deserialize_struct(
            "DiaphragmPolygonMm",
            &["outer", "holes"],
            PolygonSeed { budget, points },
        )
    }
}
impl<'de> Visitor<'de> for PolygonSeed<'_> {
    type Value = DiaphragmPolygonMm;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a bounded skin fitting polygon")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let (mut outer, mut holes) = (None, None);
        while let Some(field) = map.next_key::<PolygonField>()? {
            match field {
                PolygonField::Outer => {
                    if outer.is_some() {
                        return Err(A::Error::duplicate_field("outer"));
                    }
                    outer = Some(
                        map.next_value_seed(FitVecSeed::<[f64; 2]>::new(self.budget, self.points))?,
                    );
                }
                PolygonField::Holes => {
                    if holes.is_some() {
                        return Err(A::Error::duplicate_field("holes"));
                    }
                    holes = Some(map.next_value_seed(FitVecSeed::<Vec<[f64; 2]>>::new(
                        self.budget,
                        self.points,
                    ))?);
                }
            }
        }
        Ok(DiaphragmPolygonMm {
            outer: outer.ok_or_else(|| A::Error::missing_field("outer"))?,
            holes: holes.ok_or_else(|| A::Error::missing_field("holes"))?,
        })
    }
}

struct DefinitionSeed<'a>(&'a mut SegmentBudget);
impl DefinitionSeed<'_> {
    fn check<E: Error>(value: &str) -> Result<(), E> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(E::custom("DefinitionId must be lowercase SHA-256 hex"));
        }
        Ok(())
    }
}
impl<'de> DeserializeSeed<'de> for DefinitionSeed<'_> {
    type Value = DefinitionId;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_str(self)
    }
}
impl<'de> Visitor<'de> for DefinitionSeed<'_> {
    type Value = DefinitionId;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("64 lowercase SHA-256 hex bytes")
    }
    fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
        Self::check::<E>(value)?;
        self.0
            .charge_skin_fit_bytes(value.len())
            .map_err(E::custom)?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(value.len())
            .map_err(|_| E::custom("skin fitting identity allocation refused"))?;
        owned.push_str(value);
        DefinitionId::parse(owned).map_err(E::custom)
    }
    fn visit_string<E: Error>(self, value: String) -> Result<Self::Value, E> {
        Self::check::<E>(&value)?;
        self.0
            .charge_skin_fit_bytes(value.capacity())
            .map_err(E::custom)?;
        DefinitionId::parse(value).map_err(E::custom)
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum ContributionField {
    OccurrenceId,
    DefinitionId,
    Provenance,
}
struct ContributionSeed<'a>(&'a mut SegmentBudget);
impl FitElement for ProcessRegionContribution {
    const LIMIT: usize = MAX_PROCESS_REGION_CONTRIBUTIONS;
    fn decode<'de, D: serde::Deserializer<'de>>(
        d: D,
        budget: &mut SegmentBudget,
        _: &mut u32,
    ) -> Result<Self, D::Error> {
        d.deserialize_struct(
            "ProcessRegionContribution",
            &["occurrence_id", "definition_id", "provenance"],
            ContributionSeed(budget),
        )
    }
}
impl<'de> Visitor<'de> for ContributionSeed<'_> {
    type Value = ProcessRegionContribution;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a bounded skin fitting source contribution")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let (mut occurrence_id, mut definition_id, mut provenance) = (None, None, None);
        while let Some(field) = map.next_key::<ContributionField>()? {
            match field {
                ContributionField::OccurrenceId => {
                    if occurrence_id.is_some() {
                        return Err(A::Error::duplicate_field("occurrence_id"));
                    }
                    occurrence_id = Some(map.next_value()?);
                }
                ContributionField::DefinitionId => {
                    if definition_id.is_some() {
                        return Err(A::Error::duplicate_field("definition_id"));
                    }
                    definition_id = Some(map.next_value_seed(DefinitionSeed(&mut *self.0))?);
                }
                ContributionField::Provenance => {
                    if provenance.is_some() {
                        return Err(A::Error::duplicate_field("provenance"));
                    }
                    provenance = Some(map.next_value()?);
                }
            }
        }
        Ok(ProcessRegionContribution {
            occurrence_id: occurrence_id.ok_or_else(|| A::Error::missing_field("occurrence_id"))?,
            definition_id: definition_id.ok_or_else(|| A::Error::missing_field("definition_id"))?,
            // Match the original derived Option reader: missing and null are None.
            provenance: provenance.unwrap_or(None),
        })
    }
}

struct RegionSeed<'a>(&'a mut SegmentBudget);
impl<'de> DeserializeSeed<'de> for RegionSeed<'_> {
    type Value = ProcessRegionCoverage;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_struct(
            "ProcessRegionCoverage",
            &["area_mm2", "fraction", "contributions"],
            self,
        )
    }
}
impl<'de> Visitor<'de> for RegionSeed<'_> {
    type Value = ProcessRegionCoverage;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a bounded skin fitting process region")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let (mut area_mm2, mut fraction, mut contributions) = (None, None, None);
        while let Some(field) = map.next_key::<ProcessRegionField>()? {
            match field {
                ProcessRegionField::AreaMm2 => {
                    if area_mm2.is_some() {
                        return Err(A::Error::duplicate_field("area_mm2"));
                    }
                    area_mm2 = Some(map.next_value()?);
                }
                ProcessRegionField::Fraction => {
                    if fraction.is_some() {
                        return Err(A::Error::duplicate_field("fraction"));
                    }
                    fraction = Some(map.next_value()?);
                }
                ProcessRegionField::Contributions => {
                    if contributions.is_some() {
                        return Err(A::Error::duplicate_field("contributions"));
                    }
                    contributions = Some(map.next_value_seed(FitVecSeed::<
                        ProcessRegionContribution,
                    >::new(
                        self.0, &mut 0
                    ))?);
                }
            }
        }
        Ok(ProcessRegionCoverage {
            area_mm2: area_mm2.ok_or_else(|| A::Error::missing_field("area_mm2"))?,
            fraction: fraction.ok_or_else(|| A::Error::missing_field("fraction"))?,
            contributions: contributions.ok_or_else(|| A::Error::missing_field("contributions"))?,
        })
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum DomainField {
    RegionIndex,
    Region,
    Skin,
    Regional,
    Mandatory,
    SourceOwner,
    Fits,
}
pub(super) struct SkinCommandFitDomainSeed<'a>(pub(super) &'a mut SegmentBudget);
impl<'de> DeserializeSeed<'de> for SkinCommandFitDomainSeed<'_> {
    type Value = SkinCommandFitDomain;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_struct(
            "SkinCommandFitDomain",
            &[
                "region_index",
                "region",
                "skin",
                "regional",
                "mandatory",
                "source_owner",
                "fits",
            ],
            self,
        )
    }
}
impl FitElement for SkinCommandFitDomain {
    const LIMIT: usize = MAX_PROCESS_REGIONS;
    fn decode<'de, D: serde::Deserializer<'de>>(
        d: D,
        budget: &mut SegmentBudget,
        _: &mut u32,
    ) -> Result<Self, D::Error> {
        SkinCommandFitDomainSeed(budget).deserialize(d)
    }
}
impl<'de> Visitor<'de> for SkinCommandFitDomainSeed<'_> {
    type Value = SkinCommandFitDomain;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a source-owned minimum skin proof")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let (mut region_index, mut region, mut skin) = (None, None, None);
        let (mut regional, mut mandatory, mut source_owner, mut fits) = (None, None, None, None);
        let mut points = MAX_DEPOSITION_SEGMENTS;
        while let Some(field) = map.next_key::<DomainField>()? {
            match field {
                DomainField::RegionIndex => {
                    if region_index.is_some() {
                        return Err(A::Error::duplicate_field("region_index"));
                    }
                    region_index = Some(map.next_value()?);
                }
                DomainField::Region => {
                    if region.is_some() {
                        return Err(A::Error::duplicate_field("region"));
                    }
                    region = Some(map.next_value_seed(RegionSeed(&mut *self.0))?);
                }
                DomainField::Skin => {
                    if skin.is_some() {
                        return Err(A::Error::duplicate_field("skin"));
                    }
                    skin = Some(map.next_value()?);
                }
                DomainField::Regional => {
                    if regional.is_some() {
                        return Err(A::Error::duplicate_field("regional"));
                    }
                    regional = Some(map.next_value_seed(FitVecSeed::<DiaphragmPolygonMm>::new(
                        self.0,
                        &mut points,
                    ))?);
                }
                DomainField::Mandatory => {
                    if mandatory.is_some() {
                        return Err(A::Error::duplicate_field("mandatory"));
                    }
                    mandatory = Some(map.next_value_seed(
                        FitVecSeed::<DiaphragmPolygonMm>::new(self.0, &mut points),
                    )?);
                }
                DomainField::SourceOwner => {
                    if source_owner.is_some() {
                        return Err(A::Error::duplicate_field("source_owner"));
                    }
                    source_owner = Some(map.next_value_seed(
                        FitVecSeed::<DiaphragmPolygonMm>::new(self.0, &mut points),
                    )?);
                }
                DomainField::Fits => {
                    if fits.is_some() {
                        return Err(A::Error::duplicate_field("fits"));
                    }
                    fits =
                        Some(map.next_value_seed(FitVecSeed::<SkinCommandFit>::new(
                            self.0,
                            &mut points,
                        ))?);
                }
            }
        }
        let domain = SkinCommandFitDomain {
            region_index: region_index.ok_or_else(|| A::Error::missing_field("region_index"))?,
            region: region.ok_or_else(|| A::Error::missing_field("region"))?,
            skin: skin.ok_or_else(|| A::Error::missing_field("skin"))?,
            regional: regional.ok_or_else(|| A::Error::missing_field("regional"))?,
            mandatory: mandatory.ok_or_else(|| A::Error::missing_field("mandatory"))?,
            source_owner: source_owner.ok_or_else(|| A::Error::missing_field("source_owner"))?,
            fits: fits.ok_or_else(|| A::Error::missing_field("fits"))?,
        };
        domain.validate().map_err(A::Error::custom)?;
        Ok(domain)
    }
}

pub(super) fn domains<'de, D: serde::Deserializer<'de>>(
    budget: &mut SegmentBudget,
    d: D,
) -> Result<Vec<SkinCommandFitDomain>, D::Error> {
    FitVecSeed::<SkinCommandFitDomain>::new(budget, &mut 0).deserialize(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain() -> SkinCommandFitDomain {
        let polygon = DiaphragmPolygonMm {
            outer: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]],
            holes: vec![vec![
                [0.2, 0.2],
                [0.4, 0.2],
                [0.4, 0.4],
                [0.2, 0.4],
                [0.2, 0.2],
            ]],
        };
        SkinCommandFitDomain {
            region_index: 0,
            region: ProcessRegionCoverage {
                area_mm2: 0.96,
                fraction: 1.0,
                contributions: vec![
                    ProcessRegionContribution {
                        occurrence_id: OccurrenceId::new(1).unwrap(),
                        definition_id: DefinitionId::parse("a".repeat(64)).unwrap(),
                        provenance: None,
                    };
                    2
                ],
            },
            skin: ProcessSkinKind::BottomSkin,
            regional: vec![polygon.clone()],
            mandatory: vec![polygon.clone()],
            source_owner: vec![polygon],
            fits: vec![SkinCommandFit {
                required_mm: [[0.1, 0.5, 0.2], [0.2, 0.5, 0.2]],
                fitted_mm: [[0.0, 0.5, 0.2], [0.3, 0.5, 0.2]],
            }],
        }
    }

    #[test]
    fn fit_table_nested_capacities_share_bytes_across_domains_and_layers() {
        let allowance = MAX_MANUFACTURING_BUNDLE_BYTES as usize;
        let encoded = serde_json::to_string(&[domain(), domain()]).unwrap();
        let mut budget = SegmentBudget::new();
        assert_eq!(budget.remaining_skin_fit_bytes, 16 * 1024 * 1024);
        let decoded = domains(
            &mut budget,
            &mut serde_json::Deserializer::from_str(&encoded),
        )
        .unwrap();
        let retained = decoded.capacity() * size_of::<SkinCommandFitDomain>()
            + decoded
                .iter()
                .map(SkinCommandFitDomain::allocated_bytes)
                .sum::<usize>();
        assert_eq!(allowance - budget.remaining_skin_fit_bytes, retained);
        assert_eq!(budget.remaining_skin_fits, MAX_DEPOSITION_SEGMENTS - 2);
        assert_eq!(budget.remaining_segments, MAX_DEPOSITION_SEGMENTS);

        // Simulate the very same borrow across two layer fields. Nested mask,
        // hole, contributor, identity and witness capacity must fit together,
        // including geometric vector slack, not just serialized element counts.
        let mut budget = SegmentBudget::new();
        budget.remaining_skin_fit_bytes = 2 * retained;
        let first = domains(
            &mut budget,
            &mut serde_json::Deserializer::from_str(&encoded),
        )
        .unwrap();
        let second = domains(
            &mut budget,
            &mut serde_json::Deserializer::from_str(&encoded),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(budget.remaining_skin_fit_bytes, 0);
        let error = domains(
            &mut budget,
            &mut serde_json::Deserializer::from_str(&encoded),
        )
        .unwrap_err();
        assert!(error.to_string().contains(BYTE_LIMIT), "{error}");
        assert_eq!(budget.remaining_skin_fit_bytes, 0);

        // A refused slot cannot allocate a header or coordinate capacity first,
        // nor consume the insufficient remainder. No allocator instrumentation
        // or large adversarial fixture is needed to observe this boundary.
        let mut coordinates = Vec::<[f64; 2]>::new();
        budget.remaining_skin_fit_bytes = size_of::<[f64; 2]>() - 1;
        let before = budget.remaining_skin_fit_bytes;
        assert!(
            reserve_slot::<_, serde_json::Error>(&mut coordinates, 100_000, &mut budget).is_err()
        );
        assert_eq!(coordinates.capacity(), 0);
        assert_eq!(budget.remaining_skin_fit_bytes, before);

        // Exhaust inside a nested hole, after polygon and hole-header growth.
        // Admission must stop at its first coordinate, not at domain.validate().
        let mut budget = SegmentBudget::new();
        budget.remaining_skin_fit_bytes = 4 * size_of::<DiaphragmPolygonMm>()
            + 4 * size_of::<Vec<[f64; 2]>>()
            + size_of::<[f64; 2]>()
            - 1;
        let mut points = MAX_DEPOSITION_SEGMENTS;
        let error = FitVecSeed::<DiaphragmPolygonMm>::new(&mut budget, &mut points)
            .deserialize(&mut serde_json::Deserializer::from_str(
                r#"[{"outer":[],"holes":[[[0.0,0.0]]]}]"#,
            ))
            .unwrap_err();
        assert!(error.to_string().contains(BYTE_LIMIT), "{error}");
        assert_eq!(budget.remaining_skin_fit_bytes, size_of::<[f64; 2]>() - 1);

        // Standalone domains use the identical nested guard (without the
        // enclosing layer's vector capacity) and preserve the exact byte sum.
        let encoded = serde_json::to_string(&domain()).unwrap();
        let mut budget = SegmentBudget::new();
        let decoded = SkinCommandFitDomainSeed(&mut budget)
            .deserialize(&mut serde_json::Deserializer::from_str(&encoded))
            .unwrap();
        assert_eq!(
            allowance - budget.remaining_skin_fit_bytes,
            decoded.allocated_bytes()
        );
        assert_eq!(
            serde_json::from_str::<SkinCommandFitDomain>(&encoded).unwrap(),
            decoded
        );
    }

    #[test]
    fn fit_table_contributors_preserve_optional_provenance_and_bound_identity() {
        let mut value = serde_json::to_value(domain()).unwrap();
        let sources = value["region"]["contributions"].as_array_mut().unwrap();
        sources[0].as_object_mut().unwrap().remove("provenance");
        let decoded: SkinCommandFitDomain = serde_json::from_value(value.clone()).unwrap();
        assert!(
            decoded
                .region
                .contributions
                .iter()
                .all(|source| source.provenance.is_none())
        );
        value["region"]["contributions"][0]["definition_id"] = serde_json::json!("a".repeat(65));
        let mut budget = SegmentBudget::new();
        let before = budget.remaining_skin_fit_bytes;
        let error = DefinitionSeed(&mut budget)
            .deserialize(
                serde::de::value::BorrowedStrDeserializer::<serde_json::Error>::new(
                    &"a".repeat(65),
                ),
            )
            .unwrap_err();
        assert!(error.is_data());
        assert_eq!(budget.remaining_skin_fit_bytes, before);
        assert!(serde_json::from_value::<SkinCommandFitDomain>(value).is_err());
    }
}
