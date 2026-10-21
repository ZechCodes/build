//! MCP's caller-project fence over the existing independent task projection.
use super::{ReadService, TaskReadContext};
use crate::mcp::BridgeAction;
use crate::store::{ReadConnection, ReadTransaction};
use serde_json::{json, Value};

pub(in crate::app) enum TaskQuery {
    Get {
        task_id: String,
    },
    List {
        state: Option<String>,
        status: Option<String>,
        label: Option<String>,
    },
    Dependents {
        task_id: String,
        limit: Option<u64>,
        cursor: Option<String>,
    },
}

impl TaskQuery {
    pub(in crate::app) fn from_action(action: &BridgeAction) -> Option<Self> {
        Some(match action {
            BridgeAction::TrackerGetTask { task_id } => Self::Get {
                task_id: task_id.clone(),
            },
            BridgeAction::TrackerListTasks {
                state,
                status,
                label,
            } => Self::List {
                state: state.clone(),
                status: status.clone(),
                label: label.clone(),
            },
            BridgeAction::TrackerTaskDependents {
                task_id,
                limit,
                cursor,
            } => Self::Dependents {
                task_id: task_id.clone(),
                limit: *limit,
                cursor: cursor.clone(),
            },
            _ => return None,
        })
    }

    fn method(&self) -> &'static str {
        match self {
            Self::Get { .. } => "tasks.get",
            Self::List { .. } => "tasks.list",
            Self::Dependents { .. } => "tasks.waiting_on.blocks",
        }
    }

    pub(in crate::app) fn scope_error(&self, project: &str) -> String {
        match self {
            Self::Get { task_id } | Self::Dependents { task_id, .. } => {
                format!("unknown task_id: {task_id}")
            }
            Self::List { .. } => format!("unknown project_id: {project}"),
        }
    }

    fn params(&self, project: &str) -> Value {
        match self {
            Self::Get { task_id } => json!({"task_id":task_id}),
            Self::List {
                state,
                status,
                label,
            } => {
                let mut params = json!({"project_id":project});
                for (name, value) in [("state", state), ("status", status), ("label", label)] {
                    if let Some(value) = value {
                        params[name] = json!(value);
                    }
                }
                params
            }
            Self::Dependents {
                task_id,
                limit,
                cursor,
            } => {
                let mut params = json!({"task_id":task_id});
                if let Some(limit) = limit {
                    params["limit"] = json!(limit);
                }
                if let Some(cursor) = cursor {
                    params["cursor"] = json!(cursor);
                }
                params
            }
        }
    }

    fn read(
        &self,
        context: &TaskReadContext,
        transaction: &ReadTransaction<'_>,
        params: &Value,
    ) -> Result<Value, String> {
        match self {
            Self::Get { .. } => context.tasks_get(transaction, params),
            Self::List { .. } => context.tasks_list(transaction, params),
            Self::Dependents { .. } => context.tasks_waiting_on_blocks(transaction, params),
        }
    }
}

impl ReadService {
    pub(in crate::app) fn mcp_tasks(
        &self,
        project: &str,
        query: &TaskQuery,
        connection: Option<&mut ReadConnection>,
    ) -> Result<Value, String> {
        let connection = connection
            .ok_or_else(|| "tasks need a durable store; this bridge has none".to_string())?;
        let context = self
            .context
            .load_full()
            .expect("MCP read context initialized before admission");
        if context.deleting_project {
            return Err(crate::app::rpc::PROJECT_DELETION_IN_PROGRESS.into());
        }
        let params = query.params(project);
        let (value, fence) = connection
            .read(|transaction| -> Result<_, String> {
                let fence = context
                    .tasks
                    .validate_scope(transaction, query.method(), &params)?;
                if fence.project_id() != project {
                    return Err(query.scope_error(project));
                }
                let value = query.read(&context.tasks, transaction, &params)?;
                Ok((value, fence))
            })
            .map_err(|error| error.to_string())??;
        let latest = self.context.load_full().expect("initialized context");
        if latest.deleting_project {
            return Err(crate::app::rpc::PROJECT_DELETION_IN_PROGRESS.into());
        }
        latest.tasks.validate_fence(&fence)?;
        Ok(value)
    }
}
