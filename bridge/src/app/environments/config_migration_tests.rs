use crate::app::{AppState, ConfigPersistStep};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[path = "frozen_legacy_loader.rs"]
mod frozen_legacy_loader;

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    original: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let parked = root.path().join("away");
        std::fs::create_dir(&home).unwrap();
        let original = serde_json::to_vec_pretty(&json!({
            "next_project": 8,
            "projects": [
                {"id":"proj-2", "path":home,"name":"Home", "base_branch":"trunk", "next_source":19,
                 "isolation":"rift", "sources":[{"id":"source-3","name":"Home","mount":"code","path":home,"is_git":false,"base_branch":"trunk","sync_base":false}]},
                {"id":"proj-7", "path":parked,"base_branch":"main", "next_source":4,
                 "sources":[{"id":"source-1","name":"Away","mount":"away","path":parked,"is_git":true,"base_branch":"main","extra":"preserved"}]}
            ],
            "custom_setting": {"kept": true}
        })).unwrap();
        let config = root.path().join("config.json");
        std::fs::write(&config, &original).unwrap();
        Self {
            root,
            config,
            original,
        }
    }

    fn state(&self) -> AppState {
        state(self.root.path()).with_config(&self.config).unwrap()
    }

    fn read(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.config).unwrap()).unwrap()
    }
}

fn state(root: &Path) -> AppState {
    let context =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.join("state"))
            .unwrap();
    AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
}

#[test]
fn two_projects_migrate_to_two_environments() {
    let fixture = Fixture::new();
    let app = fixture.state();
    let config = fixture.read();
    assert_eq!(config["environments"].as_array().unwrap().len(), 2);
    assert_eq!(config["next_environment"], 3);
    for project in config["projects"].as_array().unwrap() {
        let environment = config["environments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|environment| environment["id"] == project["environment_id"])
            .unwrap();
        assert_eq!(environment["sources"], project["sources"]);
        assert_eq!(environment["next_source"], project["next_source"]);
        assert_eq!(project["added_sources"], json!([]));
        assert_eq!(project["archived_at"], Value::Null);
    }
    assert_eq!(config["projects"][0]["isolation"], "rift");
    assert_eq!(
        config["environments"][0]["isolation"],
        config["projects"][0]["isolation"]
    );
    assert_eq!(app.projects.parked().len(), 1);
    assert_eq!(config["custom_setting"]["kept"], true);
}

#[test]
fn migration_is_idempotent() {
    let fixture = Fixture::new();
    fixture.state();
    let migrated = std::fs::read(&fixture.config).unwrap();
    assert_ne!(migrated, fixture.original);
    fixture.state();
    assert_eq!(std::fs::read(&fixture.config).unwrap(), migrated);
}

#[test]
fn legacy_config_reloads() {
    let fixture = Fixture::new();
    let app = fixture.state();
    assert!(fixture.read()["projects"][0]["environment_id"].is_string());
    let row = app.project_for("proj-2").unwrap();
    assert_eq!(
        row.project_key,
        fixture.root.path().join("home").to_string_lossy()
    );
    assert_eq!(row.sources[0].id, "source-3");
    assert_eq!(row.next_source, 19);
    let before = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    app.persist();
    let reloaded = fixture.state();
    let after = reloaded.config_value(
        &reloaded.projects_dir,
        reloaded.default_harness,
        reloaded.isolation,
    );
    assert_eq!(before, after);
}

#[test]
fn migrated_project_key_is_home_path() {
    let fixture = Fixture::new();
    let mut config = fixture.read();
    let home = fixture.root.path().join("home");
    config["projects"][0]["path"] = json!(home.join("../home"));
    std::fs::write(&fixture.config, config.to_string()).unwrap();
    fixture.state();
    let config = fixture.read();
    assert_eq!(config["projects"][0]["project_key"], json!(home));
    assert_eq!(config["projects"][0]["home_path"], json!(home));
    assert_eq!(
        config["projects"][1]["project_key"],
        json!(fixture.root.path().join("away"))
    );
}

#[test]
fn migration_failure_leaves_config_untouched() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let fixture = Fixture::new();
        let mut app = state(fixture.root.path());
        app.config_persist_failure = Some(failure);
        assert!(app.with_config(&fixture.config).is_err());
        assert_eq!(std::fs::read(&fixture.config).unwrap(), fixture.original);
    }
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.config.with_extension("json.pre-environments")).unwrap();
    assert!(state(fixture.root.path())
        .with_config(&fixture.config)
        .is_err());
    assert_eq!(std::fs::read(&fixture.config).unwrap(), fixture.original);
}

#[test]
fn pre_environments_backup_written_once() {
    let fixture = Fixture::new();
    fixture.state();
    let backup = fixture.config.with_extension("json.pre-environments");
    assert_eq!(std::fs::read(&backup).unwrap(), fixture.original);
    // Even retrying a legacy config after a rollback cannot overwrite the snapshot.
    let mut original: Value = serde_json::from_slice(&fixture.original).unwrap();
    original["projects"][0]["name"] = json!("later legacy edit");
    std::fs::write(&fixture.config, original.to_string()).unwrap();
    fixture.state();
    assert_eq!(std::fs::read(&backup).unwrap(), fixture.original);
}

#[test]
fn parked_projects_survive_migration() {
    let fixture = Fixture::new();
    let mut app = fixture.state();
    let migrated = fixture.read();
    assert!(migrated["projects"][1]["environment_id"].is_string());
    assert_eq!(app.projects.parked()[0]["sources"][0]["extra"], "preserved");
    app.persist();
    assert_eq!(fixture.read()["projects"][1], migrated["projects"][1]);
    std::fs::create_dir(fixture.root.path().join("away")).unwrap();
    assert_eq!(app.revive_parked_projects(), ["proj-7"]);
    assert_eq!(
        app.project_for("proj-7").unwrap().project_key,
        fixture.root.path().join("away").to_string_lossy()
    );
}

#[test]
fn unknown_environment_is_parked() {
    let fixture = Fixture::new();
    let mut config = fixture.read();
    config["projects"][0]["environment_id"] = json!("env-missing");
    std::fs::write(&fixture.config, config.to_string()).unwrap();
    let mut app = fixture.state();
    assert!(app.project_for("proj-2").is_err());
    assert!(app.revive_parked_projects().is_empty());
    app.persist();
    assert_eq!(fixture.read()["projects"].as_array().unwrap().len(), 2);
}

#[test]
fn downgrade_with_sibling_project() {
    use frozen_legacy_loader::LegacyLoader;
    let fixture = Fixture::new();
    let original: Value = serde_json::from_slice(&fixture.original).unwrap();
    let before = LegacyLoader::load(&original);
    fixture.state();
    let mut migrated = fixture.read();
    assert!(migrated["environments"].is_array());
    let after = LegacyLoader::load(&migrated);
    assert_eq!(after.projects, before.projects);
    assert_eq!(after.parked.len(), before.parked.len());
    let mut sibling = migrated["projects"][0].clone();
    sibling["id"] = json!("proj-8");
    sibling["project_key"] = json!("project:independent-key");
    sibling["name"] = json!("Sibling work");
    sibling["next_source"] = json!(25);
    sibling["sources"][0]["name"] = json!("Sibling reference");
    sibling["isolation"] = json!("worktree");
    migrated["projects"].as_array_mut().unwrap().push(sibling);
    let downgraded = LegacyLoader::load(&migrated);
    // The legacy loader collapses identical homes into the first project. The
    // later sibling overwrites that project's sources/isolation/counter, but
    // never becomes a project or claims its opaque key.
    assert_eq!(downgraded.projects.len(), 1);
    assert_eq!(downgraded.projects[0]["id"], "proj-2");
    assert_eq!(
        downgraded.projects[0]["sources"][0]["name"],
        "Sibling reference"
    );
    assert_eq!(downgraded.projects[0]["isolation"], "worktree");
    assert_eq!(downgraded.projects[0]["next_source"], 25);
}

#[test]
fn previous_config_tracks_one_generation() {
    let fixture = Fixture::new();
    let app = fixture.state();
    let migrated = std::fs::read(&fixture.config).unwrap();
    assert_eq!(
        std::fs::read(fixture.config.with_extension("json.previous")).unwrap(),
        fixture.original
    );
    app.persist();
    assert_eq!(
        std::fs::read(fixture.config.with_extension("json.previous")).unwrap(),
        migrated
    );
    let current = std::fs::read(&fixture.config).unwrap();
    app.persist();
    assert_eq!(
        std::fs::read(fixture.config.with_extension("json.previous")).unwrap(),
        current
    );
}

#[test]
fn path_only_legacy_project_keeps_source_defaults() {
    use frozen_legacy_loader::LegacyLoader;
    let fixture = Fixture::new();
    let original = json!({"projects":[{"id":"proj-2","path":fixture.root.path().join("home"),"base_branch":"trunk"}]});
    std::fs::write(&fixture.config, original.to_string()).unwrap();
    let before = LegacyLoader::load(&original);
    let app = fixture.state();
    let after = LegacyLoader::load(&fixture.read());
    assert_eq!(after.projects, before.projects);
    assert_eq!(app.project_for("proj-2").unwrap().sources[0].sync_base, Some(true));
}

#[test]
fn migration_reserves_unknown_environment_references() {
    let fixture = Fixture::new();
    let mut config = fixture.read();
    config["projects"][0]["environment_id"] = json!("env-1");
    std::fs::write(&fixture.config, config.to_string()).unwrap();
    let app = fixture.state();
    assert!(app.project_for("proj-2").is_err());
    assert_eq!(fixture.read()["projects"][1]["environment_id"], "env-2");
}

#[test]
fn invalid_migration_keeps_original_bytes() {
    for patch in [
        json!({"sources":[{"id":"source-1"}]}),
        json!({"next_source":1,"sources":[{"id":"source-18446744073709551615","path":"/missing"}]}),
    ] {
        let fixture = Fixture::new();
        let mut config = fixture.read();
        for (key, value) in patch.as_object().unwrap() {
            config["projects"][0][key] = value.clone();
        }
        let original = config.to_string().into_bytes();
        std::fs::write(&fixture.config, &original).unwrap();
        assert!(state(fixture.root.path())
            .with_config(&fixture.config)
            .is_err());
        assert_eq!(std::fs::read(&fixture.config).unwrap(), original);
        assert!(!fixture
            .config
            .with_extension("json.pre-environments")
            .exists());
    }
}
