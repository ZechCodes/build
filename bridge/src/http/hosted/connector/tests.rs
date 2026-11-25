use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

#[test]
fn hosted_connector_private_files_and_exact_host_rule() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime");
    let runtime = PrivateRuntime::open(path.clone()).unwrap();
    assert!(PrivateRuntime::open(path.clone()).is_err());
    let assignment: Assignment = serde_json::from_value(serde_json::json!({
        "hostname": "b-aaaa.hosted.example", "generation": 5,
        "credentials": {"AccountTag":"test", "TunnelID":"fixture", "TunnelSecret":"synthetic"}
    }))
    .unwrap();
    runtime
        .prepare(&assignment, "127.0.0.1:3210".parse().unwrap(), "ca")
        .unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for file in [CONFIG, CA, CREDENTIALS] {
        assert_eq!(
            fs::metadata(path.join(file)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join(CONFIG)).unwrap()).unwrap();
    assert_eq!(config["ingress"][0]["hostname"], assignment.hostname);
    assert_eq!(config["ingress"][0]["service"], "https://127.0.0.1:3210");
    assert_eq!(
        config["ingress"][0]["originRequest"]["originServerName"],
        assignment.hostname
    );
    assert_eq!(
        config["ingress"][1],
        serde_json::json!({"service":"http_status:404"})
    );
    let command = runtime.command(Path::new("/connector"), &assignment);
    assert!(command
        .as_std()
        .get_args()
        .all(|arg| !arg.to_string_lossy().contains("synthetic")));
    drop(runtime);
    assert!(!path.join(CREDENTIALS).exists());
    symlink(temp.path().join("outside"), path.join(CREDENTIALS)).unwrap();
    assert!(PrivateRuntime::open(path).is_err());
    let alias = temp.path().join("alias");
    symlink(temp.path().join("runtime"), &alias).unwrap();
    assert!(PrivateRuntime::open(alias).is_err());
}

#[tokio::test]
async fn hosted_connector_kills_and_reaps_on_edge_config_change() {
    let scratch = tempfile::tempdir().unwrap();
    let pid_file = scratch.path().join("pid");
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            r#"
        printf '%s\n' "$$" > "$1"
        printf '%s\n' '{"message":"Registered tunnel connection"}' >&2
        printf '%s\n' '{"message":"Updated to new configuration","version":0}' >&2
        exec /bin/sleep 3600
    "#,
            "fixture",
        ])
        .arg(&pid_file);
    let child = RunningConnector::start(command).unwrap();
    let state = terminal(child.status.clone()).await;
    assert_eq!(state, ChildStatus::ConfigChanged);
    let pid = fs::read_to_string(pid_file).unwrap();
    assert!(!Path::new("/proc").join(pid.trim()).exists());
    child.stop().await;
}

async fn terminal(mut updates: watch::Receiver<ChildStatus>) -> ChildStatus {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        *updates
            .wait_for(|s| {
                matches!(
                    s,
                    ChildStatus::Exited | ChildStatus::ConfigChanged | ChildStatus::OutputFailed
                )
            })
            .await
            .unwrap()
    })
    .await
    .unwrap()
}

#[derive(Deserialize)]
struct EdgeFixture {
    edge: String,
    edge_ca: PathBuf,
    origin_ca: PathBuf,
    origin: SocketAddr,
}

/// Requires the checksum-verified release binary and a loopback-only Linux
/// namespace. The mock edge never uses a real account or a real credential.
#[tokio::test]
#[ignore = "requires the pinned cloudflared, Python h2, openssl and an isolated network"]
async fn hosted_connector_real_binary_reaps_after_mock_edge_push() {
    let temp = tempfile::tempdir().unwrap();
    let binary = PathBuf::from(std::env::var_os("HOSTED_CLOUDFLARED_FIXTURE").unwrap());
    let fixture_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hosted_connector_edge.py");
    let mut fixture = Command::new("python")
        .arg(fixture_script)
        .arg(temp.path())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let config = fixture_config(temp.path()).await;
    let private = PrivateRuntime::open(temp.path().join("runtime")).unwrap();
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../planning/v2/fixtures/hosted-tunnel-claim.json"
    ))
    .unwrap();
    let mut reply = cases["cases"][2]["response"]["body"].clone();
    reply.as_object_mut().unwrap().remove("status");
    let assignment: Assignment = serde_json::from_value(reply).unwrap();
    private
        .prepare(
            &assignment,
            config.origin,
            &fs::read_to_string(config.origin_ca).unwrap(),
        )
        .unwrap();
    let mut command = tunnel_command(&binary);
    command
        .args(["--ha-connections", "1", "--protocol", "http2", "--edge"])
        .arg(config.edge)
        .arg("--cacert")
        .arg(config.edge_ca);
    let child = RunningConnector::start(private.with_inputs(command, &assignment)).unwrap();
    let status = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        *child
            .status
            .clone()
            .wait_for(|status| *status != ChildStatus::Starting)
            .await
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(status, ChildStatus::ConfigChanged);
    assert_eq!(child.stop().await, ChildStatus::ConfigChanged);
    private.quarantine(assignment.generation).unwrap();
    assert!(fixture.wait().await.unwrap().success());
    drop(private);
    let recovered = PrivateRuntime::open(temp.path().join("runtime")).unwrap();
    assert!(recovered.quarantined(assignment.generation).unwrap());
    assert!(!recovered.quarantined(assignment.generation + 1).unwrap());
}

async fn fixture_config(path: &Path) -> EdgeFixture {
    let file = path.join("fixture.json");
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            if let Ok(bytes) = fs::read(&file) {
                return serde_json::from_slice(&bytes).unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn hosted_connector_shutdown_reaps_child_and_clears_environment() {
    let temp = tempfile::tempdir().unwrap();
    let env_file = temp.path().join("environment");
    let pid_file = temp.path().join("pid");
    let mut command = Command::new("/bin/sh");
    command
        .env("SYNTHETIC_CONNECTOR_PARENT", "must-not-reach-child")
        .args([
            "-c",
            r#"
            /usr/bin/env > "$1"
            printf '%s\n' "$$" > "$2"
            printf '%s\n' '{"message":"Registered tunnel connection"}' >&2
            exec /bin/sleep 3600
        "#,
            "fixture",
        ])
        .arg(&env_file)
        .arg(&pid_file);
    let child = RunningConnector::start(command).unwrap();
    wait_connected(&child).await;
    let environment = fs::read_to_string(env_file).unwrap();
    assert!(!environment.contains("SYNTHETIC_CONNECTOR_PARENT"));
    assert!(!environment.contains("must-not-reach-child"));
    let pid = fs::read_to_string(pid_file).unwrap();
    assert_eq!(child.stop().await, ChildStatus::Exited);
    assert!(!Path::new("/proc").join(pid.trim()).exists());
}

async fn wait_connected(connector: &RunningConnector) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let status = *connector
            .status
            .clone()
            .wait_for(|status| *status != ChildStatus::Starting)
            .await
            .unwrap();
        assert_eq!(status, ChildStatus::Connected);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn hosted_connector_bridge_crash_ends_child() {
    let temp = tempfile::tempdir().unwrap();
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "http::hosted::connector::tests::connector_crash_fixture",
            "--ignored",
        ])
        .env("CONNECTOR_CRASH_FIXTURE_DIR", temp.path())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = fs::read_to_string(temp.path().join("child-pid")) {
                break pid.trim().parse::<u32>().unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(serving_process(pid));
    parent.kill().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while serving_process(pid) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

fn serving_process(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .is_some_and(|(_, fields)| !fields.starts_with(" Z "))
    })
}

#[tokio::test]
#[ignore = "child of hosted_connector_bridge_crash_ends_child"]
async fn connector_crash_fixture() {
    let directory = PathBuf::from(std::env::var_os("CONNECTOR_CRASH_FIXTURE_DIR").unwrap());
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            r#"
        printf '%s\n' "$$" > "$1.tmp"
        /bin/mv "$1.tmp" "$1"
        exec /bin/sleep 3600
    "#,
            "fixture",
        ])
        .arg(directory.join("child-pid"));
    let _connector = RunningConnector::start(command).unwrap();
    std::future::pending::<()>().await;
}
