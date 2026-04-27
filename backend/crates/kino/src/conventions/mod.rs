//! Compile-time + test-time enforcement of project conventions.
//!
//! The conventions themselves live in
//! `docs/architecture/conventions.md`. This module owns the
//! mechanical checks that keep them honest: today the SQL
//! timestamp-comparison guard.

#[cfg(test)]
mod sql_timestamp_compare;
