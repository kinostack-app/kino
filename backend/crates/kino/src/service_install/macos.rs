//! macOS (launchd) service install.
//!
//! Writes a `LaunchDaemon` plist to
//! `/Library/LaunchDaemons/tv.kino.daemon.plist` (system mode), then
//! shells out to `launchctl load` to register + start the service.
//! Triggers a Tk-style admin prompt via `osascript` if not already
//! root, so the user gets the standard macOS auth flow rather than a
//! cryptic "permission denied".
//!
//! The plist sets:
//! - `RunAtLoad` so the service comes up when the daemon registers
//! - `KeepAlive` (only when `SuccessfulExit=false`) so a clean
//!   shutdown stays shut down, but a crash gets restarted by launchd
//! - `StandardOutPath` + `StandardErrorPath` to
//!   `/var/log/kino/{stdout,stderr}.log` so logs land somewhere
//!   `tail -f` can find them. Native packages (cargo-dist's `.pkg`)
//!   should pre-create that directory; we create it on the fly here
//!   for the tarball / `cargo install` fallback
//!
//! Compile-verified by `cross-os.yml`. Runtime verification waits
//! for the first hands-on test on a macOS host — see
//! `docs/architecture/service-install.md`.

use anyhow::{Context as _, anyhow, bail};
use std::path::Path;
use std::process::Command;

const SERVICE_LABEL: &str = "tv.kino.daemon";
const PLIST_PATH: &str = "/Library/LaunchDaemons/tv.kino.daemon.plist";
const LOG_DIR: &str = "/var/log/kino";

pub fn install() -> anyhow::Result<()> {
    if !is_root() {
        return relaunch_via_osascript();
    }

    // Make sure the log dir exists so launchd can redirect into it.
    // `mkdir -p` is idempotent; we don't fail if it's already there.
    std::fs::create_dir_all(LOG_DIR)
        .with_context(|| format!("creating log directory {LOG_DIR}"))?;

    let exe = std::env::current_exe().context("locating current binary path")?;
    let plist = render_plist(&exe)?;

    std::fs::write(PLIST_PATH, plist)
        .with_context(|| format!("writing LaunchDaemon plist to {PLIST_PATH}"))?;
    eprintln!("✓ wrote {PLIST_PATH}");

    // launchd wants the plist owned by root:wheel with mode 0644 or
    // it'll silently refuse to load it. `chown` + `chmod` here so
    // that's true even if the umask above didn't enforce it.
    chmod(PLIST_PATH, "644")?;
    chown(PLIST_PATH, "root:wheel")?;

    // `launchctl load -w` registers the daemon AND clears its
    // disabled flag, so it starts now and on every boot.
    launchctl(&["load", "-w", PLIST_PATH])?;

    eprintln!("✓ kino installed as a macOS LaunchDaemon");
    eprintln!("  Status: sudo launchctl print system/{SERVICE_LABEL}");
    eprintln!("  Logs:   tail -f {LOG_DIR}/stderr.log");
    eprintln!("  Open:   http://localhost:8080");
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    if !is_root() {
        return relaunch_via_osascript();
    }

    if !Path::new(PLIST_PATH).exists() {
        eprintln!("kino LaunchDaemon isn't installed");
        return Ok(());
    }

    // Best-effort unload — if launchd already forgot about it (e.g.
    // user did `launchctl unload` manually), still proceed to delete
    // the plist file.
    let _ = launchctl(&["unload", "-w", PLIST_PATH]);

    std::fs::remove_file(PLIST_PATH).with_context(|| format!("removing {PLIST_PATH}"))?;
    eprintln!("✓ removed {PLIST_PATH}");
    eprintln!("  User data preserved. Run `kino reset` to wipe it.");
    Ok(())
}

fn is_root() -> bool {
    // `id -u` returns the EUID; "0" is root. Using a shell-out
    // instead of `libc::geteuid()` keeps the workspace's
    // `unsafe_code = "forbid"` lint happy without an extra crate.
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

/// Re-launch the current binary with the same args via `osascript`
/// `do shell script "..." with administrator privileges`, which
/// triggers the native macOS admin prompt. After the elevated
/// process exits, we return its status to the user. If they cancel
/// the prompt, `osascript` exits non-zero — we surface that as a
/// clear "elevation declined" error rather than a cryptic exit code.
fn relaunch_via_osascript() -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("locating current binary path")?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exe_str = exe
        .to_str()
        .ok_or_else(|| anyhow!("current binary path isn't valid UTF-8"))?;

    // Build the shell command that osascript will run elevated.
    // Quote each argument so spaces survive. `osascript` itself
    // double-decodes, so we use single-quoted shell strings inside
    // the AppleScript double-quoted string.
    let mut shell_cmd = shell_quote(exe_str);
    for arg in &args {
        shell_cmd.push(' ');
        shell_cmd.push_str(&shell_quote(arg));
    }

    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell_cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );

    let status = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .context("running osascript for the admin prompt")?;

    if status.success() {
        Ok(())
    } else {
        bail!(
            "elevation declined or failed (osascript exit {}). \
             Re-run from an admin terminal with `sudo` if the \
             prompt didn't appear.",
            status.code().unwrap_or(-1)
        )
    }
}

fn shell_quote(s: &str) -> String {
    // Single-quote everything; close-quote, escape, re-open for any
    // embedded single quotes. Standard sh-quoting recipe.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn launchctl(args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("running `launchctl {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "launchctl {} failed (exit {}): {}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn chmod(path: &str, mode: &str) -> anyhow::Result<()> {
    let status = Command::new("chmod")
        .arg(mode)
        .arg(path)
        .status()
        .with_context(|| format!("running `chmod {mode} {path}`"))?;
    if !status.success() {
        bail!(
            "chmod {mode} {path} failed (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn chown(path: &str, owner: &str) -> anyhow::Result<()> {
    let status = Command::new("chown")
        .arg(owner)
        .arg(path)
        .status()
        .with_context(|| format!("running `chown {owner} {path}`"))?;
    if !status.success() {
        bail!(
            "chown {owner} {path} failed (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn render_plist(exe: &Path) -> anyhow::Result<String> {
    let exe_str = exe
        .to_str()
        .ok_or_else(|| anyhow!("current binary path isn't valid UTF-8"))?;
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <!--
      `--no-open-browser` is a TOP-LEVEL flag on `kino`, not on the
      `serve` subcommand. Putting it AFTER `serve` here would fail
      clap parsing (status=2/INVALIDARGUMENT) — same bug class that
      hit the deb's systemd unit pre-fix. We pass it via the
      KINO_NO_OPEN_BROWSER env var below instead.
    -->
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{log_dir}/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>{log_dir}/stderr.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>RUST_LOG</key>
        <string>info</string>
        <key>KINO_NO_OPEN_BROWSER</key>
        <string>1</string>
        <!--
          Opt the binary into "exit-after-restore" so a successful POST
          to /api/v1/backups/{{id}}/restore exits with EX_TEMPFAIL and
          launchd restarts the process against the freshly-restored
          database (KeepAlive.SuccessfulExit=false above catches the
          non-zero exit). No user action required.
        -->
        <key>KINO_RESTART_AFTER_RESTORE</key>
        <string>1</string>
    </dict>
</dict>
</plist>
"#,
        label = SERVICE_LABEL,
        exe = exe_str,
        log_dir = LOG_DIR,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn plist_contains_required_keys() {
        let exe = PathBuf::from("/usr/local/bin/kino");
        let body = render_plist(&exe).unwrap();
        assert!(body.contains("<string>tv.kino.daemon</string>"));
        assert!(body.contains("<string>/usr/local/bin/kino</string>"));
        // KINO_NO_OPEN_BROWSER must be set as an env var, NOT as a CLI
        // arg after `serve` — see service_install/linux.rs for the
        // bug class this guards against.
        assert!(body.contains("<key>KINO_NO_OPEN_BROWSER</key>"));
        assert!(!body.contains("<string>--no-open-browser</string>"));
        assert!(body.contains("<key>KeepAlive</key>"));
        assert!(body.contains("<key>SuccessfulExit</key>"));
        assert!(body.contains("/var/log/kino/stderr.log"));
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(shell_quote("foo"), "'foo'");
        assert_eq!(shell_quote("foo bar"), "'foo bar'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
