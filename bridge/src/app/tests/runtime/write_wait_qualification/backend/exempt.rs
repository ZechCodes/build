//! OFFSOURCE: actual finite private PTY and bounded subscription/control load.
//! No semantic write credit or throughput is awarded to these protocol frames.
use super::super::rpc::RpcHarness;
use super::super::trial_complete::{OracleAssertion, OracleWitness, Slot, WitnessScalar};
use crate::app::tests::runtime::write_wait_qualification::fixture::WorkspaceContext;
use crate::app::tests::*;
use serde_json::{json, Value};

pub(super) struct ExemptTerminal {
    pub id: String,
    pub workspace: String,
    pub source: String,
    child: Arc<dyn AgentSession>,
}

impl ExemptTerminal {
    pub async fn create(rpc: &RpcHarness, workspace: &WorkspaceContext) -> Self {
        let created = rpc.request(0, "qual/bootstrap/exempt-shell/create", "term.create", json!({
            "workspace_id":workspace.workspace_id,"source_id":workspace.source_id,"cols":80,"rows":24,
        })).await;
        assert_eq!(created.reply["ok"], true, "{}", created.reply);
        let id = created.reply["result"]["term_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let child = {
            let app = rpc.state.lock().unwrap();
            let key = app.tab_key_of_wire_id(&id).unwrap();
            let tab = app.session_registry.test_tab(&key).unwrap();
            assert!(matches!(tab.role, TabRole::Shell));
            assert!(tab.session.terminal().is_some());
            Arc::clone(&tab.session)
        };
        for caller in 0..4 {
            let attached = rpc.request(caller, &format!("qual/bootstrap/exempt-shell/attach/{caller}"), "term.attach",
                json!({"term_id":id,"workspace_id":workspace.workspace_id,"source_id":workspace.source_id,"cols":80,"rows":24})).await;
            assert_eq!(attached.reply["ok"], true, "{}", attached.reply);
            assert_eq!(attached.reply["result"]["term_id"], id);
            assert!(attached.reply["result"]["cursor"].as_u64().is_some());
        }
        Self {
            id,
            workspace: workspace.workspace_id.clone(),
            source: workspace.source_id.clone(),
            child,
        }
    }

    pub fn params(&self, slot: &Slot) -> Result<Value, String> {
        let method = slot
            .method
            .as_deref()
            .ok_or("exempt slot has no actual method")?;
        let subscription = format!("qual-exempt-spa-{}", slot.caller);
        match method {
            "term.input" => Ok(
                json!({"term_id":self.id,"data":b64encode(format!("QUALIFICATION-PING {}\n",slot.original_id).as_bytes())}),
            ),
            // Cursor0 is a valid idempotent flow-control observation, never a
            // claim that the client applied bytes it did not actually observe.
            "term.ack" => Ok(json!({"term_id":self.id,"cursor":0})),
            "changes.subscribe" => Ok(
                json!({"subscription_id":subscription,"scope":{"kind":"all"},"kinds":["state"],"mode":"realtime"}),
            ),
            "changes.unsubscribe" => Ok(json!({"subscription_id":subscription})),
            "ping" => Ok(json!({})),
            _ => Err(format!("undeclared exempt protocol method {method}")),
        }
    }

    pub async fn shutdown(self, rpc: &RpcHarness) {
        // Called only after quiet watch detachment. These are explicitly
        // recorded cleanup requests, not new unregistered trial identities.
        let closed = rpc
            .request(
                0,
                "qual/cleanup/exempt-shell/close",
                "term.close",
                json!({
                    "term_id":self.id,"workspace_id":self.workspace,"source_id":self.source,
                }),
            )
            .await;
        let reaped = tokio::task::spawn_blocking(move || {
            self.child.end();
            self.child.exited_within(Duration::from_secs(2))
        })
        .await;
        assert_eq!(closed.reply["ok"], true, "{}", closed.reply);
        assert!(
            matches!(reaped, Ok(true)),
            "actual exempt shell did not join: {reaped:?}"
        );
    }
}

/// Control frames prove actual positive protocol replies; they deliberately
/// do not claim semantic mutations, nonempty pushes or provider execution.
pub(super) fn assert_reply(slot: &Slot, reply: &Value) -> Result<OracleWitness, String> {
    if reply["id"] != slot.original_id || reply["ok"] != true || !reply["error"].is_null() {
        return Err(format!(
            "actual exempt reply failed exact ID/protocol success: {reply}"
        ));
    }
    Ok(OracleWitness::State {
        oracle: format!(
            "actual-exempt-wire/{}",
            slot.method.as_deref().ok_or("missing method")?
        ),
        assertions: vec![OracleAssertion {
            field: "positive_protocol_final".into(),
            actual: WitnessScalar::Bool(true),
            expected: WitnessScalar::Bool(true),
        }],
    })
}

pub(super) async fn install_file_observation_subscriptions(
    rpc: &RpcHarness,
    active: &[WorkspaceContext],
) {
    assert_eq!(active.len(), 8);
    for (index, workspace) in active.iter().enumerate() {
        let proof = rpc.request(index % 4, &format!("qual/bootstrap/files/subscribe/{index}"), "changes.subscribe", json!({
            "subscription_id":format!("qual-files-{index}"),"scope":{"kind":"entity","id":workspace.owner},"kinds":["files"],"mode":"realtime",
        })).await;
        assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
    }
}
