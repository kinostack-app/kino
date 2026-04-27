//! Content domain — Movie + Show + Episode + Series + Media.
//!
//! Each content type sits in its own submodule with the model, the
//! HTTP handlers, and (for show) the `episode` + `series` siblings.
//! `derived_state` owns the SQL fragments other domains (library,
//! home) join through to compute user-facing status.
//!
//! ## Public API
//!
//! Each submodule's `model.rs` exports the row + DTO types that
//! cross domain boundaries (acquisition's `ReleaseTarget` impls
//! consume `Movie` and `Episode`; import writes `Media`; library's
//! search joins through `Show`). The `handlers` files are consumed
//! only by main.rs's router. `derived_state` is pulled into queries
//! across library + home + content's own handlers — the SQL
//! fragments it returns are the contract.
//!
//! No top-level `pub use` here intentionally — callers reach in via
//! `content::movie::model::Movie` etc. so the file path tells the
//! reader where to look. The path is the discoverability mechanism.

pub mod derived_state;
pub mod media;
pub mod movie;
pub mod show;
