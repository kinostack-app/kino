// @ts-check

import sitemap from "@astrojs/sitemap";
import starlight from "@astrojs/starlight";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";

// Docs site — docs.kinostack.app.
// Astro 6 + Starlight 0.38 + Tailwind v4. Marketing apex
// (kinostack.app) lives in the sibling `web/site/` project.
//
// `vite` is pinned to ^7 in package.json because Astro 6 itself
// runs on Vite 7; without the explicit pin npm dedup floats vite
// to 8.x (which ships Rolldown by default) and the Tailwind plugin's
// resolver hits a known shape mismatch — Astro 6.1 also warns on
// startup when it detects Vite 8 at the top level for this reason.
export default defineConfig({
  site: "https://docs.kinostack.app",
  trailingSlash: "never",
  output: "static",
  integrations: [
    sitemap(),
    starlight({
      title: "Kino docs",
      logo: {
        src: "./src/assets/wordmark.svg",
        replacesTitle: true,
      },
      favicon: "/favicon.ico",
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/kinostack-app/kino",
        },
      ],
      // Sidebar buckets — uv's taxonomy (Getting Started / Setup /
      // Integrations / Features / Reference / Troubleshooting).
      // See docs/SITE_PLAN.md §2 for the locked content map.
      sidebar: [
        {
          label: "Getting started",
          autogenerate: { directory: "getting-started" },
        },
        {
          label: "Setup",
          autogenerate: { directory: "setup" },
        },
        {
          label: "Integrations",
          autogenerate: { directory: "integrations" },
        },
        {
          label: "Features",
          autogenerate: { directory: "features" },
        },
        {
          label: "Reference",
          autogenerate: { directory: "reference" },
        },
        {
          label: "Troubleshooting",
          autogenerate: { directory: "troubleshooting" },
        },
      ],
      customCss: ["./src/styles/global.css"],
      // Edit-this-page links into the docs source tree, not the
      // top-level repo root.
      editLink: {
        baseUrl: "https://github.com/kinostack-app/kino/edit/main/web/docs/",
      },
      // Pagefind ships built-in client-side search.
      lastUpdated: true,
    }),
  ],
  vite: {
    plugins: /** @type {any} */ ([tailwindcss()]),
  },
  image: {
    service: { entrypoint: "astro/assets/services/sharp" },
  },
});
