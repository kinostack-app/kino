//! Cross-cutting type bag — only enums that other domains depend on
//! land here. Per-domain models live in their domain module
//! (content/movie/model.rs, download/model.rs, etc.); models/ exists
//! so the shared `enums` namespace doesn't pull a domain prefix that
//! it isn't owned by.

pub mod enums;
