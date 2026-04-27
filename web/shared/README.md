# web/shared/

Master copies of brand assets (logos, icons, favicons) used by
both `web/site/` (kinostack.app) and `web/docs/` (docs.kinostack.app).

`brand/` contains the canonical files. Each Astro project keeps a
copy in its own `public/brand/` so the per-project static-build
output is self-contained — Cloudflare Pages can't symlink across
build roots, and Astro doesn't follow symlinks during `import` for
asset processing.

If a brand asset is updated:

1. Edit it in `shared/brand/`
2. Copy to both project copies:

   ```sh
   cp -r web/shared/brand/* web/site/public/brand/
   cp -r web/shared/brand/* web/docs/public/brand/
   ```

3. Commit all three copies in one commit so the diff is auditable

This is fine at v1 with ~6 brand files. If asset count grows or
drift becomes a problem, add a `prebuild` script to each project
that does the copy automatically, OR move to a single-asset CDN.

`tokens.css` (planned) will hold the shared CSS custom-properties
for color, spacing, type. Today both projects duplicate the
contents of `src/styles/global.css`. Extract when divergence
becomes painful.

## Naming convention

Two slightly different conventions across the tree, intentional:

- **`web/{site,docs,shared}/public/brand/`** — unprefixed
  (`icon-512.png`, `wordmark.svg`, `og-default.png`). The path
  `/brand/` already namespaces these in the URL space.
- **`frontend/public/`** — `kino-*` prefixed (`kino-mark.svg`,
  `kino-app-icon-512.png`). Files land at the SPA root (`/`) so
  the prefix carries the namespace.

Both serve different surfaces with different URL conventions; the
divergence isn't a bug.
