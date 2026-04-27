# Releasing kino

How to cut a release, end-to-end. The pipeline is automated through
GitHub Actions; this doc covers the manual steps + recovery
procedures.

## TL;DR

Releases are driven by **release-please** + conventional-commit
discipline. The maintainer doesn't bump versions or write CHANGELOG
entries by hand.

```sh
# 1. Land PRs on main with conventional-commit titles. release-please
#    keeps an open "Release PR" reflecting the next version + entries
# 2. When ready to ship: review + merge the Release PR
# 3. release-please cuts a vX.Y.Z tag → release.yml fires plan + build
# 4. Click "Run workflow" on release.yml's host job (dispatch-releases
#    gate) to upload archives to a draft GitHub Release
# 5. Review the auto-generated draft GitHub Release; edit notes if needed
# 6. Publish the Release in the GitHub UI — this fires channels.yml
# 7. Monitor channel jobs; re-run any flakes
# 8. Announce in Discord / forum / RSS
```

Total maintainer time: ~10 minutes of review + monitoring per release.

The extra "Run workflow" step (step 4) comes from
`dispatch-releases = true` in `[workspace.metadata.dist]` — a pre-1.0
safety gate so a green-CI build with a subtle regression doesn't
auto-upload before a human eyeballs it. Flip to `false` once releases
are routine.

## How release-please works

After every push to `main`, the `release-please` workflow:

1. Reads conventional-commit messages since the last `vX.Y.Z` tag
2. Computes the next version per SemVer + the `bump-minor-pre-major: true`
   rule (pre-1.0: `feat:` → minor, `fix:` → patch)
3. Updates (or opens) a single "Release PR" that bumps
   `backend/crates/kino/Cargo.toml`'s version field + writes a
   CHANGELOG.md entry
4. Sits there until you merge it. Merging:
   - Creates the `vX.Y.Z` tag
   - Creates a draft GitHub Release with the changelog as body
   - Fires `release.yml` (cargo-dist build pipeline)
5. Once you publish the GitHub Release in the UI, `channels.yml`
   fans out

Conventional commit types we recognise (config in
`release-please-config.json`):

| Type | Bump (pre-1.0) | Changelog section |
|---|---|---|
| `feat:` / `feat!:` | minor | Added |
| `fix:` | patch | Fixed |
| `perf:` | patch | Changed |
| `revert:` | patch | Changed |
| `refactor:` | patch | Changed |
| `docs:` | patch | Docs |
| `build:` / `ci:` / `chore:` / `style:` / `test:` | patch | hidden from changelog |

## Pre-release checklist

Before merging the Release PR:

- [ ] All open PRs intended for this release are merged into `main`
- [ ] CI on `main` is green (`ci.yml` + `cross-os.yml` + `zizmor.yml`)
- [ ] The Release PR's CHANGELOG diff matches the actual scope — if a `feat:` PR doesn't appear, check the commit-message type
- [ ] No new deps with known CVEs (`cargo-deny` step in `ci.yml` covers advisories + license drift in one pass)
- [ ] (Pre-1.0 only) No backwards-incompatible API changes since the last release without a clear migration note in the relevant subsystem doc

## Tag format

`vMAJOR.MINOR.PATCH`. Pre-releases use `vMAJOR.MINOR.PATCH-betaN` or
`vMAJOR.MINOR.PATCH-rcN`. Anything else fails CI's tag-format guard
in `release.yml`.

| Tag | What it does |
|---|---|
| `v0.5.0` | Stable release — winget / AUR / Pi image / channels all fire |
| `v0.5.0-beta1` | Pre-release — channels.yml's `winget` / `aur` / `pi-image` jobs skip (they have `if: !contains(...tag_name, '-')` guards) |
| `dry-run` (workflow_dispatch only) | Exercises the pipeline without publishing anything |

## What fires when

```
PR merged to main with feat:/fix:/etc. title
        │
        ▼
release-please.yml runs → updates the open "Release PR"
                         (no tag yet, just keeps the PR fresh)
        │
        ▼
[manual: review + merge the Release PR when ready to ship]
        │
        ▼
release-please cuts vX.Y.Z tag + creates draft GitHub Release
        │
        ▼
release.yml triggers
        │
        ├─ plan job (cargo dist plan)
        ├─ build matrix (5 OS targets via cargo dist build)
        │    └─ smoke test: every built binary --version + --help
        ├─ linux-packages (.deb + .rpm via cargo-deb + cargo-generate-rpm)
        └─ publish job
              ├─ uploads archives to GitHub Release (cargo dist host)
              ├─ bumps Homebrew tap formula
              └─ attaches .deb + .rpm to the Release
        │
        ▼
[manual: review draft GitHub Release in UI]
        │
        ▼
[manual: click Publish on the Release]
        │
        ▼
channels.yml triggers (release: published webhook)
        │
        ├─ winget         → submit PR to microsoft/winget-pkgs
        ├─ aur            → bump kino-bin PKGBUILD + push to AUR
        ├─ docker         → multi-arch build + push to ghcr.io/kinostack-app/kino
        ├─ docker-manifest → combine arch-specific images into one tag
        └─ pi-image       → pi-gen builds .img.xz + attaches to Release
```

## Dry-run before tagging

Before the first release of a major or minor version, exercise the
pipeline without publishing:

1. Go to **Actions → Release → Run workflow** in the GitHub UI
2. In the `tag` input, enter `dry-run` (or any string not matching `v[0-9]+`)
3. Click **Run workflow**

This runs `plan` + `build` + `linux-packages` + smoke-tests but
skips the `publish` job (it's gated on `needs.plan.outputs.publishing == 'true'`).
Lets you catch YAML / config / build breakage in a low-stakes way.

Nothing is uploaded to GitHub Releases or to any channel.

## Recovery — when a channel fails

Each channel job is independent. A flake in winget doesn't block
AUR; a flake in Docker doesn't block winget. To re-trigger one
channel:

1. Go to **Actions → Channels → Run workflow**
2. Enter the existing release tag (e.g. `v0.5.0`) in the `tag` input
3. Click **Run workflow**

The whole channels.yml re-runs. Jobs that already succeeded re-run
idempotently — winget will silently skip if its PR exists, AUR will
push the same content (no-op), Docker will re-tag (latest = same
SHA). Safe to re-trigger.

### Per-channel failure modes

| Channel | Common failures | Fix |
|---|---|---|
| **winget** | wingetcreate validation rejects manifest | Manually edit + open PR to `microsoft/winget-pkgs`; ping the kino repo for next release to fix the manifest template |
| **AUR** | SSH key rejected | Rotate `AUR_SSH_PRIVATE_KEY` in repo secrets; re-run |
| **Docker** | GHCR auth fails | Verify `GITHUB_TOKEN` permissions include `packages: write`; re-run |
| **Pi image** | pi-gen build fails inside QEMU chroot | Often a transient apt mirror issue; re-run. If persistent, check the latest pi-gen-action upstream issues |
| **Homebrew tap** (in publish job) | Tap repo write access denied | Verify `HOMEBREW_TAP_TOKEN` PAT hasn't expired; rotate + re-run the publish job |

## Secrets

Stored in repo Settings → Secrets and variables → Actions:

| Secret | Purpose | Rotation cadence |
|---|---|---|
| `GITHUB_TOKEN` | Built-in — covers GitHub Release creation, GHCR push, PR comments | Auto, no action needed |
| `HOMEBREW_TAP_TOKEN` | PAT with write access to `kinostack-app/homebrew-kino` | Yearly; rotate immediately if leaked |
| `WINGET_TOKEN` | PAT for `vedantmgoyal9/winget-releaser` to PR to `microsoft/winget-pkgs` | Yearly |
| `AUR_SSH_PRIVATE_KEY` | SSH key registered with the AUR account | Yearly; rotate immediately if leaked |
| `GPG_PRIVATE_KEY` + `GPG_PASSPHRASE` | Signs release archives + APT/RPM repo metadata | Multi-year; primary cert + 1-year subkey |

### Secret rotation procedure

For PATs (HOMEBREW_TAP_TOKEN, WINGET_TOKEN):

1. Generate new PAT in the GitHub account that owns it (kinostack-app
   bot or maintainer account)
2. Scope: minimal — `public_repo` for tap-bump and winget-PR are sufficient
3. Replace in repo Secrets
4. Test by re-running the previous channels.yml and confirming the
   bump still works
5. Revoke the old PAT

For AUR SSH key:

1. Generate new ED25519 keypair locally
2. Update the AUR account's SSH key list (https://aur.archlinux.org/account/...)
3. Replace `AUR_SSH_PRIVATE_KEY` secret with new private key
4. Test by re-running channels.yml `aur` job
5. Remove the old public key from the AUR account

For GPG:

1. Generate new subkey (don't replace the primary)
2. Update `GPG_PRIVATE_KEY` secret with new subkey export
3. Update `kinostack.app/keys/release-signing.asc` to publish the
   new public subkey
4. Old subkey stays valid for verifying old releases; new subkey
   signs going forward

## Code-signing posture

We don't pay for Apple Developer Program or Authenticode certs.
Direct-download macOS users see a Gatekeeper warning;
direct-download Windows users see SmartScreen. Package channels
(Homebrew, winget, AUR, deb, rpm, Docker) bypass both. See
[ADR 0007](./docs/decisions/0007-no-paid-signing-at-launch.md).

When/if we adopt signing:

- Apple Developer Program ($99/yr) — first investment; bigger UX win
- Windows EV cert ($300-500/yr) — second; SmartScreen is a less scary dialog

Document the change here; release.yml gains signing steps; the
"unsigned-binary posture" section of the install docs gets removed.

## After publishing

- [ ] Monitor channels.yml for ~30 min after Publish; address flakes
- [ ] Update kinostack.app's "current version" widget (auto-pulls from GitHub Releases; manual fallback if it lags)
- [ ] Post release announcement (Discord / forum / RSS / mastodon)
- [ ] Watch issue tracker for early-adopter regression reports

(release-please owns CHANGELOG.md — no manual update step.)

## When the worst happens — emergency rollback

If a release ships a critical bug (data loss, crash on startup,
security regression) that affects many users:

1. **Immediately** mark the GitHub Release as "Pre-release" in the UI.
   This signals package channels (and end users running update checks)
   that the release isn't stable
2. Ship a `vX.Y.Z+1` patch ASAP with the fix
3. The autoupdate subsystem (#27, when shipped) will detect a new
   version and prompt users to upgrade past the bad release
4. For users on the bad release without auto-update: post a
   pinned issue + Discord announcement with manual rollback steps
5. Post-mortem: write up the failure mode in
   `docs/runbooks/incidents/`. Each incident gets an entry; pattern
   matches `0001-bad-migration.md` etc.

## Pre-1.0 caveats

- We're pre-1.0; expect breaking changes between minor versions
- No backports — security fixes go in `main`; users upgrade
- Once we cut v1.0, this section gets replaced with the actual
  branching / backport policy

## Related docs

- [`docs/roadmap/21-cross-platform-deployment.md`](./docs/roadmap/21-cross-platform-deployment.md) §8 — release engineering details
- [`docs/architecture/testing-strategy.md`](./docs/architecture/testing-strategy.md) — pre-release verification layers
- [`SECURITY.md`](./SECURITY.md) — vulnerability reporting + handling
