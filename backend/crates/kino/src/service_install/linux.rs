//! Linux (systemd) service install.
//!
//! Writes a unit file, reloads the daemon, enables + starts the
//! service. System mode targets `/etc/systemd/system/kino.service`
//! (requires root); user mode targets
//! `~/.config/systemd/user/kino.service` (no privilege required;
//! service runs only while the user is logged in).
//!
//! Idempotent — running `kino install-service` twice is a no-op
//! after the first run. Uninstall stops + disables + removes the
//! unit; user data is preserved.

use anyhow::{Context as _, anyhow, bail};
use std::path::PathBuf;
use std::process::Command;

const UNIT_NAME: &str = "kino.service";

pub fn install(user_mode: bool) -> anyhow::Result<()> {
    if !user_mode {
        require_root()?;
    }

    let unit_path = unit_path(user_mode)?;
    let unit_body = render_unit(user_mode)?;

    if let Some(parent) = unit_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating unit directory {}", parent.display()))?;
    }

    std::fs::write(&unit_path, unit_body)
        .with_context(|| format!("writing systemd unit to {}", unit_path.display()))?;
    eprintln!("✓ wrote {}", unit_path.display());

    systemctl(user_mode, &["daemon-reload"])?;
    systemctl(user_mode, &["enable", UNIT_NAME])?;
    systemctl(user_mode, &["start", UNIT_NAME])?;

    let scope = if user_mode { "--user" } else { "system" };
    eprintln!("✓ kino installed as a {scope} systemd service");
    eprintln!(
        "  Status: systemctl{} status kino",
        if user_mode { " --user" } else { "" }
    );
    eprintln!(
        "  Logs:   journalctl{} -u kino -f",
        if user_mode { " --user" } else { "" }
    );
    eprintln!("  Open:   http://localhost  (or http://kino.local from any LAN device)");
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    // Walk the system path first; if the unit doesn't live there,
    // fall back to user. Either we find it or we have nothing to do.
    for &user_mode in &[false, true] {
        let unit_path = unit_path(user_mode)?;
        if !unit_path.exists() {
            continue;
        }
        if !user_mode {
            require_root()?;
        }
        // Best-effort stop + disable; ignore errors so we still
        // remove the unit file even if the service was already gone.
        let _ = systemctl(user_mode, &["stop", UNIT_NAME]);
        let _ = systemctl(user_mode, &["disable", UNIT_NAME]);
        std::fs::remove_file(&unit_path)
            .with_context(|| format!("removing {}", unit_path.display()))?;
        let _ = systemctl(user_mode, &["daemon-reload"]);
        eprintln!("✓ removed {}", unit_path.display());
        eprintln!("  User data preserved. Run `kino reset` to wipe it.");
        return Ok(());
    }
    eprintln!("kino service isn't installed");
    Ok(())
}

fn unit_path(user_mode: bool) -> anyhow::Result<PathBuf> {
    if user_mode {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow!("HOME env var not set; can't pick user-mode unit path"))?;
        Ok(PathBuf::from(home)
            .join(".config/systemd/user")
            .join(UNIT_NAME))
    } else {
        Ok(PathBuf::from("/etc/systemd/system").join(UNIT_NAME))
    }
}

fn render_unit(user_mode: bool) -> anyhow::Result<String> {
    let exe = std::env::current_exe().context("locating current binary path")?;
    let exe_str = exe
        .to_str()
        .ok_or_else(|| anyhow!("current binary path isn't valid UTF-8"))?;

    // System mode runs as the kino service user (created by .deb / .rpm
    // postinst). User mode runs as the invoking user.
    let user_block = if user_mode {
        ""
    } else {
        "User=kino\nGroup=kino\n"
    };

    // Pass --data-path via env var rather than CLI flag. `kino`'s
    // `--data-path` is a TOP-LEVEL flag (declared on the parent CLI
    // struct, not on the `serve` subcommand), so writing
    // `kino serve --data-path X` fails with INVALIDARGUMENT. Env var
    // is position-independent + doesn't leak into the visible
    // `ps` listing.
    //
    // User mode keeps data in $XDG_DATA_HOME — the binary's path
    // resolver picks this up when KINO_DATA_PATH is unset.
    let data_path_env = if user_mode {
        String::new()
    } else {
        "Environment=KINO_DATA_PATH=/var/lib/kino\n".to_string()
    };

    // System mode grants CAP_NET_BIND_SERVICE so the unprivileged
    // kino user can bind port 80 (the schema default). User mode
    // skips it — per-user units can't grant capabilities without
    // root, and Settings → Port can flip the user-mode install to
    // 8080 (or any unprivileged port) if needed. KINO_PORT is NOT
    // set in either descriptor: it only seeds the DB on first run,
    // and setting it permanently in the unit would mask user
    // changes from Settings.
    let cap_net_bind = if user_mode {
        ""
    } else {
        " CAP_NET_BIND_SERVICE"
    };
    // System-mode CWD pinned to /var/lib/kino so relative-path I/O
    // (notably librqbit's first-piece file open) lands in writable
    // territory rather than the invoking shell's CWD — which can
    // sit under /home and be empty under ProtectHome=true.
    // User-mode units inherit a sensible CWD from the user
    // session, so no override needed there.
    let working_dir = if user_mode {
        String::new()
    } else {
        "WorkingDirectory=/var/lib/kino\n         ".to_string()
    };

    Ok(format!(
        "[Unit]\n\
         Description=Kino — single-binary media automation and streaming server\n\
         Documentation=https://kinostack.app\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         {user_block}\
         {working_dir}\
         Environment=KINO_RESTART_AFTER_RESTORE=1\n\
         {data_path_env}\
         Environment=KINO_NO_OPEN_BROWSER=1\n\
         Environment=KINO_INSTALL_KIND=linux-systemd\n\
         ExecStart={exe_str} serve\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         AmbientCapabilities=CAP_NET_RAW CAP_NET_ADMIN{cap_net_bind}\n\
         CapabilityBoundingSet=CAP_NET_RAW CAP_NET_ADMIN{cap_net_bind}\n\
         NoNewPrivileges=true\n\
         \n\
         [Install]\n\
         WantedBy={target}\n",
        target = if user_mode {
            "default.target"
        } else {
            "multi-user.target"
        },
    ))
}

fn systemctl(user_mode: bool, args: &[&str]) -> anyhow::Result<()> {
    let mut cmd = Command::new("systemctl");
    if user_mode {
        cmd.arg("--user");
    }
    cmd.args(args);
    let output = cmd
        .output()
        .with_context(|| format!("running `systemctl {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "systemctl {} failed (exit {}): {}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn require_root() -> anyhow::Result<()> {
    // Best-effort EUID check via the `id -u` shell-out — saves us
    // pulling in libc just for one call. Returns the user's EUID as
    // a string; "0" means root.
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("running `id -u` to check root privileges")?;
    let euid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if euid != "0" {
        bail!(
            "system-wide service install requires root. Re-run with sudo, \
             or pass `--user` to install a per-user systemd unit instead."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_renders_user_mode() {
        let body = render_unit(true).unwrap();
        assert!(body.contains("WantedBy=default.target"));
        assert!(!body.contains("User=kino"));
        assert!(body.contains("Environment=KINO_NO_OPEN_BROWSER=1"));
        // User mode lets the binary fall back to $XDG_DATA_HOME — no
        // KINO_DATA_PATH override.
        assert!(!body.contains("KINO_DATA_PATH"));
        // User-mode units can't grant CAP_NET_BIND_SERVICE without
        // root, so the privileged-port bits are gated to system mode.
        assert!(!body.contains("KINO_PORT=80"));
        assert!(!body.contains("CAP_NET_BIND_SERVICE"));
        // User-mode units inherit a sensible CWD from the login
        // session; system-only ProtectHome workaround.
        assert!(!body.contains("WorkingDirectory="));
        // Top-level flags must NOT appear after `serve`; they're parent
        // flags and clap rejects them past the subcommand.
        assert!(!body.contains("serve --"));
    }

    #[test]
    fn unit_renders_system_mode() {
        let body = render_unit(false).unwrap();
        assert!(body.contains("WantedBy=multi-user.target"));
        assert!(body.contains("User=kino"));
        assert!(body.contains("Environment=KINO_DATA_PATH=/var/lib/kino"));
        assert!(body.contains("Environment=KINO_NO_OPEN_BROWSER=1"));
        // CAP_NET_BIND_SERVICE so the unprivileged kino user can
        // bind the schema-default port 80. KINO_PORT is NOT set in
        // the unit — it's a one-shot first-boot env that seeds the
        // DB; setting it permanently would mask Settings edits.
        assert!(!body.contains("KINO_PORT"));
        assert!(
            body.contains("AmbientCapabilities=CAP_NET_RAW CAP_NET_ADMIN CAP_NET_BIND_SERVICE")
        );
        assert!(
            body.contains("CapabilityBoundingSet=CAP_NET_RAW CAP_NET_ADMIN CAP_NET_BIND_SERVICE")
        );
        // CWD pinned so librqbit's first-piece file open lands in
        // writable territory under ProtectHome=true.
        assert!(body.contains("WorkingDirectory=/var/lib/kino"));
        // Backup-restore exit-after-restore opt-in
        assert!(body.contains("Environment=KINO_RESTART_AFTER_RESTORE=1"));
        // Regression guard: top-level flags must not be passed after
        // `serve` — clap rejects them and systemd reports
        // status=2/INVALIDARGUMENT.
        assert!(!body.contains("serve --"));
        assert!(!body.contains("--data-path"));
        assert!(!body.contains("--no-open-browser"));
    }

    #[test]
    fn user_unit_path_lands_under_home_xdg() {
        // Read live HOME — tests don't mutate env (workspace forbids
        // unsafe, including the now-unsafe std::env::set_var). Just
        // assert the suffix and that the prefix matches HOME.
        let Some(home) = std::env::var_os("HOME") else {
            // Unlikely on any tester's machine; skip if HOME isn't set.
            return;
        };
        let path = unit_path(true).unwrap();
        assert!(path.ends_with(".config/systemd/user/kino.service"));
        assert!(path.starts_with(PathBuf::from(home)));
    }

    #[test]
    fn system_unit_path_is_etc() {
        let path = unit_path(false).unwrap();
        assert_eq!(
            path,
            std::path::PathBuf::from("/etc/systemd/system/kino.service")
        );
    }
}
