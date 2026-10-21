use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

const SCRIPT: &str = "#!/bin/sh\nprintf 'QUAL-READY\\n'\ncount=0\nwhile IFS= read -r line; do\n  count=$((count + 1))\n  case \"$line\" in\n    *QUAL-PING*) printf 'QUAL-PONG\\n' ;;\n    *QUAL-EXIT*) exit 0 ;;\n  esac\n  [ \"$count\" -lt 16 ] || exit 0\ndone\n";

struct EndOwnedChild(Arc<dyn AgentSession>);

impl Drop for EndOwnedChild {
    fn drop(&mut self) {
        self.0.end();
    }
}

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repository) = init_repo();
    let script = directory.path().join("qualification-terminal");
    std::fs::write(&script, SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut app = qa_state(&repository, directory.path());
    app.term_shell = script.to_string_lossy().into_owned();
    let project = app.project_at(0).id.clone();
    let mut fixture = RareFixture::new("terminal", app);

    let started = Instant::now();
    let created = fixture
        .call(
            "term.create",
            json!({"project_id":project,"cols":80,"rows":24}),
        )
        .await;
    let create_elapsed = started.elapsed();
    let id = created.reply["result"]["term_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (pid, root, owned_child) = {
        let app = fixture.state().lock().unwrap();
        let key = app.tab_key_of_wire_id(&id).unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        assert!(matches!(tab.role, TabRole::Shell));
        assert_eq!(app.shell_tab_count(), 1);
        (
            agent_pid(tab).expect("owned terminal child"),
            key.root,
            EndOwnedChild(Arc::clone(&tab.session)),
        )
    };
    assert_eq!(root, std::fs::canonicalize(&repository).unwrap());
    fixture
        .verify(created, json!({"term_id":id,"pid":pid,"root":root,"private_program":script,"create_wall_ms":create_elapsed.as_secs_f64()*1000.0}))
        .await;

    fixture.call("term.attach", json!({"term_id":id})).await;
    fixture
        .call(
            "term.input",
            json!({"term_id":id,"data":b64encode(b"QUAL-PING\n")}),
        )
        .await;
    let deadline = Instant::now() + Duration::from_secs(2);
    let pong = loop {
        let text = {
            let app = fixture.state().lock().unwrap();
            let key = app.tab_key_of_wire_id(&id).unwrap();
            let tab = app.session_registry.test_tab(&key).unwrap();
            String::from_utf8_lossy(&b64decode(&screen_of(tab).snapshot().snapshot).unwrap())
                .into_owned()
        };
        if text.contains("QUAL-PONG") || Instant::now() >= deadline {
            break text.contains("QUAL-PONG");
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    };
    // Always retire the owned child before reporting an output failure.
    let closed = fixture.call("term.close", json!({"term_id":id})).await;
    let reaped = tokio::task::spawn_blocking(move || process_reaped(pid))
        .await
        .unwrap();
    let listed = fixture
        .call("term.list", json!({"project_id":project}))
        .await;
    assert!(listed.reply["result"]["terminals"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(fixture.state().lock().unwrap().shell_tab_count(), 0);
    assert!(pong, "private terminal program did not answer its input");
    assert!(reaped, "owned private terminal child was not reaped");
    fixture
        .verify(
            closed,
            json!({"term_id":id,"pid_reaped":pid,"terminal_count":0,"protocol_reply":"QUAL-PONG"}),
        )
        .await;
    drop(owned_child);
    fixture.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_terminal_create_close_uses_private_protocol_and_reaps_owned_child() {
    let steps = execute().await;
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].method, "term.create");
    assert_eq!(steps[1].method, "term.close");
}
