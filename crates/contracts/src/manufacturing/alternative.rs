// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManufacturingInputGroup {
    Hardware,
    MaterialProfile,
    Printer,
    Recipe,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingInputChange {
    pub group: ManufacturingInputGroup,
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingAlternativeEvaluation {
    #[serde(deserialize_with = "required_option")]
    pub interior_policy: Option<Arc<InteriorPolicyAssessment>>,
    pub project_id: ProjectId,
    pub captured_revision: ProjectRevision,
    pub context_hash: SourceHash,
    pub intent: Arc<ManufacturingIntent>,
    pub changed_inputs: Vec<ManufacturingInputChange>,
    pub changed_inputs_complete: bool,
    pub record: ManufacturingArtifactRecord,
    pub report: VerificationReport,
    pub provenance: ManufacturingInspectionProvenance,
    pub resource: crate::ArtifactView,
    pub replay_display: ReplayDisplayDescriptor,
}

impl ManufacturingAlternativeEvaluation {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        self.intent.validate()?;
        if let Some(policy) = &self.interior_policy {
            policy.validate_for(&self.intent, &self.report)?;
        }
        self.report.validate()?;
        self.provenance.validate_for_record(&self.record)?;
        self.replay_display.validate_for_record(&self.record)?;
        if self.project_id != self.provenance.project_id
            || self.captured_revision != self.provenance.input_revision
            || self.intent.printer.workflow != self.provenance.workflow
            || self.resource.sha256 != self.record.hash.as_str()
            || self.resource.size_bytes != self.record.byte_count.to_string()
            || self.resource.media_type != "application/x-spiling-manufacturing-bundle"
            || self.changed_inputs.len() > 128
            || self
                .changed_inputs
                .iter()
                .any(|change| change.path.is_empty())
            || self
                .changed_inputs
                .iter()
                .map(|change| {
                    change.path.len()
                        + change.before.as_ref().map_or(0, String::len)
                        + change.after.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
                > 16384
        {
            return Err(invalid(
                "alternative evaluation binding or diff budget mismatch",
            ));
        }
        crate::native::validate_artifact_view(
            &self.resource,
            MAX_MANUFACTURING_BUNDLE_BYTES as u64,
        )
        .map_err(|message| invalid(&message))?;
        Ok(())
    }
}

pub fn manufacturing_input_changes(
    before: Option<&ManufacturingIntent>,
    after: &ManufacturingIntent,
) -> Result<(Vec<ManufacturingInputChange>, bool), ManufacturingError> {
    fn visit(
        group: ManufacturingInputGroup,
        path: String,
        before: Option<&serde_json::Value>,
        after: Option<&serde_json::Value>,
        changes: &mut Vec<ManufacturingInputChange>,
        bytes: &mut usize,
        complete: &mut bool,
    ) {
        if before == after {
            return;
        }
        if let (Some(serde_json::Value::Object(left)), Some(serde_json::Value::Object(right))) =
            (before, after)
        {
            let keys: BTreeSet<_> = left.keys().chain(right.keys()).collect();
            for key in keys {
                visit(
                    group,
                    format!("{path}/{key}"),
                    left.get(key),
                    right.get(key),
                    changes,
                    bytes,
                    complete,
                );
            }
            return;
        }
        if let (Some(serde_json::Value::Array(left)), Some(serde_json::Value::Array(right))) =
            (before, after)
        {
            for index in 0..left.len().max(right.len()) {
                visit(
                    group,
                    format!("{path}/{index}"),
                    left.get(index),
                    right.get(index),
                    changes,
                    bytes,
                    complete,
                );
            }
            return;
        }
        let before = before.map(ToString::to_string);
        let after = after.map(ToString::to_string);
        let count = path.len()
            + before.as_ref().map_or(0, String::len)
            + after.as_ref().map_or(0, String::len);
        if changes.len() == 128 || *bytes + count > 16384 {
            *complete = false;
            return;
        }
        *bytes += count;
        changes.push(ManufacturingInputChange {
            group,
            path,
            before,
            after,
        });
    }
    let before = before
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| invalid(&error.to_string()))?;
    let after = serde_json::to_value(after).map_err(|error| invalid(&error.to_string()))?;
    let mut changes = Vec::new();
    let mut bytes = 0;
    let mut complete = true;
    for (key, group) in [
        ("hardware", ManufacturingInputGroup::Hardware),
        ("material_profile", ManufacturingInputGroup::MaterialProfile),
        ("printer", ManufacturingInputGroup::Printer),
        ("recipe", ManufacturingInputGroup::Recipe),
    ] {
        visit(
            group,
            key.into(),
            before.as_ref().and_then(|value| value.get(key)),
            after.get(key),
            &mut changes,
            &mut bytes,
            &mut complete,
        );
    }
    Ok((changes, complete))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> ManufacturingIntent {
        decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/solid-fill.intent.json"
        ))
        .unwrap()
    }

    #[test]
    fn changes_cover_printer_and_recipe_leaves_without_mutation_inputs() {
        let before = intent();
        let mut after = before.clone();
        after.printer.name = "Alternate declaration".into();
        after.recipe.print_speed_mm_s += 1.0;
        after.recipe.seam_policy = SeamPolicy::Distributed;
        let (changes, complete) = manufacturing_input_changes(Some(&before), &after).unwrap();
        assert!(complete);
        assert_eq!(changes.len(), 3);
        assert!(changes.iter().any(|change| change.path == "printer/name"
            && change.group == ManufacturingInputGroup::Printer));
        assert!(
            changes
                .iter()
                .any(|change| change.path == "recipe/print_speed_mm_s"
                    && change.group == ManufacturingInputGroup::Recipe)
        );
        assert!(changes.iter().all(|change| change.before != change.after));
    }

    #[test]
    fn bounded_diff_explicitly_reports_incomplete_without_dropping_candidate_input() {
        let mut before = intent();
        let template = before.printer.components[0].clone();
        before.printer.components = (0..MAX_MACHINE_COMPONENTS)
            .map(|index| {
                let mut component = template.clone();
                component.id = format!("component-{index}");
                component
            })
            .collect();
        before.validate().unwrap();
        let mut after = before.clone();
        for component in &mut after.printer.components {
            component.id.push_str("-alternate");
            component.pose.translation_mm = [1.0, 2.0, 3.0];
        }
        after.validate().unwrap();
        let (changes, complete) = manufacturing_input_changes(Some(&before), &after).unwrap();
        assert!(!complete);
        assert_eq!(changes.len(), 128);
        assert_eq!(
            after.printer.components.len(),
            MAX_MACHINE_COMPONENTS as usize
        );
        assert!(
            changes
                .iter()
                .all(|change| change.group == ManufacturingInputGroup::Printer)
        );
    }
}
