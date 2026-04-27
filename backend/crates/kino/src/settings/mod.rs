//! Settings — user-configurable application settings: the singleton
//! `config` row + quality profiles. Per the layout doc indexer
//! settings also belong here, but indexer CRUD lives in
//! `indexers/handlers.rs` next to the engine it drives — the
//! indexers/ module owns enough operational logic that splitting the
//! handlers across two trees would obscure more than it clarifies.
//!
//! ## Public API
//!
//! - `config::{ConfigRow, ConfigResponse, ConfigUpdate, REDACTED}` —
//!   the row model + masked response shape + the secret sentinel
//!   (`"***"`) the frontend echoes back on no-change saves
//! - `quality_profile::{QualityProfile, QualityProfileWithUsage,
//!   QualityTier, CreateQualityProfile, UpdateQualityProfile,
//!   default_quality_items, resolve_quality_profile}` — the row
//!   model + usage-augmented variant + the tier shape acquisition's
//!   policy gate scores against, plus the create/update DTOs and the
//!   resolver content/movie + content/show use to default a new
//!   row's profile
//! - `config::*_handlers` + `quality_profile::*_handlers` — HTTP
//!   surface, registered in main.rs

pub mod config;
pub mod quality_profile;
