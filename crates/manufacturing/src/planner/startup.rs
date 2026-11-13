use super::{LayerElevation, SectionSpan, WallCacheKey, walls};
use std::sync::Arc;

pub(super) struct StartupWall {
    pub key: WallCacheKey,
    pub layout: Arc<walls::WallPlan>,
    elevation: [u64; 3],
}

impl StartupWall {
    pub fn permits(input: &crate::CompilerInput<'_>) -> bool {
        !matches!(
            input.intent.recipe.sparse_infill,
            spiling_contracts::manufacturing::SparseInfillPolicy::WebRoof { .. }
        ) && !matches!(&input.intent.printer.workflow,
            spiling_contracts::manufacturing::JobWorkflow::FixedDualMarlin { job }
                if job.functional_construction.is_some() || !job.surface_constructions.is_empty())
    }

    pub fn new(key: WallCacheKey, layout: Arc<walls::WallPlan>, first: &LayerElevation) -> Self {
        Self {
            key,
            layout,
            elevation: Self::elevation(first),
        }
    }

    fn elevation(first: &LayerElevation) -> [u64; 3] {
        [
            first.lower_z.to_bits(),
            first.z.to_bits(),
            first.sample.to_bits(),
        ]
    }

    pub fn matches(&self, sections: &[SectionSpan], schedule: &[LayerElevation]) -> bool {
        self.key.0 == 0
            && sections
                .last()
                .is_some_and(|span| span.end as usize == schedule.len())
            && schedule
                .first()
                .is_some_and(|first| Self::elevation(first) == self.elevation)
    }

    pub fn layout_bytes(&self) -> usize {
        self.layout
            .retained_bytes
            .saturating_add(2 * std::mem::size_of::<usize>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::MultiPolygon;

    fn first() -> LayerElevation {
        LayerElevation {
            lower_z: 0.0,
            z: 0.2,
            sample: 0.1,
            height: 0.2,
            course: 0,
        }
    }

    fn wall() -> StartupWall {
        StartupWall::new(
            (0, None, [0; 3], None),
            Arc::new(walls::WallPlan {
                paths: Vec::new(),
                fill: MultiPolygon(Vec::new()),
                correspondence: Vec::new(),
                retained_points: 0,
                retained_bytes: std::mem::size_of::<walls::WallPlan>(),
                section_area_mm2: 0.0,
                wall_area_mm2: 0.0,
                protected_hole_area_mm2: 0.0,
            }),
            &first(),
        )
    }

    #[test]
    fn startup_handoff_requires_complete_capture_and_exact_first_interval() {
        let wall = wall();
        let mut schedule = vec![
            first(),
            LayerElevation {
                lower_z: 0.2,
                z: 0.4,
                sample: 0.3,
                height: 0.2,
                course: 1,
            },
        ];
        let mut sections = vec![SectionSpan {
            start: 0,
            end: 1,
            material: Arc::new(MultiPolygon(Vec::new())),
            process_domains: Vec::new(),
            occupied: None,
            source_area_mm2: 0.0,
        }];
        assert!(!wall.matches(&sections, &schedule));
        sections[0].end = 2;
        assert!(wall.matches(&sections, &schedule));
        schedule[0].lower_z = -0.0;
        assert!(!wall.matches(&sections, &schedule));
        schedule[0] = first();
        schedule[0].z = schedule[0].z.next_up();
        assert!(!wall.matches(&sections, &schedule));
        schedule[0] = first();
        schedule[0].sample = schedule[0].sample.next_down();
        assert!(!wall.matches(&sections, &schedule));
    }
}
