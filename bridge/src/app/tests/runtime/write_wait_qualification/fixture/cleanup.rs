//! Always attempt owned fixture cleanup before surfacing setup/oracle failures.
use super::*;
use std::time::Instant;

impl QualificationFixture {
    /// Native setup-only snapshot, never called by a timed read.
    pub(in crate::app::tests::runtime::write_wait_qualification) fn qualification_count_receipt(
        &self,
    ) -> serde_json::Value {
        serde_json::to_value(self.native_counts()).expect("native fixture receipt serializes")
    }
    fn native_counts(&self) -> NativeCounts {
        let app = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut canonical = std::collections::BTreeMap::new();
        for run in app.runs.values() {
            for agent in run.agents.iter() {
                canonical.insert(
                    agent.conversation_id().to_string(),
                    agent.thread.total_item_count(),
                );
            }
        }
        for plan in app.plans.values() {
            for agent in plan.agents.iter() {
                canonical.insert(
                    agent.conversation_id().to_string(),
                    agent.thread.total_item_count(),
                );
            }
        }
        NativeCounts {
            projects: self.projects.len(),
            task_counts: self
                .projects
                .iter()
                .map(|project| {
                    app.tracker_store()
                        .unwrap()
                        .list_tracker_tasks(
                            project.repository.to_str().unwrap(),
                            Default::default(),
                        )
                        .unwrap()
                        .len()
                })
                .collect(),
            primary_checkout_counts: self
                .projects
                .iter()
                .map(|project| {
                    let repository = git2::Repository::open(&project.repository).unwrap();
                    let expected = std::fs::canonicalize(&project.repository).unwrap();
                    let is_primary = !repository.is_worktree()
                        && repository
                            .workdir()
                            .is_some_and(|path| std::fs::canonicalize(path).unwrap() == expected)
                        && app
                            .workspaces
                            .list(Some(&project.project_id))
                            .iter()
                            .any(|workspace| {
                                workspace.id == format!("legacy-{}", project.project_id)
                                    && std::fs::canonicalize(&workspace.root).unwrap() == expected
                            });
                    usize::from(is_primary)
                })
                .collect(),
            linked_checkout_counts: self
                .projects
                .iter()
                .map(|project| {
                    git2::Repository::open(&project.repository)
                        .unwrap()
                        .worktrees()
                        .unwrap()
                        .len()
                })
                .collect(),
            registry_residents: app.workspaces.list(None).len(),
            active_workspaces: self.active_workspaces.len(),
            canonical_histories: canonical.len(),
            total_thread_items: canonical.values().sum(),
            live_sessions: self
                .sessions
                .iter()
                .filter(|(key, session)| {
                    app.session_registry.test_tab(key).is_some_and(|tab| {
                        tab.session_is_live() && Arc::ptr_eq(&tab.session, session)
                    })
                })
                .count(),
        }
    }
    pub(super) async fn counts_and_shutdown(self) -> NativeCounts {
        let counts =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.native_counts()));
        // Cardinality/Store failures are retained, but never skip teardown.
        let cleanup = self.cleanup_owned_resources().await;
        for error in &cleanup {
            eprintln!("qualification cleanup error: {error}");
        }
        let counts = match counts {
            Ok(counts) => counts,
            Err(original) => std::panic::resume_unwind(original),
        };
        assert!(
            cleanup.is_empty(),
            "qualification cleanup failed: {cleanup:?}"
        );
        eprintln!("qualification_native_fixture_counts={counts:?}");
        counts
    }

    pub(in crate::app::tests::runtime::write_wait_qualification) async fn cleanup_owned_resources(
        mut self,
    ) -> Vec<String> {
        use futures_util::FutureExt;
        let mut errors = Vec::new();
        if let Some(reads) = self.read_inputs.take() {
            // A retained oracle Arc can defer ReadCleanup::drop. The explicit
            // all-tab retirements below are the fallback; any unconfirmed read
            // shutdown preserves the private roots even after AppState drops.
            let outcome = std::panic::AssertUnwindSafe(async {
                reads.shutdown(&self.rpc).await;
                drop(reads);
            })
            .catch_unwind()
            .await;
            if let Err(payload) = outcome {
                errors.push(cleanup_panic("read prerequisites", &payload));
            }
        }
        let (retirements, retirement_errors) = request_retirements(&self.state);
        errors.extend(retirement_errors);
        errors.extend(join_retirements(retirements).await);
        let Self {
            state,
            rpc,
            sessions,
            _root,
            ..
        } = self;
        let weak = Arc::downgrade(&state);
        drop(sessions);
        drop(state);
        match std::panic::AssertUnwindSafe(rpc.finish_report())
            .catch_unwind()
            .await
        {
            Ok(report) => {
                if !report.is_clean() {
                    errors.push(format!("encrypted carrier cleanup: errors={:?}, pending_at_close={:?}, pending_after_join={:?}, joined_receivers={}", report.errors, report.pending_at_close, report.pending_after_join, report.joined_receivers));
                }
            }
            Err(payload) => errors.push(cleanup_panic("encrypted carrier shutdown", &payload)),
        }
        let until = Instant::now() + Duration::from_secs(10);
        while weak.upgrade().is_some() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if weak.upgrade().is_some() || !errors.is_empty() {
            // A reaper/session can retain paths without retaining AppState.
            // Any unconfirmed cleanup is therefore unsafe to delete even if
            // the weak app handle is gone. Counts/oracle errors are separate.
            let retained = _root.keep();
            errors.push(format!(
                "cleanup unconfirmed; private root preserved: {}",
                retained.display()
            ));
        } else {
            drop(_root);
        }
        errors
    }
}

fn cleanup_panic(stage: &str, payload: &Box<dyn std::any::Any + Send>) -> String {
    let detail = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_string())
        })
        .unwrap_or_else(|| "non-string panic".into());
    format!("{stage}: {detail}")
}

fn request_retirements(
    state: &Arc<Mutex<AppState>>,
) -> (Vec<crate::reaper::Retirement>, Vec<String>) {
    let mut retirements = Vec::new();
    let mut errors = Vec::new();
    let keys = {
        let app = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        app.session_registry
            .test_tabs()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>()
    };
    for key in keys {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retire_tab(&key, "qualification_fixture_cleanup")
        }));
        match outcome {
            Ok(Some(retirement)) => retirements.push(retirement),
            Ok(None) => {}
            Err(payload) => errors.push(cleanup_panic("native retirement request", &payload)),
        }
    }
    (retirements, errors)
}

async fn join_retirements(retirements: Vec<crate::reaper::Retirement>) -> Vec<String> {
    match tokio::task::spawn_blocking(move || {
        let mut errors = Vec::new();
        // Every receipt is waited even if a preceding receipt fails or panics.
        for retirement in retirements {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                retirement.wait(Duration::from_secs(2))
            }));
            match result {
                Ok(true) => {}
                Ok(false) => errors.push("native retirement deadline elapsed".to_string()),
                Err(payload) => errors.push(cleanup_panic("native retirement join", &payload)),
            }
        }
        errors
    })
    .await
    {
        Ok(errors) => errors,
        Err(error) => vec![format!("native retirement worker failed: {error}")],
    }
}
