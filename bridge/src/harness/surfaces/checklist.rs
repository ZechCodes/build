use serde::{Serialize, Serializer};
use std::collections::HashSet;
use std::ops::Deref;

use super::bounds::{
    bounded, bounded_optional, CHECKLIST_ITEM_LIMIT, CHECKLIST_TEXT_LIMIT, PROVIDER_TOKEN_LIMIT,
};
use super::{SurfaceCoverage, SurfaceObservation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecklistState {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Unknown(String),
}

impl ChecklistState {
    pub fn from_provider(token: &str) -> Self {
        Self::from_provider_bounded(token).0
    }
    pub fn from_provider_bounded(token: &str) -> (Self, bool) {
        match token {
            "pending" => (Self::Pending, false),
            "in_progress" | "inProgress" => (Self::InProgress, false),
            "completed" => (Self::Completed, false),
            "blocked" => (Self::Blocked, false),
            token => {
                let (token, cut) = bounded(token, PROVIDER_TOKEN_LIMIT);
                (Self::Unknown(token), cut)
            }
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Unknown(token) => token,
        }
    }
}

impl Deref for ChecklistState {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl Serialize for ChecklistState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceChecklistItem {
    pub id: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ChecklistState>,
}

impl SurfaceChecklistItem {
    pub fn bounded(
        id: impl Into<String>,
        subject: impl Into<String>,
        description: Option<String>,
        state: Option<ChecklistState>,
    ) -> (Self, bool) {
        let (id, id_cut) = bounded(id, PROVIDER_TOKEN_LIMIT);
        let (subject, subject_cut) = bounded(subject, CHECKLIST_TEXT_LIMIT);
        let (description, description_cut) = bounded_optional(description, CHECKLIST_TEXT_LIMIT);
        let (state, state_cut) = bound_state(state);
        (
            Self {
                id,
                subject,
                description,
                state,
            },
            id_cut || subject_cut || description_cut || state_cut,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistSource {
    TodoWrite,
    TaskCreate,
    TurnPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChecklistProvenance {
    pub source: ChecklistSource,
    pub provider_session_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub collection_epoch: u64,
    pub carried_from_prior_turn: bool,
}

impl ChecklistProvenance {
    pub fn new(
        source: ChecklistSource,
        provider_session_generation: u64,
        turn_id: Option<String>,
        collection_epoch: u64,
    ) -> Self {
        Self {
            source,
            provider_session_generation,
            turn_id: turn_id.map(|id| bounded(id, PROVIDER_TOKEN_LIMIT).0),
            explanation: None,
            collection_epoch,
            carried_from_prior_turn: false,
        }
    }

    pub fn with_explanation(mut self, explanation: Option<String>) -> (Self, bool) {
        let (explanation, truncated) = bounded_optional(explanation, super::PLAN_EXPLANATION_LIMIT);
        self.explanation = explanation;
        (self, truncated)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChecklistCollection {
    items: Vec<SurfaceChecklistItem>,
    provenance: Option<ChecklistProvenance>,
    observation: Option<SurfaceObservation>,
    omitted_ids: HashSet<String>,
    omitted_count: Option<usize>,
    unidentified_omissions: bool,
}

impl ChecklistCollection {
    pub fn replace(
        &mut self,
        items: Vec<SurfaceChecklistItem>,
        provenance: ChecklistProvenance,
        observed_at: impl Into<String>,
        original_count: Option<usize>,
    ) -> bool {
        self.replace_with_coverage(
            items,
            provenance,
            observed_at,
            original_count,
            SurfaceCoverage::Complete,
        )
    }

    pub fn replace_with_coverage(
        &mut self,
        mut items: Vec<SurfaceChecklistItem>,
        provenance: ChecklistProvenance,
        observed_at: impl Into<String>,
        original_count: Option<usize>,
        evidence_coverage: SurfaceCoverage,
    ) -> bool {
        let (provenance, provenance_cut) = bounded_provenance(provenance);
        let mut text_cut = false;
        items = items
            .into_iter()
            .map(|item| {
                let (item, cut) = bounded_item(item);
                text_cut |= cut;
                item
            })
            .collect();
        let supplied_count = items.len();
        let stated_count = original_count.unwrap_or(supplied_count).max(supplied_count);
        let omitted_ids: HashSet<_> = items
            .iter()
            .skip(CHECKLIST_ITEM_LIMIT)
            .take(CHECKLIST_ITEM_LIMIT)
            .map(|item| item.id.clone())
            .collect();
        items.truncate(CHECKLIST_ITEM_LIMIT);
        let omitted = stated_count.saturating_sub(items.len());
        let coverage = if omitted == 0 && !text_cut && !provenance_cut {
            evidence_coverage
        } else {
            SurfaceCoverage::Partial
        };
        let observation =
            SurfaceObservation::current(coverage, observed_at).with_omitted_count(omitted);
        if self.items == items
            && self.provenance.as_ref() == Some(&provenance)
            && same_observation_claim(self.observation.as_ref(), &observation)
        {
            return false;
        }
        self.items = items;
        self.provenance = Some(provenance);
        self.observation = Some(observation);
        self.omitted_ids = omitted_ids;
        self.omitted_count = Some(omitted);
        self.unidentified_omissions =
            stated_count > supplied_count || omitted > self.omitted_ids.len();
        true
    }

    pub fn upsert(
        &mut self,
        item: SurfaceChecklistItem,
        provenance: ChecklistProvenance,
        observed_at: impl Into<String>,
    ) -> bool {
        let (item, _) = bounded_item(item);
        let (provenance, _) = bounded_provenance(provenance);
        let same_collection = self.provenance.as_ref().is_some_and(|held| {
            held.source == provenance.source
                && held.provider_session_generation == provenance.provider_session_generation
                && held.collection_epoch == provenance.collection_epoch
        });
        let mut items = if same_collection {
            self.items.clone()
        } else {
            Vec::new()
        };
        let mut omitted_ids = if same_collection {
            self.omitted_ids.clone()
        } else {
            HashSet::new()
        };
        let mut omitted_count = if same_collection {
            self.omitted_count
        } else {
            Some(0)
        };
        let mut unidentified_omissions = same_collection && self.unidentified_omissions;
        match items.iter().position(|held| held.id == item.id) {
            Some(at) => items[at] = item,
            None if items.len() < CHECKLIST_ITEM_LIMIT => items.push(item),
            None if omitted_ids.contains(&item.id) => {}
            None if unidentified_omissions => {
                omitted_count = None;
            }
            None if omitted_ids.len() < CHECKLIST_ITEM_LIMIT => {
                omitted_ids.insert(item.id);
                omitted_count = omitted_count.map(|count| count.saturating_add(1));
            }
            None => {
                unidentified_omissions = true;
                omitted_count = None;
            }
        }
        items.truncate(CHECKLIST_ITEM_LIMIT);
        let observation = match omitted_count {
            Some(omitted) => SurfaceObservation::current(SurfaceCoverage::Partial, observed_at)
                .with_omitted_count(omitted),
            None => SurfaceObservation::current(SurfaceCoverage::Partial, observed_at),
        };
        if self.items == items
            && self.provenance.as_ref() == Some(&provenance)
            && same_observation_claim(self.observation.as_ref(), &observation)
        {
            return false;
        }
        self.items = items;
        self.provenance = Some(provenance);
        self.observation = Some(observation);
        self.omitted_ids = omitted_ids;
        self.omitted_count = omitted_count;
        self.unidentified_omissions = unidentified_omissions;
        true
    }

    pub fn mark_stale(&mut self) -> bool {
        let stale = self
            .observation
            .as_ref()
            .map(SurfaceObservation::as_stale)
            .unwrap_or_else(SurfaceObservation::unknown_stale);
        if self.observation.as_ref() == Some(&stale) {
            false
        } else {
            self.observation = Some(stale);
            true
        }
    }

    pub fn set_carried_from_prior_turn(&mut self, carried: bool) -> bool {
        let Some(provenance) = self.provenance.as_mut() else {
            return false;
        };
        if provenance.carried_from_prior_turn == carried {
            return false;
        }
        provenance.carried_from_prior_turn = carried;
        true
    }
    pub fn items(&self) -> &[SurfaceChecklistItem] {
        &self.items
    }
    pub fn provenance(&self) -> Option<&ChecklistProvenance> {
        self.provenance.as_ref()
    }
    pub fn observation(&self) -> Option<&SurfaceObservation> {
        self.observation.as_ref()
    }
}

fn bounded_item(item: SurfaceChecklistItem) -> (SurfaceChecklistItem, bool) {
    SurfaceChecklistItem::bounded(item.id, item.subject, item.description, item.state)
}

fn bound_state(state: Option<ChecklistState>) -> (Option<ChecklistState>, bool) {
    match state {
        Some(ChecklistState::Unknown(token)) => {
            let (token, cut) = bounded(token, PROVIDER_TOKEN_LIMIT);
            (Some(ChecklistState::Unknown(token)), cut)
        }
        state => (state, false),
    }
}

fn bounded_provenance(mut provenance: ChecklistProvenance) -> (ChecklistProvenance, bool) {
    let (turn_id, turn_cut) = bounded_optional(provenance.turn_id, PROVIDER_TOKEN_LIMIT);
    let (explanation, explanation_cut) =
        bounded_optional(provenance.explanation, super::PLAN_EXPLANATION_LIMIT);
    provenance.turn_id = turn_id;
    provenance.explanation = explanation;
    (provenance, turn_cut || explanation_cut)
}

fn same_observation_claim(left: Option<&SurfaceObservation>, right: &SurfaceObservation) -> bool {
    left.is_some_and(|left| {
        left.support() == right.support()
            && left.freshness() == right.freshness()
            && left.coverage() == right.coverage()
            && left.omitted_count() == right.omitted_count()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> ChecklistProvenance {
        ChecklistProvenance::new(ChecklistSource::TodoWrite, 7, Some("turn".into()), 2)
    }

    #[test]
    fn duplicate_snapshot_does_not_churn_the_receipt_time() {
        let mut collection = ChecklistCollection::default();
        assert!(collection.replace(Vec::new(), provenance(), "first", Some(0)));
        assert!(!collection.replace(Vec::new(), provenance(), "second", Some(0)));
        assert_eq!(
            collection.observation().unwrap().observed_at(),
            Some("first")
        );
    }

    #[test]
    fn known_empty_snapshot_retains_complete_metadata() {
        let mut collection = ChecklistCollection::default();
        collection.replace(Vec::new(), provenance(), "now", Some(0));
        assert!(collection.items().is_empty());
        assert_eq!(
            collection.observation().unwrap().coverage(),
            Some(SurfaceCoverage::Complete)
        );
    }

    #[test]
    fn list_bound_reports_omissions() {
        let items = (0..CHECKLIST_ITEM_LIMIT + 2)
            .map(|index| SurfaceChecklistItem {
                id: index.to_string(),
                subject: "step".into(),
                description: None,
                state: Some(ChecklistState::Pending),
            })
            .collect();
        let mut collection = ChecklistCollection::default();
        collection.replace(items, provenance(), "now", Some(CHECKLIST_ITEM_LIMIT + 2));
        assert_eq!(collection.items().len(), CHECKLIST_ITEM_LIMIT);
        assert_eq!(collection.observation().unwrap().omitted_count(), Some(2));
    }

    #[test]
    fn repeated_overflow_upsert_does_not_invent_more_omissions() {
        let items = (0..CHECKLIST_ITEM_LIMIT)
            .map(|index| SurfaceChecklistItem {
                id: index.to_string(),
                subject: "step".into(),
                description: None,
                state: Some(ChecklistState::Pending),
            })
            .collect();
        let mut collection = ChecklistCollection::default();
        collection.replace(items, provenance(), "first", Some(CHECKLIST_ITEM_LIMIT));
        let overflow = SurfaceChecklistItem {
            id: "overflow".into(),
            subject: "later".into(),
            description: None,
            state: None,
        };
        assert!(collection.upsert(overflow.clone(), provenance(), "second"));
        assert!(!collection.upsert(overflow, provenance(), "third"));
        assert_eq!(collection.observation().unwrap().omitted_count(), Some(1));
        assert_eq!(
            collection.observation().unwrap().observed_at(),
            Some("second")
        );
    }

    #[test]
    fn retained_item_update_preserves_unidentified_omissions() {
        let items = (0..CHECKLIST_ITEM_LIMIT)
            .map(|index| SurfaceChecklistItem {
                id: index.to_string(),
                subject: "step".into(),
                description: None,
                state: Some(ChecklistState::Pending),
            })
            .collect();
        let mut collection = ChecklistCollection::default();
        collection.replace(items, provenance(), "first", Some(300));
        let changed = SurfaceChecklistItem {
            id: "0".into(),
            subject: "updated".into(),
            description: None,
            state: Some(ChecklistState::InProgress),
        };
        assert!(collection.upsert(changed, provenance(), "second"));
        assert_eq!(collection.observation().unwrap().omitted_count(), Some(44));
    }

    #[test]
    fn unidentifiable_overflow_stops_claiming_an_exact_omission_count() {
        let items = (0..CHECKLIST_ITEM_LIMIT)
            .map(|index| SurfaceChecklistItem {
                id: index.to_string(),
                subject: "step".into(),
                description: None,
                state: None,
            })
            .collect();
        let mut collection = ChecklistCollection::default();
        collection.replace(items, provenance(), "first", Some(300));
        collection.upsert(
            SurfaceChecklistItem {
                id: "possibly-already-omitted".into(),
                subject: "step".into(),
                description: None,
                state: None,
            },
            provenance(),
            "second",
        );
        assert_eq!(collection.observation().unwrap().omitted_count(), None);
        assert_eq!(
            collection.observation().unwrap().coverage(),
            Some(SurfaceCoverage::Partial)
        );
    }
}
