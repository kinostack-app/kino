//! Two-tier Cloudflare bypass:
//!
//! **Tier 1: TLS fingerprint impersonation** (`wreq`)
//! Handles ~96% of CF-protected sites without any browser.
//!
//! **Tier 2: Camoufox via embedded Node solver** (`playwright-rs` downloads
//! the Playwright Node driver; we spawn it with our bundled `solver.js`).
//! For sites that force JS challenges (1337x etc). Implements the same
//! recipe Byparr uses — Camoufox + uBlock Origin + an init-script addon
//! that patches `Element.prototype.attachShadow` so we can walk closed
//! shadow roots + shadow-DOM walk to find & click the Turnstile checkbox.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Cached Cloudflare clearance — cookies from a successful bypass.
#[derive(Debug, Clone)]
pub struct CfClearance {
    pub cookies: Vec<(String, String)>,
    pub user_agent: String,
    pub obtained_at: Instant,
}

impl CfClearance {
    pub fn is_valid(&self) -> bool {
        self.obtained_at.elapsed() < Duration::from_secs(25 * 60)
    }
}

/// Two-tier Cloudflare bypass manager.
#[derive(Debug, Clone)]
pub struct CloudflareSolver {
    cache: Arc<RwLock<HashMap<String, CfClearance>>>,
    data_path: PathBuf,
}

impl CloudflareSolver {
    pub fn new(data_path: PathBuf) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            data_path,
        }
    }

    pub async fn get_cached(&self, domain: &str) -> Option<CfClearance> {
        let cache = self.cache.read().await;
        cache.get(domain).filter(|c| c.is_valid()).cloned()
    }

    pub fn is_challenge(status: u16, body: &str) -> bool {
        status == 403
            && (body.contains("Just a moment...")
                || body.contains("challenge-platform")
                || body.contains("_cf_chl_opt"))
    }

    /// Solve via Tier 1 (wreq), fall back to Tier 2 (Camoufox).
    pub async fn solve(&self, url: &str) -> anyhow::Result<CfClearance> {
        let domain = extract_domain(url);

        if let Some(cached) = self.get_cached(&domain).await {
            return Ok(cached);
        }

        // Tier 1: TLS fingerprint impersonation
        tracing::info!(url, "attempting CF bypass via TLS fingerprinting");
        match self.solve_with_wreq(url).await {
            Ok(clearance) => {
                self.cache.write().await.insert(domain, clearance.clone());
                return Ok(clearance);
            }
            Err(e) => {
                tracing::info!(url, error = %e, "TLS fingerprinting insufficient, trying Camoufox");
            }
        }

        // Tier 2: Camoufox remote server
        let clearance = self.solve_with_camoufox(url).await?;
        self.cache.write().await.insert(domain, clearance.clone());
        Ok(clearance)
    }

    /// Tier 1: wreq with Chrome TLS fingerprint.
    async fn solve_with_wreq(&self, url: &str) -> anyhow::Result<CfClearance> {
        let client = wreq::Client::builder()
            .emulation(wreq_util::Emulation::Chrome131)
            .cookie_store(true)
            .build()
            .map_err(|e| anyhow::anyhow!("wreq client: {e}"))?;

        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("wreq: {e}"))?;

        let status = resp.status().as_u16();
        let mut cookies = Vec::new();
        for cookie in resp.cookies() {
            cookies.push((cookie.name().to_string(), cookie.value().to_string()));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("wreq body: {e}"))?;

        if Self::is_challenge(status, &body) {
            anyhow::bail!("TLS fingerprint bypass failed — JS challenge required");
        }

        Ok(CfClearance {
            cookies,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".into(),
            obtained_at: Instant::now(),
        })
    }

    /// Tier 2: Launch Camoufox via playwright-rs, solve JS challenge.
    async fn solve_with_camoufox(&self, url: &str) -> anyhow::Result<CfClearance> {
        let browser_path = self.ensure_camoufox().await?;
        tracing::info!(url, path = %browser_path.display(), "solving via Camoufox");
        solve_with_camoufox_browser(&self.data_path, &browser_path, url).await
    }

    async fn ensure_camoufox(&self) -> anyhow::Result<PathBuf> {
        let camoufox_dir = self.data_path.join("camoufox");
        if let Some(path) = find_browser_in_dir(&camoufox_dir) {
            return Ok(path);
        }
        tracing::info!("Camoufox not found — downloading (~680MB, one time)...");
        download_camoufox(&camoufox_dir).await
    }

    pub async fn cleanup_expired(&self) {
        let mut cache = self.cache.write().await;
        cache.retain(|_, v| v.is_valid());
    }
}

// ── Camoufox via embedded Node solver ───────────────────────────────

const SOLVER_JS: &str = include_str!("cf_assets/solver.js");
const INIT_ADDON_MANIFEST: &str = include_str!("cf_assets/init_manifest.json");
const INIT_ADDON_INJECT_JS: &str = include_str!("cf_assets/init_inject.js");
const INIT_ADDON_PATCH_JS: &str = include_str!("cf_assets/scripts/patch.js");
const INIT_ADDON_REGISTRY: &str = include_str!("cf_assets/scripts/registry.json");

/// Spawn the bundled Node + Playwright driver with our embedded solver,
/// feed it a JSON config via stdin, parse the JSON result from stdout.
async fn solve_with_camoufox_browser(
    data_path: &Path,
    browser_path: &Path,
    url: &str,
) -> anyhow::Result<CfClearance> {
    let assets = prepare_solver_assets(data_path, browser_path).await?;
    let (node_path, cli_js) = ensure_playwright_node().await?;

    let mut camou_config = if let Ok(path) = std::env::var("CAMOU_CONFIG_OVERRIDE") {
        let raw = tokio::fs::read_to_string(&path).await?;
        tracing::warn!(path, "using CAMOU_CONFIG_OVERRIDE (test diagnostic)");
        serde_json::from_str(&raw)?
    } else {
        generate_camou_config()
    };
    // Always force our own addons path (even with override) so the init
    // addon we bundle is still injected.
    camou_config["addons"] = serde_json::json!([assets.addon_dir.to_string_lossy()]);
    camou_config["forceScopeAccess"] = serde_json::json!(true);
    camou_config["humanize"] = serde_json::json!(true);
    camou_config["allowMainWorld"] = serde_json::json!(true);

    let solver_input = serde_json::json!({
        "browserPath": browser_path.to_string_lossy(),
        "url": url,
        "camouConfig": camou_config,
        "fontconfigFile": assets.fontconfig_file.as_ref().map(|p| p.to_string_lossy()),
        "timeoutMs": 60000,
    });

    tracing::info!(url, "spawning CF solver (node + camoufox)");
    let mut child = tokio::process::Command::new(&node_path)
        .arg(&assets.solver_js)
        .arg(&cli_js)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn node: {e}"))?;

    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("no stdin on child"))?;
        stdin.write_all(solver_input.to_string().as_bytes()).await?;
        stdin.shutdown().await.ok();
    }

    // Stream stderr in real time so we see progress and can diagnose hangs.
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stderr on child"))?;
    let stderr_task = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(target: "cf_solver", "{line}");
        }
    });

    let output = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("CF solver did not exit within 120s"))?
        .map_err(|e| anyhow::anyhow!("CF solver wait: {e}"))?;
    stderr_task.abort();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout.lines().last().unwrap_or("").trim();
    if last_line.is_empty() {
        anyhow::bail!(
            "CF solver produced no output (exit: {:?}, stderr tail: {})",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }

    let result: serde_json::Value = serde_json::from_str(last_line)
        .map_err(|e| anyhow::anyhow!("parse solver output: {e} (line: {last_line})"))?;

    if !result["ok"].as_bool().unwrap_or(false) {
        let err = result["error"].as_str().unwrap_or("no cf_clearance");
        let title = result["title"].as_str().unwrap_or("");
        anyhow::bail!("CF solver failed: {err} (final title: {title:?})");
    }

    let cookies: Vec<(String, String)> = result["cookies"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    Some((
                        c["name"].as_str()?.to_string(),
                        c["value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let user_agent = result["userAgent"].as_str().unwrap_or("").to_string();

    Ok(CfClearance {
        cookies,
        user_agent,
        obtained_at: Instant::now(),
    })
}

struct SolverAssets {
    solver_js: PathBuf,
    addon_dir: PathBuf,
    fontconfig_file: Option<PathBuf>,
}

/// Write embedded solver assets + init-script addon + patched fontconfig
/// to `data_path` so they can be referenced by Camoufox / Node at runtime.
async fn prepare_solver_assets(
    data_path: &Path,
    browser_path: &Path,
) -> anyhow::Result<SolverAssets> {
    tokio::fs::create_dir_all(data_path).await?;

    let solver_js = data_path.join("cf-solver.js");
    tokio::fs::write(&solver_js, SOLVER_JS).await?;

    let addon_dir = data_path.join("cf-init-addon");
    let scripts_dir = addon_dir.join("scripts");
    tokio::fs::create_dir_all(&scripts_dir).await?;
    tokio::fs::write(addon_dir.join("manifest.json"), INIT_ADDON_MANIFEST).await?;
    tokio::fs::write(addon_dir.join("inject.js"), INIT_ADDON_INJECT_JS).await?;
    tokio::fs::write(scripts_dir.join("patch.js"), INIT_ADDON_PATCH_JS).await?;
    tokio::fs::write(scripts_dir.join("registry.json"), INIT_ADDON_REGISTRY).await?;

    // Match the UA (Windows) by using Camoufox's `fontconfigs/windows`
    // fonts.conf, patched so `prefix="cwd"` becomes an absolute path.
    // We point FONTCONFIG_PATH at the dir (Byparr style), not the file.
    let fontconfig_file = if let Some(browser_parent) = browser_path.parent() {
        let src = browser_parent.join("fontconfigs/windows/fonts.conf");
        let fonts_dir = browser_parent.join("fonts");
        if src.exists() && fonts_dir.exists() {
            let fc_dir = data_path.join("cf-fontconfig");
            tokio::fs::create_dir_all(&fc_dir).await?;
            let dst = fc_dir.join("fonts.conf");
            let content = tokio::fs::read_to_string(&src).await?;
            let patched = content.replace(
                "<dir prefix=\"cwd\">fonts</dir>",
                &format!("<dir>{}</dir>", fonts_dir.display()),
            );
            tokio::fs::write(&dst, patched).await?;
            Some(dst)
        } else {
            None
        }
    } else {
        None
    };

    Ok(SolverAssets {
        solver_js,
        addon_dir,
        fontconfig_file,
    })
}

/// Locate (or download on first use) the Playwright Node driver that
/// `playwright-rs` manages. Returns (node binary, cli.js path).
async fn ensure_playwright_node() -> anyhow::Result<(PathBuf, PathBuf)> {
    if let Some(paths) = find_playwright_installation() {
        return Ok(paths);
    }
    tracing::info!("downloading Playwright Node driver (first use)");
    let pw_handle = tokio::time::timeout(
        Duration::from_secs(120),
        playwright_rs::Playwright::launch(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("playwright driver download timed out"))?
    .map_err(|e| anyhow::anyhow!("playwright driver download: {e}"))?;
    drop(pw_handle);
    find_playwright_installation()
        .ok_or_else(|| anyhow::anyhow!("playwright driver not found after install"))
}

fn find_playwright_installation() -> Option<(PathBuf, PathBuf)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let cache_dir = PathBuf::from(home).join(".cache/playwright-rust/drivers");
    if !cache_dir.exists() {
        return None;
    }
    let entry = std::fs::read_dir(&cache_dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("playwright-"))
        .max_by_key(|e| e.file_name().to_string_lossy().to_string())?;
    let base = entry.path();
    let node = base.join("node");
    let cli = base.join("package").join("cli.js");
    (node.exists() && cli.exists()).then_some((node, cli))
}

// ── Camoufox download + discovery ───────────────────────────────────

fn find_browser_in_dir(dir: &Path) -> Option<PathBuf> {
    if !dir.exists() {
        return None;
    }
    for entry in walkdir(dir) {
        if let Some(name) = entry.file_name().and_then(|n| n.to_str())
            && (name == "camoufox" || name == "firefox" || name == "firefox-bin")
            && entry.is_file()
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&entry)
                    && meta.permissions().mode() & 0o111 != 0
                {
                    return Some(entry);
                }
            }
            #[cfg(not(unix))]
            return Some(entry);
        }
    }
    None
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                results.extend(walkdir(&path));
            } else if path.is_file() {
                results.push(path);
            }
        }
    }
    results
}

async fn download_camoufox(dest_dir: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;

    let asset_name = if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "lin.x86_64"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "lin.arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "mac.arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "mac.x86_64"
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        "win.x86_64"
    } else {
        anyhow::bail!("unsupported platform for Camoufox");
    };

    let client = reqwest::Client::builder().user_agent("kino/0.1").build()?;
    let release: serde_json::Value = client
        .get("https://api.github.com/repos/daijro/camoufox/releases/latest")
        .send()
        .await?
        .json()
        .await?;

    let download_url = release["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no assets"))?
        .iter()
        .find(|a| {
            a["name"].as_str().is_some_and(|n| {
                n.contains(asset_name)
                    && Path::new(n)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
            })
        })
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| anyhow::anyhow!("no matching asset for {asset_name}"))?
        .to_string();

    tracing::info!(url = %download_url, "downloading Camoufox");

    let zip_path = dest_dir.join("camoufox.zip");
    let mut response = client.get(&download_url).send().await?;
    let mut file = tokio::fs::File::create(&zip_path).await?;
    while let Some(chunk) = response.chunk().await? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    drop(file);

    tracing::info!("extracting Camoufox...");
    let output = tokio::process::Command::new("unzip")
        .arg("-o")
        .arg("-q")
        .arg(&zip_path)
        .arg("-d")
        .arg(dest_dir)
        .output()
        .await?;

    if !output.status.success() {
        tracing::warn!(
            "unzip returned non-zero (may be Unicode warnings): {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(200)
                .collect::<String>()
        );
    }

    if let Err(e) = tokio::fs::remove_file(&zip_path).await {
        tracing::debug!(path = %zip_path.display(), error = %e, "failed to clean up camoufox zip");
    }
    find_browser_in_dir(dest_dir)
        .ok_or_else(|| anyhow::anyhow!("Camoufox binary not found after extraction"))
}

/// Windows Firefox fingerprint — matches the working config Byparr produces.
/// Linux fingerprints on a containerised datacenter IP get flagged by
/// Cloudflare; Windows reads as the modal client and passes.
fn generate_camou_config() -> serde_json::Value {
    const WIN_FONTS: &str = include_str!("cf_assets/fonts_win.json");
    let fonts: serde_json::Value =
        serde_json::from_str(WIN_FONTS).unwrap_or_else(|_| serde_json::json!([]));

    serde_json::json!({
        "navigator.userAgent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:135.0) Gecko/20100101 Firefox/135.0",
        "navigator.appCodeName": "Mozilla",
        "navigator.appName": "Netscape",
        "navigator.appVersion": "5.0 (Windows)",
        "navigator.oscpu": "Windows NT 10.0; Win64; x64",
        "navigator.platform": "Win32",
        "navigator.product": "Gecko",
        "navigator.hardwareConcurrency": 8,
        "navigator.doNotTrack": "1",
        "navigator.globalPrivacyControl": true,
        "headers.Accept-Encoding": "gzip, deflate, br, zstd",
        "screen.width": 1920,
        "screen.height": 1080,
        "screen.availWidth": 1920,
        "screen.availHeight": 1040,
        "screen.colorDepth": 24,
        "screen.pixelDepth": 24,
        "window.outerWidth": 1920,
        "window.outerHeight": 1040,
        "window.innerWidth": 1920,
        "window.innerHeight": 953,
        "window.devicePixelRatio": 1.0,
        "window.history.length": 2,
        "fonts": fonts,
        // Locale + timezone consistency — CF flags UTC as datacenter tell.
        "timezone": "America/New_York",
        "locale:region": "US",
        "locale:language": "en",
        "locale:script": "Latn",
        // Plausible WebGL — Turnstile checks vendor/renderer for real-GPU
        // shape. Values mirror Byparr's working config.
        "webGl:vendor": "Google Inc. (Intel)",
        "webGl:renderer": "ANGLE (Intel, Intel(R) UHD Graphics 620 Direct3D11 vs_5_0 ps_5_0, D3D11)",
    })
}

fn extract_domain(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_lowercase()
}

/// Make an HTTP request using wreq with browser TLS fingerprint.
pub async fn wreq_get(url: &str) -> anyhow::Result<(u16, String)> {
    let client = wreq::Client::builder()
        .emulation(wreq_util::Emulation::Chrome131)
        .cookie_store(true)
        .build()
        .map_err(|e| anyhow::anyhow!("wreq client: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("wreq: {e}"))?;

    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("wreq body: {e}"))?;

    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_wreq_against_1337x() {
        match wreq_get("https://1337x.to/search/test/1/").await {
            Ok((status, body)) => {
                eprintln!("1337x: status={status} body={}bytes", body.len());
                if status == 403 {
                    eprintln!("1337x: CF JS challenge (expected — needs Camoufox)");
                } else {
                    eprintln!("1337x: bypassed via TLS fingerprinting!");
                }
            }
            Err(e) => eprintln!("1337x error: {e}"),
        }
    }

    #[tokio::test]
    async fn test_wreq_against_nyaa() {
        match wreq_get("https://nyaa.si/?q=test").await {
            Ok((status, body)) => {
                eprintln!("Nyaa: status={status} body={}bytes", body.len());
                assert_eq!(status, 200);
            }
            Err(e) => eprintln!("Nyaa error: {e}"),
        }
    }

    #[tokio::test]
    async fn test_wreq_against_limetorrents() {
        match wreq_get("https://www.limetorrents.lol/search/all/test/").await {
            Ok((status, body)) => {
                eprintln!("LimeTorrents: status={status} body={}bytes", body.len());
            }
            Err(e) => eprintln!("LimeTorrents error: {e}"),
        }
    }

    #[tokio::test]
    #[ignore = "downloads 680MB Camoufox — run with --ignored"]
    async fn test_full_solver_against_1337x() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "kino::indexers::cloudflare=info".into()),
            )
            .with_test_writer()
            .try_init();
        let solver = CloudflareSolver::new(PathBuf::from("/workspace/data"));
        match solver.solve("https://1337x.to/search/test/1/").await {
            Ok(clearance) => {
                eprintln!(
                    "SUCCESS! Got {} cookies, UA: {}...",
                    clearance.cookies.len(),
                    &clearance.user_agent[..clearance.user_agent.len().min(50)]
                );
                for (name, val) in &clearance.cookies {
                    eprintln!("  cookie: {name}={}", &val[..val.len().min(30)]);
                }
                assert!(!clearance.cookies.is_empty(), "should have cookies");
            }
            Err(e) => {
                eprintln!("FAILED: {e}");
            }
        }
    }
}
