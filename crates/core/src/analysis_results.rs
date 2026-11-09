use super::*;
use spiling_contracts::analysis::{AnalysisResultRecord, fields::AnalysisFields, mechanics::*};

pub const MAX_RETAINED_ANALYSIS_FIELD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct AnalysisResultAsset {
    pub record: AnalysisResultRecord,
    pub bytes: Arc<Vec<u8>>,
    pub summary: AnalysisRunSummary,
}
impl AnalysisResultAsset {
    pub fn new(record: AnalysisResultRecord, bytes: Vec<u8>) -> Result<Self, ProjectError> {
        if bytes.len() != record.byte_count as usize
            || SourceHash::from_bytes(&bytes) != record.hash
        {
            return Err(invalid("retained analysis byte count/hash mismatch"));
        }
        let fields = AnalysisFields::decode(&bytes)?;
        if fields.summary.case.case.id != record.case_id
            || fields.summary.input_hash != record.input_hash
            || fields.summary.captured_revision != record.captured_revision
        {
            return Err(invalid("retained analysis descriptor/capture mismatch"));
        }
        let summary = fields.summary;
        Ok(Self {
            record,
            bytes: Arc::new(bytes),
            summary,
        })
    }
    pub fn matches_inputs(&self, snapshot: &Snapshot) -> bool {
        snapshot.analysis.cases.contains(&self.summary.case)
            && snapshot.surface_overlays.layers.is_empty()
            && snapshot.process_document.layers.is_empty()
            && snapshot.layouts.iter().any(|layout| {
                layout.layout_id == self.summary.case.case.layout_id
                    && layout
                        .occurrences
                        .get(&self.summary.case.case.occurrence_id)
                        .is_some_and(|occurrence| {
                            occurrence.definition_id == self.summary.case.case.definition_id
                                && occurrence.pose == self.summary.pose
                        })
            })
    }
    pub(crate) fn validate_binding(
        &self,
        snapshot: &Snapshot,
        project: &ProjectId,
        revision: ProjectRevision,
    ) -> Result<(), ProjectError> {
        let fields = AnalysisFields::decode(&self.bytes)?;
        if fields.summary != self.summary
            || self.bytes.len() != self.record.byte_count as usize
            || SourceHash::from_bytes(&self.bytes) != self.record.hash
            || self.summary.case.case.id != self.record.case_id
            || self.summary.input_hash != self.record.input_hash
            || self.summary.captured_revision != self.record.captured_revision
        {
            return Err(invalid(
                "retained analysis immutable content/descriptor mismatch",
            ));
        }
        let case = &self.summary.case.case;
        if &self.summary.project_id != project
            || self.record.captured_revision > revision
            || !snapshot.analysis.cases.iter().any(|current| {
                current.case.id == case.id
                    && current.case.definition_id == case.definition_id
                    && current.case.source_hash == case.source_hash
                    && current.case.occurrence_id == case.occurrence_id
            })
            || !snapshot
                .sources
                .get(&case.definition_id)
                .is_some_and(|source| {
                    source.record.provenance.source_hash == self.summary.source_hash
                })
        {
            return Err(invalid(
                "retained analysis source/project/case closure mismatch",
            ));
        }
        Ok(())
    }
}

pub(crate) fn retained_bytes<'a>(snapshots: impl Iterator<Item = &'a Snapshot>) -> usize {
    let mut allocations = BTreeMap::new();
    for snapshot in snapshots {
        for asset in snapshot.analysis_results.values() {
            allocations.insert(Arc::as_ptr(&asset.bytes), asset.bytes.capacity());
        }
    }
    allocations.values().sum()
}
