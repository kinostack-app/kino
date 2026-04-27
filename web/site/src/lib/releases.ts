// Build-time fetcher for kino releases from the GitHub API.
//
// Called from Astro pages at build (Astro `output: "static"`); the
// fetched data is baked into the static HTML — no runtime API
// calls, no rate-limit risk for site visitors.
//
// Failure modes (all collapse to "no releases yet" rather than
// breaking the build):
// - Repo doesn't exist publicly yet (404) — early dev / pre-org-
//   migration. Returns empty list.
// - GH API rate-limited (403) — CF Pages build runners are
//   shared-IP; if we ever hit it, gate behind GITHUB_TOKEN env var.
//   Today we treat as empty.
// - Network failure (timeout / DNS) — same.
//
// The pages render a "first release coming soon" state when the
// list is empty, so the failure mode is always graceful.

const GH_REPO = "kinostack-app/kino";
const GH_API = `https://api.github.com/repos/${GH_REPO}/releases?per_page=50`;

export interface ReleaseAsset {
  name: string;
  size: number;
  /** Absolute URL to the asset binary (browser-redirects ok). */
  download_url: string;
  /** GitHub-tracked download count. */
  download_count: number;
}

export interface Release {
  /** Semver tag, e.g. "v0.5.0". */
  tag: string;
  /** Display name, often the same as tag — or `tag — headline`. */
  name: string;
  /** ISO 8601 timestamp. */
  published_at: string;
  /** Markdown body (release-please / cargo-dist generated). */
  body: string;
  /** Direct GitHub URL — fallback when the user wants the source. */
  html_url: string;
  /** True for `vX.Y.Z-rc1` / `-alpha` / etc. tags. */
  prerelease: boolean;
  assets: ReleaseAsset[];
}

/** OS bucket for download-button grouping. */
export type OsGroup = "linux" | "macos" | "windows" | "raspberry-pi" | "source" | "other";

/** Architecture bucket. `universal` is e.g. macOS .pkg with both x86_64 + aarch64. */
export type Arch = "x86_64" | "aarch64" | "universal" | "any";

export interface AssetGrouped extends ReleaseAsset {
  os: OsGroup;
  arch: Arch;
  /** Human-friendly label, e.g. "Linux x64 archive (.tar.xz)". */
  label: string;
}

/** Map an asset name to its OS group + display label.
 *  cargo-dist publishes Linux/macOS archives as `.tar.xz` (NOT
 *  `.tar.gz`) and Windows as `.zip`. Don't drift those labels. */
export function classifyAsset(asset: ReleaseAsset): AssetGrouped {
  const n = asset.name.toLowerCase();
  // Order matters — longest / most specific patterns first.
  if (n.endsWith(".deb") && n.includes("arm64"))
    return { ...asset, os: "linux", arch: "aarch64", label: "Debian / Ubuntu — ARM64 (.deb)" };
  if (n.endsWith(".deb"))
    return { ...asset, os: "linux", arch: "x86_64", label: "Debian / Ubuntu — x64 (.deb)" };
  if (n.endsWith(".rpm") && n.includes("aarch64"))
    return { ...asset, os: "linux", arch: "aarch64", label: "Fedora / RHEL — ARM64 (.rpm)" };
  if (n.endsWith(".rpm"))
    return { ...asset, os: "linux", arch: "x86_64", label: "Fedora / RHEL — x64 (.rpm)" };
  if (n.endsWith(".appimage") && n.includes("aarch64"))
    return { ...asset, os: "linux", arch: "aarch64", label: "AppImage — ARM64" };
  if (n.endsWith(".appimage"))
    return { ...asset, os: "linux", arch: "x86_64", label: "AppImage — x64" };
  if (n.includes("rpi") || n.includes("raspberry") || n.includes("kino-rpi"))
    return {
      ...asset,
      os: "raspberry-pi",
      arch: "aarch64",
      label: "Raspberry Pi appliance image (.img.xz)",
    };
  if (n.includes("aarch64-unknown-linux-gnu") || n.includes("arm64-unknown-linux"))
    return { ...asset, os: "linux", arch: "aarch64", label: "Linux ARM64 archive (.tar.xz)" };
  if (n.includes("x86_64-unknown-linux-gnu"))
    return { ...asset, os: "linux", arch: "x86_64", label: "Linux x64 archive (.tar.xz)" };
  if (n.endsWith(".pkg") && n.includes("aarch64"))
    return {
      ...asset,
      os: "macos",
      arch: "aarch64",
      label: "macOS Apple Silicon installer (.pkg)",
    };
  if (n.endsWith(".pkg") && n.includes("x86_64"))
    return { ...asset, os: "macos", arch: "x86_64", label: "macOS Intel installer (.pkg)" };
  if (n.endsWith(".pkg"))
    return { ...asset, os: "macos", arch: "universal", label: "macOS installer (.pkg)" };
  if (n.includes("aarch64-apple-darwin"))
    return {
      ...asset,
      os: "macos",
      arch: "aarch64",
      label: "macOS Apple Silicon archive (.tar.xz)",
    };
  if (n.includes("x86_64-apple-darwin"))
    return { ...asset, os: "macos", arch: "x86_64", label: "macOS Intel archive (.tar.xz)" };
  if (n.endsWith(".msi"))
    return { ...asset, os: "windows", arch: "x86_64", label: "Windows installer (.msi)" };
  if (n.includes("x86_64-pc-windows"))
    return { ...asset, os: "windows", arch: "x86_64", label: "Windows x64 archive (.zip)" };
  if (n === "source.tar.gz")
    return { ...asset, os: "source", arch: "any", label: "Source archive (.tar.gz)" };
  return { ...asset, os: "other", arch: "any", label: asset.name };
}

/** Fetch + normalise releases. Returns [] on any failure. */
export async function fetchReleases(): Promise<Release[]> {
  try {
    const res = await fetch(GH_API, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!res.ok) {
      // 404 (repo private/missing), 403 (rate-limited), 5xx — all
      // collapse to empty so the build doesn't break on a missing
      // public repo or a transient GH outage.
      console.warn(`fetchReleases: GitHub API returned ${res.status}; rendering empty state`);
      return [];
    }
    const data = (await res.json()) as Array<{
      tag_name: string;
      name: string | null;
      published_at: string | null;
      body: string | null;
      html_url: string;
      prerelease: boolean;
      assets: Array<{
        name: string;
        size: number;
        browser_download_url: string;
        download_count: number;
      }>;
    }>;
    return data
      .filter((r) => r.published_at != null)
      .map((r) => ({
        tag: r.tag_name,
        name: r.name || r.tag_name,
        published_at: r.published_at as string,
        body: r.body || "",
        html_url: r.html_url,
        prerelease: r.prerelease,
        assets: r.assets
          // Drop per-file sidecar `*.sha256` files — we use the
          // combined `sha256.sum` as the SHA source. KEEP
          // `sha256.sum` itself (used by fetchShaMap to populate
          // per-file SHA pills on the download page).
          .filter((a) => !a.name.toLowerCase().endsWith(".sha256"))
          .map((a) => ({
            name: a.name,
            size: a.size,
            download_url: a.browser_download_url,
            download_count: a.download_count,
          })),
      }));
  } catch (err) {
    console.warn(`fetchReleases: ${err instanceof Error ? err.message : String(err)}`);
    return [];
  }
}

/** Latest non-prerelease, or null when nothing's been published. */
export function latestStable(releases: Release[]): Release | null {
  return releases.find((r) => !r.prerelease) ?? null;
}

/** Latest non-prerelease that has at least one downloadable asset.
 *  Avoids the v0.2.0 case where release-please created the release
 *  object before release.yml uploaded artefacts — the page would
 *  otherwise show the empty release as "latest". */
export function latestStableWithAssets(releases: Release[]): Release | null {
  return releases.find((r) => !r.prerelease && r.assets.length > 0) ?? null;
}

/** Group an asset list by OS, in display order. */
export function groupByOs(assets: ReleaseAsset[]): Map<OsGroup, AssetGrouped[]> {
  const order: OsGroup[] = ["linux", "macos", "windows", "raspberry-pi", "source", "other"];
  const out = new Map<OsGroup, AssetGrouped[]>();
  for (const os of order) out.set(os, []);
  for (const a of assets) {
    const g = classifyAsset(a);
    out.get(g.os)?.push(g);
  }
  // Drop empty buckets so the UI doesn't render empty headers.
  for (const [k, v] of out) if (v.length === 0) out.delete(k);
  return out;
}

/** Fetch the release's `sha256.sum` file and parse it into a
 *  filename → SHA256 map. cargo-dist publishes a single combined
 *  file; format is `<sha>  <filename>` per line.
 *
 *  Returns an empty map on any failure (graceful degradation —
 *  the UI just hides per-file SHA pills if we can't load them). */
export async function fetchShaMap(release: Release): Promise<Map<string, string>> {
  const sumFile = release.assets.find((a) => a.name === "sha256.sum");
  if (!sumFile) return new Map();
  try {
    const res = await fetch(sumFile.download_url);
    if (!res.ok) return new Map();
    const text = await res.text();
    const out = new Map<string, string>();
    for (const line of text.split("\n")) {
      const m = line.match(/^([a-f0-9]{64})\s+\*?(.+)$/i);
      if (m) out.set(m[2].trim(), m[1].toLowerCase());
    }
    return out;
  } catch {
    return new Map();
  }
}

/** Format an absolute date string for human display. */
export function formatDate(iso: string): string {
  return new Date(iso).toLocaleDateString("en-GB", {
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

/** Format a byte count compactly: 38291024 → "37 MB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${Math.round(bytes / (1024 * 1024))} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
