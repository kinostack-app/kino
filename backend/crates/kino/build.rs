//! kino's build script.
//!
//! Ensures `frontend/dist/` exists before `cargo build` so
//! `rust-embed` (in `src/spa.rs`) can compile the SPA into the
//! binary. If `dist/index.html` is missing or empty, runs
//! `npm ci && npm run build` in the `frontend/` workspace.
//!
//! Behaviour by environment:
//!
//! - **Skip the build entirely** if `KINO_SKIP_FRONTEND_BUILD=1`.
//!   CI sets this when it's already run `npm run build` in a
//!   prior step (avoids duplicate npm work per matrix target).
//!   Devs setting it must remember to keep `frontend/dist/` warm.
//! - **Skip the build if `dist/index.html` already exists.** The
//!   devcontainer's frontend service runs `npm run build` once
//!   on container start, so subsequent `cargo build` runs reuse.
//! - **Run `npm` if `npm` is on PATH** and dist is missing.
//! - **Bail with a clear error** if `npm` isn't available — the
//!   user needs to either install Node + run the npm commands
//!   manually, or set `KINO_SKIP_FRONTEND_BUILD=1` and
//!   pre-populate `frontend/dist/` themselves.
//!
//! `cargo:rerun-if-changed=` lines below tell cargo to re-run
//! this script when frontend sources change, so a `cargo build`
//! after editing a `.tsx` triggers a rebuild of the embedded
//! bundle.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let frontend_dir = PathBuf::from(&manifest_dir).join("../../../frontend");
    let dist_index = frontend_dir.join("dist").join("index.html");

    // Tell cargo when to re-run us. Frontend source edits invalidate
    // the embedded bundle, so we want to rebuild on those.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=KINO_SKIP_FRONTEND_BUILD");
    println!(
        "cargo:rerun-if-changed={}",
        frontend_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        frontend_dir.join("src").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        frontend_dir.join("vite.config.ts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        frontend_dir.join("tsconfig.json").display()
    );

    if std::env::var_os("KINO_SKIP_FRONTEND_BUILD").is_some_and(|v| v == "1") {
        if dist_index.exists() && file_nonempty(&dist_index) {
            // Pre-built (CI ran `npm run build` in a prior step;
            // Dockerfile copied dist/ from the frontend stage).
            // Trust it.
            return;
        }
        // Stub-dist mode. Used by the devcontainer's backend service
        // where (a) npm isn't installed, (b) the SPA doesn't matter
        // because devs hit the Vite dev server on :5173 directly.
        // Synthesize a placeholder index.html so rust-embed has
        // something to bind against; the embedded SPA is non-
        // functional but the binary compiles + runs.
        println!(
            "cargo:warning=KINO_SKIP_FRONTEND_BUILD=1 + missing dist — synthesizing stub. \
             Production builds must populate {} before compiling.",
            dist_index.display()
        );
        let dist_dir = frontend_dir.join("dist");
        std::fs::create_dir_all(&dist_dir)
            .unwrap_or_else(|e| panic!("create stub dist dir at {}: {e}", dist_dir.display()));
        std::fs::write(
            &dist_index,
            "<!doctype html><meta charset=\"utf-8\"><title>kino</title>\n\
             <p>kino's embedded SPA was not bundled into this build.</p>\n\
             <p>Open the dev frontend at <a href=\"http://localhost:5173/\">localhost:5173</a> \
             or rebuild with <code>KINO_SKIP_FRONTEND_BUILD</code> unset.</p>\n",
        )
        .unwrap_or_else(|e| panic!("write stub index.html: {e}"));
        return;
    }

    if dist_index.exists() && file_nonempty(&dist_index) {
        // Already built. cargo:rerun-if-changed lines above will
        // re-trigger us if frontend sources change.
        return;
    }

    // Need to build. Locate npm.
    assert!(
        Command::new("npm").arg("--version").output().is_ok(),
        "kino's build script needs to run `npm install && npm run build` in {}, \
         but `npm` is not on PATH. Either install Node.js (>= 22 recommended), \
         or pre-build the frontend yourself and set KINO_SKIP_FRONTEND_BUILD=1.",
        frontend_dir.display()
    );

    println!(
        "cargo:warning=building frontend SPA in {} (this is a one-time ~30s step)",
        frontend_dir.display()
    );

    // Use `npm ci` if package-lock.json is present (reproducible),
    // otherwise fall back to `npm install`. Most checkouts have
    // the lockfile.
    let install_cmd = if frontend_dir.join("package-lock.json").exists() {
        "ci"
    } else {
        "install"
    };

    run_npm(&frontend_dir, &[install_cmd]);
    run_npm(&frontend_dir, &["run", "build"]);

    assert!(
        dist_index.exists(),
        "frontend build completed but {} still missing — Vite config or build script is wrong",
        dist_index.display()
    );
}

fn run_npm(cwd: &Path, args: &[&str]) {
    let status = Command::new("npm")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn `npm {}`: {e}", args.join(" ")));
    assert!(
        status.success(),
        "`npm {}` exited with {} in {}",
        args.join(" "),
        status,
        cwd.display(),
    );
}

fn file_nonempty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() > 0)
}
