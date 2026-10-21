//! Shell admission holds its checkout and one capacity slot while the process
//! opens outside the application mutex. Only a revalidated shell is registered.
use super::{require_shell_kind, resolve_terminal_scope, shell_harness_spec, terminal_size};
use super::{TerminalScope, MAX_USER_TERMINALS, MOVED_TERMINAL_SOURCE, SHELL_TAB_KIND};
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
    let mut reserved = reserve(state, params, timer)?;
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
    tab.project_source = reserved.scope.source.clone();
    let mut opened = UnpublishedShell(Some((tab, output)));
    #[cfg(test)]
    if let Some(gate) = &reserved.spawn_gate {
        gate.arrive();
    }
    let pumps = {
        let mut app = timer.lock(state);
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("term.create.commit");
        let current = admitted_scope(&mut app, params)?;
        if current.root != reserved.scope.root || current.source != reserved.scope.source {
            return Err(MOVED_TERMINAL_SOURCE.into());
        }
        let (tab, output) = opened
            .0
            .take()
            .expect("an unpublished shell owns its child");
        let pumps = app
            .session_registry
            .insert_shell(reserved.key.clone(), tab, output);
        reserved.hold.settle(&mut app);
        app.note_terminals_at(&reserved.key.root);
        pumps
    };
    spawn_tab_pumps(state, reserved.key.clone(), pumps);
    Ok(json!({"term_id":reserved.key.tab_id,"kind":SHELL_TAB_KIND,"cols":cols,"rows":rows}))
}

struct ReservedShell<'timer> {
    key: TabKey,
    scope: TerminalScope,
    shell: String,
    hold: ShellHold<'timer>,
    #[cfg(test)]
    spawn_gate: Option<crate::app::OffLockGate>,
}

fn reserve<'timer>(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &'timer FrameTimer,
) -> Result<ReservedShell<'timer>, String> {
    let mut app = timer.lock(state);
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("term.create.reserve");
    let scope = admitted_scope(&mut app, params)?;
    if app.shell_tab_count() + app.session_registry.pending_shell_count() >= MAX_USER_TERMINALS {
        return Err(format!(
            "terminal limit reached ({MAX_USER_TERMINALS} open terminals) — close one first"
        ));
    }
    let key = TabKey {
        root: scope.root.clone(),
        tab_id: app.session_registry.next_terminal_id(),
    };
    Ok(ReservedShell {
        shell: app.term_shell.clone(),
        #[cfg(test)]
        spawn_gate: app.terminal_create_spawn_gate.clone(),
        hold: ShellHold::take(&mut app, &key, state, timer),
        key,
        scope,
    })
}

fn admitted_scope(app: &mut AppState, params: &Value) -> Result<TerminalScope, String> {
    if app.project_deletion_in_progress {
        return Err(crate::app::rpc::PROJECT_DELETION_IN_PROGRESS.into());
    }
    let scope = resolve_terminal_scope(app, params)?;
    app.refuse_source_shell_start_at(&scope.root)?;
    app.refuse_writers_while_reserved(&scope.root)?;
    Ok(scope)
}

/// The existing filesystem-job hold keeps deletion, lock, rename and reclaim
/// from changing the checkout before the spawn commits. A registry claim also
/// holds source movement and counts against the terminal cap.
struct ShellHold<'timer> {
    token: Option<SpawnClaimToken>,
    state: Weak<Mutex<AppState>>,
    timer: &'timer FrameTimer,
}

impl<'timer> ShellHold<'timer> {
    fn take(
        app: &mut AppState,
        key: &TabKey,
        state: &Arc<Mutex<AppState>>,
        timer: &'timer FrameTimer,
    ) -> Self {
        app.active_deferred_filesystem_jobs += 1;
        Self {
            token: Some(app.session_registry.take_spawn_claim(key.clone())),
            state: Arc::downgrade(state),
            timer,
        }
    }

    fn settle(&mut self, app: &mut AppState) {
        if let Some(token) = self.token.take() {
            app.session_registry.settle_spawn_claim(token);
            app.active_deferred_filesystem_jobs -= 1;
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
                    app.active_deferred_filesystem_jobs -= 1;
                } else {
                    let mut app = self.timer.lock(&state);
                    #[cfg(test)]
                    let _phase = crate::timing::phase_samples::phase("term.create.cancel");
                    app.session_registry.settle_spawn_claim(token);
                    app.active_deferred_filesystem_jobs -= 1;
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
            // gone, so a source move cannot race its asynchronous retirement.
            tab.session.end();
        }
    }
}
