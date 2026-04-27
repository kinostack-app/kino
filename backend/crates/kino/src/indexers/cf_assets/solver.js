// Cloudflare challenge solver — runs Camoufox via Playwright-JS and clears
// CF's JS / Turnstile challenges using the Byparr recipe (shadow-DOM walk +
// checkbox click + patched attachShadow init-script).
//
// argv[2] = path to Playwright's package cli.js (for require)
// stdin   = JSON { browserPath, url, camouConfig, fontconfigFile, timeoutMs }
// stdout  = JSON { ok, cookies[], userAgent, title, error? }

const path = require("path");
const log = (m) =>
  process.stderr.write(
    `[cf-solver ${new Date().toISOString().slice(11, 23)}] ${m}\n`,
  );
log(`starting, argv=${JSON.stringify(process.argv)}`);
const PLAYWRIGHT_PKG = path.dirname(process.argv[2]);
log(`requiring playwright from ${PLAYWRIGHT_PKG}`);
const { firefox } = require(PLAYWRIGHT_PKG);
log("playwright loaded");

function withTimeout(p, ms, label) {
  return Promise.race([
    p,
    new Promise((_, rej) =>
      setTimeout(() => rej(new Error(`${label} timeout ${ms}ms`)), ms),
    ),
  ]);
}

function findCfIframe(page) {
  for (const f of page.frames()) {
    if (
      f
        .url()
        .includes("challenges.cloudflare.com/cdn-cgi/challenge-platform")
    ) {
      return f;
    }
  }
  return null;
}

// Walks light + shadow + shadowRootUnl (init-addon exposes closed shadows
// here) and returns the first checkbox element. We do not call .click() from
// inside evaluate() — CF's Turnstile checks event.isTrusted, so clicking
// from JS produces a synthetic (untrusted) event CF rejects. Instead we
// return a handle and let Playwright dispatch a real mouse click.
const WALK_FOR_CHECKBOX = `
  () => {
    function walk(root, found) {
      if (!root) return;
      const els = root.querySelectorAll ? root.querySelectorAll("*") : [];
      for (const e of els) {
        if (e.tagName === "INPUT" && e.type === "checkbox") found.push(e);
        if (e.shadowRoot) walk(e.shadowRoot, found);
        if (e.shadowRootUnl) walk(e.shadowRootUnl, found);
      }
    }
    const found = [];
    walk(document, found);
    return found[0] || null;
  }
`;

async function solve(opts) {
  const env = {
    ...process.env,
    CAMOU_CONFIG_1: JSON.stringify(opts.camouConfig),
  };
  // Fontconfig env var intentionally omitted — Byparr's points at a
  // non-existent path and its fonts still work, so real font substitution
  // isn't load-bearing for CF detection. Camoufox spoofs document.fonts
  // at the C++ layer using CAMOU_CONFIG.fonts.

  log(`launching camoufox (url=${opts.url})`);
  const browser = await withTimeout(
    firefox.launch({
      executablePath: opts.browserPath,
      headless: true,
      env,
      // disable_coop lets Turnstile's cross-origin postMessage render the
      // checkbox iframe. webgl.force-enabled matches Camoufox's launch_options().
      firefoxUserPrefs: {
        "dom.webdriver.enabled": false,
        "marionette.enabled": false,
        "browser.tabs.remote.useCrossOriginOpenerPolicy": false,
        "browser.tabs.remote.useCrossOriginEmbedderPolicy": false,
        "browser.tabs.remote.useCrossOriginPolicy": false,
        "privacy.window.name.update.enabled": false,
        "webgl.force-enabled": true,
        "webgl.enable-webgl2": true,
      },
    }),
    30000,
    "launch",
  );

  try {
    const ctx = await browser.newContext({ locale: "en-US" });
    const page = await ctx.newPage();

    try {
      await withTimeout(
        page.goto(opts.url, {
          waitUntil: "domcontentloaded",
          timeout: 20000,
        }),
        25000,
        "goto",
      );
    } catch (e) {
      log(`goto err: ${e.message}`);
    }

    try {
      await page.waitForLoadState("networkidle", { timeout: 10000 });
    } catch {}

    const deadline = Date.now() + (opts.timeoutMs || 60000);
    while (Date.now() < deadline) {
      const title = await page.title().catch(() => "");
      const frameUrls = page.frames().map((f) => f.url());
      log(`poll title=${JSON.stringify(title)} frames=${JSON.stringify(frameUrls)}`);

      if (
        title &&
        !title.includes("Just a moment") &&
        !title.includes("Checking")
      ) {
        break;
      }

      const iframe = findCfIframe(page);
      if (!iframe) {
        log("no cf iframe this poll");
      } else {
        let handle = null;
        try {
          const js = await withTimeout(
            iframe.evaluateHandle(WALK_FOR_CHECKBOX),
            5000,
            "evaluateHandle",
          );
          handle = js.asElement();
          if (!handle) log("iframe present but checkbox not yet rendered");
        } catch (e) {
          log(`evaluateHandle err: ${e.message}`);
        }
        if (handle) {
          try {
            await withTimeout(
              handle.click({ timeout: 4000, force: true }),
              6000,
              "click",
            );
            log("clicked CF checkbox (trusted)");
            try {
              await page.waitForLoadState("networkidle", { timeout: 10000 });
            } catch {}
          } catch (e) {
            log(`checkbox click err: ${e.message}`);
          }
        }
      }

      await new Promise((r) => setTimeout(r, 2000));
    }

    const finalTitle = await page.title().catch(() => "");
    const cookies = await ctx.cookies(opts.url).catch(() => []);
    const ua = await page
      .evaluate(() => navigator.userAgent)
      .catch(() => "");
    const cfClearance = cookies.find((c) => c.name === "cf_clearance");

    return {
      ok: !!cfClearance,
      title: finalTitle,
      userAgent: ua,
      cookies: cookies.map((c) => ({ name: c.name, value: c.value })),
    };
  } finally {
    await browser.close().catch(() => {});
  }
}

(async () => {
  let input = "";
  for await (const c of process.stdin) input += c;
  try {
    const opts = JSON.parse(input);
    const result = await solve(opts);
    process.stdout.write(JSON.stringify(result) + "\n");
    process.exit(result.ok ? 0 : 1);
  } catch (e) {
    process.stdout.write(
      JSON.stringify({ ok: false, error: e.message }) + "\n",
    );
    process.exit(1);
  }
})();
