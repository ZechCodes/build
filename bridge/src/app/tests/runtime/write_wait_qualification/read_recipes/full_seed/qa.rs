//! Final startup metadata only; canonical item count must be identical
//! before and after these actual RPCs. Their requests are bootstrap, not read/
//! semantic trial samples. Retain every proof and own the finite shell child.
use crate::app::tests::runtime::write_wait_qualification::rpc::RpcHarness;
use crate::app::tests::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) struct ReadyAuxiliary {
    pub stream: String,
    pub shell: String,
    pub child: Arc<dyn AgentSession>,
    pub shell_root: PathBuf,
}

pub(super) fn prepare_shell(app: &mut AppState, root: &Path) {
    let shell = root.join("qualification-read-shell");
    std::fs::write(&shell,b"#!/bin/sh\nprintf 'QUALIFICATION-READY\\n'\nwhile IFS= read -r line; do\n case \"$line\" in\n  QUALIFICATION-PING*) printf 'QUALIFICATION-PONG\\n' ;;\n  QUALIFICATION-EXIT*) exit 0 ;;\n esac\ndone\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    app.term_shell = shell.to_str().unwrap().to_owned();
}

pub(super) async fn finalize(rpc: &RpcHarness, workspace: &str, source: &str) -> ReadyAuxiliary {
    let stream = rpc
        .request(
            0,
            "qual/bootstrap/read-stream/start",
            "stream.start",
            json!({"count":8,"interval_ms":0}),
        )
        .await;
    assert_eq!(stream.reply["ok"], true, "{}", stream.reply);
    let stream = stream.reply["result"]["stream_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut poll = 0;
    loop {
        let id = format!("qual/bootstrap/read-stream/state/{poll}");
        let proof = rpc
            .request(0, &id, "stream.state", json!({"stream_id":stream}))
            .await;
        assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
        if proof.reply["result"]["complete"] == true {
            assert_eq!(proof.reply["result"]["head"], 9);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native eight-item stream failed startup"
        );
        poll += 1;
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let shell = rpc
        .request(
            0,
            "qual/bootstrap/read-shell/create",
            "term.create",
            json!({
        "workspace_id":workspace,"source_id":source,"cols":80,"rows":24}),
        )
        .await;
    assert_eq!(shell.reply["ok"], true, "{}", shell.reply);
    let shell = shell.reply["result"]["term_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (child, shell_root) = {
        let app = rpc.state.lock().unwrap();
        let key = app.tab_key_of_wire_id(&shell).unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        assert!(matches!(tab.role, TabRole::Shell));
        assert!(tab.session.terminal().is_some());
        (tab.session.clone(), key.root)
    };
    ReadyAuxiliary {
        stream,
        shell,
        child,
        shell_root,
    }
}

impl ReadyAuxiliary {
    pub(super) async fn shutdown(&self, rpc: &RpcHarness, workspace: &str, source: &str) {
        assert!(
            self.shell_root.is_dir(),
            "fixture root dropped before native shell retirement"
        );
        let close = rpc
            .request(
                0,
                "qual/cleanup/read-shell/close",
                "term.close",
                json!({
            "term_id":self.shell,"workspace_id":workspace,"source_id":source}),
            )
            .await;
        let child = self.child.clone();
        // term.close already requested native retirement. Idempotent end joins
        // this exact retained child even if the retirement worker is still due.
        tokio::task::spawn_blocking(move || {
            child.end();
            assert!(
                child.exited_within(Duration::from_secs(2)),
                "fixture shell was not reaped"
            );
        })
        .await
        .expect("fixture shell reap worker joined");
        assert_eq!(close.reply["ok"], true, "{}", close.reply);
        let app = rpc.state.lock().unwrap();
        assert!(
            app.tab_key_of_wire_id(&self.shell).is_err(),
            "closed shell remains addressable"
        );
    }
}
