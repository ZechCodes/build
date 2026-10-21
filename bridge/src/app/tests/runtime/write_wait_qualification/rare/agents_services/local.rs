use super::*;
use crate::session_summary::{UserSession, USER_SESSION_GAP_MS};

const SID: &str = "fktz0HaQPrTBONQORgaYdrkQLm41LU5PVktHaqho6To";
const KEY: &str =
    "BHKs2HtDjA-uaSJ6Pgf6fSznoiTDkYCYcz7DWi9OXLG0awHT6hXC01Suu7R305cuZOrOj_tPPOhZuv3udC929qc";

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repository) = init_repo();
    let config = directory.path().join("config.json");
    let app = qa_state(&repository, directory.path())
        .with_config(&config)
        .unwrap();
    assert!(!app.watch_agent_filed_tasks);
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .list_push_keys()
        .unwrap()
        .is_empty());
    let mut fixture = RareFixture::new("local-settings-keys-session", app);

    let saved = fixture
        .call("settings.set", json!({"watch_agent_filed_tasks":true}))
        .await;
    let disk: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    assert_eq!(disk["watch_agent_filed_tasks"], true);
    assert!(fixture.state().lock().unwrap().watch_agent_filed_tasks);
    let read = fixture.call("settings.get", json!({})).await;
    assert_eq!(read.reply["result"]["watch_agent_filed_tasks"], true);
    fixture
        .verify(
            saved,
            json!({"before":false,"persisted":disk["watch_agent_filed_tasks"]}),
        )
        .await;

    let registered = fixture
        .call(
            "push.registerKey",
            json!({"subscription_id":SID,"public_key":KEY}),
        )
        .await;
    let keys = fixture
        .state()
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .list_push_keys()
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].subscription_id, SID);
    assert_eq!(keys[0].public_key, KEY);
    fixture
        .verify(
            registered,
            json!({"before_count":0,"stored_subscription":keys[0].subscription_id,"after_count":1}),
        )
        .await;

    let revoked = fixture
        .call("push.revokeKey", json!({"subscription_id":SID}))
        .await;
    let reopened = Store::new(directory.path().join("store")).unwrap();
    assert!(reopened.list_push_keys().unwrap().is_empty());
    fixture
        .verify(
            revoked,
            json!({"before_count":1,"revoked_subscription":SID,"after_count":0}),
        )
        .await;

    let now = i64::try_from(crate::agent::now_ms()).unwrap();
    let old = UserSession {
        session_started_ms: Some(now - 30 * 60 * 60 * 1000),
        last_activity_ms: Some(now - 20 * 60 * 60 * 1000),
        previous_session_ended_ms: None,
    };
    {
        let mut app = fixture.state().lock().unwrap();
        app.user_session = old;
        app.store.as_ref().unwrap().save_user_session(&old).unwrap();
    }
    let before = i64::try_from(crate::agent::now_ms()).unwrap();
    let present = fixture.call("user.present", json!({})).await;
    let stored = reopened.load_user_session().unwrap().unwrap();
    assert!(stored.session_started_ms >= Some(before));
    assert_eq!(stored.previous_session_ended_ms, old.last_activity_ms);
    assert_eq!(
        present.reply["result"]["user_session"]["session_started_ms"],
        json!(stored.session_started_ms)
    );
    assert_eq!(
        present.reply["result"]["user_session"]["gap_ms"],
        USER_SESSION_GAP_MS
    );
    fixture.verify(present, json!({"previous_last_activity_ms":old.last_activity_ms,"saved_session_started_ms":stored.session_started_ms,"saved_previous_ended_ms":stored.previous_session_ended_ms})).await;
    fixture.finish().await
}
