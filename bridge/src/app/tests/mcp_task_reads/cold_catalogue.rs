//! A listener created before snapshot warming follows successful cold config.
use super::*;
use crate::app::read_context::mcp_metadata_metrics;
use crate::harness::installed::{CliProbe, CliReading, Readings};

struct StableClis;
impl CliProbe for StableClis {
    fn read(&self, _: &str) -> CliReading {
        CliReading::default()
    }
}
static STABLE_CLIS: StableClis = StableClis;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_cold_catalogue_existing_listener_follows_only_committed_settings() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = super::super::tracker::tracked(root.path());
    let mut app = app
        .with_config(root.path().join("config.json"))
        .unwrap()
        .with_cli_readings(Readings::answering_inline(&STABLE_CLIS));
    let (_owner, agent) = super::super::project_agent::project_agent(&mut app, &project);
    app.delivery_queue.clear_queued();
    app.test_install_mcp_token(agent.clone(), TOKEN.into());
    app.initialize_read_context();
    assert!(!app.snapshot_projection_initialized());
    let initial_view = app.read_view.load();
    assert!(initial_view.observations.is_none());
    assert!(initial_view.global.is_none());
    assert!(initial_view.get_shared("settings.get", "").is_none());
    let addresses = app.read_service().mcp_addresses();
    let original_default = app.default_harness;
    let selected_default = if original_default == crate::models::AgentProvider::Pi {
        crate::models::AgentProvider::ClaudeAdk
    } else {
        crate::models::AgentProvider::Pi
    };
    let clock = app.frame_clock.clone();
    let state = Arc::new(Mutex::new(app));
    // Keep this same listener/service across every successful and failed save.
    let plane = ControlPlane::new(&state.lock().unwrap());
    let frame =
        json!({"task_id":agent,"session_token":TOKEN,"request":BridgeAction::ListHarnesses});
    let mut held = Vec::new();
    for _ in 0..crate::app::mcp::CONTROL_FRAMES_AT_THE_LOCK {
        held.push(ControlTurn::take(&plane, &clock.frame(MCP_CONTROL_METHOD)).await);
    }
    let mut replies = Vec::new();
    let mut expected = Vec::new();
    expected.push(cold_table(&state));
    replies.push(socket_reply(state.clone(), plane.clone(), frame.clone()).await);

    let roles = json!([
        {"model":"cold-first","roles":["reviewer"],"capability":"generalist"},
        {"provider":"claude_adk","model":"cold-second","roles":["reviewer"],"capability":"generalist"}
    ]);
    let mut source_work = Vec::new();
    for patch in [
        json!({"default_harness":selected_default}),
        json!({"role_models":roles}),
    ] {
        let mut app = state.lock().unwrap();
        let (result, work) = mcp_metadata_metrics::measure(|| app.settings_set(&patch));
        result.unwrap();
        source_work.push(work);
        assert_cold_unchanged(&app, &initial_view, &addresses);
        drop(app);
        expected.push(cold_table(&state));
        replies.push(socket_reply(state.clone(), plane.clone(), frame.clone()).await);
    }
    let disk = std::fs::read(root.path().join("config.json")).unwrap();
    let retained = cold_table(&state);
    for failure in [
        crate::app::ConfigPersistStep::Write,
        crate::app::ConfigPersistStep::Rename,
    ] {
        let mut app = state.lock().unwrap();
        app.config_persist_failure = Some(failure);
        let (result, work) = mcp_metadata_metrics::measure(|| {
            app.settings_set(&json!({
            "default_harness":original_default,
            "role_models":[{"model":"failed-role","roles":["planner"],"capability":"generalist"}]
        }))
        });
        app.config_persist_failure = None;
        assert!(result.is_err());
        source_work.push(work);
        assert_eq!(app.default_harness, selected_default);
        assert_eq!(serde_json::to_value(&app.role_models).unwrap(), roles);
        assert_cold_unchanged(&app, &initial_view, &addresses);
        assert_eq!(
            std::fs::read(root.path().join("config.json")).unwrap(),
            disk
        );
        drop(app);
        expected.push(retained.clone());
        replies.push(socket_reply(state.clone(), plane.clone(), frame.clone()).await);
    }
    drop(held);
    {
        let app = state.lock().unwrap();
        assert_cold_unchanged(&app, &initial_view, &addresses);
    }
    // Complete all writes/refusals and join socket tasks before asserting the
    // intended RED; a stale bootstrap must not hide failure controls.
    assert_eq!(
        expected[1]["result"]["default_harness"],
        json!(selected_default)
    );
    assert_eq!(expected[2]["result"]["role_models"], roles);
    assert_eq!(
        expected[2]["result"]["roles_in_effect"]["reviewer"]["model"],
        "cold-first"
    );
    assert_eq!(
        source_work,
        vec![(0, 0); 4],
        "config publication must not recapture saved rosters/indexes"
    );
    assert_eq!(
        replies, expected,
        "pre-existing cold MCP service retained stale accepted settings"
    );
}

fn cold_table(state: &Arc<Mutex<AppState>>) -> Value {
    let app = state.lock().unwrap();
    crate::app::mcp::mcp_action_response(Ok(app.harness_table()))
}

fn assert_cold_unchanged(
    app: &AppState,
    previous: &Arc<crate::app::read_view::ReadView>,
    addresses: &Arc<crate::app::read_context::McpAddresses>,
) {
    assert!(
        !app.snapshot_projection_initialized(),
        "cold config/read must not initialize board observations"
    );
    assert!(
        Arc::ptr_eq(previous, &app.read_view.load()),
        "cold settings/read recaptured an accepted projection"
    );
    assert!(
        Arc::ptr_eq(addresses, &app.read_service().mcp_addresses()),
        "settings hook rebuilt saved identity metadata"
    );
}
