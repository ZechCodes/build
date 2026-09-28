//! What `models.list` offers once the installed CLIs have answered, and the
//! `models.changed` push that says they answered differently (#203).

use std::sync::Mutex;

use super::*;
use crate::harness::installed::{CliProbe, CliReading, ListedModel, Readings};

/// Every installed CLI at once: Claude Code at whatever version the test set,
/// and a codex that lists two models, one hidden.
struct InstalledClis {
    claude: Mutex<&'static str>,
}

impl InstalledClis {
    fn leaked(claude: &'static str) -> &'static Self {
        Box::leak(Box::new(Self {
            claude: Mutex::new(claude),
        }))
    }
}

impl CliProbe for InstalledClis {
    fn read(&self, binary: &str) -> CliReading {
        let version = |raw: &str| Some(semver::Version::parse(raw).unwrap());
        match binary {
            "claude" => CliReading {
                version: version(&self.claude.lock().unwrap()),
                listed: None,
            },
            "codex" => CliReading {
                version: version("0.155.1"),
                listed: Some(vec![
                    ListedModel {
                        id: "gpt-6-sol".into(),
                        label: "GPT-6-Sol".into(),
                        hidden: false,
                        efforts: vec!["low".into(), "ultra".into()],
                    },
                    ListedModel {
                        id: "gpt-reserve".into(),
                        label: "GPT-Reserve".into(),
                        hidden: true,
                        efforts: vec![],
                    },
                ]),
            },
            _ => CliReading::default(),
        }
    }
}

fn provider<'a>(listed: &'a Value, id: &str) -> &'a Value {
    listed["result"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {listed}"))
}

fn model_ids(provider: &Value) -> Vec<&str> {
    provider["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap())
        .collect()
}

#[test]
fn models_list_offers_what_the_installed_clis_run() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock")
        .with_cli_readings(Readings::answering_inline(InstalledClis::leaked("2.1.280")));

    let listed = state.handle(req("models.list", json!({})));

    assert_eq!(listed["ok"], true, "{listed}");
    for id in ["claude", "claude_adk"] {
        let claude = provider(&listed, id);
        assert!(!model_ids(claude).contains(&"claude-sonnet-5-5"), "{id}");
        assert!(model_ids(claude).contains(&"claude-opus-5-5"), "{id}");
        assert_eq!(claude["cli_name"], "Claude Code");
        assert_eq!(claude["cli_version"], "2.1.280");
        assert_eq!(
            claude["unavailable"],
            json!([{ "id": "claude-sonnet-5-5", "label": "Claude Sonnet 5.5", "requires_cli": "2.1.284" }])
        );
    }
    assert!(
        !listed["result"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["id"] == "claude-sonnet-5-5"),
        "the flat list older clients read is the default harness's offer: {listed}"
    );
    for id in ["codex", "codex_app_server"] {
        let codex = provider(&listed, id);
        assert_eq!(model_ids(codex), ["gpt-6-sol"], "{id}");
        assert_eq!(codex["models"][0]["efforts"], json!(["low", "ultra"]));
        assert_eq!(codex["cli_name"], "Codex");
        assert_eq!(codex["cli_version"], "0.155.1");
        assert_eq!(codex["unavailable"], json!([]));
    }
}

#[test]
fn list_harnesses_answers_the_same_offer() {
    let (dir, repo) = init_repo();
    let state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock")
        .with_cli_readings(Readings::answering_inline(InstalledClis::leaked("2.1.280")));

    let table = state.harness_table();
    let claude = table["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|harness| harness["id"] == "claude_adk")
        .unwrap();

    assert_eq!(claude["cli_version"], "2.1.280");
    assert_eq!(claude["unavailable"][0]["requires_cli"], "2.1.284");
}

async fn next_models_changed(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: &str,
    within: Duration,
) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let outbound = tokio::time::timeout_at(deadline, rx.recv()).await.ok()??;
        let event = SessionSender::decrypt_push(key, &outbound);
        if event["type"] == "models.changed" {
            return Some(event);
        }
    }
}

#[tokio::test]
async fn a_greeted_session_hears_when_an_installed_cli_changes() {
    let (dir, repo) = init_repo();
    let clis = InstalledClis::leaked("2.1.280");
    let readings = Readings::answering_inline(clis);
    let state = qa_state(&repo, dir.path())
        .with_cli_readings(Arc::clone(&readings))
        .shared();
    let handler = AppState::handler(state);
    let (session, mut rx, key) = SessionSender::observable("models");

    let greeting = handler.call(session.clone(), req("session.hello", json!({})));
    assert!(greeting["result"]["events"]
        .as_array()
        .unwrap()
        .contains(&json!("models.changed")));
    assert!(greeting["result"]["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("models.installedCli")));
    let first = handler.call(session.clone(), req("models.list", json!({})));
    assert_eq!(first["ok"], true, "{first}");
    let first_news = next_models_changed(&mut rx, &key, Duration::from_secs(5)).await;
    assert_eq!(first_news, Some(json!({ "type": "models.changed" })));

    // The same answer asked again is no news.
    readings.observe_version("claude", &crate::harness::installed::VERSION_FLAG, &semver::Version::parse("2.1.280").unwrap());
    assert_eq!(
        next_models_changed(&mut rx, &key, Duration::from_millis(300)).await,
        None
    );

    // Claude Code is updated, and a session says so.
    *clis.claude.lock().unwrap() = "2.1.284";
    readings.observe_version("claude", &crate::harness::installed::VERSION_FLAG, &semver::Version::parse("2.1.284").unwrap());
    assert!(next_models_changed(&mut rx, &key, Duration::from_secs(5))
        .await
        .is_some());
    let after = handler.call(session, req("models.list", json!({})));
    assert!(model_ids(provider(&after, "claude_adk")).contains(&"claude-sonnet-5-5"));
    assert_eq!(provider(&after, "claude_adk")["unavailable"], json!([]));
}
