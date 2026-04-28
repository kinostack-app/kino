//! `kino tray` — system-tray / menu-bar icon with a 5-second status
//! poll and a "Open Kino in browser" / "Quit tray" menu.
//!
//! Runs on the calling thread (which is `main()` after the sync
//! subcommand dispatch, so the tao event loop has the main thread on
//! macOS where `AppKit` demands it). A worker thread runs a small
//! single-threaded tokio runtime that polls `/api/v1/status` every
//! 5 seconds and forwards each result back to the GUI thread via a
//! `tao` user-event proxy. A second worker thread bridges
//! `tray-icon`'s `MenuEvent` channel into the same proxy so the main
//! event loop has a single dispatch point.
//!
//! The tray polls `/api/v1/status` (public) rather than
//! `/api/v1/health` (auth-protected) — the status payload carries
//! enough to derive a four-state colour (operational / degraded /
//! critical / unreachable) without the tray having to discover the
//! API key. Promotion to `/api/v1/health` is gated on the credential-
//! pickup story (env var passthrough or a per-user "tray token" file)
//! and tracked alongside the spec deviation noted in
//! `docs/roadmap/22-desktop-tray.md`.

use std::time::Duration;

use anyhow::Context as _;
use tao::event::Event as TaoEvent;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use super::lock;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const POLL_TIMEOUT: Duration = Duration::from_secs(3);
const ICON_SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Operational,
    Degraded,
    Critical,
    Unreachable,
}

impl Status {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Operational => (0x4c, 0xaf, 0x50),
            Self::Degraded => (0xff, 0x9c, 0x00),
            Self::Critical => (0xe5, 0x39, 0x35),
            Self::Unreachable => (0x90, 0x90, 0x90),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Operational => "Status: Running ✓",
            Self::Degraded => "Status: Degraded ⚠",
            Self::Critical => "Status: Error ✗",
            Self::Unreachable => "Status: Disconnected from service",
        }
    }
}

#[derive(Debug)]
enum UserEvent {
    HealthUpdate(Status),
    MenuClicked(MenuId),
}

pub fn run() -> anyhow::Result<()> {
    let _lock = lock::acquire()?;

    // Resolve the live server URL. The server writes
    // `/run/kino/url` after a successful bind — composed from the
    // configured `mdns_hostname` + actual bound port, so the tray
    // opens `http://kino.local` (or whatever hostname the user
    // chose) when mDNS is enabled, and `http://localhost:<port>`
    // when not. Falls back to localhost when the file is missing
    // (server hasn't started yet, macOS / Windows where we don't
    // write it). Same resolver `kino open` uses, so menu + .desktop
    // entry always agree.
    let server_url = crate::discovered_url();

    // tao's event loop must own the main thread (AppKit assumption on
    // macOS; matches Linux GTK and Windows message-pump conventions).
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    spawn_health_poll(server_url.clone(), proxy.clone());
    spawn_menu_bridge(proxy);

    let menu = tray_icon::menu::Menu::new();
    let info_item = MenuItem::new("Status: connecting…", false, None);
    let version_item = MenuItem::new(
        format!("Version {}", env!("CARGO_PKG_VERSION")),
        false,
        None,
    );
    let open_item = MenuItem::new("Open Kino in browser", true, None);
    let quit_item = MenuItem::new("Quit tray", true, None);
    menu.append(&info_item)
        .context("appending info menu item")?;
    menu.append(&version_item)
        .context("appending version menu item")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("appending separator")?;
    menu.append(&open_item)
        .context("appending open menu item")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("appending separator")?;
    menu.append(&quit_item)
        .context("appending quit menu item")?;

    let open_id = open_item.id().clone();
    let quit_id = quit_item.id().clone();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(make_icon(Status::Unreachable))
        .with_tooltip("Kino")
        .build()
        .context("building tray icon")?;

    let mut current_status = Status::Unreachable;
    let browser_url = server_url;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        let TaoEvent::UserEvent(user) = event else {
            return;
        };
        match user {
            UserEvent::HealthUpdate(status) => {
                if status == current_status {
                    return;
                }
                current_status = status;
                if let Err(e) = tray.set_icon(Some(make_icon(status))) {
                    tracing::warn!(error = %e, "tray: failed to update icon");
                }
                info_item.set_text(status.label());
            }
            UserEvent::MenuClicked(id) => {
                if id == open_id {
                    if let Err(e) = webbrowser::open(&browser_url) {
                        tracing::warn!(error = %e, url = %browser_url, "tray: failed to open browser");
                    }
                } else if id == quit_id {
                    *control_flow = ControlFlow::Exit;
                }
            }
        }
    });
}

fn spawn_health_poll(server_url: String, proxy: EventLoopProxy<UserEvent>) {
    std::thread::Builder::new()
        .name("kino-tray-health".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(error = %e, "tray: failed to build poll runtime");
                    return;
                }
            };
            rt.block_on(async move {
                let client = reqwest::Client::builder()
                    .timeout(POLL_TIMEOUT)
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new());
                loop {
                    let status = poll_status(&client, &server_url).await;
                    if proxy.send_event(UserEvent::HealthUpdate(status)).is_err() {
                        // Event loop closed — tray is shutting down.
                        break;
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            });
        })
        .expect("spawning kino-tray-health thread");
}

fn spawn_menu_bridge(proxy: EventLoopProxy<UserEvent>) {
    // tray-icon delivers menu clicks via a global crossbeam channel,
    // not the tao event loop. Bridge them so the GUI thread has a
    // single dispatch point and we don't have to add a wake-up timer
    // to the otherwise idle event loop.
    std::thread::Builder::new()
        .name("kino-tray-menu".into())
        .spawn(move || {
            let rx = MenuEvent::receiver();
            while let Ok(event) = rx.recv() {
                if proxy.send_event(UserEvent::MenuClicked(event.id)).is_err() {
                    break;
                }
            }
        })
        .expect("spawning kino-tray-menu thread");
}

async fn poll_status(client: &reqwest::Client, server_url: &str) -> Status {
    let url = format!("{server_url}/api/v1/status");
    let Ok(resp) = client.get(&url).send().await else {
        return Status::Unreachable;
    };
    if !resp.status().is_success() {
        return Status::Critical;
    }
    let Ok(body) = resp.json::<serde_json::Value>().await else {
        return Status::Critical;
    };
    let ok = body.get("status").and_then(serde_json::Value::as_str) == Some("ok");
    let setup_required = body
        .get("setup_required")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let warnings = body
        .get("warnings")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);

    if !ok {
        Status::Critical
    } else if setup_required || warnings > 0 {
        Status::Degraded
    } else {
        Status::Operational
    }
}

// Branded tray icon: the kino mark (compiled in as a 32×32 PNG)
// with a small status-coloured badge overlaid in the bottom-right
// corner. The badge is what changes between status transitions —
// the mark itself stays put so the icon remains recognisable as
// kino at a glance, with status as a secondary signal.
//
// Built fresh each time. We previously cached per-status in a `OnceLock`
// but that broke Windows: tray-icon's Windows `Icon` wraps a HICON
// (`*mut c_void`) which isn't `Send + Sync`, so it can't sit in a
// `static`. Rebuilding is ~4 KB allocation + a 32×32 RGBA composite —
// fires at most once per 5 s poll on a status transition. Trivial.

/// 32×32 RGBA brand mark, decoded once per icon rebuild. The bytes
/// are baked into the binary at compile time so there's nothing to
/// fail at startup — if decoding ever does fail (it won't unless we
/// ship a corrupt PNG), `make_icon` falls back to a tinted disc.
const BRAND_PNG: &[u8] = include_bytes!("../../../../../packaging/icons/linux/kino-32.png");

fn make_icon(status: Status) -> Icon {
    let pixels = (ICON_SIZE * ICON_SIZE) as usize;
    let mut buf = decode_brand_rgba().unwrap_or_else(|| {
        // Fallback: solid disc tinted to status colour. Only fires
        // if BRAND_PNG fails to decode, which means we shipped a
        // broken PNG. Better to show *something* than panic.
        let rgb = status.rgb();
        let mut b = vec![0u8; pixels * 4];
        let size_f: f32 = 32.0;
        let center = size_f / 2.0;
        let radius = center - 1.0;
        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let dx = f32::from(u16::try_from(x).unwrap_or(0)) - center;
                let dy = f32::from(u16::try_from(y).unwrap_or(0)) - center;
                if dx.hypot(dy) <= radius {
                    let i = ((y * ICON_SIZE + x) * 4) as usize;
                    b[i] = rgb.0;
                    b[i + 1] = rgb.1;
                    b[i + 2] = rgb.2;
                    b[i + 3] = 255;
                }
            }
        }
        b
    });

    overlay_status_badge(&mut buf, status);
    Icon::from_rgba(buf, ICON_SIZE, ICON_SIZE).expect("32x32 RGBA icon buffer is valid")
}

fn decode_brand_rgba() -> Option<Vec<u8>> {
    // Decode the embedded PNG to RGBA8. The shipped asset is already
    // 32×32 so we don't resize; if a future packaging change ships a
    // different size we'd add a resize step here.
    let img = image::load_from_memory(BRAND_PNG).ok()?.into_rgba8();
    if img.width() != ICON_SIZE || img.height() != ICON_SIZE {
        return None;
    }
    Some(img.into_raw())
}

fn overlay_status_badge(buf: &mut [u8], status: Status) {
    // Bottom-right circular badge, ~10px diameter with a 1px dark
    // ring so it stays visible on light AND dark themes (KDE light,
    // GNOME dark, etc). Centre at (24, 24) for a 32×32 canvas.
    let rgb = status.rgb();
    let cx: f32 = 23.5;
    let cy: f32 = 23.5;
    let outer: f32 = 5.5; // outer radius incl. ring
    let inner: f32 = 4.5; // inner radius (filled body)
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = f32::from(u16::try_from(x).unwrap_or(0)) - cx;
            let dy = f32::from(u16::try_from(y).unwrap_or(0)) - cy;
            let dist = dx.hypot(dy);
            let i = ((y * ICON_SIZE + x) * 4) as usize;
            if dist <= inner {
                buf[i] = rgb.0;
                buf[i + 1] = rgb.1;
                buf[i + 2] = rgb.2;
                buf[i + 3] = 255;
            } else if dist <= outer {
                // Dark ring for separation from the brand mark
                // behind it.
                buf[i] = 0;
                buf[i + 1] = 0;
                buf[i + 2] = 0;
                buf[i + 3] = 255;
            }
        }
    }
}
