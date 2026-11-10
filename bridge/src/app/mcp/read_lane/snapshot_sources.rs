//! Current source references from one accepted immutable producer root.
use super::super::service::Inputs;
use super::super::{PreparedRead, ReadSession, ReadWork};
use super::json::JsonRef;
use crate::app::read_view::SharedJson;
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn prepare(inputs: &Arc<Inputs>, session: &ReadSession) -> Result<PreparedRead, String> {
    let owner = &session.coding()?.owner;
    let (project, _) = inputs.project(owner)?;
    let scope = session.project(inputs, format!("unknown project_id: {project}"))?;
    let project = scope.id.clone();
    let projects = snapshot(inputs, "project.list")?;
    let environments = snapshot(inputs, "environment.list")?;
    Ok(PreparedRead::query(None, move |_| {
        Ok(ReadWork::external(move || {
            inventory(&project, &projects, &environments)
        }))
    })
    .for_project(scope))
}

fn snapshot(inputs: &Inputs, method: &'static str) -> Result<SharedJson, String> {
    inputs
        .view
        .get_shared(method, "")
        .cloned()
        .ok_or_else(|| format!("{method}: snapshot was not initialized"))
}

fn inventory(id: &str, projects: &SharedJson, environments: &SharedJson) -> Result<Value, String> {
    let project = JsonRef::Shared(projects)
        .field("projects")
        .rows()
        .find(|row| row.field("project_id").string() == Some(id))
        .ok_or_else(|| format!("unknown project_id: {id}"))?;
    let environment_id = project
        .field("environment_id")
        .string()
        .ok_or_else(|| format!("project {id} has no environment"))?;
    let environment = JsonRef::Shared(environments)
        .field("environments")
        .rows()
        .find(|row| row.field("environment_id").string() == Some(environment_id))
        .ok_or_else(|| format!("unknown environment: {environment_id}"))?;
    let sources = environment
        .field("sources")
        .rows()
        .map(|source| source_row(source, "environment"))
        .chain(
            project
                .field("added_sources")
                .rows()
                .map(|source| source_row(source, "project")),
        )
        .collect::<Vec<_>>();
    Ok(json!({"project_id":id,"environment_id":environment_id,"sources":sources}))
}

fn source_row(source: JsonRef<'_>, scope: &str) -> Value {
    json!({"id":source.field("id").value(),"path":source.field("path").value(),"scope":scope})
}
