# Roadmap

Features that don't exist yet. Specs here describe **intent**,
not reality. A doc here is a design sketch + scope marker; the
code may or may not exist.

## What's here

| # | Doc | Status |
|---|---|---|
| 21 | [Cross-platform deployment](./21-cross-platform-deployment.md) | Code-complete; awaits real release-tag dry-run firing the full pipeline |
| 22 | [Desktop tray](./22-desktop-tray.md) | `kino tray` runtime shipped; `install-tray` / `uninstall-tray` are bail stubs pending `auto-launch` |
| 23 | [Help & docs site](./23-help-and-docs.md) | Starlight site (`docs.kinostack.app`) shipped; in-app `<HelpLink>` + `/help` route pending |
| 24 | [Attributions](./24-attributions.md) | Mostly shipped (LICENSE, NOTICE, CONTRIBUTING DCO, PR-template DCO checkbox, `THIRD_PARTY_LICENSES.md` via cargo-about, FFmpeg source-offer, public `/attributions` page, in-app TMDB disclaimer in Settings → Metadata). Outstanding: in-app `/about` route, DCO GitHub App on the org, SPDX header sweep, npm-side licence aggregation |
| 26 | [CLI companion](./26-cli-companion.md) | Not started — no `kino-cli` / `kino-client` / `kino-api` workspace crates |
| 27 | [Auto-update](./27-auto-update.md) | Not started — gated on subsystem 21 emitting attestations to verify against |
| 28 | [Ratings](./28-ratings.md) | Trakt user-rating slice shipped via subsystem 16; multi-source aggregator outstanding |
| 30 | [Native clients](./30-native-clients.md) | Not started (very low pre-launch priority) |
| 33 | [VPN killswitch](./33-vpn-killswitch.md) | Phases A (soft pause-all) + B (5-min IP-leak self-test) shipped; Phases C (nftables) + D (UI surface) outstanding |

## Promotion path

When a feature lands in the binary:

1. `git mv` the doc from here to [`../subsystems/`](../subsystems/).
2. Rewrite to describe what was actually built (often differs
   from the original sketch).
3. Update any references to the old path.
