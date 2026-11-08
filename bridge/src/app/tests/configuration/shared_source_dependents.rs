//! An environment reference must stay unique in every dependent project.
use super::*;

#[test]
fn inherited_source_repoint_refuses_duplicate_in_sibling_project() {
    for parked_sibling in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let original = root.path().join("original");
        let target = root.path().join("target");
        for path in [&home, &original, &target] {
            std::fs::create_dir(path).unwrap();
        }
        let sibling_home = if parked_sibling {
            root.path().join("away")
        } else {
            home.clone()
        };
        let config = root.path().join("config.json");
        std::fs::write(
            &config,
            json!({
                "projects_dir":root.path().join("projects"),
                "environments":[{
                    "id":"env-1","name":"shared","next_source":4,
                    "sources":[
                        {"id":"source-1","path":home,"name":"home","mount":"home"},
                        {"id":"source-2","path":original,"name":"original","mount":"original"}
                    ]
                }],
                "projects":[
                    {"id":"proj-1","environment_id":"env-1","project_key":"project:first",
                     "path":home,"home_path":home,"added_sources":[]},
                    {"id":"proj-2","environment_id":"env-1","project_key":"project:second",
                     "path":sibling_home,"home_path":sibling_home,
                     "added_sources":[{"id":"source-3","path":target,"name":"target","mount":"target"}]}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let context = crate::harness::HarnessContext::resolved(
            root.path().join("mcp.sock"),
            root.path().join("state"),
        )
        .unwrap();
        let mut app = AppState::new_unrooted_configured(
            root.path().join("workspaces"),
            "main",
            true,
            context,
        )
        .with_config(&config)
        .unwrap();
        assert_eq!(app.projects.parked().len(), usize::from(parked_sibling));
        let before = std::fs::read(&config).unwrap();
        let answer = app.handle(req(
            "project.update_source",
            json!({"project_id":"proj-1","source_id":"source-2","path":target}),
        ));
        assert_eq!(answer["ok"], false, "parked={parked_sibling}: {answer}");
        assert!(
            answer["error"]
                .as_str()
                .unwrap()
                .contains("already included in this project"),
            "parked={parked_sibling}: {answer}"
        );
        assert_eq!(std::fs::read(&config).unwrap(), before);
        assert_eq!(app.sources_for("proj-1").unwrap()[1].path, original);
        if !parked_sibling {
            let sibling = app.sources_for("proj-2").unwrap();
            assert_eq!(sibling.len(), 3);
            assert_eq!(sibling[1].path, original);
            assert_eq!(sibling[2].path, target);
        }
    }
}

#[cfg(unix)]
#[test]
fn managed_clone_destination_symlink_into_registered_source_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let shared = root.path().join("shared");
    let projects = root.path().join("projects");
    for path in [&a, &b, &shared, &projects] {
        std::fs::create_dir(path).unwrap();
    }
    std::os::unix::fs::symlink(&shared, projects.join("b-sources")).unwrap();
    let config = root.path().join("config.json");
    std::fs::write(
        &config,
        json!({
            "projects_dir":projects,
            "projects":[
                {"id":"proj-1","path":a,"sources":[
                    {"id":"source-1","path":a,"name":"a","mount":"a"},
                    {"id":"source-2","path":shared,"name":"shared","mount":"shared"}
                ]},
                {"id":"proj-2","path":b,"sources":[
                    {"id":"source-1","path":b,"name":"b","mount":"b"}
                ]}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let context = crate::harness::HarnessContext::resolved(
        root.path().join("mcp.sock"),
        root.path().join("state"),
    )
    .unwrap();
    let mut app =
        AppState::new_unrooted_configured(root.path().join("workspaces"), "main", true, context)
            .with_config(&config)
            .unwrap();
    let before = std::fs::read(&config).unwrap();
    let answer = app.handle(req(
        "project.add_source",
        json!({"project_id":"proj-2","name":"nested","remote":"https://example.invalid/nested.git"}),
    ));
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("managed source destination"),
        "{answer}"
    );
    assert_eq!(std::fs::read(&config).unwrap(), before);
    assert!(!shared.join("nested").exists());
}
