//! HTTP request builder for the Cardigann indexer engine.
//!
//! Builds requests from a Cardigann YAML definition + search query,
//! handles login flows (post, form, cookie), and executes HTTP requests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::cookie::Jar;
use reqwest::header::{self, HeaderMap, HeaderValue};

use super::definition::{
    CardigannDefinition, ErrorBlock, LoginBlock, LoginMethod, PageTestBlock, SearchBlock,
    SearchPathBlock, SelectorBlock,
};
use super::filters::apply_filter;
use super::parser::extract_field_from_document;
use super::template::{SearchQuery, TemplateContext, render};

/// User-Agent string mimicking a modern browser.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// A fully-built HTTP request ready for execution.
#[derive(Debug, Clone)]
pub struct RequestSpec {
    pub method: String,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Option<String>,
    /// The response type hint from the search path (html, json, xml).
    pub response_type: Option<String>,
}

/// HTTP client with cookie jar for a single indexer session.
///
/// Maintains login state and cookies across requests to the same indexer.
/// Optionally integrates with the Cloudflare solver for challenge bypass.
#[derive(Debug)]
pub struct IndexerClient {
    http: reqwest::Client,
    cookie_jar: Arc<Jar>,
    logged_in: AtomicBool,
    cf_solver: Option<Arc<super::cloudflare::CloudflareSolver>>,
}

impl Default for IndexerClient {
    fn default() -> Self {
        Self::new(None)
    }
}

impl IndexerClient {
    /// Create a new client with a shared cookie jar and browser-like defaults.
    pub fn new(cf_solver: Option<Arc<super::cloudflare::CloudflareSolver>>) -> Self {
        let cookie_jar = Arc::new(Jar::default());

        let http = reqwest::Client::builder()
            .cookie_provider(cookie_jar.clone())
            .user_agent(BROWSER_USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            cookie_jar,
            logged_in: AtomicBool::new(false),
            cf_solver,
        }
    }

    /// Ensure we are authenticated with the indexer.
    ///
    /// For public trackers (no login block), this is a no-op.
    /// For private trackers, performs the login flow if not already logged in.
    pub async fn ensure_login(
        &self,
        definition: &CardigannDefinition,
        config: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        if self.logged_in.load(Ordering::Relaxed) {
            return Ok(());
        }

        let Some(ref login) = definition.login else {
            // Public tracker — no login needed.
            return Ok(());
        };

        let base_url = resolve_base_url(definition, config);
        let context = TemplateContext {
            config: config.clone(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        match login.method.as_ref().unwrap_or(&LoginMethod::Post) {
            LoginMethod::Post => self.login_post(login, &base_url, &context).await?,
            LoginMethod::Form => self.login_form(login, &base_url, &context).await?,
            LoginMethod::Cookie => self.login_cookie(login, &base_url, config),
            LoginMethod::Get | LoginMethod::OneUrl => {
                self.login_get(login, &base_url, &context).await?;
            }
        }

        // Verify login if a test block is provided.
        if let Some(ref test) = login.test {
            self.verify_login(test, &base_url).await?;
        }

        self.logged_in.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Execute an HTTP request and return the response body.
    pub async fn execute(&self, spec: &RequestSpec) -> anyhow::Result<String> {
        let start = std::time::Instant::now();
        tracing::debug!(method = %spec.method, url = %spec.url, "cardigann request");

        let request = match spec.method.to_uppercase().as_str() {
            "POST" => {
                let mut req = self.http.post(&spec.url);
                if let Some(ref body) = spec.body {
                    req = req
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(body.clone());
                }
                req
            }
            _ => self.http.get(&spec.url),
        };

        let request = request.headers(spec.headers.clone());

        let response = request.send().await.map_err(|e| {
            tracing::warn!(url = %spec.url, error = %e, "cardigann request send failed");
            anyhow::anyhow!("HTTP request failed for {}: {e}", spec.url)
        })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            tracing::warn!(url = %spec.url, status = status.as_u16(), error = %e, "cardigann body read failed");
            anyhow::anyhow!("failed to read response body: {e}")
        })?;
        tracing::debug!(
            url = %spec.url,
            status = status.as_u16(),
            body_bytes = body.len(),
            duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            "cardigann response",
        );

        // Detect Cloudflare challenge and solve if we have a solver
        if super::cloudflare::CloudflareSolver::is_challenge(status.as_u16(), &body) {
            if let Some(ref solver) = self.cf_solver {
                tracing::info!(url = %spec.url, "Cloudflare challenge detected, solving...");
                let clearance = solver.solve(&spec.url).await?;

                // Add cf_clearance cookies to the jar
                for (name, value) in &clearance.cookies {
                    let cookie_url = spec
                        .url
                        .parse::<reqwest::Url>()
                        .map_err(|e| anyhow::anyhow!("parse url: {e}"))?;
                    self.cookie_jar
                        .add_cookie_str(&format!("{name}={value}"), &cookie_url);
                }

                // Retry the original request with the clearance cookies
                let retry_request = match spec.method.to_uppercase().as_str() {
                    "POST" => {
                        let mut req = self.http.post(&spec.url);
                        if let Some(ref rb) = spec.body {
                            req = req
                                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                                .body(rb.clone());
                        }
                        req
                    }
                    _ => self.http.get(&spec.url),
                };

                let retry_response = retry_request
                    .headers(spec.headers.clone())
                    .header(header::USER_AGENT, &clearance.user_agent)
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("retry after CF solve failed: {e}"))?;

                let retry_status = retry_response.status();
                if !retry_status.is_success() && !retry_status.is_redirection() {
                    anyhow::bail!(
                        "HTTP {} from {} (after CF solve)",
                        retry_status.as_u16(),
                        spec.url
                    );
                }

                return retry_response
                    .text()
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to read retry response body: {e}"));
            }

            anyhow::bail!(
                "Cloudflare challenge detected on {} — Chrome not available for solving",
                spec.url
            );
        }

        if !status.is_success() && !status.is_redirection() {
            anyhow::bail!("HTTP {} from {}", status.as_u16(), spec.url);
        }

        Ok(body)
    }

    // ── Login strategies ────────────────────────────────────────────

    /// POST credentials directly to the login path.
    async fn login_post(
        &self,
        login: &LoginBlock,
        base_url: &str,
        context: &TemplateContext,
    ) -> anyhow::Result<()> {
        let path = login.path.as_deref().unwrap_or("/login");
        let url = build_absolute_url(base_url, path);
        let form_data = render_inputs(&login.inputs, context);

        let body = form_encode(&form_data);

        let response = self
            .http
            .post(&url)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("login POST to {url} failed: {e}"))?;

        if response.status().is_client_error() || response.status().is_server_error() {
            anyhow::bail!(
                "login POST returned HTTP {} from {url}",
                response.status().as_u16(),
            );
        }

        Ok(())
    }

    /// GET the login page, find the form, fill inputs, then POST.
    async fn login_form(
        &self,
        login: &LoginBlock,
        base_url: &str,
        context: &TemplateContext,
    ) -> anyhow::Result<()> {
        let path = login.path.as_deref().unwrap_or("/login");
        let page_url = build_absolute_url(base_url, path);

        // Fetch the login page to get hidden form fields and the form action.
        let page_body = self
            .http
            .get(&page_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("failed to GET login page {page_url}: {e}"))?
            .text()
            .await?;

        // Extract action URL and hidden fields from DOM synchronously (before
        // the next `.await`) so that the non-Send `scraper::ElementRef` is
        // dropped before crossing an await boundary.
        let (action, body) = {
            let document = scraper::Html::parse_document(&page_body);

            let form_selector_str = login.form.as_deref().unwrap_or("form");
            let form_selector = scraper::Selector::parse(form_selector_str).map_err(|e| {
                anyhow::anyhow!("invalid form selector '{form_selector_str}': {e:?}")
            })?;

            let form_el = document.select(&form_selector).next().ok_or_else(|| {
                anyhow::anyhow!("no form found on login page with selector '{form_selector_str}'")
            })?;

            let mut submit_url = login
                .submitpath
                .as_deref()
                .map(|p| build_absolute_url(base_url, p))
                .or_else(|| {
                    form_el
                        .value()
                        .attr("action")
                        .map(|a| build_absolute_url(base_url, a))
                })
                .unwrap_or_else(|| page_url.clone());

            let mut form_data: HashMap<String, String> = HashMap::new();
            let input_selector =
                scraper::Selector::parse("input[type=hidden]").expect("valid static selector");
            for input in form_el.select(&input_selector) {
                if let (Some(name), Some(value)) =
                    (input.value().attr("name"), input.value().attr("value"))
                {
                    form_data.insert(name.to_string(), value.to_string());
                }
            }

            let rendered = render_inputs(&login.inputs, context);
            form_data.extend(rendered);

            // selectorinputs → additional form fields extracted from the
            // landing page via CSS selectors (CSRF tokens not in the form,
            // computed values, etc.). Required unless marked `optional: true`.
            extract_selector_inputs_into(
                &mut form_data,
                &document,
                &login.selectorinputs,
                context,
                "selectorinput",
            )?;

            // getselectorinputs → values that go on the submit URL query string
            // rather than the body.
            let mut query_pairs: HashMap<String, String> = HashMap::new();
            extract_selector_inputs_into(
                &mut query_pairs,
                &document,
                &login.getselectorinputs,
                context,
                "getselectorinput",
            )?;
            if !query_pairs.is_empty() {
                let qs = form_encode(&query_pairs);
                let separator = if submit_url.contains('?') { '&' } else { '?' };
                submit_url.push(separator);
                submit_url.push_str(&qs);
            }

            (submit_url, form_encode(&form_data))
        };

        let response = self
            .http
            .post(&action)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("login form POST to {action} failed: {e}"))?;

        let status = response.status();
        let response_body = response.text().await.unwrap_or_default();

        if status.is_client_error() || status.is_server_error() {
            anyhow::bail!(
                "login form POST returned HTTP {} from {action}",
                status.as_u16(),
            );
        }

        // Definition-level error: block — pattern-match the response for
        // known login failure markers.
        check_login_errors(&login.error, &response_body, context)?;

        Ok(())
    }

    /// Set cookies directly from user-supplied config values.
    fn login_cookie(&self, login: &LoginBlock, base_url: &str, config: &HashMap<String, String>) {
        let context = TemplateContext {
            config: config.clone(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };
        let cookie_values = render_inputs(&login.inputs, &context);

        // Build a cookie header value from individual key=value pairs.
        let cookie_str: String = cookie_values
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");

        if let Ok(url) = base_url.parse::<reqwest::Url>() {
            self.cookie_jar.add_cookie_str(&cookie_str, &url);
        }

        // Also support raw cookie strings from the `cookies` field.
        for cookie in &login.cookies {
            let rendered = render(cookie, &context);
            if let Ok(url) = base_url.parse::<reqwest::Url>() {
                self.cookie_jar.add_cookie_str(&rendered, &url);
            }
        }

        // Also support a raw "cookie" config field.
        if let Some(raw_cookie) = config.get("cookie")
            && let Ok(url) = base_url.parse::<reqwest::Url>()
        {
            self.cookie_jar.add_cookie_str(raw_cookie, &url);
        }
    }

    /// GET the login path to trigger cookie-based auth (for `get` / `oneurl` methods).
    async fn login_get(
        &self,
        login: &LoginBlock,
        base_url: &str,
        context: &TemplateContext,
    ) -> anyhow::Result<()> {
        let path = login.path.as_deref().unwrap_or("/login");
        let mut url = build_absolute_url(base_url, path);

        // Add inputs as query parameters.
        let rendered = render_inputs(&login.inputs, context);
        if !rendered.is_empty() {
            let separator = if url.contains('?') { "&" } else { "?" };
            let qs = rendered
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}{separator}{qs}");
        }

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("login GET to {url} failed: {e}"))?;

        if response.status().is_client_error() || response.status().is_server_error() {
            anyhow::bail!(
                "login GET returned HTTP {} from {url}",
                response.status().as_u16(),
            );
        }

        Ok(())
    }

    /// Verify login succeeded by fetching the test path and checking the CSS selector.
    async fn verify_login(&self, test: &PageTestBlock, base_url: &str) -> anyhow::Result<()> {
        let path = test.path.as_deref().unwrap_or("/");
        let url = build_absolute_url(base_url, path);

        let body = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("login verification GET {url} failed: {e}"))?
            .text()
            .await?;

        // If there's a CSS selector to check, verify it matches something.
        if let Some(ref selector_str) = test.selector {
            let document = scraper::Html::parse_document(&body);
            let selector = scraper::Selector::parse(selector_str)
                .map_err(|e| anyhow::anyhow!("invalid test selector '{selector_str}': {e:?}"))?;

            if document.select(&selector).next().is_none() {
                anyhow::bail!(
                    "login verification failed: selector '{selector_str}' not found on {url}"
                );
            }
        }

        Ok(())
    }
}

// ── Request building ────────────────────────────────────────────────

/// Build search requests from a Cardigann definition and template context.
///
/// Returns one `RequestSpec` per search path defined in the definition.
pub fn build_search_requests(
    definition: &CardigannDefinition,
    context: &TemplateContext,
) -> anyhow::Result<Vec<RequestSpec>> {
    let Some(ref search) = definition.search else {
        return Ok(Vec::new());
    };

    let base_url = definition.links.first().cloned().unwrap_or_default();

    // Apply keyword filters to the query string.
    let mut keywords = context.query.keywords.clone();
    if keywords.is_empty() {
        keywords.clone_from(&context.query.q);
    }

    for filter in &search.keywordsfilters {
        match apply_filter(&keywords, &filter.name, &filter.args) {
            Ok(filtered) => keywords = filtered,
            Err(e) => {
                tracing::warn!(filter = %filter.name, error = %e, "keyword filter failed");
            }
        }
    }

    // Build a modified context with the filtered keywords.
    let mut search_context = context.clone();
    search_context
        .result
        .insert("Keywords".to_string(), keywords);

    // If there are explicit search paths, use those. Otherwise fall back
    // to the top-level `path` field on the search block.
    let paths: Vec<&SearchPathBlock> = if search.paths.is_empty() {
        // Build a synthetic path entry from the top-level search fields.
        Vec::new()
    } else {
        search.paths.iter().collect()
    };

    // Handle the case where there are no search paths at all — use top-level path.
    if paths.is_empty() {
        if let Some(ref top_path) = search.path {
            let spec = build_single_request_from_parts(
                top_path,
                None, // method
                &search.inputs,
                &HashMap::new(),
                None, // response_type
                search,
                &base_url,
                &search_context,
            );
            return Ok(vec![spec]);
        }
        return Ok(Vec::new());
    }

    let requests: Vec<RequestSpec> = paths
        .iter()
        .map(|search_path| build_single_request(search_path, search, &base_url, &search_context))
        .collect();

    Ok(requests)
}

/// Build a single `RequestSpec` from a `SearchPathBlock`.
fn build_single_request(
    search_path: &SearchPathBlock,
    search: &SearchBlock,
    base_url: &str,
    context: &TemplateContext,
) -> RequestSpec {
    let path_str = search_path.path.as_deref().unwrap_or("/");

    // Merge inputs: start with search-level, override with path-level.
    let mut merged_inputs = if search_path.inheritinputs {
        search.inputs.clone()
    } else {
        HashMap::new()
    };
    merged_inputs.extend(search_path.inputs.clone());

    // Determine response type from the path's response block.
    let response_type = search_path
        .response
        .as_ref()
        .and_then(|r| r.response_type.clone());

    build_single_request_from_parts(
        path_str,
        search_path.method.as_deref(),
        &merged_inputs,
        &HashMap::new(), // path-level headers not in SearchPathBlock
        response_type,
        search,
        base_url,
        context,
    )
}

/// Build a `RequestSpec` from individual components.
#[expect(clippy::too_many_arguments)]
fn build_single_request_from_parts(
    path_template: &str,
    method_override: Option<&str>,
    inputs: &HashMap<String, String>,
    extra_headers: &HashMap<String, String>,
    response_type: Option<String>,
    search: &SearchBlock,
    base_url: &str,
    context: &TemplateContext,
) -> RequestSpec {
    // Render the path template.
    let rendered_path = render(path_template, context);

    // Build the full URL.
    let mut url = build_absolute_url(base_url, &rendered_path);

    // Render inputs as query parameters / form body.
    let mut query_params: Vec<(String, String)> = Vec::new();
    for (key, value_template) in inputs {
        let rendered_value = render(value_template, context);
        // Include empty values only if allow_empty_inputs is set.
        if !rendered_value.is_empty() || search.allow_empty_inputs {
            query_params.push((key.clone(), rendered_value));
        }
    }

    // Determine HTTP method (default GET).
    let method = method_override.unwrap_or("GET").to_uppercase();

    let body = if method == "POST" {
        // For POST, encode params as form body instead of query string.
        let encoded = query_params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        if encoded.is_empty() {
            None
        } else {
            Some(encoded)
        }
    } else {
        // For GET, append params to URL.
        if !query_params.is_empty() {
            let separator = if url.contains('?') { "&" } else { "?" };
            let qs = query_params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}{separator}{qs}");
        }
        None
    };

    // Build headers from the search block (Vec<String> values are joined).
    let mut headers = HeaderMap::new();
    for (key, values) in &search.headers {
        let joined = values
            .iter()
            .map(|v| render(v, context))
            .collect::<Vec<_>>()
            .join(", ");
        if let (Ok(name), Ok(val)) = (
            key.parse::<header::HeaderName>(),
            HeaderValue::from_str(&joined),
        ) {
            headers.insert(name, val);
        }
    }

    // Apply extra headers (override search-level).
    for (key, value) in extra_headers {
        let rendered = render(value, context);
        if let (Ok(name), Ok(val)) = (
            key.parse::<header::HeaderName>(),
            HeaderValue::from_str(&rendered),
        ) {
            headers.insert(name, val);
        }
    }

    RequestSpec {
        method,
        url,
        headers,
        body,
        response_type,
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Resolve the base URL from config or definition links.
fn resolve_base_url(definition: &CardigannDefinition, config: &HashMap<String, String>) -> String {
    config
        .get("sitelink")
        .cloned()
        .or_else(|| definition.links.first().cloned())
        .unwrap_or_default()
}

/// Build an absolute URL from a base URL and a (possibly relative) path.
fn build_absolute_url(base_url: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }

    let base = base_url.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };

    format!("{base}{path}")
}

/// Render template inputs into concrete key-value pairs.
fn render_inputs(
    inputs: &HashMap<String, String>,
    context: &TemplateContext,
) -> HashMap<String, String> {
    inputs
        .iter()
        .map(|(key, template)| (key.clone(), render(template, context)))
        .collect()
}

/// URL-encode a map of key-value pairs into a form body string.
fn form_encode(params: &HashMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Extract values via a map of `SelectorBlock`s applied to an HTML document
/// and merge them into `target`. Used for login `selectorinputs` (form fields)
/// and `getselectorinputs` (URL query params). Non-optional selectors that
/// fail to match cause the whole login to abort with a useful error.
fn extract_selector_inputs_into(
    target: &mut HashMap<String, String>,
    document: &scraper::Html,
    selectors: &HashMap<String, SelectorBlock>,
    context: &TemplateContext,
    label: &str,
) -> anyhow::Result<()> {
    for (key, block) in selectors {
        match extract_field_from_document(document, block, context) {
            Some(value) => {
                target.insert(key.clone(), value);
            }
            None if block.optional => {
                // Definition marks this selector optional — skip silently.
            }
            None => {
                anyhow::bail!(
                    "{label} '{key}' selector '{}' produced no value",
                    block.selector.as_deref().unwrap_or("<text>"),
                );
            }
        }
    }
    Ok(())
}

/// Check the post-login response body against any `error:` blocks in the
/// definition. Returns `Err` if any match — the indexer uses these to
/// surface "Invalid credentials" / "Account disabled" etc. that otherwise
/// look like a successful 200 response.
fn check_login_errors(
    errors: &[ErrorBlock],
    response_body: &str,
    context: &TemplateContext,
) -> anyhow::Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    let document = scraper::Html::parse_document(response_body);
    for err in errors {
        let Some(selector_str) = err.selector.as_deref() else {
            continue;
        };
        let Ok(selector) = scraper::Selector::parse(selector_str) else {
            continue;
        };
        let Some(el) = document.select(&selector).next() else {
            continue;
        };

        // Build the message: prefer the `message:` sub-block if present,
        // else fall back to the matched element's text.
        let message = if let Some(ref msg_block) = err.message {
            extract_field_from_document(&document, msg_block, context)
                .unwrap_or_else(|| "login failed".to_owned())
        } else {
            let text = el.text().collect::<Vec<_>>().concat();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                "login failed".to_owned()
            } else {
                trimmed.to_owned()
            }
        };

        anyhow::bail!("indexer rejected login: {message}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::definition::{FilterBlock, ResponseBlock};
    use super::*;

    fn make_definition() -> CardigannDefinition {
        CardigannDefinition {
            id: "test".into(),
            name: "Test Tracker".into(),
            links: vec!["https://example.com".into()],
            search: Some(SearchBlock {
                paths: vec![SearchPathBlock {
                    path: Some("/search/{{ .Result.Keywords }}/1/".into()),
                    inheritinputs: true,
                    ..Default::default()
                }],
                inputs: HashMap::from([("q".into(), "{{ .Query.Q }}".into())]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn build_search_requests_basic() {
        let def = make_definition();
        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery {
                q: "test query".into(),
                keywords: "test query".into(),
                ..Default::default()
            },
            result: HashMap::new(),
        };

        let requests = build_search_requests(&def, &ctx).unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].url.contains("/search/test query/1/"));
        assert_eq!(requests[0].method, "GET");
    }

    #[test]
    fn build_absolute_url_relative() {
        assert_eq!(
            build_absolute_url("https://example.com", "/search"),
            "https://example.com/search"
        );
    }

    #[test]
    fn build_absolute_url_already_absolute() {
        assert_eq!(
            build_absolute_url("https://example.com", "https://other.com/path"),
            "https://other.com/path"
        );
    }

    #[test]
    fn build_absolute_url_no_leading_slash() {
        assert_eq!(
            build_absolute_url("https://example.com", "search/foo"),
            "https://example.com/search/foo"
        );
    }

    #[test]
    fn build_absolute_url_trailing_slash_on_base() {
        assert_eq!(
            build_absolute_url("https://example.com/", "/search"),
            "https://example.com/search"
        );
    }

    #[test]
    fn build_search_with_keyword_filters() {
        let def = CardigannDefinition {
            id: "test".into(),
            name: "Test".into(),
            links: vec!["https://example.com".into()],
            search: Some(SearchBlock {
                paths: vec![SearchPathBlock {
                    path: Some("/search/{{ .Result.Keywords }}/".into()),
                    inheritinputs: true,
                    ..Default::default()
                }],
                keywordsfilters: vec![FilterBlock {
                    name: "re_replace".into(),
                    args: vec!["\\s+".into(), "+".into()],
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery {
                q: "hello world".into(),
                keywords: "hello world".into(),
                ..Default::default()
            },
            result: HashMap::new(),
        };

        let requests = build_search_requests(&def, &ctx).unwrap();
        assert_eq!(requests.len(), 1);
        // After re_replace \s+ with +, "hello world" becomes "hello+world"
        assert!(requests[0].url.contains("/search/hello+world/"));
    }

    #[test]
    fn post_method_encodes_body() {
        let def = CardigannDefinition {
            id: "test".into(),
            name: "Test".into(),
            links: vec!["https://example.com".into()],
            search: Some(SearchBlock {
                paths: vec![SearchPathBlock {
                    path: Some("/api/search".into()),
                    method: Some("POST".into()),
                    inheritinputs: true,
                    ..Default::default()
                }],
                inputs: HashMap::from([("q".into(), "{{ .Query.Q }}".into())]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery {
                q: "test".into(),
                keywords: "test".into(),
                ..Default::default()
            },
            result: HashMap::new(),
        };

        let requests = build_search_requests(&def, &ctx).unwrap();
        assert_eq!(requests[0].method, "POST");
        assert!(requests[0].body.is_some());
        assert!(!requests[0].url.contains('?'));
    }

    #[test]
    fn response_type_from_path() {
        let def = CardigannDefinition {
            id: "test".into(),
            name: "Test".into(),
            links: vec!["https://example.com".into()],
            search: Some(SearchBlock {
                paths: vec![SearchPathBlock {
                    path: Some("/api/search".into()),
                    response: Some(ResponseBlock {
                        response_type: Some("json".into()),
                        no_results_message: None,
                    }),
                    inheritinputs: true,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery::default(),
            result: HashMap::new(),
        };

        let requests = build_search_requests(&def, &ctx).unwrap();
        assert_eq!(requests[0].response_type.as_deref(), Some("json"));
    }

    #[test]
    fn inherit_inputs_false_excludes_search_inputs() {
        let def = CardigannDefinition {
            id: "test".into(),
            name: "Test".into(),
            links: vec!["https://example.com".into()],
            search: Some(SearchBlock {
                paths: vec![SearchPathBlock {
                    path: Some("/search".into()),
                    inheritinputs: false,
                    inputs: HashMap::from([("custom".into(), "value".into())]),
                    ..Default::default()
                }],
                inputs: HashMap::from([("q".into(), "{{ .Query.Q }}".into())]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let ctx = TemplateContext {
            config: HashMap::new(),
            query: SearchQuery {
                q: "test".into(),
                keywords: "test".into(),
                ..Default::default()
            },
            result: HashMap::new(),
        };

        let requests = build_search_requests(&def, &ctx).unwrap();
        // Should NOT contain q= from search-level inputs
        assert!(!requests[0].url.contains("q="));
        // Should contain custom= from path-level inputs
        assert!(requests[0].url.contains("custom=value"));
    }
}
