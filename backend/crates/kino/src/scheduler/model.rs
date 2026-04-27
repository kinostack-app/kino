#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SchedulerState {
    pub task_name: String,
    pub last_run_at: Option<String>,
}
