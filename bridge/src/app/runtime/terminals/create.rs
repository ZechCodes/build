//! Shell admission holds its checkout and one capacity slot while the process
//! opens outside the application mutex. Only a revalidated shell is registered.
use super::create_scope::{CapturedScope, ResolvedScope, Selector};
use super::{require_shell_kind, shell_harness_spec, terminal_size};
use super::{MAX_USER_TERMINALS, MOVED_TERMINAL_SOURCE, SHELL_TAB_KIND};
use crate::app::{spawn_tab_pumps, AppState, SpawnClaimToken, Tab, TabKey};
use crate::harness::SessionOutput;
use crate::timing::FrameTimer;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, Weak};

pub(super) fn create(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    require_shell_kind(params)?;
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
    let selector = Selector::parse(params)?;
    let captured = {
        let app = timer.lock(state);
        #[cfg(test)]
        let mut app = app;
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("term.create.capture");
        refuse_project_deletion(&app)?;
        let scope = selector.capture(&app)?;
        #[cfg(test)]
        let gate = app.terminal_create_scope_probe_gate.take();
        ScopeCapture {
            scope,
            #[cfg(test)]
            gate,
        }
    };
    #[cfg(test)]
    if let Some(gate) = captured.gate {
        gate.arrive();
    }
    let captured = captured.scope;
    let resolved = captured.probe()?;
    let mut reserved = reserve(state, &selector, &captured, &resolved, timer)?;
    let (mut tab, output) = {
        // The collector records this phase only if an app guard is held.
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("term.create.spawn");
        Tab::spawn_shell(
            &shell_harness_spec(&reserved.shell),
            reserved.key.tab_id.clone(),
            reserved.key.root.clone(),
            terminal_size(cols, rows),
        )
    }?;
    tab.project_source = resolved.terminal.source.clone();
    let mut opened = UnpublishedShell(Some((tab, output)));
    #[cfg(test)]
    if let Some(gate) = &reserved.spawn_gate {
        gate.arrive();
    }
    let checked = captured.probe()?;
    if checked.terminal.root != resolved.terminal.root
        || checked.terminal.source != resolved.terminal.source
        || checked.project_id != resolved.project_id
    {
        return Err(MOVED_TERMINAL_SOURCE.into());
    }
    let pumps = {
        let mut app = timer.lock(state);
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("term.create.commit");
        validate_identity(&app, &selector, &captured, &resolved)?;
        let (tab, output) = opened
            .0
            .take()
            .expect("an unpublished shell owns its child");
        let pumps = app
            .session_registry
            .insert_shell(reserved.key.clone(), tab, output);
        reserved.hold.settle(&mut app);
        if let Some(workspace) = resolved.adoption {
            app.workspaces.install_prepared_root_adoption(workspace);
        }
        app.record_snapshot_root_observation(&resolved.raw_root, &resolved.terminal.root);
        app.note_terminals_at(&reserved.key.root);
        pumps
    };
    spawn_tab_pumps(state, reserved.key.clone(), pumps);
    Ok(json!({"term_id":reserved.key.tab_id,"kind":SHELL_TAB_KIND,"cols":cols,"rows":rows}))
}

struct ScopeCapture {
    scope: CapturedScope,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}

struct ReservedShell<'timer> {
    key: TabKey,
    shell: String,
    hold: ShellHold<'timer>,
    #[cfg(test)]
    spawn_gate: Option<crate::app::OffLockGate>,
}

fn reserve<'timer>(
    state: &Arc<Mutex<AppState>>,
    selector: &Selector,
    captured: &CapturedScope,
    resolved: &ResolvedScope,
    timer: &'timer FrameTimer,
) -> Result<ReservedShell<'timer>, String> {
    let mut app = timer.lock(state);
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("term.create.reserve");
    validate_identity(&app, selector, captured, resolved)?;
    if app.shell_tab_count() + app.session_registry.pending_shell_count() >= MAX_USER_TERMINALS {
        return Err(format!(
            "terminal limit reached ({MAX_USER_TERMINALS} open terminals) — close one first"
        ));
    }
    let key = TabKey {
        root: resolved.terminal.root.clone(),
        tab_id: app.session_registry.next_terminal_id(),
    };
    Ok(ReservedShell {
        shell: app.term_shell.clone(),
        #[cfg(test)]
        spawn_gate: app.terminal_create_spawn_gate.clone(),
        hold: ShellHold::take(&mut app, &key, resolved, state, timer),
        key,
    })
}

fn refuse_project_deletion(app: &AppState) -> Result<(), String> {
    if app.project_deletion_in_progress {
        return Err(crate::app::rpc::PROJECT_DELETION_IN_PROGRESS.into());
    }
    Ok(())
}

fn validate_identity(
    app: &AppState,
    selector: &Selector,
    captured: &CapturedScope,
    resolved: &ResolvedScope,
) -> Result<(), String> {
    refuse_project_deletion(app)?;
    if !captured.revalidates(&selector.capture(app)?, resolved, app) {
        return Err(MOVED_TERMINAL_SOURCE.into());
    }
    app.refuse_source_shell_start_at(&resolved.terminal.root)?;
    if app.reclaim_reserved_at(&resolved.terminal.root) {
        return Err(crate::reclaim::RESERVED.into());
    }
    Ok(())
}

/// A checkout-scoped claim keeps its lifecycle and source identity stable
/// until the child is registered or reaped, without blocking other checkouts.
struct ShellHold<'timer> {
    token: Option<SpawnClaimToken>,
    state: Weak<Mutex<AppState>>,
    timer: &'timer FrameTimer,
}

impl<'timer> ShellHold<'timer> {
    fn take(
        app: &mut AppState,
        key: &TabKey,
        resolved: &ResolvedScope,
        state: &Arc<Mutex<AppState>>,
        timer: &'timer FrameTimer,
    ) -> Self {
        Self {
            token: Some(app.session_registry.take_shell_spawn_claim(
                key.clone(),
                resolved.project_id.clone(),
                resolved.workspace_id.clone(),
                resolved.raw_root.clone(),
            )),
            state: Arc::downgrade(state),
            timer,
        }
    }

    fn settle(&mut self, app: &mut AppState) {
        if let Some(token) = self.token.take() {
            app.session_registry.settle_spawn_claim(token);
        }
    }
}

impl Drop for ShellHold<'_> {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            if let Some(state) = self.state.upgrade() {
                if std::thread::panicking() || state.is_poisoned() {
                    // Destructors must not panic a second time while recovering
                    // a poisoned app; ordinary refusals use the frame timer.
                    let mut app = state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    app.session_registry.settle_spawn_claim(token);
                } else {
                    let mut app = self.timer.lock(&state);
                    #[cfg(test)]
                    let _phase = crate::timing::phase_samples::phase("term.create.cancel");
                    app.session_registry.settle_spawn_claim(token);
                }
            }
        }
    }
}

/// Revalidation errors and unwinding retire the exact unregistered child after
/// the timed guard is released. A child can never escape with no registry owner.
struct UnpublishedShell(Option<(Tab, SessionOutput)>);

impl Drop for UnpublishedShell {
    fn drop(&mut self) {
        if let Some((tab, _output)) = self.0.take() {
            // Keep the checkout reservation until this unregistered child is
            // gone, so a source move cannot race its retirement.
            tab.session.end();
        }
    }
}
