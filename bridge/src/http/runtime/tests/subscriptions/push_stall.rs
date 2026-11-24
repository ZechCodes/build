use super::*;
use crate::app::AppState;
use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
};

#[tokio::test(flavor = "multi_thread")]
async fn mounted_thread_receives_pushes_alongside_project_terminals() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (mut app, owner, agent) = super::super::project_unread::project_with_unread(&root);
    for index in 1..5 {
        let project = root.join(format!("project-{index}"));
        std::fs::create_dir(&project).unwrap();
        app.add_project(project, "main".into());
    }
    let app = app.shared();
    let handler = AppState::handler(app.clone());
    handler.start_read_workers().unwrap();
    let mounted = mounted_tls(runtime_with(
        ExecutionService::new(handler),
        verifier(authority()),
    ))
    .await;
    let cert = mounted._dir.path().join("cert.pem");
    let origin = mounted.base.clone();
    let result = tokio::task::spawn_blocking(move || {
        let witness = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../spa/test/fixtures/httpPushStallWitness.mjs");
        let mut child = Command::new("node")
            .args([
                witness.as_os_str(),
                origin.as_ref(),
                owner.as_ref(),
                agent.as_ref(),
            ])
            .env("NODE_EXTRA_CA_CERTS", cert)
            .env("NODE_USE_ENV_PROXY", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the SPA's HTTPS client");
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let line = line.unwrap();
            if let Some(body) = line.strip_prefix("publish ") {
                app.lock()
                    .unwrap()
                    .post_agent_message_for_test(&owner, &agent, body);
            } else {
                println!("{line}");
            }
        }
        child.wait().unwrap()
    })
    .await
    .unwrap();
    mounted.shutdown().await;
    assert!(
        result.success(),
        "the HTTPS push witness failed with {result}"
    );
}
