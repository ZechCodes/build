use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceSupport {
    Unknown,
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFreshness {
    Loading,
    Current,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCoverage {
    Complete,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceObservation {
    support: SurfaceSupport,
    #[serde(skip_serializing_if = "Option::is_none")]
    freshness: Option<SurfaceFreshness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coverage: Option<SurfaceCoverage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    omitted_count: Option<usize>,
}

impl SurfaceObservation {
    pub fn unknown() -> Self {
        Self::new(SurfaceSupport::Unknown, None, None, None)
    }

    pub fn loading() -> Self {
        Self::new(
            SurfaceSupport::Unknown,
            Some(SurfaceFreshness::Loading),
            None,
            None,
        )
    }

    pub fn unknown_stale() -> Self {
        Self::new(
            SurfaceSupport::Unknown,
            Some(SurfaceFreshness::Stale),
            None,
            None,
        )
    }

    pub fn unsupported() -> Self {
        Self::new(SurfaceSupport::Unsupported, None, None, None)
    }

    pub fn supported_loading() -> Self {
        Self::new(
            SurfaceSupport::Supported,
            Some(SurfaceFreshness::Loading),
            None,
            None,
        )
    }

    pub fn current(coverage: SurfaceCoverage, observed_at: impl Into<String>) -> Self {
        Self::new(
            SurfaceSupport::Supported,
            Some(SurfaceFreshness::Current),
            Some(coverage),
            Some(observed_at.into()),
        )
    }

    pub fn current_unstamped(coverage: SurfaceCoverage) -> Self {
        Self::new(
            SurfaceSupport::Supported,
            Some(SurfaceFreshness::Current),
            Some(coverage),
            None,
        )
    }

    pub fn stale(coverage: Option<SurfaceCoverage>, observed_at: Option<String>) -> Self {
        Self::new(
            SurfaceSupport::Supported,
            Some(SurfaceFreshness::Stale),
            coverage,
            observed_at,
        )
    }

    pub fn with_omitted_count(mut self, count: usize) -> Self {
        if count > 0 && self.support == SurfaceSupport::Supported {
            self.omitted_count = Some(count);
            self.coverage = Some(SurfaceCoverage::Partial);
        }
        self
    }

    pub fn as_stale(&self) -> Self {
        let freshness = match self.support {
            SurfaceSupport::Unsupported => None,
            _ => Some(SurfaceFreshness::Stale),
        };
        Self {
            support: self.support,
            freshness,
            coverage: self.coverage,
            observed_at: self.observed_at.clone(),
            omitted_count: self.omitted_count,
        }
    }

    pub fn support(&self) -> SurfaceSupport {
        self.support
    }
    pub fn freshness(&self) -> Option<SurfaceFreshness> {
        self.freshness
    }
    pub fn coverage(&self) -> Option<SurfaceCoverage> {
        self.coverage
    }
    pub fn observed_at(&self) -> Option<&str> {
        self.observed_at.as_deref()
    }
    pub fn omitted_count(&self) -> Option<usize> {
        self.omitted_count
    }

    fn new(
        support: SurfaceSupport,
        freshness: Option<SurfaceFreshness>,
        coverage: Option<SurfaceCoverage>,
        observed_at: Option<String>,
    ) -> Self {
        Self {
            support,
            freshness,
            coverage,
            observed_at,
            omitted_count: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceObservations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<SurfaceObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checklist: Option<SurfaceObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflows: Option<SurfaceObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagents: Option<SurfaceObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shells: Option<SurfaceObservation>,
}

impl SurfaceObservations {
    pub fn is_empty(&self) -> bool {
        self.goal.is_none()
            && self.checklist.is_none()
            && self.workflows.is_none()
            && self.subagents.is_none()
            && self.shells.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_makes_no_freshness_or_coverage_claim() {
        let written = serde_json::to_value(SurfaceObservation::unsupported()).unwrap();
        assert_eq!(written, serde_json::json!({ "support": "unsupported" }));
    }

    #[test]
    fn omissions_force_partial_only_for_supported_evidence() {
        let current =
            SurfaceObservation::current(SurfaceCoverage::Complete, "now").with_omitted_count(3);
        assert_eq!(current.coverage(), Some(SurfaceCoverage::Partial));
        assert_eq!(current.omitted_count(), Some(3));
        assert_eq!(
            SurfaceObservation::unsupported()
                .with_omitted_count(3)
                .omitted_count(),
            None
        );
    }

    #[test]
    fn stale_preserves_the_last_meaningful_receipt_and_omissions() {
        let current =
            SurfaceObservation::current(SurfaceCoverage::Complete, "first").with_omitted_count(2);
        let stale = current.as_stale();
        assert_eq!(stale.observed_at(), Some("first"));
        assert_eq!(stale.omitted_count(), Some(2));
        assert_eq!(stale.freshness(), Some(SurfaceFreshness::Stale));
    }
}
