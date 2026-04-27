//! Import — completed download → library file. The pipeline is two
//! cleanly-separated paths: `single` for one-file releases (movie or
//! single episode), `pack` for season-pack torrents that ship many
//! episodes in one download. Both share the helpers in `trigger`
//! (naming context, materialise-with-staged-rename, file pick,
//! subtitle fetch).
//!
//! ## Public API
//!
//! `trigger::import_download` is the only entry point. The download
//! monitor calls it the moment a torrent reports `completed`; the
//! flow tests call it directly to skip librqbit. Everything else in
//! this module is internal to the import flow:
//! - `single::do_import` / `pack::do_pack_import` — dispatched to by
//!   `import_download` based on `linked_episodes.len()`. `pub(crate)`
//!   only so the dispatch hop works across files.
//! - `trigger`-level helpers (`NamingFormats`, `ParsedQuality`,
//!   `materialise_into_library`, `find_media_file`,
//!   `episode/movie_naming_context`, `fetch_subtitles_best_effort`, etc)
//!   are `pub(crate)` siblings used only by single + pack.
//! - `archive`, `ffprobe`, `naming`, `pipeline`, `transfer` are
//!   primitives the import path composes; not consumed elsewhere.

pub mod archive;
pub mod ffprobe;
pub mod naming;
pub mod pack;
pub mod pipeline;
pub mod single;
pub mod transfer;
pub mod trigger;

#[cfg(test)]
mod tests;
