// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

/// A construction preference is separate from an upper bound the operator protects.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayerHeightPolicy {
    Preferred,
    ProtectedMaximum,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayerHeightReason {
    Requested,
    ProcessMinimum,
    ProcessMaximum,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayerHeightSelection {
    pub height_mm: f64,
    pub reason: LayerHeightReason,
}

impl ManufacturingIntent {
    /// Fine schedule limits are distinct from a bulk tool's thicker occupied interval.
    pub fn fine_layer_deposition_limits(&self) -> Result<DepositionLimits, ManufacturingError> {
        let mut limits = self.effective_deposition_limits();
        limits.validate()?;
        if let JobWorkflow::FixedDualMarlin { job } = &self.printer.workflow {
            let primary = if job.has_bound_interior() {
                let assignment = &job.assignments[0];
                let OccurrenceMaterial::Uniform { material_id } = &assignment.material else {
                    return Err(invalid(
                        "bound interior requires a uniform primary material",
                    ));
                };
                usize::from(job.resolve_tool(material_id, assignment.tool_id.as_deref())?)
            } else {
                0
            };
            for (index, tool) in job.tools.iter().enumerate() {
                let mut capture = tool.deposition_limits;
                capture.validate()?;
                if let Some(profile) = &tool.material_profile {
                    let profile = profile.selection.profile.deposition_limits;
                    profile.validate()?;
                    capture.min_bead_width_mm =
                        capture.min_bead_width_mm.max(profile.min_bead_width_mm);
                    capture.max_bead_width_mm =
                        capture.max_bead_width_mm.min(profile.max_bead_width_mm);
                    capture.min_bead_height_mm =
                        capture.min_bead_height_mm.max(profile.min_bead_height_mm);
                    capture.max_bead_height_mm =
                        capture.max_bead_height_mm.min(profile.max_bead_height_mm);
                }
                if index == primary {
                    limits.min_bead_width_mm =
                        limits.min_bead_width_mm.max(capture.min_bead_width_mm);
                    limits.max_bead_width_mm =
                        limits.max_bead_width_mm.min(capture.max_bead_width_mm);
                }
                // Both supported waste strips still occupy every fine interval.
                limits.min_bead_height_mm =
                    limits.min_bead_height_mm.max(capture.min_bead_height_mm);
                limits.max_bead_height_mm =
                    limits.max_bead_height_mm.min(capture.max_bead_height_mm);
            }
        }
        limits.validate()?;
        Ok(limits)
    }

    pub fn layer_height_ceiling_mm(&self) -> Result<f64, ManufacturingError> {
        let selected = self.select_layer_height()?;
        Ok(match self.recipe.layer_height_policy {
            LayerHeightPolicy::ProtectedMaximum => selected.height_mm,
            LayerHeightPolicy::Preferred => self
                .fine_layer_deposition_limits()?
                .max_bead_height_mm
                .min(self.recipe.bead_width_mm),
        })
    }

    /// Pure draft/admission calculation. It grants no geometry or output authority.
    pub fn select_layer_height(&self) -> Result<LayerHeightSelection, ManufacturingError> {
        let requested = self.recipe.layer_height_mm;
        positive(&[requested, self.recipe.bead_width_mm])?;
        let limits = self.fine_layer_deposition_limits()?;
        limits.validate()?;
        let maximum = limits.max_bead_height_mm.min(self.recipe.bead_width_mm);
        if limits.min_bead_height_mm > maximum
            || self.recipe.bead_width_mm < limits.min_bead_width_mm
            || self.recipe.bead_width_mm > limits.max_bead_width_mm
        {
            return Err(invalid(
                "no admissible layer height for the captured bead width",
            ));
        }
        let selected = match self.recipe.layer_height_policy {
            LayerHeightPolicy::Preferred => requested.clamp(limits.min_bead_height_mm, maximum),
            LayerHeightPolicy::ProtectedMaximum => {
                if requested < limits.min_bead_height_mm || requested > maximum {
                    return Err(invalid(
                        "protected layer maximum is outside the captured process envelope",
                    ));
                }
                requested
            }
        };
        if matches!(&self.printer.workflow, JobWorkflow::FixedDualMarlin { job } if !job.has_bound_interior())
            && self.recipe.layer_height_policy != LayerHeightPolicy::ProtectedMaximum
        {
            return Err(invalid(
                "fixed-dual physical slots cannot adjust the requested layer height",
            ));
        }
        Ok(LayerHeightSelection {
            height_mm: selected,
            reason: if selected > requested {
                LayerHeightReason::ProcessMinimum
            } else if selected < requested {
                LayerHeightReason::ProcessMaximum
            } else {
                LayerHeightReason::Requested
            },
        })
    }
}

/// Command-projected physical layer intervals, not a strength or deposition claim.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayerHeightAssessment {
    pub policy: LayerHeightPolicy,
    pub requested_mm: f64,
    pub selected_mm: f64,
    pub reason: LayerHeightReason,
    pub realized_min_mm: f64,
    pub realized_max_mm: f64,
    pub layer_count: u32,
    pub differing_layer_count: u32,
    #[serde(deserialize_with = "required_option")]
    pub first_differing_layer: Option<u32>,
}

impl LayerHeightAssessment {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        positive(&[
            self.requested_mm,
            self.selected_mm,
            self.realized_min_mm,
            self.realized_max_mm,
        ])?;
        if self.layer_count == 0
            || self.layer_count > MAX_PRINT_LAYERS
            || self.differing_layer_count > self.layer_count
            || self.realized_min_mm > self.realized_max_mm
            || (self.differing_layer_count == 0) != self.first_differing_layer.is_none()
            || self
                .first_differing_layer
                .is_some_and(|i| i >= self.layer_count)
            || match self.reason {
                LayerHeightReason::Requested => self.selected_mm != self.requested_mm,
                LayerHeightReason::ProcessMinimum => self.selected_mm <= self.requested_mm,
                LayerHeightReason::ProcessMaximum => self.selected_mm >= self.requested_mm,
            }
            || (self.policy == LayerHeightPolicy::ProtectedMaximum
                && self.reason != LayerHeightReason::Requested)
        {
            return Err(invalid("inconsistent layer-height goal assessment"));
        }
        Ok(())
    }

    pub fn from_plan(
        intent: &ManufacturingIntent,
        layers: &[PrintLayer],
    ) -> Result<Self, ManufacturingError> {
        if layers.is_empty() || layers.len() > MAX_PRINT_LAYERS as usize {
            return Err(invalid(
                "layer-height assessment requires a bounded nonempty schedule",
            ));
        }
        let selected = intent.select_layer_height()?;
        let mut result = Self {
            policy: intent.recipe.layer_height_policy,
            requested_mm: intent.recipe.layer_height_mm,
            selected_mm: selected.height_mm,
            reason: selected.reason,
            realized_min_mm: f64::INFINITY,
            realized_max_mm: 0.0,
            layer_count: layers.len() as u32,
            differing_layer_count: 0,
            first_differing_layer: None,
        };
        for (index, layer) in layers.iter().enumerate() {
            let height = program_coordinates::project_xyz6_coordinate_mm(layer.z_mm)?
                - program_coordinates::project_xyz6_coordinate_mm(layer.lower_z_mm)?;
            positive(&[height])?;
            result.realized_min_mm = result.realized_min_mm.min(height);
            result.realized_max_mm = result.realized_max_mm.max(height);
            // Ignore only one command quantum plus subtraction roundoff in this
            // descriptive comparison; it never relaxes deposition admission.
            let comparison = 0.000001
                + 4.0 * f64::EPSILON * layer.z_mm.abs().max(layer.lower_z_mm.abs()).max(1.0);
            if (height - result.requested_mm).abs() > comparison {
                result.differing_layer_count += 1;
                result.first_differing_layer.get_or_insert(index as u32);
            }
        }
        result.validate()?;
        Ok(result)
    }
}
