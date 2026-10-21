//! OFFSOURCE: real producer bodies, not qualification evidence.
//! The ActualProducer wrapper owns the BackgroundWork root and binds it before
//! invoking these closures. It reports callback completion separately from
//! delivered messages, fetched commits and subscriber pushes.
use super::super::producer_adapter::{ActualBody, ProductionBodies};
use crate::app::runtime::pumps::status_timing::MeasuredStatusPump;
use crate::app::tests::runtime::write_wait_qualification::fixture::QualificationFixture;
use crate::app::{AppState, DeliveryRunner};
use crate::harness::{AgentSession, SessionStatusSnapshot};
use crate::timing::write_wait::{mutex::acquire_mutex, WaitKind};
use std::sync::{Arc, Mutex};

pub(super) fn from_fixture(fixture: &QualificationFixture) -> Result<ProductionBodies, String> {
    Ok(ProductionBodies {
        status_observations: status(fixture)?,
        delivery_consumption: delivery(fixture),
        source_git_refresh: source(fixture),
        watcher_publication: files(fixture),
    })
}

struct StatusInput {
    pump: MeasuredStatusPump,
    session: Arc<dyn AgentSession>,
    updates: tokio::sync::watch::Receiver<SessionStatusSnapshot>,
}

fn status(fixture: &QualificationFixture) -> Result<ActualBody, String> {
    let mut inputs = Vec::with_capacity(8);
    for (key, session) in &fixture.sessions {
        let updates = session
            .status_changed()
            .ok_or("finite long sessions require an owned real status receiver")?;
        let instance = fixture
            .state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(key)
            .ok_or("warm status tab missing")?
            .session_instance
            .clone();
        let pump = MeasuredStatusPump::new(&fixture.state, key.clone(), session, instance);
        inputs.push(StatusInput {
            pump,
            session: Arc::clone(session),
            updates,
        });
    }
    if inputs.len() != 8 {
        return Err("exact eight warm status inputs required".into());
    }
    Ok(Box::new(move |ordinal| {
        let input = &mut inputs[ordinal as usize % 8];
        if !input.session.is_alive() {
            return Err("finite warm session exited before its producer stopped".into());
        }
        // Read the actual fake provider's owned snapshot. Never manufacture a
        // Working state or relabel callback completion as a provider turn.
        let snapshot = input.updates.borrow_and_update().clone();
        input.pump.observe(&snapshot);
        Ok(())
    }))
}

fn delivery(fixture: &QualificationFixture) -> ActualBody {
    let state = Arc::clone(&fixture.state);
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    Box::new(move |_| {
        DeliveryRunner::drain(&state, &clock.frame("tasks.delivery_service"));
        // SessionLog and logical child settlement prove actual induced sends.
        // An empty real drain is still only a completed producer invocation.
        Ok(())
    })
}

fn source(fixture: &QualificationFixture) -> ActualBody {
    let state = Arc::clone(&fixture.state);
    let sources = fixture
        .projects
        .iter()
        .map(|p| {
            (
                p.project_id.clone(),
                p.source_id.clone(),
                p.repository.clone(),
            )
        })
        .collect::<Vec<_>>();
    Box::new(move |ordinal| {
        let (project, source, directory) = &sources[ordinal as usize % sources.len()];
        let requested = crate::app::projects::base_sync::now_ms();
        {
            let mut app =
                acquire_mutex(&state, WaitKind::AppLock).map_err(|error| error.to_string())?;
            app.request_source_sync(project.clone(), source.clone());
        }
        AppState::sync_sources(
            &state,
            crate::app::projects::base_sync::SyncPass::Requested,
            requested,
        );
        let app = acquire_mutex(&state, WaitKind::AppLock).map_err(|error| error.to_string())?;
        let source = app
            .projects
            .get(project)
            .ok_or("source project vanished")?
            .sources
            .iter()
            .find(|row| row.id == *source)
            .ok_or("source row vanished")?;
        let status = source
            .sync_status
            .as_ref()
            .ok_or("real source service published no status")?;
        if status.last_attempt_ms < requested
            || status.last_fetched_ms.is_none_or(|at| at < requested)
        {
            return Err(format!(
                "private local-origin refresh did not fetch: {}",
                directory.display()
            ));
        }
        Ok(())
    })
}

fn files(fixture: &QualificationFixture) -> ActualBody {
    let state = Arc::clone(&fixture.state);
    let roots = fixture
        .active_workspaces
        .iter()
        .map(|w| (w.owner.clone(), w.directory.clone()))
        .collect::<Vec<_>>();
    let (watchers, changes) = {
        let app = state.lock().unwrap();
        (Arc::clone(&app.watchers), Arc::clone(&app.changes))
    };
    Box::new(move |ordinal| {
        let (owner, root) = &roots[ordinal as usize % roots.len()];
        watchers.reconcile(&changes);
        let body = format!("qualification file-observation {ordinal}\n");
        let path = root.join("qualified-watch.txt");
        std::fs::write(&path, body.as_bytes()).map_err(|error| error.to_string())?;
        if std::fs::read(&path).map_err(|error| error.to_string())? != body.as_bytes() {
            return Err("actual private watcher file did not retain the written bytes".into());
        }
        // This explicitly labelled observation supplies the attributable note.
        // Native notify callbacks are additional unassociated activity; this
        // does NOT claim origin propagation through their separate OS thread.
        changes.note_files(owner, &["qualified-watch.txt".into()]);
        changes.flush();
        Ok(())
    })
}
