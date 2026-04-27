//! `backup` table row + API response shape.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;

/// One generated archive. The file lives on disk under
/// `config.backup_location_path`; this row is the index entry.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct Backup {
    pub id: i64,
    /// `manual` | `scheduled` | `pre_restore`. Stored as `TEXT`;
    /// `value_type` override emits the typed enum on the `OpenAPI`
    /// surface so the frontend gets the narrow union.
    #[schema(value_type = BackupKind)]
    pub kind: String,
    /// File name relative to `config.backup_location_path`.
    pub filename: String,
    pub size_bytes: i64,
    pub kino_version: String,
    pub schema_version: i64,
    pub checksum_sha256: String,
    pub created_at: String,
}

/// Trigger source for a backup row. Frontend renders kind-specific
/// badges (Manual = neutral, Scheduled = blue, Pre-restore = amber)
/// so the visually-distinct pre-restore rows are harder to delete
/// by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    Manual,
    Scheduled,
    PreRestore,
}

impl BackupKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
            Self::PreRestore => "pre_restore",
        }
    }
}
