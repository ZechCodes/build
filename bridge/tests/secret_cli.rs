//! The runtime command is exercised as a Cargo-built executable, without a daemon.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output};

const ID: &str = "sec_00000000-0000-4000-8000-000000000001";

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_build-bridge"));
    command.env_clear().args(["secrets", "get", "--id", ID]);
    command
}

fn assert_error(output: Output, code: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, format!("{code}\n").as_bytes());
}

#[test]
fn missing_context_is_unauthorized_before_daemon_configuration() {
    assert_error(
        command().env("BRIDGE_CHANNEL", "invalid").output().unwrap(),
        "unauthorized",
    );
}

#[test]
fn invalid_unicode_arguments_have_fixed_diagnostics() {
    use std::os::unix::ffi::OsStringExt;
    assert_error(
        command()
            .arg(std::ffi::OsString::from_vec(vec![0xff]))
            .output()
            .unwrap(),
        "secret_invalid",
    );
}

#[test]
fn exact_output_and_fixed_errors_over_private_socket() {
    for value in [
        b" \t leading\nsecond line\n ".to_vec(),
        vec![b'x'; 16 * 1024],
    ] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("release.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let expected = value.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["agent_id"], "agent-fixture");
            assert_eq!(request["capability"], "synthetic-token");
            assert_eq!(request["secret_id"], ID);
            writeln!(stream, "{{\"ok\":true,\"length\":{}}}", value.len()).unwrap();
            stream.write_all(&value).unwrap();
        });
        let output = authorized(&socket).output().unwrap();
        server.join().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, expected);
        assert!(output.stderr.is_empty());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

fn authorized(socket: &std::path::Path) -> Command {
    let mut command = command();
    command
        .env("BRIDGE_SECRET_SOCKET", socket)
        .env("BRIDGE_SECRET_CHANNEL", "dev")
        .env("BRIDGE_SECRET_AGENT", "agent-fixture")
        .env("BRIDGE_SECRET_OWNER", "owner-fixture")
        .env("BRIDGE_SECRET_TOKEN", "synthetic-token");
    command
}

#[test]
fn mismatch_and_stale_socket_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("absent.sock");
    assert_error(
        authorized(&socket)
            .args(["--env", "prod"])
            .output()
            .unwrap(),
        "unauthorized",
    );
    assert_error(authorized(&socket).output().unwrap(), "unauthorized");
}

#[test]
fn untrusted_error_and_partial_value_never_reach_output() {
    for response in [
        b"{\"ok\":false,\"error\":\"synthetic-sensitive-diagnostic\"}\n".as_slice(),
        b"{\"ok\":true,\"length\":42}\nsynthetic-partial-value".as_slice(),
        b"{\"ok\":false,\"error\":\"secret_revoked\"}\n".as_slice(),
    ] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("release.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            stream.write_all(response).unwrap();
        });
        let output = authorized(&socket).output().unwrap();
        server.join().unwrap();
        assert_error(
            output,
            if response.windows(14).any(|s| s == b"secret_revoked") {
                "secret_revoked"
            } else {
                "secret_unavailable"
            },
        );
    }
}

#[test]
fn both_facades_share_the_runtime_contract() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("build-bridge");
    symlink(env!("CARGO_BIN_EXE_build-bridge"), &binary).unwrap();
    build_bridge::runtime_launcher::install_sibling(&binary).unwrap();
    let launcher = root.path().join(".build-runtime").join("buildctl");
    for facade in ["runtime", "management"] {
        let make_command = |environment: Option<&str>| {
            let mut cmd = if facade == "runtime" {
                Command::new(&launcher)
            } else {
                let mut cmd = Command::new("/usr/bin/python3");
                cmd.args(["-m", "buildctl"]);
                cmd
            };
            cmd.env_clear()
                .current_dir(root.path())
                .env("PATH", "")
                .env(
                    "PYTHONPATH",
                    format!("{}/../tools/buildctl", env!("CARGO_MANIFEST_DIR")),
                )
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .env("BRIDGE_SECRET_BRIDGE", &binary);
            if let Some(environment) = environment {
                cmd.args(["--env", environment]);
            }
            cmd.args(["secrets", "get", "--id", ID]);
            cmd
        };
        assert_error(make_command(Some("dev")).output().unwrap(), "unauthorized");
        assert_error(make_command(None).output().unwrap(), "invalid_arguments");
        assert_error(
            make_command(Some("prod"))
                .env("BRIDGE_SECRET_SOCKET", root.path().join("absent.sock"))
                .env("BRIDGE_SECRET_CHANNEL", "dev")
                .env("BRIDGE_SECRET_AGENT", "agent-fixture")
                .env("BRIDGE_SECRET_OWNER", "owner-fixture")
                .env("BRIDGE_SECRET_TOKEN", "synthetic-token")
                .output()
                .unwrap(),
            "unauthorized",
        );
        let mut large = b"{\"ok\":true,\"length\":16384}\n".to_vec();
        large.extend(vec![b'x'; 16384]);
        for (response, expected) in [
            (
                b"{\"ok\":true,\"length\":8}\n a\nb \t\nz".to_vec(),
                Some(b" a\nb \t\nz".to_vec()),
            ),
            (large, Some(vec![b'x'; 16384])),
            (
                b"{\"ok\":false,\"error\":\"unauthorized\"}\n".to_vec(),
                None,
            ),
            (b"{\"ok\":true,\"length\":16385}\n".to_vec(), None),
            (vec![b'z'; 257], None),
        ] {
            let unauthorized = response.windows(12).any(|s| s == b"unauthorized");
            let socket = root.path().join("facade.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                assert!(!line.contains(" a\nb"));
                stream.write_all(&response).unwrap();
            });
            let output = make_command(Some("dev"))
                .env("BRIDGE_SECRET_SOCKET", &socket)
                .env("BRIDGE_SECRET_CHANNEL", "dev")
                .env("BRIDGE_SECRET_AGENT", "agent-fixture")
                .env("BRIDGE_SECRET_OWNER", "owner-fixture")
                .env("BRIDGE_SECRET_TOKEN", "synthetic-token")
                .output()
                .unwrap();
            server.join().unwrap();
            std::fs::remove_file(socket).unwrap();
            if let Some(expected) = expected {
                assert!(output.status.success(), "{facade}");
                assert_eq!(output.stdout, expected);
                assert!(output.stderr.is_empty());
            } else {
                assert_error(
                    output,
                    if unauthorized {
                        "unauthorized"
                    } else {
                        "secret_unavailable"
                    },
                );
            }
        }
    }
}

#[test]
fn broken_stdout_and_termination_leave_no_files() {
    use std::process::Stdio;
    for (terminate, length) in [(false, 40), (false, 16384), (true, 0)] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("release.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut child = authorized(&socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        if terminate {
            child.kill().unwrap();
        } else {
            drop(child.stdout.take());
            writeln!(stream, "{{\"ok\":true,\"length\":{length}}}").unwrap();
            stream.write_all(&vec![b'x'; length]).unwrap();
        }
        drop(stream);
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.windows(8).any(|s| s == b"xxxxxxxx"));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn small_value_to_full_device_is_a_fixed_nonzero_error() {
    use std::process::Stdio;
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("release.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let child = authorized(&socket)
        .stdout(Stdio::from(full))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    stream.write_all(b"{\"ok\":true,\"length\":40}\n").unwrap();
    stream.write_all(&[b'x'; 40]).unwrap();
    drop(stream);
    assert_error(child.wait_with_output().unwrap(), "secret_unavailable");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}
