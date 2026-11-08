use super::runtime_tests::{state, rpc};
use serde_json::{json, Value};
use crate::app::AppState;

fn fixture(root: &std::path::Path) -> AppState {
    for name in ["home", "extra", "shared", "target"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    let config = json!({"projects_dir":root.join("managed"), "environments":[{
        "id":"env-1","name":"Original","sources":[
            {"id":"source-1","name":"home","mount":"home","path":root.join("home")},
            {"id":"source-2","name":"shared","mount":"shared","path":root.join("shared")}
        ],"next_source":4
    }],"projects":[
        {"id":"proj-1","environment_id":"env-1","project_key":"project:first","path":root.join("home"),"name":"First","added_sources":[]},
        {"id":"proj-2","environment_id":"env-1","project_key":"project:second","path":root.join("home"),"name":"Second","added_sources":[
            {"id":"source-3","name":"extra","mount":"extra","path":root.join("extra")}
        ]}
    ]});
    let path=root.join("config.json");
    std::fs::write(&path,config.to_string()).unwrap();
    state(root).with_config(path).unwrap()
}
fn ok(app: &mut AppState, method: &str, params: Value) -> Value {
    let reply=rpc(app,method,params);
    assert_eq!(reply["ok"],true,"{method}: {reply}");
    reply["result"].clone()
}

#[test]
fn environment_rename_keeps_ids_and_paths() {
    let root=tempfile::tempdir().unwrap();
    let mut app=fixture(root.path());
    let before=app.environment_list()["environments"][0].clone();
    let row=ok(&mut app,"environment.rename",json!({"environment_id":"env-1","name":"Display / name"}));
    assert_eq!(row["name"],"Display / name");
    for key in ["environment_id","home_path","sources","project_ids"] {assert_eq!(row[key],before[key]);}
    let before=app.project_json(app.projects.get("proj-1").unwrap());
    let row=ok(&mut app,"project.rename",json!({"project_id":"proj-1","name":"New project"}));
    for key in ["project_id","project_key","path","environment_id","sources"] {assert_eq!(row[key],before[key]);}
    assert_eq!(row["name"],"New project");
    let app=state(root.path()).with_config(root.path().join("config.json")).unwrap();
    assert_eq!(app.projects.environment("env-1").unwrap().name,"Display / name");
    assert_eq!(app.projects.get("proj-1").unwrap().name,"New project");
}

#[test]
fn rename_is_one_bounded_line() {
    let root=tempfile::tempdir().unwrap(); let mut app=fixture(root.path());
    for (method,key,id) in [("environment.rename","environment_id","env-1"),("project.rename","project_id","proj-1")] {
        for name in ["".to_string(),"   ".into(),"two\nlines".into(),"trailing\n".into(),"tab\tname".into(),"unicode\u{2028}line".into(),"x".repeat(201)] {
            let reply=rpc(&mut app,method,json!({key:id,"name":name}));
            assert_eq!(reply["ok"],false,"{method}: {reply}");
        }
        assert_eq!(ok(&mut app,method,json!({key:id,"name":" x "}))["name"],"x");
    }
}

#[test]
fn isolation_resolution_order() {
    use crate::isolation::Isolation;
    let root=tempfile::tempdir().unwrap(); let mut app=fixture(root.path());
    app.isolation=Isolation::Rift;
    ok(&mut app,"environment.set_isolation",json!({"environment_id":"env-1","isolation":"worktree"}));
    assert_eq!(app.project_isolation(app.projects.get("proj-1").unwrap()),Isolation::Worktree);
    ok(&mut app,"project.set_isolation",json!({"project_id":"proj-1","isolation":"worktree"}));
    ok(&mut app,"environment.set_isolation",json!({"environment_id":"env-1","isolation":null}));
    assert_eq!(app.project_isolation(app.projects.get("proj-1").unwrap()),Isolation::Worktree);
    assert_eq!(app.project_isolation(app.projects.get("proj-2").unwrap()),Isolation::Rift);
    ok(&mut app,"project.set_isolation",json!({"project_id":"proj-1","isolation":null}));
    assert_eq!(app.project_isolation(app.projects.get("proj-1").unwrap()),Isolation::Rift);
}

#[test]
fn environment_source_verbs_add_remove_update_sync() {
    let root=tempfile::tempdir().unwrap(); let mut app=fixture(root.path());
    let target=root.path().join("target");
    crate::git_fixture::init_repo_named(root.path(),"git");
    let row=ok(&mut app,"environment.add_source",json!({"environment_id":"env-1","path":target}));
    let source=row["sources"].as_array().unwrap().iter().find(|s|s["path"]==json!(target)).unwrap();
    let id=source["id"].as_str().unwrap().to_string();
    for p in ["proj-1","proj-2"] {assert!(app.projects.get(p).unwrap().sources.iter().any(|s|s.id==id));}
    let row=ok(&mut app,"environment.update_source",json!({"environment_id":"env-1","source_id":id,"name":"Renamed","path":root.path().join("git"),"sync_base":false}));
    assert_eq!(row["checkouts_updated"],0);
    assert!(row["sources"].as_array().unwrap().iter().any(|s|s["id"]==id && s["name"]=="Renamed"));
    ok(&mut app,"environment.sync_source",json!({"environment_id":"env-1","source_id":id}));
    ok(&mut app,"environment.remove_source",json!({"environment_id":"env-1","source_id":id}));
    assert!(root.path().join("git").is_dir());
    for p in ["proj-1","proj-2"] {assert!(!app.projects.get(p).unwrap().sources.iter().any(|s|s.id==id));}
    assert_eq!(app.projects.get("proj-2").unwrap().added_sources.len(),1);
    let app=state(root.path()).with_config(root.path().join("config.json")).unwrap();
    assert!(!app.projects.environment("env-1").unwrap().sources.iter().any(|s|s.id==id));
}

#[test]
fn environment_sources_refuse_containment_and_dependent_duplicates() {
    let root=tempfile::tempdir().unwrap(); let mut app=fixture(root.path());
    std::fs::create_dir(root.path().join("home/nested")).unwrap();
    for method in ["environment.add_source","environment.update_source"] {
        for path in [root.path().join("extra"),root.path().join("home/nested"),root.path().join("home"),root.path().to_path_buf()] {
            let before=std::fs::read(root.path().join("config.json")).unwrap();
            let reply=rpc(&mut app,method,json!({"environment_id":"env-1","source_id":"source-2","path":path}));
            assert_eq!(reply["ok"],false,"{method}: {reply}");
            assert_eq!(std::fs::read(root.path().join("config.json")).unwrap(),before);
        }
    }
}

#[test]
fn environment_edits_work_without_projects_and_rollback_on_save_failure() {
    let root=tempfile::tempdir().unwrap(); let mut app=fixture(root.path());
    app.projects.clear();
    ok(&mut app,"environment.rename",json!({"environment_id":"env-1","name":"Orphan"}));
    ok(&mut app,"environment.add_source",json!({"environment_id":"env-1","path":root.path().join("target")}));
    ok(&mut app,"environment.update_source",json!({"environment_id":"env-1","source_id":"source-2","name":"Edited"}));
    ok(&mut app,"environment.set_isolation",json!({"environment_id":"env-1","isolation":"worktree"}));
    let before=app.environment_list();
    let bytes=std::fs::read(root.path().join("config.json")).unwrap();
    app.config_persist_failure=Some(crate::app::ConfigPersistStep::Write);
    let reply=rpc(&mut app,"environment.rename",json!({"environment_id":"env-1","name":"Cannot save"}));
    assert_eq!(reply["ok"],false,"{reply}");
    assert_eq!(app.environment_list(),before);
    assert_eq!(std::fs::read(root.path().join("config.json")).unwrap(),bytes);
}

#[test]
fn environment_source_sync_runs_for_an_orphan_reference() {
    let root=tempfile::tempdir().unwrap(); let mut app=fixture(root.path());
    let git=crate::git_fixture::init_repo_named(root.path(),"git");
    let row=ok(&mut app,"environment.add_source",json!({"environment_id":"env-1","path":git}));
    let id=row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    app.projects.clear();
    assert_eq!(ok(&mut app,"environment.sync_source",json!({"environment_id":"env-1","source_id":id})),json!({"pending":true}));
    let state=std::sync::Arc::new(std::sync::Mutex::new(app));
    crate::app::AppState::sync_sources(&state,crate::app::projects::base_sync::SyncPass::Requested,1000);
    let app=state.lock().unwrap();
    let sources=app.environment_list()["environments"][0]["sources"].clone();
    assert_eq!(sources.as_array().unwrap().last().unwrap()["sync"]["state"],"no_remote");
}
