# Cross-platform distribution references

Best-in-class Rust projects we cloned to study how they handle
multi-platform builds, packaging, and release CI/CD. Useful for
subsystems 21 (cross-platform deployment) and 22 (desktop tray).

The other directories in `ref/` are upstream tools we're replacing
(arr stack, jellyfin, etc.) or integrating with (librqbit, rust-cast).
Those aren't on this list.

## What we cloned (2026-04-26)

| Repo | Why we cloned it |
|---|---|
| [`uv/`](./uv/) | astral-sh/uv. Built **by the cargo-dist authors**. Authoritative reference for our `release.yml` + `dist-workspace.toml` |
| [`starship/`](./starship/) | Broadest packaging coverage of any Rust CLI — every distro, every package manager. Reference for what "fully covered" looks like |
| [`bottom/`](./bottom/) | Famous for cross-compilation matrix (`Cross.toml`) and release workflow organisation |
| [`helix/`](./helix/) | Mature text editor with `.desktop`, AppStream metainfo, completions, Nix flake — model for our `packaging/` and any future AppImage |
| [`lapce/`](./lapce/) | Rust GUI editor. `extra/` directory is the model for per-platform packaging asset organisation. **Note: doesn't actually use `tray-icon`** — included for packaging patterns, not tray patterns |
| [`espanso/`](./espanso/) | Rust text expander with cross-platform tray (Linux + macOS + Windows). Closest reference for the tray work — though uses *handcrafted* per-platform tray code (Win32 C++ + Cocoa) rather than `tray-icon` |

## Where to look for what

### Cargo-dist setup → `uv/`

- [`uv/dist-workspace.toml`](./uv/dist-workspace.toml) — the modern
  cargo-dist 0.20+ pattern: separate config file instead of inline
  `[workspace.metadata.dist]` in `Cargo.toml`. Cleaner than what we
  currently have. **Worth migrating to.**
- [`uv/.github/workflows/release.yml`](./uv/.github/workflows/release.yml) —
  full cargo-dist-generated workflow. Multi-stage with `release-gate`
  environment for 2-factor approval, plan → build → host → publish
  separation
- [`uv/.github/workflows/build-docker.yml`](./uv/.github/workflows/build-docker.yml) —
  pattern for hooking custom workflows into cargo-dist via
  `local-artifacts-jobs` config. We currently put Docker in
  `channels.yml` (post-release) instead — uv puts it in the release
  pipeline itself. Different trade-off; theirs is more atomic
- [`uv/Dockerfile`](./uv/Dockerfile) — multi-stage Docker layout,
  including the use of `scratch` for the smallest possible base
  image. Also shows the "extra images" pattern (alpine variant,
  python-bundled variant, etc.)

### Multi-channel publishing → `starship/`

- [`starship/install/install.sh`](./starship/install/install.sh) —
  the canonical `curl | sh` installer. ~300 lines, handles every
  platform/arch combo. Reference for what cargo-dist's generated
  installer should be doing
- [`starship/install/macos_packages/`](./starship/install/macos_packages/) —
  Homebrew formula + cask + MacPorts portfile. Reference for
  per-channel manifest layout
- [`starship/.github/workflows/release.yml`](./starship/.github/workflows/release.yml) +
  [`post_release.yml`](./starship/.github/workflows/post_release.yml) —
  same release/post-release split we've adopted, validated at scale

### Cross-compilation matrix → `bottom/`

- [`bottom/Cross.toml`](./bottom/Cross.toml) — config for the `cross`
  cargo-cross-compiler. Faster than QEMU-emulated builds for
  ARM / RISC-V / etc. We don't use `cross` yet; if we want broader
  Linux arch coverage (Pi Zero, Pi 3, RISC-V), this is the path
- [`bottom/.github/workflows/build_releases.yml`](./bottom/.github/workflows/build_releases.yml) —
  the most thorough cross-compilation matrix in the Rust CLI world.
  ~20 targets including BSD, ARM variants, musl, etc.
- [`bottom/wix/`](./bottom/wix/) — WiX 4 manifests for the Windows
  `.msi`. cargo-dist generates a basic MSI; if we ever need a
  proper installer with components/features, this is the reference

### Per-platform packaging assets → `helix/contrib/` + `lapce/extra/`

- [`helix/contrib/`](./helix/contrib/) — `.desktop` file (Linux),
  `.appdata.xml` (Linux AppStream), `.ico` (Windows), `.png` (icon).
  Exactly what an AppImage needs (we deferred AppImage; this is the
  shopping list)
- [`lapce/extra/`](./lapce/extra/) — `linux/`, `macos/`, `windows/`
  per-platform asset organisation. Their `entitlements.plist` for
  macOS is what we'd need if we ever sign + notarise
- [`lapce/extra/macos/Lapce.app/`](./lapce/extra/macos/Lapce.app/) —
  full `.app` bundle layout. Reference if we hand-build the macOS
  bundle instead of relying on cargo-dist's `.pkg`
- [`helix/flake.nix`](./helix/flake.nix) — Nix packaging in-repo.
  We marked Nix as Tier 3 (community-maintained); this shows what
  a maintained flake looks like

### Tray icon (cross-platform GUI) → `espanso/espanso-ui/`

- [`espanso/espanso-ui/src/`](./espanso/espanso-ui/src/) — the
  per-platform tray code. **Important caveat**: espanso predates the
  `tray-icon` crate maturing and rolls its own per-platform code:
  - [`linux/`](./espanso/espanso-ui/src/linux/mod.rs) — D-Bus / SNI directly
  - [`mac/`](./espanso/espanso-ui/src/mac/mod.rs) — Cocoa via Objective-C bridges
  - [`win32/native.cpp`](./espanso/espanso-ui/src/win32/native.cpp) — raw Win32
- We use `tray-icon` (Tauri team's crate) which abstracts all this.
  Reference espanso for: icon-state management, click event handling
  patterns, single-instance enforcement, lifecycle on logout/sleep

## Patterns I'd recommend we adopt now

Distilled from the above, ordered by effort:

### 1. Migrate `[workspace.metadata.dist]` → `dist-workspace.toml`

Low effort. Move our cargo-dist config into a top-level
`dist-workspace.toml` (cargo-dist 0.20+ pattern, see uv). Keeps
`backend/Cargo.toml` focused on Rust deps, makes the dist config
discoverable at repo root, matches what `cargo dist init` writes
today.

### 2. Pin GitHub Action commits, not tags

uv pins every action by commit SHA + comment with the version tag
(e.g. `actions/checkout = "<sha>" # v6.0.2`). Defends against
supply-chain attacks via tag mutation. Our workflows use floating
`@v4` tags. Worth tightening before the workflows go live.

### 3. Use `workflow_dispatch` for releases, not `push: tags: [v*]`

uv's `release.yml` triggers via `workflow_dispatch` with a tag
input. Lets us dry-run the release pipeline by typing
`tag=dry-run`, which uv's workflow then short-circuits. Much safer
than "push a tag and hope it works." We can keep tag-push as a
fallback.

### 4. Add a `release-gate` environment

uv requires 2-factor approval before a release runs (a second team
member clicks "approve"). Single-maintainer projects can skip this,
but if there are ever multiple maintainers, it's a free safety net.

### 5. Consider `cross` for ARM Linux builds

bottom uses `Cross.toml` to cross-compile via the `cross`
cargo-extension. Faster than native ARM runners for some targets,
and supports more architectures (Pi Zero armv6, RISC-V, MIPS).
We currently use GitHub's native `ubuntu-24.04-arm` runner — fine
for aarch64, but no good for armv6/v7 if we ever want Pi Zero
support beyond what the appliance image gives.

### 6. Defer: GitHub Attestations

uv ships with build attestations enabled. Higher-assurance supply
chain provenance. Not blocking for v0; revisit when the project has
a security model document.

## Updating these refs

Re-clone with `--depth 1` to pull current state. Don't `git pull` —
the shallow history can't fast-forward cleanly:

```sh
rm -rf ref/<name> && git clone --depth 1 https://github.com/<owner>/<name> ref/<name>
```

These refs are NOT vendored dependencies — they're read-only study
material. Don't import code from them; either copy a small snippet
with attribution in a code comment, or note the reference in a
subsystem doc and write our own.
