//! Offline cross-language interop: the Rust transport binding talks to the Python
//! reference implementation, in both directions, through the reference's own
//! `interop_helper.py` stdin/stdout harness.
//!
//! The default reference lives in this checkout's `transport/python`. Required
//! tooling/source failures fail the test. `BUILD_SECURE_TRANSPORT_PY` is only a
//! deliberate reference override; `BST_UV` selects an existing uv executable.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use build_bridge::transport::{self, Envelope, FrameFields, OuterFields, SessionInit};
use serde_json::{json, Value};

#[test]
fn reference_defaults_to_this_checkout() {
    let reference = reference_dir(None);
    assert_eq!(
        reference,
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../transport/python")
    );
    check_reference(&reference).expect("in-tree locked Python reference exists");
}

#[test]
fn missing_reference_is_an_error() {
    let reference = reference_dir(Some("/definitely-missing/reference".into()));
    let error = check_reference(&reference).unwrap_err();
    assert!(error.contains("required Python reference"));
}

fn reference_dir(override_dir: Option<OsString>) -> PathBuf {
    match override_dir.filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .expect("interop working directory")
                    .join(path)
            }
        }
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../transport/python"),
    }
}

fn check_reference(py_dir: &Path) -> Result<(), String> {
    for required in [
        "pyproject.toml",
        "uv.lock",
        "src/build_secure_transport/__init__.py",
        "tests/interop_helper.py",
    ] {
        let path = py_dir.join(required);
        if !path.is_file() {
            return Err(format!(
                "required Python reference missing: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Run one action through the locked Python reference, returning its JSON reply.
fn py(py_dir: &Path, request: Value) -> Value {
    let script = py_dir.join("tests/interop_helper.py");
    let uv = std::env::var("BST_UV").unwrap_or_else(|_| "uv".to_string());
    let mut child = Command::new(&uv)
        .args(["run", "--frozen", "--quiet", "--project"])
        .arg(py_dir)
        .arg("python")
        .arg(&script)
        .current_dir(py_dir)
        .env("PYTHONPATH", py_dir.join("src"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("required uv executable '{uv}' could not run: {error}; set BST_UV to an existing uv binary"));
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("python helper output");
    assert!(
        out.status.success(),
        "Python interop helper failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("python helper returned JSON")
}

#[test]
fn rust_and_python_interop_round_trip() {
    let py_dir = reference_dir(std::env::var_os("BUILD_SECURE_TRANSPORT_PY"));
    check_reference(&py_dir).unwrap_or_else(|error| panic!("{error}"));

    // 1. Python (device) produces an identity-signed transport key.
    let bundle = py(&py_dir, json!({"action": "generate_transport_bundle"}));
    let identity_pub = bundle["identity_public_key_b64"].as_str().unwrap();
    let transport_pub = bundle["transport_public_key_b64"].as_str().unwrap();
    let transport_priv = bundle["transport_private_key_b64"].as_str().unwrap();
    let signature = bundle["signature_b64"].as_str().unwrap();

    // Rust verifies the Python-made Ed25519 signature (verified-device mode).
    transport::verify_transport_key_signature(identity_pub, transport_pub, signature)
        .expect("rust verifies python's transport-key signature");

    // Fingerprints agree across languages.
    let py_fp = py(
        &py_dir,
        json!({"action": "fingerprint_identity_key", "identity_public_key_b64": identity_pub}),
    );
    assert_eq!(
        py_fp["fingerprint"].as_str().unwrap(),
        transport::fingerprint_identity_key(identity_pub).unwrap()
    );

    // 2. Rust (client) wraps a fresh session key to the Python device key...
    let session_key = transport::generate_session_key();
    let wrapped = transport::wrap_session_key(transport_pub, &session_key).unwrap();
    let opened = py(
        &py_dir,
        json!({
            "action": "open_session_init",
            "transport_private_key_b64": transport_priv,
            "session_init": {
                "session_id": "s1",
                "device_id": "d1",
                "wrapped_session_key": wrapped,
            }
        }),
    );
    // ...and Python unseals the identical key. (Rust sealed box → Python.)
    assert_eq!(opened["session_key_b64"].as_str().unwrap(), session_key);

    // 3. Python builds session_accept; Rust verifies it. (Python secretbox → Rust.)
    let accept = py(
        &py_dir,
        json!({
            "action": "build_session_accept",
            "session_key_b64": session_key,
            "session_id": "s1",
            "route_to": "client:c1",
        }),
    );
    let accept_env: Envelope = serde_json::from_value(accept).unwrap();
    transport::verify_session_accept(&session_key, &accept_env, "s1")
        .expect("rust verifies python's session_accept");

    // 4. Rust encrypts a frame; Python decrypts it. (Rust secretbox → Python.)
    let env = transport::encrypt_frame(
        &session_key,
        &OuterFields {
            session_id: "s1".into(),
            route_to: "device:d1".into(),
        },
        &FrameFields {
            frame_type: "data".into(),
            sender: "client".into(),
            payload: json!({"from": "rust", "n": 42}),
            message_id: None,
            created_at: None,
        },
        None,
    )
    .unwrap();
    let decoded = py(
        &py_dir,
        json!({
            "action": "decrypt_envelope",
            "session_key_b64": session_key,
            "envelope": serde_json::to_value(&env).unwrap(),
        }),
    );
    assert_eq!(decoded["payload"], json!({"from": "rust", "n": 42}));
    assert_eq!(decoded["sender"].as_str().unwrap(), "client");

    // 5. Python encrypts a frame; Rust decrypts it. (Python secretbox → Rust.)
    let py_env = py(
        &py_dir,
        json!({
            "action": "encrypt_frame",
            "session_key_b64": session_key,
            "outer_fields": {"session_id": "s1", "route_to": "client:c1"},
            "frame_fields": {"frame_type": "data", "sender": "device", "payload": {"from": "python"}},
        }),
    );
    let py_env: Envelope = serde_json::from_value(py_env).unwrap();
    let frame = transport::decrypt_envelope(&session_key, &py_env).unwrap();
    assert_eq!(frame.payload, json!({"from": "python"}));
    assert_eq!(frame.sender, "device");

    // 6. A close frame from Rust round-trips through Python too.
    let close =
        transport::build_close_frame(&session_key, "s1", "device:d1", "client", Value::Null)
            .unwrap();
    let closed = py(
        &py_dir,
        json!({
            "action": "decrypt_envelope",
            "session_key_b64": session_key,
            "envelope": serde_json::to_value(&close).unwrap(),
        }),
    );
    assert_eq!(closed["frame_type"].as_str().unwrap(), "close");

    // The session_init type is part of the public surface; keep it referenced.
    let _ = SessionInit {
        session_id: "s1".into(),
        device_id: "d1".into(),
        wrapped_session_key: "x".into(),
    };
}
