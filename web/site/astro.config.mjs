// @ts-check

import sitemap from "@astrojs/sitemap";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";

// Marketing site — kinostack.app apex.
// Astro 6 + Tailwind v4. No Starlight; docs live on
// docs.kinostack.app via the sibling `web/docs/` project.
//
// `vite` is pinned to ^7 in package.json because Astro 6 itself
// runs on Vite 7; without the explicit pin npm dedup floats vite
// to 8.x (which ships Rolldown by default) and the Tailwind plugin's
// resolver hits a known Rolldown shape mismatch.
export default defineConfig({
  site: "https://kinostack.app",
  trailingSlash: "never",
  output: "static",
  integrations: [
    sitemap({
      // The Cast receiver isn't a navigable page — exclude from
      // the sitemap so search engines don't crawl it. The Cast
      // SDK pins the URL `https://kinostack.app/cast/receiver.html`
      // against the registered application ID; it must be served
      // verbatim from the marketing site root, but it's not part
      // of the navigable site.
      filter: (page) => !page.includes("/cast/"),
    }),
  ],
  vite: {
    // Astro vendors a vite version slightly behind our direct vite
    // dep, so the tailwindcss plugin's `Plugin<any>` type doesn't
    // structurally satisfy Astro's vite expectations even though
    // it works at runtime. JSDoc cast to bypass the dual-package
    // type-identity issue — safe because the plugin surface is
    // tested by tailwind's own CI.
    plugins: /** @type {any} */ ([tailwindcss()]),
  },
  image: {
    service: { entrypoint: "astro/assets/services/sharp" },
  },
});
