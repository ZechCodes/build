//! Legacy (pre-split) persistence shapes, kept **solely** for boot migration.
//!
//! Before the plan/run split a single fused task interleaved
//! `Planning → PlanReview → Building → Review` in one lifecycle, and
//! [`crate::store::PersistedTask`] serialized it. Those `<task_id>.json` records
//! still exist in users' stores, so these deserialization shapes must survive
//! verbatim until every store has migrated. Nothing here drives live behavior:
//! there is no state machine and no methods, just the serde shapes the migration
//! reads. On boot [`crate::store::Store::migrate_legacy_tasks`] loads a
//! `PersistedTask`, maps it onto a `PersistedPlan` and/or `PersistedRun`, and
//! renames the legacy file to `*.migrated`. The live lifecycle lives in
//! [`crate::plan`] and [`crate::run`].

use serde::{Deserialize, Serialize};

/// Whether a legacy task carried the full plan gate or skipped to building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    /// goal → plan → approve → build → review → merge
    Standard,
    /// goal → build → review → merge (no plan phase)
    Quick,
}

/// The working phase a legacy interruption remembered, so recovery could route
/// a reply back to the right working state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Plan,
    Build,
}

/// Every state a legacy fused task could occupy. The migration maps each onto a
/// plan state, a run state, or both (see [`crate::store`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Created,
    Planning,
    PlanReview,
    Building,
    Review,
    Blocked(Phase),
    Failed(Phase),
    IdleUnreported(Phase),
    Interrupted(Phase),
    Merged,
    Abandoned,
    Archived,
}

/// Position of one legacy stage in its per-stage lifecycle. Distinguishes a plan
/// that never progressed (`Planned`/`Approved`) from one whose stages had begun
/// running — the migration's signal that a run record is owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    Planned,
    Approved,
    Building,
    Built,
    Validating,
    Validated { passed: bool },
}

/// The validation agent's verdict for one legacy stage, embedded in [`Stage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub passed: bool,
    pub findings: String,
    pub notes_for_next_stage: String,
}

/// One legacy stage: manifest data + lifecycle sub-state + validation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub summary: String,
    pub state: StageState,
    #[serde(default)]
    pub start_sha: Option<String>,
    #[serde(default)]
    pub validation: Option<ValidationReport>,
}

/// Whether a legacy stage comment was still open or already addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentState {
    Open,
    Addressed,
}

/// Where a legacy plan comment anchored inside a stage doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentAnchor {
    /// The chain of enclosing heading *texts*, outermost first. Empty for a
    /// top-of-doc anchor.
    pub heading_path: Vec<String>,
    /// The selected passage, trimmed.
    pub snippet: String,
}

/// One persisted legacy plan-review comment on a stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageComment {
    pub id: String,
    pub stage_id: String,
    /// None = a general comment on the stage (no text anchor).
    #[serde(default)]
    pub anchor: Option<CommentAnchor>,
    pub body: String,
    pub state: CommentState,
    #[serde(default)]
    pub agent_reply: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // These lock the exact on-disk shapes the migration deserializes. If a field
    // name, default, or enum representation ever drifts, a real user's legacy
    // record would silently fail to load — so the shapes are pinned here.

    #[test]
    fn stage_state_serde_round_trips() {
        for (state, json) in [
            (StageState::Planned, "\"planned\""),
            (StageState::Approved, "\"approved\""),
            (StageState::Building, "\"building\""),
            (StageState::Built, "\"built\""),
            (StageState::Validating, "\"validating\""),
            (
                StageState::Validated { passed: true },
                "{\"validated\":{\"passed\":true}}",
            ),
            (
                StageState::Validated { passed: false },
                "{\"validated\":{\"passed\":false}}",
            ),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<StageState>(json).unwrap(),
                state,
                "round-trip of {json}"
            );
        }
    }

    #[test]
    fn stage_serde_round_trips_including_validation_report() {
        let stage = Stage {
            id: "database-schema".into(),
            title: "Database schema".into(),
            path: ".build/plan/01-database-schema.md".into(),
            summary: "Tables and migration.".into(),
            state: StageState::Validated { passed: false },
            start_sha: Some("abc123".into()),
            validation: Some(ValidationReport {
                passed: false,
                findings: "- migration missing".into(),
                notes_for_next_stage: "".into(),
            }),
        };
        let json = serde_json::to_string(&stage).unwrap();
        assert_eq!(serde_json::from_str::<Stage>(&json).unwrap(), stage);

        // start_sha/summary/validation are #[serde(default)]: a bare stage loads.
        let bare: Stage = serde_json::from_str(
            r#"{"id":"s","title":"S","path":".build/plan/01-s.md","state":"planned"}"#,
        )
        .unwrap();
        assert_eq!(bare.summary, "");
        assert_eq!(bare.start_sha, None);
        assert_eq!(bare.validation, None);
    }

    #[test]
    fn stage_comment_serde_with_and_without_anchor() {
        let anchored = StageComment {
            id: "c-3".into(),
            stage_id: "database-schema".into(),
            anchor: Some(CommentAnchor {
                heading_path: vec!["Database schema".into(), "Tables".into()],
                snippet: "users table gets a soft-delete column".into(),
            }),
            body: "use a deleted_at timestamp".into(),
            state: CommentState::Open,
            agent_reply: None,
        };
        let json = serde_json::to_string(&anchored).unwrap();
        assert!(
            json.contains("\"open\""),
            "CommentState is lowercase: {json}"
        );
        assert_eq!(
            serde_json::from_str::<StageComment>(&json).unwrap(),
            anchored
        );

        // anchor/agent_reply are #[serde(default)]: a minimal comment loads.
        let minimal: StageComment =
            serde_json::from_str(r#"{"id":"c-1","stage_id":"s","body":"b","state":"open"}"#)
                .unwrap();
        assert_eq!(minimal.anchor, None);
        assert_eq!(minimal.agent_reply, None);
    }

    #[test]
    fn task_state_round_trips_including_phase_carrying_arms() {
        for state in [
            TaskState::Created,
            TaskState::Planning,
            TaskState::PlanReview,
            TaskState::Building,
            TaskState::Review,
            TaskState::Blocked(Phase::Plan),
            TaskState::Failed(Phase::Build),
            TaskState::IdleUnreported(Phase::Plan),
            TaskState::Interrupted(Phase::Build),
            TaskState::Merged,
            TaskState::Abandoned,
            TaskState::Archived,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(
                serde_json::from_str::<TaskState>(&json).unwrap(),
                state,
                "round-trip of {json}"
            );
        }
    }
}
