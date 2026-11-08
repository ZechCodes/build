use super::*;
use crate::harness::{AgentSession, AgentStatus, HarnessError, Turn};
use crate::reaper::Retirement;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

struct HeldEnd {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    ended: AtomicBool,
}

struct EndControl {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

impl HeldEnd {
    fn new() -> (Arc<dyn AgentSession>, EndControl) {
        let (entered, arrived) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        (
            Arc::new(Self {
                entered,
                release: Mutex::new(blocked),
                ended: AtomicBool::new(false),
            }),
            EndControl {
                entered: arrived,
                release,
            },
        )
    }
}

impl AgentSession for HeldEnd {
    fn send_turn(&self, _: &Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> AgentStatus {
        if self.ended.load(Ordering::SeqCst) {
            AgentStatus::Ended { code: Some(0) }
        } else {
            AgentStatus::Working
        }
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
    fn backdate_last_output(&self, _: Duration) {}
    fn end(&self) {
        self.entered.send(()).ok();
        self.release.lock().unwrap().recv().ok();
        self.ended.store(true, Ordering::SeqCst);
    }
}

fn held_tab(key: &TabKey, role: crate::app::TabRole) -> (crate::app::Tab, EndControl) {
    let (session, control) = HeldEnd::new();
    (
        crate::app::Tab {
            tab_id: key.tab_id.clone(),
            root: key.root.clone(),
            role,
            project_source: None,
            created_at: crate::store::now_rfc3339(),
            session,
            session_instance: None,
            screen: None,
            live: true,
            call_sequences: std::collections::HashMap::new(),
            last_delivered_at: None,
        },
        control,
    )
}

fn install_shell(f: &Fixture) -> (TabKey, EndControl) {
    let mut app = f.state.lock().unwrap();
    let root = AppState::canonical_root(&app.project(&f.project).unwrap().repo_path);
    let key = TabKey {
        root: root.clone(),
        tab_id: "term-held".into(),
    };
    let (tab, control) = held_tab(&key, crate::app::TabRole::Shell);
    let claim =
        app.session_registry
            .take_shell_spawn_claim(key.clone(), f.project.clone(), None, root);
    app.session_registry.test_insert_tab(key.clone(), tab);
    app.session_registry.settle_spawn_claim(claim);
    (key, control)
}

fn finish(control: EndControl, retirement: &Retirement) {
    control
        .entered
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    control.release.send(()).unwrap();
    assert!(
        retirement.wait(Duration::from_secs(5)),
        "wait only after releasing the app guard"
    );
}

fn closing_refusal(value: &Value) {
    assert_eq!(value["error_code"], "conflict", "{value}");
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("wait for the closed terminal to exit"),
        "{value}"
    );
}

#[test]
fn archive_retiring_closed_shell_blocks_until_reaped() {
    let f = Fixture::new();
    let (key, control) = install_shell(&f);
    // The common close primitive returns the receipt term.close discards.
    let retirement = f.state.lock().unwrap().retire_tab(&key, "closed").unwrap();
    let generation = f.state.lock().unwrap().session_registry.shell_generation();
    let refused = f.archive();
    assert!(f
        .state
        .lock()
        .unwrap()
        .session_registry
        .shell_tabs()
        .is_empty());
    finish(control, &retirement);
    closing_refusal(&refused);
    assert_eq!(f.archive()["ok"], true);
    assert_eq!(
        f.state.lock().unwrap().session_registry.shell_generation(),
        generation
    );
}

#[test]
fn archive_retiring_replacement_shell_blocks_until_reaped() {
    let f = Fixture::new();
    let (key, control) = install_shell(&f);
    let retirement = f
        .state
        .lock()
        .unwrap()
        .retire_tab_keeping_screen(&key)
        .unwrap()
        .0;
    let refused = f.archive();
    finish(control, &retirement);
    closing_refusal(&refused);
    assert_eq!(f.archive()["ok"], true);
}

#[test]
fn archive_retiring_shell_of_another_project_does_not_block() {
    let f = Fixture::new();
    let (key, control) = install_shell(&f);
    let other = {
        let mut app = f.state.lock().unwrap();
        let other = crate::app::projects::test_support::register_sibling(&mut app, &f.project);
        app.refresh_workspace_snapshot_observations();
        for mut workspace in app
            .workspaces
            .list(Some(&other))
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
        {
            app.workspaces.record_finished(&mut workspace).unwrap();
            app.workspaces.apply_local_capabilities(workspace).unwrap();
        }
        other
    };
    let retirement = f.state.lock().unwrap().retire_tab(&key, "closed").unwrap();
    let archived = call(&f.handler, "project.archive", json!({"project_id":other}));
    let refused = f.archive();
    finish(control, &retirement);
    assert_eq!(archived["ok"], true, "{archived}");
    closing_refusal(&refused);
    assert_eq!(f.archive()["ok"], true);
}

#[test]
fn archive_retiring_project_agent_blocks_until_reaped() {
    let f = Fixture::new();
    let (key, control) = {
        let mut app = f.state.lock().unwrap();
        let (owner, agent) =
            super::super::super::project_agent::project_agent(&mut app, &f.project);
        let root = AppState::canonical_root(&app.entity_agent_root(&owner).unwrap());
        let key = TabKey::agent(&root, &agent);
        let (tab, control) = held_tab(
            &key,
            crate::app::TabRole::Agent {
                owner,
                agent_id: agent,
                provider: crate::models::AgentProvider::default(),
            },
        );
        app.session_registry.insert_opened(key.clone(), tab);
        (key, control)
    };
    let retirement = f.state.lock().unwrap().retire_tab(&key, "closed").unwrap();
    let refused = f.archive();
    finish(control, &retirement);
    assert_eq!(refused["error_code"], "conflict", "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("wait for the stopped agent to exit"),
        "{refused}"
    );
    assert_eq!(f.archive()["ok"], true);
}
