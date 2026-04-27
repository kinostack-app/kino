# Packaging

Per-channel manifests + scripts that don't fit alongside the crate
they describe. Driven by the workflows in
[`../.github/workflows/channels.yml`](../.github/workflows/channels.yml)
on every GitHub Release published.

| Subdir | Channel | Notes |
|---|---|---|
| [`aur/`](./aur/) | Arch Linux User Repository | `kino-bin` — pre-built binary. Bumped + pushed by the `aur` job in `channels.yml` |
| [`pi-gen/stage-kino/`](./pi-gen/stage-kino/) | Raspberry Pi appliance image | Layered on top of Pi OS Lite via the `pi-image` job in `channels.yml` |

## What lives elsewhere (and why)

Not everything packaging-related lands here. Some manifests have to
live where the consuming tool looks for them:

| Lives at | Why |
|---|---|
| `/Dockerfile`, `/.dockerignore` | Docker convention — buildkit / GHCR / scanners default to `./Dockerfile` |
| `/.github/workflows/{ci,release,channels}.yml` | GitHub-required path |
| `/LICENSE`, `/README.md` | Universal expectation; cargo-dist, crates.io, license scanners look here |
| `/backend/Cargo.toml` `[workspace.metadata.dist]` | cargo-dist reads workspace metadata at the workspace root |
| `/backend/crates/kino/Cargo.toml` `[package.metadata.deb]` + `[package.metadata.generate-rpm]` | `cargo-deb` and `cargo-generate-rpm` look adjacent to the crate's `Cargo.toml` |
| `/backend/crates/kino/debian/`, `/backend/crates/kino/rpm/` | Postinst scripts + systemd units consumed by the above |

## Adding a new channel

1. Add a subdir here with its manifest and any helper scripts
2. Add a job to `.github/workflows/channels.yml` — independent of
   the others (a flake in the new channel shouldn't block existing
   ones)
3. Update [`../docs/roadmap/21-cross-platform-deployment.md`](../docs/roadmap/21-cross-platform-deployment.md)
   §7 to list the channel + its tier
