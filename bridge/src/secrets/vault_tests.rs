use super::*;
use crate::secrets::{GrantScope, StageContext};
use std::os::unix::fs::{symlink, PermissionsExt};

fn fixture() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn context() -> SecretContext {
    SecretContext {
        id: format!("sec_{}", uuid::Uuid::new_v4()),
        device_id: "synthetic-device".into(),
        creator: "synthetic-creator".into(),
        name: "SYNTHETIC_TEST_VALUE".into(),
        ttl_seconds: Some(3600),
        grant: GrantScope::Agent {
            agent_id: "synthetic-agent".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "synthetic-generation".into(),
        },
        stage: StageContext {
            owner_id: "synthetic-owner".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "synthetic-generation".into(),
            draft_id: Some("synthetic-draft".into()),
        },
    }
}

#[test]
fn encrypted_round_trip_fresh_nonce_and_restart() {
    let root = fixture();
    let context = context();
    let value = SecretValue::new(b"synthetic fixture secret, not a credential".to_vec()).unwrap();
    let vault = Vault::open(root.path(), false).unwrap();
    vault.write(&context, &value).unwrap();
    let path = root.path().join("secrets").join(&context.id);
    let first = std::fs::read(&path).unwrap();
    assert!(!first
        .windows(value.as_bytes().len())
        .any(|w| w == value.as_bytes()));
    assert_eq!(vault.read(&context).unwrap().as_bytes(), value.as_bytes());
    vault.delete(&context.id).unwrap();
    vault.write(&context, &value).unwrap();
    assert_ne!(first, std::fs::read(path).unwrap());
    drop(vault);
    let reopened = Vault::open(root.path(), true).unwrap();
    assert_eq!(
        reopened.read(&context).unwrap().as_bytes(),
        value.as_bytes()
    );
}

#[test]
fn private_permissions_and_no_plaintext_temp_files() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let context = context();
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic fixture".to_vec()).unwrap(),
        )
        .unwrap();
    let dir = root.path().join("secrets");
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["vault.key", context.id.as_str()] {
        assert_eq!(
            std::fs::metadata(dir.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 2);
}

#[test]
fn ciphertext_header_nonce_tag_and_body_tamper_fail_closed() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let context = context();
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic fixture".to_vec()).unwrap(),
        )
        .unwrap();
    let path = root.path().join("secrets").join(&context.id);
    let original = std::fs::read(&path).unwrap();
    for offset in 0..original.len() {
        let mut changed = original.clone();
        changed[offset] ^= 1;
        std::fs::write(&path, changed).unwrap();
        assert!(vault.read(&context).is_err(), "tamper at {offset}");
    }
    for length in [0, 1, 8, original.len() - 1] {
        std::fs::write(&path, &original[..length]).unwrap();
        assert!(vault.read(&context).is_err());
    }
}

#[test]
fn every_authenticated_context_field_is_bound() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let original = context();
    vault
        .write(
            &original,
            &SecretValue::new(b"synthetic fixture".to_vec()).unwrap(),
        )
        .unwrap();
    let mut changes: Vec<SecretContext> = Vec::new();
    macro_rules! changed {
        ($field:ident, $value:expr) => {{
            let mut c = original.clone();
            c.$field = $value;
            changes.push(c);
        }};
    }
    changed!(device_id, "another-device".into());
    changed!(creator, "another-creator".into());
    changed!(name, "ANOTHER_SYNTHETIC_NAME".into());
    changed!(ttl_seconds, Some(3601));
    changed!(ttl_seconds, None);
    changed!(
        grant,
        GrantScope::Project {
            project_id: "synthetic-project".into()
        }
    );
    changed!(
        grant,
        GrantScope::Agent {
            agent_id: "another-agent".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "synthetic-generation".into()
        }
    );
    changed!(
        grant,
        GrantScope::Agent {
            agent_id: "synthetic-agent".into(),
            conversation_id: "another-conversation".into(),
            generation: "synthetic-generation".into()
        }
    );
    changed!(
        grant,
        GrantScope::Agent {
            agent_id: "synthetic-agent".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "another-generation".into()
        }
    );
    for field in ["owner", "conversation", "generation", "draft"] {
        let mut c = original.clone();
        match field {
            "owner" => c.stage.owner_id = "another-owner".into(),
            "conversation" => c.stage.conversation_id = "another-conversation".into(),
            "generation" => c.stage.generation = "another-generation".into(),
            _ => c.stage.draft_id = Some("another-draft".into()),
        }
        changes.push(c);
    }
    for c in changes {
        assert!(vault.read(&c).is_err());
    }
    let mut changed_id = original.clone();
    changed_id.id = format!("sec_{}", uuid::Uuid::new_v4());
    std::fs::copy(
        root.path().join("secrets").join(&original.id),
        root.path().join("secrets").join(&changed_id.id),
    )
    .unwrap();
    assert!(vault.read(&changed_id).is_err());
}

#[test]
fn missing_corrupt_or_wrong_key_is_never_replaced() {
    let root = fixture();
    let context = context();
    let vault = Vault::open(root.path(), false).unwrap();
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic fixture".to_vec()).unwrap(),
        )
        .unwrap();
    drop(vault);
    let key = root.path().join("secrets/vault.key");
    let original = std::fs::read(&key).unwrap();
    std::fs::remove_file(&key).unwrap();
    assert!(Vault::open(root.path(), true).is_err());
    assert!(!key.exists());
    assert!(Vault::open(root.path(), false).is_err());
    std::fs::write(&key, b"bad").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Vault::open(root.path(), true).is_err());
    assert_eq!(std::fs::read(&key).unwrap(), b"bad");
    let mut wrong = original;
    wrong[0] ^= 1;
    std::fs::write(&key, wrong).unwrap();
    let wrong_key_vault = Vault::open(root.path(), true).unwrap();
    assert!(wrong_key_vault.read(&context).is_err());
}

#[test]
fn key_and_blob_symlinks_and_fifos_are_refused_without_blocking() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let context = context();
    let key = root.path().join("secrets/vault.key");
    let saved_key = root.path().join("saved.key");
    std::fs::rename(&key, &saved_key).unwrap();
    symlink(&saved_key, &key).unwrap();
    assert!(Vault::open(root.path(), true).is_err());
    std::fs::remove_file(&key).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &key,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_bits_truncate(0o600),
        0,
    )
    .unwrap();
    assert!(Vault::open(root.path(), true).is_err());
    std::fs::remove_file(&key).unwrap();
    std::fs::rename(&saved_key, &key).unwrap();
    let blob = root.path().join("secrets").join(&context.id);
    symlink(&key, &blob).unwrap();
    assert!(vault.read(&context).is_err());
    std::fs::remove_file(&blob).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &blob,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_bits_truncate(0o600),
        0,
    )
    .unwrap();
    assert!(vault.read(&context).is_err());
    assert!(vault
        .write(&context, &SecretValue::new(b"synthetic".to_vec()).unwrap())
        .is_err());
}

#[test]
fn directory_symlink_insecure_modes_and_hardlinks_are_refused() {
    let root = fixture();
    let outside = fixture();
    symlink(outside.path(), root.path().join("secrets")).unwrap();
    assert!(Vault::open(root.path(), false).is_err());
    std::fs::remove_file(root.path().join("secrets")).unwrap();
    let vault = Vault::open(root.path(), false).unwrap();
    std::fs::set_permissions(
        root.path().join("secrets"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(Vault::open(root.path(), false).is_err());
    std::fs::set_permissions(
        root.path().join("secrets"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let key = root.path().join("secrets/vault.key");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Vault::open(root.path(), false).is_err());
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&key, root.path().join("linked-key")).unwrap();
    assert!(Vault::open(root.path(), false).is_err());
    drop(vault);
}

#[test]
fn leaf_paths_are_validated_and_cleanup_does_not_follow_symlinks() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    for id in [
        "../outside",
        "sec_invalid",
        "/absolute",
        "vault.key",
        "sec_../../x",
    ] {
        assert!(vault.delete(id).is_err());
        assert!(vault.delete_entry(id).is_err());
    }
    let context = context();
    let outside = root.path().join("outside");
    std::fs::write(&outside, b"synthetic outside fixture").unwrap();
    symlink(&outside, root.path().join("secrets").join(&context.id)).unwrap();
    assert!(vault.entries().unwrap().contains(&context.id));
    vault.delete(&context.id).unwrap();
    assert_eq!(
        std::fs::read(outside).unwrap(),
        b"synthetic outside fixture"
    );
    vault.delete(&context.id).unwrap();
}

#[test]
fn pinned_directory_handles_ignore_path_replacement() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let original = root.path().join("original-vault");
    std::fs::rename(root.path().join("secrets"), &original).unwrap();
    std::fs::create_dir(root.path().join("secrets")).unwrap();
    let context = context();
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic fixture".to_vec()).unwrap(),
        )
        .unwrap();
    assert!(original.join(&context.id).exists());
    assert!(!root.path().join("secrets").join(&context.id).exists());
    assert_eq!(
        vault.read(&context).unwrap().as_bytes(),
        b"synthetic fixture"
    );
}

#[test]
fn maximum_value_and_oversized_ciphertext_are_bounded() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let context = context();
    let bytes = vec![b's'; crate::secrets::MAX_VALUE_BYTES];
    vault
        .write(&context, &SecretValue::new(bytes.clone()).unwrap())
        .unwrap();
    assert_eq!(vault.read(&context).unwrap().as_bytes(), bytes);
    let path = root.path().join("secrets").join(&context.id);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(1024 * 1024)
        .unwrap();
    assert!(vault.read(&context).is_err());
}

#[test]
fn live_handle_fails_closed_after_key_disappears_or_changes() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let context = context();
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic fixture".to_vec()).unwrap(),
        )
        .unwrap();
    let key = root.path().join("secrets/vault.key");
    let original = std::fs::read(&key).unwrap();
    std::fs::remove_file(&key).unwrap();
    assert!(vault.read(&context).is_err());
    assert!(vault
        .write(
            &context,
            &SecretValue::new(b"synthetic other fixture".to_vec()).unwrap()
        )
        .is_err());
    let mut changed = original;
    changed[0] ^= 1;
    std::fs::write(&key, changed).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(vault.read(&context).is_err());
}

#[test]
fn empty_existing_vault_and_missing_state_with_records_fail_closed() {
    let root = fixture();
    std::fs::create_dir(root.path().join("secrets")).unwrap();
    std::fs::set_permissions(
        root.path().join("secrets"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(Vault::open(root.path(), false).is_err());
    assert!(!root.path().join("secrets/vault.key").exists());
    let missing = fixture();
    assert!(Vault::open(missing.path(), true).is_err());
    assert!(!missing.path().join("secrets/vault.key").exists());
}

#[test]
fn insecure_and_linked_blobs_are_refused() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let context = context();
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic fixture".to_vec()).unwrap(),
        )
        .unwrap();
    let path = root.path().join("secrets").join(&context.id);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(vault.read(&context).is_err());
    assert!(vault
        .write(
            &context,
            &SecretValue::new(b"synthetic replacement".to_vec()).unwrap()
        )
        .is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&path, root.path().join("linked-blob")).unwrap();
    assert!(vault.read(&context).is_err());
    assert!(vault
        .write(
            &context,
            &SecretValue::new(b"synthetic replacement".to_vec()).unwrap()
        )
        .is_err());
}

#[test]
fn existing_blob_is_never_replaced() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let context = context();
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic original".to_vec()).unwrap(),
        )
        .unwrap();
    let original = std::fs::read(root.path().join("secrets").join(&context.id)).unwrap();
    assert!(vault
        .write(
            &context,
            &SecretValue::new(b"synthetic replacement".to_vec()).unwrap()
        )
        .is_err());
    assert_eq!(
        std::fs::read(root.path().join("secrets").join(&context.id)).unwrap(),
        original
    );
    assert_eq!(
        vault.read(&context).unwrap().as_bytes(),
        b"synthetic original"
    );
}

#[test]
fn canonical_project_id_is_authenticated() {
    let root = fixture();
    let vault = Vault::open(root.path(), false).unwrap();
    let mut context = context();
    context.grant = GrantScope::Project {
        project_id: "synthetic-project-one".into(),
    };
    vault
        .write(
            &context,
            &SecretValue::new(b"synthetic project fixture".to_vec()).unwrap(),
        )
        .unwrap();
    context.grant = GrantScope::Project {
        project_id: "synthetic-project-two".into(),
    };
    assert!(vault.read(&context).is_err());
}

#[test]
fn concurrent_publishers_cannot_replace_each_others_blob() {
    let root = fixture();
    let first = Vault::open(root.path(), false).unwrap();
    let second = Vault::open(root.path(), false).unwrap();
    let context = context();
    let start = std::sync::Barrier::new(2);
    let (one, two) = std::thread::scope(|scope| {
        let one = scope.spawn(|| {
            start.wait();
            first
                .write(
                    &context,
                    &SecretValue::new(b"synthetic first".to_vec()).unwrap(),
                )
                .is_ok()
        });
        let two = scope.spawn(|| {
            start.wait();
            second
                .write(
                    &context,
                    &SecretValue::new(b"synthetic second".to_vec()).unwrap(),
                )
                .is_ok()
        });
        (one.join().unwrap(), two.join().unwrap())
    });
    assert_ne!(one, two);
    let expected: &[u8] = if one {
        b"synthetic first"
    } else {
        b"synthetic second"
    };
    assert_eq!(first.read(&context).unwrap().as_bytes(), expected);
    assert_eq!(
        std::fs::read_dir(root.path().join("secrets"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn explicit_retirement_recovery_recreates_missing_vault_or_key() {
    for whole_directory in [false, true] {
        let root = fixture();
        let old = Vault::open(root.path(), false).unwrap();
        let old_id = old.key_id();
        let old_context = context();
        old.write(
            &old_context,
            &SecretValue::new(b"synthetic retired fixture".to_vec()).unwrap(),
        )
        .unwrap();
        if whole_directory {
            std::fs::remove_dir_all(root.path().join("secrets")).unwrap();
        } else {
            std::fs::remove_file(root.path().join("secrets/vault.key")).unwrap();
        }
        assert!(Vault::open(root.path(), true).is_err());
        let recovered = Vault::reinitialize_after_retirement(root.path()).unwrap();
        assert_ne!(recovered.key_id(), old_id);
        assert_eq!(recovered.key_id().len(), 32);
        assert!(recovered.read(&old_context).is_err());
        assert!(recovered.entries().unwrap().is_empty());
        let fresh_context = context();
        recovered
            .write(
                &fresh_context,
                &SecretValue::new(b"synthetic fresh fixture".to_vec()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            recovered.read(&fresh_context).unwrap().as_bytes(),
            b"synthetic fresh fixture"
        );
        assert_eq!(
            Vault::open(root.path(), true).unwrap().key_id(),
            recovered.key_id()
        );
    }
}

#[test]
fn explicit_retirement_recovery_refuses_every_present_key_without_modification() {
    for damage in ["valid", "short", "substituted"] {
        let root = fixture();
        let old = Vault::open(root.path(), false).unwrap();
        let context = context();
        old.write(
            &context,
            &SecretValue::new(b"synthetic preserved fixture".to_vec()).unwrap(),
        )
        .unwrap();
        let key_path = root.path().join("secrets/vault.key");
        let blob_path = root.path().join("secrets").join(&context.id);
        let mut key = std::fs::read(&key_path).unwrap();
        match damage {
            "short" => key.truncate(1),
            "substituted" => key[0] ^= 1,
            _ => {}
        }
        std::fs::write(&key_path, &key).unwrap();
        let ciphertext = std::fs::read(&blob_path).unwrap();
        assert!(Vault::reinitialize_after_retirement(root.path()).is_err());
        assert_eq!(std::fs::read(&key_path).unwrap(), key);
        assert_eq!(std::fs::read(&blob_path).unwrap(), ciphertext);
    }
}

#[test]
fn retirement_recovery_refuses_directory_and_key_symlinks_without_touching_targets() {
    let root = fixture();
    let outside = fixture();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"synthetic untouched fixture").unwrap();
    symlink(outside.path(), root.path().join("secrets")).unwrap();
    assert!(Vault::reinitialize_after_retirement(root.path()).is_err());
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"synthetic untouched fixture"
    );
    std::fs::remove_file(root.path().join("secrets")).unwrap();
    let old = Vault::open(root.path(), false).unwrap();
    let key = root.path().join("secrets/vault.key");
    std::fs::remove_file(&key).unwrap();
    symlink(&sentinel, &key).unwrap();
    assert!(Vault::reinitialize_after_retirement(root.path()).is_err());
    assert!(std::fs::symlink_metadata(&key)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"synthetic untouched fixture"
    );
    drop(old);
}

#[test]
fn retirement_recovery_refuses_fifo_insecure_and_linked_keys() {
    let root = fixture();
    let _old = Vault::open(root.path(), false).unwrap();
    let key = root.path().join("secrets/vault.key");
    std::fs::remove_file(&key).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &key,
        FileType::Fifo,
        Mode::from_bits_truncate(0o600),
        0,
    )
    .unwrap();
    assert!(Vault::reinitialize_after_retirement(root.path()).is_err());
    assert_eq!(
        FileType::from_raw_mode(rustix::fs::stat(&key).unwrap().st_mode),
        FileType::Fifo
    );
    std::fs::remove_file(&key).unwrap();
    std::fs::write(&key, b"synthetic corrupt key fixture").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Vault::reinitialize_after_retirement(root.path()).is_err());
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&key, root.path().join("linked-key")).unwrap();
    assert!(Vault::reinitialize_after_retirement(root.path()).is_err());
    assert_eq!(
        std::fs::read(&key).unwrap(),
        b"synthetic corrupt key fixture"
    );
}

#[test]
fn recovery_cleanup_failure_prevents_new_key_creation() {
    let root = fixture();
    let _old = Vault::open(root.path(), false).unwrap();
    let key = root.path().join("secrets/vault.key");
    std::fs::remove_file(&key).unwrap();
    std::fs::create_dir(root.path().join("secrets").join(context().id)).unwrap();
    assert!(Vault::reinitialize_after_retirement(root.path()).is_err());
    assert!(!key.exists());
}

#[test]
fn owner_validation_can_refuse_drift_without_changing_ownership_or_bytes() {
    let root = fixture();
    let _vault = Vault::open(root.path(), false).unwrap();
    let file = File::open(root.path().join("secrets/vault.key")).unwrap();
    let before = std::fs::read(root.path().join("secrets/vault.key")).unwrap();
    let owner = unsafe { libc::geteuid() };
    assert!(validate_private_with_owner(&file, FileType::RegularFile, 0o600, owner).is_ok());
    assert!(validate_private_with_owner(
        &file,
        FileType::RegularFile,
        0o600,
        owner.wrapping_add(1)
    )
    .is_err());
    assert_eq!(
        std::fs::read(root.path().join("secrets/vault.key")).unwrap(),
        before
    );
}

#[test]
fn key_absence_check_refuses_unsafe_paths_and_distinguishes_present_keys() {
    let root = fixture();
    assert!(Vault::key_is_absent(root.path()).unwrap());
    let _vault = Vault::open(root.path(), false).unwrap();
    assert!(!Vault::key_is_absent(root.path()).unwrap());
    let key = root.path().join("secrets/vault.key");
    std::fs::remove_file(&key).unwrap();
    assert!(Vault::key_is_absent(root.path()).unwrap());
    let target = root.path().join("synthetic-target");
    std::fs::write(&target, b"synthetic target fixture").unwrap();
    symlink(&target, &key).unwrap();
    assert!(Vault::key_is_absent(root.path()).is_err());
    std::fs::remove_file(&key).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &key,
        FileType::Fifo,
        Mode::from_bits_truncate(0o600),
        0,
    )
    .unwrap();
    assert!(Vault::key_is_absent(root.path()).is_err());
    std::fs::remove_file(&key).unwrap();
    std::fs::set_permissions(
        root.path().join("secrets"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(Vault::key_is_absent(root.path()).is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"synthetic target fixture");
}
