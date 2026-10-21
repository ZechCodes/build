use super::*;

#[test]
fn malformed_and_hash_mismatched_inputs_retain_exact_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.json");
    for (bytes, hash) in [
        (b"{not-json}\n\xff".as_slice(), None),
        (b" {\"valid\":true}\n".as_slice(), Some("wrong-frozen-hash")),
    ] {
        std::fs::write(&path, bytes).unwrap();
        let mut captured = Vec::new();
        assert!(read_input::<Value>(&path, "input.json", hash, &mut captured).is_err());
        assert_eq!(captured, vec![("input.json", bytes.to_vec())]);
    }
}

#[test]
fn later_missing_input_keeps_earlier_valid_input() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("manifest.json");
    let bytes = b" {\"valid\":true} \n";
    std::fs::write(&path, bytes).unwrap();
    let mut captured = Vec::new();
    let digest = format!("{:x}", Sha256::digest(bytes));
    let value: Value = read_input(&path, "manifest.json", Some(&digest), &mut captured).unwrap();
    assert_eq!(value, json!({"valid":true}));
    assert!(read_input::<Value>(
        &directory.path().join("missing.json"),
        "compiler.json",
        None,
        &mut captured,
    )
    .is_err());
    assert_eq!(captured, vec![("manifest.json", bytes.to_vec())]);
}

#[test]
fn actual_binary_hash_reads_file_bytes_and_rejects_directory() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("binary");
    std::fs::write(&path, b"abc").unwrap();
    assert_eq!(
        binary_hash(&path).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    std::fs::write(&path, b"changed").unwrap();
    assert_ne!(
        binary_hash(&path).unwrap(),
        format!("{:x}", Sha256::digest(b"abc"))
    );
    assert!(binary_hash(directory.path()).is_err());
}
