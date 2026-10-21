use super::*;

#[test]
fn captured_release_receipts_bind_clean_source_and_one_successful_ni10_unit() {
    let (provenance, binary, compiler) = receipts();
    validate_build(&provenance, &binary, &compiler).unwrap();
}

#[test]
fn either_build_receipt_rejects_each_source_or_cleanliness_mismatch() {
    let (provenance, binary, compiler) = receipts();
    for (field, changed) in [
        ("head", json!("c".repeat(40))),
        ("tree", json!("d".repeat(40))),
        ("status", json!(" M src/lib.rs\n")),
        ("patch_sha256", json!("e".repeat(64))),
        ("untracked_hashes", json!({"unexpected.rs":"f".repeat(64)})),
    ] {
        let mut changed_binary = binary.clone();
        changed_binary["source"][field] = changed.clone();
        assert!(
            validate_build(&provenance, &changed_binary, &compiler).is_err(),
            "binary {field}"
        );
        let mut changed_compiler = compiler.clone();
        changed_compiler["source"][field] = changed;
        assert!(
            validate_build(&provenance, &binary, &changed_compiler).is_err(),
            "compiler {field}"
        );
    }
}

#[test]
fn release_receipts_reject_binary_profile_exit_priority_and_cfg_mismatches() {
    let (provenance, binary, compiler) = receipts();
    for (pointer, changed) in [
        ("/sha256", json!("a".repeat(64))),
        ("/cargo_profile", json!("debug")),
    ] {
        let mut bad = binary.clone();
        *bad.pointer_mut(pointer).unwrap() = changed;
        assert!(
            validate_build(&provenance, &bad, &compiler).is_err(),
            "{pointer}"
        );
    }
    for (pointer, changed) in [
        ("/profile", json!("debug")),
        ("/opt_level", json!(2)),
        ("/debug_assertions", json!(true)),
        ("/environment_label_used_as_proof", json!(true)),
        ("/units/0/exit_code", json!(1)),
        ("/units/0/actual_ni", json!(0)),
        ("/units/0/actual_opt_level", json!(2)),
        ("/units/0/debug_assertions", json!(true)),
        ("/units/0/cfg", json!(["test", "debug_assertions"])),
        ("/units", json!([])),
    ] {
        let mut bad = compiler.clone();
        *bad.pointer_mut(pointer).unwrap() = changed;
        assert!(
            validate_build(&provenance, &binary, &bad).is_err(),
            "{pointer}"
        );
    }
    let mut duplicated = compiler.clone();
    duplicated["units"]
        .as_array_mut()
        .unwrap()
        .push(compiler["units"][0].clone());
    assert!(validate_build(&provenance, &binary, &duplicated).is_err());
}

#[test]
fn compiler_receipt_rejects_missing_crate_test_and_optimization_flags() {
    let (provenance, binary, compiler) = receipts();
    for token in ["build_bridge", "--test", "opt-level=3"] {
        let mut bad = compiler.clone();
        bad["units"][0]["actual_compile_flags"]
            .as_array_mut()
            .unwrap()
            .retain(|value| value != token);
        assert!(
            validate_build(&provenance, &binary, &bad).is_err(),
            "{token}"
        );
    }
}

#[test]
fn compiler_receipt_rejects_conflicting_flags_even_with_valid_summary_fields() {
    let (provenance, binary, compiler) = receipts();
    for extra in [
        vec!["-C", "opt-level=0"],
        vec!["-Copt-level=2"],
        vec!["--codegen=opt-level=s"],
        vec!["-C", "debug-assertions=yes"],
        vec!["--crate-name", "other_crate"],
        vec!["--crate-name=other_crate"],
    ] {
        let mut bad = compiler.clone();
        bad["units"][0]["actual_compile_flags"]
            .as_array_mut()
            .unwrap()
            .extend(extra.iter().map(|value| json!(value)));
        assert!(
            validate_build(&provenance, &binary, &bad).is_err(),
            "{extra:?}"
        );
    }
}
