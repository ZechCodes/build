use crate::store::Store;
use std::fs;

#[test]
fn database_backup_preserves_metadata_without_copying_device_vault() {
    let fixture = tempfile::tempdir().unwrap();
    let state = fixture.path().join("synthetic-device-state");
    let store = Store::new(state.join("tasks")).unwrap();
    let secret_id = "sec_00000000-0000-4000-8000-000000000001";
    // A database-only reference can survive even when its device-local value
    // is unavailable after restore. Neither the key nor blob belongs in SQL.
    store
        .writer_for_test()
        .execute(
            "INSERT INTO meta(key,value) VALUES ('synthetic_secret_reference',?1)",
            [secret_id],
        )
        .unwrap();
    let vault = state.join("secrets");
    fs::create_dir(&vault).unwrap();
    let key = b"synthetic vault key fixture only";
    let blob = b"synthetic encrypted blob fixture only";
    fs::write(vault.join("vault.key"), key).unwrap();
    fs::write(vault.join(secret_id), blob).unwrap();

    let export = fixture.path().join("synthetic-export");
    let database = export.join("build.db");
    store.backup_to(&database).unwrap();

    let copied = rusqlite::Connection::open(&database).unwrap();
    let reference: String = copied
        .query_row(
            "SELECT value FROM meta WHERE key='synthetic_secret_reference'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reference, secret_id);
    let files: Vec<_> = fs::read_dir(&export)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, ["build.db"]);
    let database_bytes = fs::read(database).unwrap();
    assert!(!database_bytes.windows(key.len()).any(|bytes| bytes == key));
    assert!(!database_bytes
        .windows(blob.len())
        .any(|bytes| bytes == blob));
    assert_eq!(fs::read(vault.join("vault.key")).unwrap(), key);
    assert_eq!(fs::read(vault.join(secret_id)).unwrap(), blob);
}
