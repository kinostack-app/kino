// Astro Content Collections schema. Starlight 0.37 requires this to
// be defined explicitly — the auto-generation deprecation warning is
// load-bearing, future versions remove it. Single collection bound to
// Starlight's loader so `src/content/docs/**` is picked up.

import { defineCollection } from "astro:content";
import { docsLoader } from "@astrojs/starlight/loaders";
import { docsSchema } from "@astrojs/starlight/schema";

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    schema: docsSchema(),
  }),
};
