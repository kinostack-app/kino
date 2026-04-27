use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::error::{AppError, AppResult};
use crate::scheduler::TaskInfo;
use crate::state::AppState;

/// List all scheduled tasks.
#[utoipa::path(
    get, path = "/api/v1/tasks",
    responses((status = 200, body = Vec<TaskInfo>)),
    tag = "tasks", security(("api_key" = []))
)]
pub async fn list_tasks(State(state): State<AppState>) -> AppResult<Json<Vec<TaskInfo>>> {
    let scheduler = state
        .scheduler
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("scheduler not initialized".into()))?;
    Ok(Json(scheduler.list_tasks().await))
}

/// Trigger a task to run immediately.
#[utoipa::path(
    post, path = "/api/v1/tasks/{name}/run",
    params(("name" = String, Path)),
    responses((status = 200), (status = 404)),
    tag = "tasks", security(("api_key" = []))
)]
pub async fn run_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<StatusCode> {
    let scheduler = state
        .scheduler
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("scheduler not initialized".into()))?;

    let tasks = scheduler.list_tasks().await;
    if !tasks.iter().any(|t| t.name == name) {
        return Err(AppError::NotFound(format!("task '{name}' not found")));
    }

    // Send to scheduler loop via channel — it will execute the task
    state
        .trigger_tx
        .send(crate::scheduler::TaskTrigger::fire(name))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("trigger send failed: {e}")))?;

    Ok(StatusCode::OK)
}
