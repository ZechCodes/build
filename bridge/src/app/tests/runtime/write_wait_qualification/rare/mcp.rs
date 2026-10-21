//! Actual authenticated MCP socket requests, sharing the RPC origin collector.
use super::*;
use crate::app::mcp::{handle_done_stream, ControlPlane};
use crate::mcp::{BridgeAction, DoneServer, McpSurface};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

impl RareFixture {
    pub(super) async fn call_mcp(
        &self,
        caller: &str,
        token: &str,
        action: BridgeAction,
    ) -> ReplyProof {
        let proof = self.call_mcp_response(caller, token, action).await;
        assert_eq!(proof.reply["ok"], true, "{}: {}", proof.method, proof.reply);
        proof
    }

    pub(super) async fn call_mcp_tool(
        &self,
        caller: &str,
        token: &str,
        surface: McpSurface,
        name: &str,
        arguments: Value,
    ) -> ReplyProof {
        let server = DoneServer::launched(caller, Some(surface.as_str()));
        let parsed = server.handle_message(
            &json!({"jsonrpc":"2.0","id":"rare-tool-parser","method":"tools/call",
                "params":{"name":name,"arguments":arguments}})
            .to_string(),
        );
        assert!(
            parsed.report.is_none(),
            "terminal reports need the no-reply report recipe"
        );
        let action = parsed.action.unwrap_or_else(|| {
            panic!(
                "{name} did not parse to a daemon action: {:?}",
                parsed.reply
            )
        });
        self.call_mcp(caller, token, action).await
    }

    pub(super) async fn call_mcp_response(
        &self,
        caller: &str,
        token: &str,
        action: BridgeAction,
    ) -> ReplyProof {
        assert!(!self.breached.load(Ordering::Acquire));
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        let original_id = format!(
            "qual/rare/{}/{}/mcp/{sequence:04}",
            self.family, self.fixture_id
        );
        let method = format!("mcp.control:{}", action.tool_name());
        let action = serde_json::to_value(action).unwrap();
        let plane = ControlPlane::new(&self.state().lock().unwrap());
        let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(handle_done_stream(Arc::clone(self.state()), daemon, plane));
        let mut client = tokio::io::BufReader::new(client);
        let frame =
            json!({"id":original_id,"task_id":caller,"session_token":token,"request":action});
        let sent_at = Instant::now();
        client
            .get_mut()
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        let read = tokio::time::timeout(WAIT_BUDGET, client.read_line(&mut response)).await;
        let final_reply_at = Instant::now();
        // Close and join the actual connection before inspecting its response.
        drop(client);
        let joined = tokio::time::timeout(WAIT_BUDGET, server).await;
        joined
            .expect("rare MCP server did not stop after owned socket close")
            .unwrap();
        assert!(
            read.expect("rare MCP response exceeded five seconds")
                .unwrap()
                > 0
        );
        let reply: Value = serde_json::from_str(&response).unwrap();
        eprintln!(
            "rare_mcp_reply={}",
            json!({"original_id":original_id,"method":method,"request":action,"reply":reply})
        );
        self.mcp_actions
            .lock()
            .unwrap()
            .insert(original_id.clone(), action);
        ReplyProof {
            original_id,
            method,
            reply,
            sent_at,
            final_reply_at,
        }
    }
}
