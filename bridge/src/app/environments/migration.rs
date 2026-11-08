//! Transform legacy records without opening or dropping their source folders.
use crate::app::config::expand_tilde;
use crate::app::projects::configured_primary_path;
use serde_json::{json, Value};
use std::path::PathBuf;

pub(in crate::app) fn migrate_config(config: &Value) -> Result<Option<Value>, String> {
    let projects = match config.get("projects") {
        None => return Ok(None),
        Some(value) => value.as_array().ok_or("projects must be an array")?,
    };
    if projects
        .iter()
        .all(|project| project.get("environment_id").is_some())
    {
        return Ok(None);
    }
    let mut environments = config
        .get("environments")
        .map(|value| {
            value
                .as_array()
                .cloned()
                .ok_or("environments must be an array")
        })
        .transpose()?
        .unwrap_or_default();
    let mut next = next_environment(config, &environments)?;
    let projects = projects
        .iter()
        .map(|project| {
            if project.get("environment_id").is_some() {
                return Ok(project.clone());
            }
            let id = format!("env-{next}");
            next = next
                .checked_add(1)
                .ok_or("environment id counter exhausted")?;
            let (project, environment) = migrate_project(project, &id)?;
            environments.push(environment);
            Ok(project)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut migrated = config.clone();
    migrated["environments"] = json!(environments);
    migrated["next_environment"] = json!(next);
    migrated["projects"] = json!(projects);
    Ok(Some(migrated))
}

fn next_environment(config: &Value, environments: &[Value]) -> Result<u64, String> {
    let owned = environments
        .iter()
        .filter_map(|environment| environment.get("id")?.as_str());
    let referenced = config
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|project| project.get("environment_id")?.as_str());
    let maximum = owned
        .chain(referenced)
        .filter_map(|id| id.strip_prefix("env-")?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    let next = maximum
        .checked_add(1)
        .ok_or("environment id counter exhausted")?;
    Ok(next.max(
        config
            .get("next_environment")
            .and_then(Value::as_u64)
            .unwrap_or(1),
    ))
}

fn migrate_project(project: &Value, environment_id: &str) -> Result<(Value, Value), String> {
    if !project.is_object() {
        return Err("project entry must be an object".into());
    }
    let named = configured_primary_path(project)
        .filter(|path| !path.is_empty())
        .ok_or("legacy project has no home path")?;
    let home = canonical_home(named)?;
    let home_text = home.display().to_string();
    let name = project
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            home.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "project".into());
    let sources = legacy_sources(project, &home, &name)?;
    let next_source = source_counter(project, &sources)?;
    let isolation = project.get("isolation").cloned().unwrap_or(Value::Null);
    let environment = json!({"id":environment_id,"name":name,"sources":sources,
        "next_source":next_source,"isolation":isolation});
    let mut migrated = project.clone();
    migrated["environment_id"] = json!(environment_id);
    migrated["project_key"] = json!(home_text);
    migrated["home_path"] = json!(home_text);
    migrated["name"] = json!(name);
    migrated["added_sources"] = json!([]);
    migrated["archived_at"] = Value::Null;
    // Retain the override too: M0.1 mirrors edits to both 1:1 records. The
    // legacy reader ignores the environment, so removing this loses isolation.
    migrated["isolation"] = isolation;
    migrated["path"] = json!(home_text);
    migrated["sources"] = json!(sources);
    migrated["next_source"] = json!(next_source);
    if migrated.get("base_branch").is_none() {
        migrated["base_branch"] = json!("main");
    }
    Ok((migrated, environment))
}

fn canonical_home(named: &str) -> Result<PathBuf, String> {
    let path = expand_tilde(named);
    match std::fs::canonicalize(&path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path.is_absolute() {
                return Ok(path);
            }
            std::env::current_dir()
                .map(|current| current.join(path))
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(format!("resolve project home {}: {error}", path.display())),
    }
}

fn legacy_sources(
    project: &Value,
    home: &std::path::Path,
    name: &str,
) -> Result<Vec<Value>, String> {
    let sources = project
        .get("sources")
        .map(|sources| {
            sources
                .as_array()
                .cloned()
                .ok_or("project sources must be an array")
        })
        .transpose()?
        .unwrap_or_default();
    if sources.is_empty() {
        return Ok(vec![
            json!({"id":"source-1","name":name,"mount":crate::app::projects::safe_mount_name(name),
            "path":home.display().to_string(),"is_git":home.join(".git").exists(),"sync_base":true,
            "base_branch":project.get("base_branch").and_then(Value::as_str).unwrap_or("main")}),
        ]);
    }
    for source in &sources {
        if source
            .get("path")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err("legacy source has no path".into());
        }
    }
    Ok(sources)
}

fn source_counter(project: &Value, sources: &[Value]) -> Result<u64, String> {
    let maximum = sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            source
                .get("id")
                .and_then(Value::as_str)
                .map(|id| id.strip_prefix("source-")?.parse::<u64>().ok())
                .unwrap_or(Some(index as u64 + 1))
        })
        .max()
        .unwrap_or(0);
    let next = maximum
        .checked_add(1)
        .ok_or("source id counter exhausted")?;
    Ok(next.max(
        project
            .get("next_source")
            .and_then(Value::as_u64)
            .unwrap_or(1),
    ))
}
