//! End-to-end flow tests — the `journey-level` coverage called out in
//! `docs/roadmap/31-integration-testing.md`. One file per user
//! flow, each exercising a happy path plus the most load-bearing
//! edge cases.
//!
//! Kept as a `#[cfg(test)]` module tree inside the binary crate (vs.
//! `tests/` directory) because kino ships as a single `[[bin]]` and
//! integration tests in `tests/*.rs` would need a separate `[lib]`
//! target. The in-module approach skips that restructuring and
//! still gives us full access to the public `test_support` harness.
//!
//! Test style follows the arrange-act-assert convention. Each test
//! builds its own `TestApp` from scratch; no shared state, no test
//! ordering, no `#[serial]`. Assertions are on *outcomes* (what the
//! user sees via the API) not on *side effects* (what internal
//! functions were called).

mod blocklist_api;
mod calendar_ics;
mod calendar_movie;
mod calendar_with_content;
mod cast_token;
mod client_logs;
mod config_api;
mod continue_watching;
mod delete_movie_cascade;
mod delete_show;
mod download_actions;
mod download_extra_actions;
mod download_files;
mod downloads_api;
mod edge_cases;
mod episode_watched;
mod follow_conflicts;
mod follow_movie;
mod follow_show;
mod fs_browse;
mod fs_browse_dir;
mod fs_browse_extra;
mod grab_to_import;
mod history_api;
mod history_episode_filter;
mod history_filters;
mod indexer_actions;
mod indexer_crud;
mod integrations_status;
mod library_search;
mod library_search_extras;
mod library_views;
mod lists_actions;
mod lists_api;
mod logo_api;
mod logs_api;
mod logs_export;
mod logs_source_filter;
mod media_api;
mod metadata_test_api;
mod misc_404s;
mod movie_watched;
mod movies_list_detail;
mod preferences_api;
mod quality_profiles;
mod quality_profiles_actions;
mod redownload_episode;
mod releases_api;
mod scheduler_trigger;
mod setup_wizard;
mod show_monitor;
mod shows_detail;
mod shows_episodes;
mod shows_seasons;
mod status_warnings;
mod system_list_delete;
mod tasks_api;
mod tmdb_genres_discover;
mod tmdb_proxy;
mod trakt_actions;
mod trakt_extras;
mod transcode_status;
mod upgrade_eligibility;
mod upgrade_flow;
mod vpn_actions;
mod watch_now_api;
mod webhook_test_endpoint;
mod webhooks_crud;
