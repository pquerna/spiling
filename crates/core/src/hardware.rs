// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::{Snapshot, error};
use spiling_contracts::{
    hardware::ProjectHardwareCapture,
    project::{ProjectError, ProjectErrorCode},
};
use std::{collections::BTreeMap, sync::Arc};
pub(crate) const MAX_RETAINED_BYTES: usize = 2 * 1024 * 1024;
pub(crate) fn bytes<'a>(
    snapshots: impl Iterator<Item = &'a Snapshot>,
    extra: impl Iterator<Item = Arc<ProjectHardwareCapture>>,
) -> Result<usize, ProjectError> {
    let mut allocations = BTreeMap::new();
    for capture in snapshots
        .flat_map(|snapshot| snapshot.layouts.iter())
        .flat_map(|layout| {
            layout.hardware.iter().cloned().chain(
                layout
                    .manufacturing_options
                    .iter()
                    .filter_map(|option| option.intent.hardware.clone()),
            )
        })
        .chain(extra)
    {
        allocations.entry(Arc::as_ptr(&capture)).or_insert_with(|| {
            serde_json::to_vec(capture.as_ref())
                .map(|v| v.len())
                .map_err(|_| error(ProjectErrorCode::InvalidProject, "invalid hardware capture"))
        });
    }
    allocations.into_values().try_fold(0usize, |sum, n| {
        sum.checked_add(n?)
            .ok_or_else(|| error(ProjectErrorCode::ResourceLimit, "hardware budget overflow"))
    })
}
/// Strict readers validate equality first; retain a single capture allocation per snapshot.
pub(crate) fn share_capture(snapshot: &mut Snapshot) {
    for layout in &mut snapshot.layouts {
        if let Some(intent) = &mut layout.manufacturing_intent {
            Arc::make_mut(intent).hardware = layout.hardware.clone();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, Project, storage};
    use std::sync::atomic::AtomicBool;
    fn capture() -> Arc<ProjectHardwareCapture> {
        let c = spiling_contracts::hardware::builtin_catalog().unwrap();
        Arc::new(ProjectHardwareCapture {
            setup: c.setups[0].clone(),
            plate: c.plates[0].clone(),
            face: "textured".into(),
            accessories: vec![],
        })
    }
    #[test]
    fn setup_before_recipe_is_portable_and_history_preserves_exact_capture() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("job.spil");
        let mut p = Project::new();
        let c = capture();
        let edit = p
            .prepare(Edit::SetHardware {
                capture: Some(c.clone()),
            })
            .unwrap();
        p.commit(edit).unwrap();
        assert_eq!(p.info().hardware, Some(c.clone()));
        assert!(p.info().dirty);
        p.undo().unwrap();
        assert!(p.info().hardware.is_none());
        p.redo().unwrap();
        let intent = serde_json::from_str(include_str!(
            "../../../fixtures/manufacturing/solid-fill.intent.json"
        ))
        .unwrap();
        let edit = p
            .prepare(Edit::SetManufacturingIntent {
                intent: Arc::new(intent),
            })
            .unwrap();
        p.commit(edit).unwrap();
        let receipt = storage::save(
            p.capture(),
            None,
            Some(path.clone()),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        p.mark_saved(receipt).unwrap();
        drop(p);
        let p = Project::from_loaded(
            storage::open(&path, true, false, &AtomicBool::new(false)).unwrap(),
        )
        .unwrap();
        assert_eq!(p.info().hardware, Some(c));
        assert!(!p.info().dirty);
        let hardware = p.current.hardware.as_ref().unwrap();
        assert!(Arc::ptr_eq(
            hardware,
            p.current
                .manufacturing_intent
                .as_ref()
                .unwrap()
                .hardware
                .as_ref()
                .unwrap()
        ));
        assert!(Arc::ptr_eq(
            hardware,
            p.saved.as_ref().unwrap().1.layouts[0]
                .manufacturing_intent
                .as_ref()
                .unwrap()
                .hardware
                .as_ref()
                .unwrap()
        ));
    }
    #[test]
    fn selecting_incompatible_plate_does_not_mutate_project() {
        let p = Project::new();
        let mut c = (*capture()).clone();
        c.plate.mounting_interface = "other-mount".into();
        assert!(
            p.prepare(Edit::SetHardware {
                capture: Some(Arc::new(c))
            })
            .is_err()
        );
        assert!(p.info().hardware.is_none());
        assert!(!p.info().dirty);
    }
}
