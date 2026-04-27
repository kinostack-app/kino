//! Windows SCM dispatcher — bridges `kino serve` invoked by the
//! Service Control Manager into the same async server bootstrap used
//! by interactive runs.
//!
//! How SCM differs from a normal process:
//! - SCM doesn't deliver Unix-style signals. `SERVICE_CONTROL_STOP`
//!   arrives via the registered control-handler callback, not as
//!   `SIGTERM`.
//! - SCM expects the service binary to call `service_dispatcher::start`
//!   *first*, before doing anything else. That call blocks the main
//!   thread until the service exits; SCM invokes our `service_main`
//!   callback on a background thread.
//! - Failure to call `service_dispatcher::start` when launched by SCM
//!   manifests as the service hanging in "Starting" until SCM times
//!   out (default 30s) and declares the launch failed.
//!
//! Approach: try `service_dispatcher::start` first. If it fails with
//! `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT` (1063) we know we're
//! running interactively, not under SCM, and fall back to the
//! standard async runtime path. This keeps `kino serve` working from
//! both `cmd.exe` and `services.msc`.
//!
//! Pairs with `service_install/windows.rs`. Compile-verified by
//! `cross-os.yml` (Windows clippy on PRs that touch this file).

#![cfg(target_os = "windows")]

use std::ffi::OsString;
use std::sync::Mutex;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use windows_service::Error as WsError;
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

const SERVICE_NAME: &str = "kino";
// Standard Windows error code: "the service process could not connect
// to the service controller". Returned by StartServiceCtrlDispatcher
// when the binary was launched outside of SCM (i.e. interactively).
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;

pub type ServerRunner = Box<dyn FnOnce(CancellationToken) -> i32 + Send>;

/// Outcome of attempting to attach to the SCM dispatcher.
pub enum ScmOutcome {
    /// SCM ran the service. The dispatcher blocked until the
    /// service stopped; exit status was reported through
    /// `set_service_status`. Caller should return success.
    Claimed,
    /// SCM declined — we're not running under it (interactive
    /// invocation from a console). Caller should run the returned
    /// runner directly on the main thread.
    NotUnderScm(ServerRunner),
}

// Bridge state passed from `try_run_under_scm` into `service_main`
// (which SCM calls on a background thread, with a `extern "system"`
// signature that can't capture). `Mutex<Option<_>>` because the
// dispatcher hands ownership through once and we Take it out of the
// slot before invoking.
static SERVER_RUNNER: Mutex<Option<ServerRunner>> = Mutex::new(None);

/// Try to attach to the SCM dispatcher. See `ScmOutcome` for the
/// possible results.
///
/// The `runner` closure is invoked on a background thread by SCM
/// once the dispatcher hands us control. It must own the full
/// server lifecycle and exit when its `CancellationToken` fires —
/// that's how `SERVICE_CONTROL_STOP` translates into a graceful
/// shutdown of the axum listener.
pub fn try_run_under_scm<F>(runner: F) -> anyhow::Result<ScmOutcome>
where
    F: FnOnce(CancellationToken) -> i32 + Send + 'static,
{
    {
        let mut slot = SERVER_RUNNER
            .lock()
            .map_err(|_| anyhow::anyhow!("service runner mutex poisoned"))?;
        if slot.is_some() {
            anyhow::bail!("service runner already registered");
        }
        *slot = Some(Box::new(runner));
    }

    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => Ok(ScmOutcome::Claimed),
        Err(WsError::Winapi(e))
            if e.raw_os_error() == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT) =>
        {
            // Not under SCM — drain the runner so the caller can
            // run it directly on the main thread.
            let runner = SERVER_RUNNER
                .lock()
                .ok()
                .and_then(|mut s| s.take())
                .ok_or_else(|| anyhow::anyhow!("service runner missing after SCM decline"))?;
            Ok(ScmOutcome::NotUnderScm(runner))
        }
        Err(e) => Err(anyhow::anyhow!(e).context("starting Windows service dispatcher")),
    }
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    // Implicit opt-in to "exit-after-restore" when running under
    // SCM. Recorded via an `AtomicBool` rather than `std::env::set_var`
    // because the workspace forbids `unsafe_code` and `set_var` is
    // unsafe under Rust 2024. SCM Recovery actions (configured in
    // `service_install/windows.rs`) restart the service when it
    // exits non-zero — same shape as systemd's Restart=on-failure
    // and launchd's KeepAlive.SuccessfulExit=false.
    crate::backup::handlers::set_restart_after_restore_marker();

    // Any panic in here would crash the SCM thread without setting
    // our service status to Stopped — SCM would leave the service in
    // "Starting" until it timed out. Catch + report to keep the
    // failure mode graceful.
    let _ = std::panic::catch_unwind(run_service);
}

fn run_service() {
    // Cancellation token shared between the control-handler closure
    // (signals on Stop) and the server runner (waits on cancelled).
    let cancel = CancellationToken::new();

    // Register the control handler. SCM calls our closure for every
    // control event — Stop / Shutdown / Preshutdown all funnel into
    // the same cancellation. Other events are reported as
    // "not implemented" so SCM doesn't think they succeeded.
    let cancel_for_handler = cancel.clone();
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                cancel_for_handler.cancel();
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let Ok(status_handle) = service_control_handler::register(SERVICE_NAME, event_handler) else {
        // No handle = no way to report status. Best we can do is
        // bail; SCM will eventually time out and mark us failed.
        return;
    };

    // Tell SCM we're up and accepting Stop / Shutdown.
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });

    let runner = SERVER_RUNNER.lock().ok().and_then(|mut s| s.take());
    let exit_code = runner.map_or(1, |r| r(cancel.clone()));

    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(u32::try_from(exit_code).unwrap_or(1)),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });
}
