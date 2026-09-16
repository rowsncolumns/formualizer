use crate::engine::{
    EvalConfig, SpillBoundsPolicy, SpillBufferMode, SpillCancellationPolicy, SpillConflictPolicy,
    SpillTiebreaker, SpillVisibility,
};

#[test]
fn spill_config_defaults() {
    let cfg = EvalConfig::default();
    assert_eq!(cfg.spill.conflict_policy, SpillConflictPolicy::Error);
    assert_eq!(cfg.spill.tiebreaker, SpillTiebreaker::FirstWins);
    assert_eq!(cfg.spill.bounds_policy, SpillBoundsPolicy::Strict);
    assert_eq!(cfg.spill.buffer_mode, SpillBufferMode::ShadowBuffer);
    assert_eq!(cfg.spill.memory_budget_bytes, None);
    assert_eq!(cfg.spill.cancellation, SpillCancellationPolicy::Cooperative);
    assert_eq!(cfg.spill.visibility, SpillVisibility::OnCommit);
    // Excel's limit: a spill is bounded by the sheet's edges, not by an engine cap.
    assert_eq!(cfg.spill.max_spill_cells, 1_048_576 * 16_384);
    assert_eq!(cfg.spill.max_spill_cells, crate::engine::EXCEL_SHEET_CELLS);
    assert_eq!(
        EvalConfig::default()
            .with_max_spill_cells(10_000)
            .spill
            .max_spill_cells,
        10_000
    );
}
