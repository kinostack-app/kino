//! Windows (Service Control Manager) service install via the
//! `windows-service` crate (Mullvad's; the production reference).
//!
//! Registers `kino` as an SCM service that runs as `LocalSystem`,
//! starts on boot, and restarts on failure with a 5-second delay.
//! Pairs with the Windows SCM dispatcher in `main.rs` — without that
//! dispatcher the service registers but `kino serve` doesn't respond
//! to `SERVICE_CONTROL_STOP`.
//!
//! Compile-verified by `cross-os.yml` (Windows clippy on PRs that
//! touch `service_install/`). Runtime verification waits for the
//! first hands-on test on a Windows host — see
//! `docs/architecture/service-install.md`.

use anyhow::{Context as _, anyhow};
use std::ffi::OsString;
use std::time::Duration;
use windows_service::Error as WsError;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl, ServiceFailureActions,
    ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

const SERVICE_NAME: &str = "kino";
const SERVICE_DISPLAY_NAME: &str = "Kino — media server";
const SERVICE_DESCRIPTION: &str =
    "Single-binary media automation and streaming server. https://kinostack.app";

// Standard Windows error code for "service not installed". Returned
// by `OpenServiceW` when the named service doesn't exist. Used to
// distinguish "nothing to uninstall" (exit clean) from real failures.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

pub fn install() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("connecting to the Service Control Manager — admin/elevated required")?;

    let exe = std::env::current_exe().context("locating current binary path")?;
    // `--no-open-browser` is a TOP-LEVEL flag on `kino`, not on the
    // `serve` subcommand. clap rejects it with INVALIDARGUMENT if it
    // sits AFTER `serve`. Order: top-level flags first, subcommand
    // last. (Service env-vars on Windows would require an SCM
    // registry edit; passing as args is the cleaner Windows path.)
    //
    // Native packages set --data-path explicitly via the MSI's
    // service ImagePath; this fallback lets the binary fall back to
    // `paths::default_data_dir()` (%LOCALAPPDATA%\kino on Windows).
    let launch_args = vec![OsString::from("--no-open-browser"), OsString::from("serve")];

    let service_info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: launch_args,
        dependencies: vec![],
        // None = LocalSystem account, matching Sonarr / Jellyfin.
        // See docs/architecture/service-install.md "Open design
        // decisions" for the rationale.
        account_name: None,
        account_password: None,
    };

    let service = manager
        .create_service(
            &service_info,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
        )
        .context("registering the kino service with SCM")?;

    service
        .set_description(SERVICE_DESCRIPTION)
        .context("setting service description")?;

    // Service Recovery: restart on failure after 5s, twice, then
    // give up (so a wedged binary doesn't pin a CPU core
    // restarting forever). `reset_period: Never` keeps the failure
    // counter — the SCM default — so we don't hide repeated crashes
    // by clearing the count.
    let recovery = ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::Never,
        reboot_msg: None,
        command: None,
        actions: Some(vec![
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(5),
            },
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(5),
            },
            ServiceAction {
                action_type: ServiceActionType::None,
                delay: Duration::from_secs(0),
            },
        ]),
    };
    service
        .update_failure_actions(recovery)
        .context("setting service failure-recovery actions")?;

    service
        .start::<&str>(&[])
        .context("starting the kino service")?;

    eprintln!("✓ kino installed as a Windows service (SCM)");
    eprintln!("  Status: sc query kino");
    eprintln!("  Logs:   Event Viewer → Windows Logs → Application");
    eprintln!("  Open:   http://localhost:8080");
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("connecting to the Service Control Manager — admin/elevated required")?;

    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    let service = match manager.open_service(SERVICE_NAME, access) {
        Ok(s) => s,
        Err(WsError::Winapi(e)) if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) => {
            eprintln!("kino service isn't installed");
            return Ok(());
        }
        Err(e) => return Err(anyhow!(e).context("opening the kino service")),
    };

    // Best-effort stop — if it's already stopped, ignore the error
    // and proceed straight to deletion.
    if let Ok(status) = service.query_status() {
        if status.current_state != ServiceState::Stopped {
            let _ = service.stop();
        }
    }

    service.delete().context("deleting the kino service")?;
    eprintln!("✓ removed the kino Windows service");
    eprintln!("  User data preserved. Run `kino reset` to wipe it.");
    Ok(())
}
