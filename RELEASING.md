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
release-please cuts vX.Y.Z tag + creates the GitHub Release
(non-draft; `draft: false` in release-please-config.json)
        │
        ▼
[CURRENT GOTCHA: GitHub doesn't fire workflows on tag pushes
 done by GITHUB_TOKEN. release.yml needs a manual nudge:]

   gh workflow run release.yml -R kinostack-app/kino \
       -f tag=vX.Y.Z

[Alternative future fix: have release-please use a PAT, or
 add `release: published` to release.yml's `on:` so the
 release-creation event fires it directly. Tracked TODO.]
        │
        ▼
release.yml runs (workflow_dispatch with tag=vX.Y.Z)
        │
        ├─ plan job (cargo dist plan)
        ├─ build matrix (5 OS targets via cargo dist build)
        │    └─ smoke test: every built binary --version + --help
        ├─ AppImage (x86_64 + aarch64)
        ├─ linux-packages (.deb + .rpm via cargo-deb + cargo-generate-rpm)
        └─ publish job
              ├─ downloads `artifacts-*` to backend/target/distrib/
              │  (cargo-dist's expected location — earlier bug
              │  was downloading to dist/ which made cargo-dist
              │  silently skip uploads)
              ├─ uploads archives + MSI + .pkg + source.tar.gz
              │  + sha256.sum + installers via `cargo dist host`
              ├─ bumps Homebrew tap formula
              └─ attaches .deb / .rpm / AppImage via softprops
        │
        ▼
release.yml succeeds → channels.yml fires via `workflow_run`
(NOT via `release: published` — release-please races the
artefact upload, so workflow_run + a `guard` job that checks
conclusion=success is what guarantees artefacts exist)
        │
        ├─ winget         → submit PR to microsoft/winget-pkgs
        │                   (FIRST submission of a new package
        │                   needs a manual `wingetcreate new`
        │                   seed — see Phase G in setup runbook)
        ├─ aur            → bump kino-bin PKGBUILD + push to AUR
        ├─ docker         → multi-arch build + push to ghcr.io/kinostack-app/kino
        ├─ docker-manifest → combine arch-specific images into one tag
        ├─ pi-image       → downloads aarch64 .deb from release,
        │                   pi-gen stages it into the chroot,
        │                   builds .img.xz + attaches to Release
        └─ msstore        → uploads MSIX from the release to
                            Microsoft Store via msstore CLI.
                            FIRST submission for a new product
                            MUST be done manually via Partner
                            Center; this job soft-skips until the
                            four MSSTORE_* secrets + product-id
                            variable are configured.
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
| **Microsoft Store** | `msstore publish` returns "submission already in progress" | Cancel or wait for the in-flight submission in Partner Center, then re-run |
| **Microsoft Store** | "first submission must be made through Partner Center" | The product hasn't been seeded with a manual first submission yet — upload the MSIX from the GitHub Release manually in Partner Center, get it approved, then enable the secrets |
| **Microsoft Store** | Authentication fails (401) | `MSSTORE_CLIENT_SECRET` likely expired (24-month max). Generate a new secret in Azure Portal → App registrations → Certificates & secrets, replace in repo Secrets, re-run |
| **Microsoft Store** | `MSSTORE_PRODUCT_ID` looks invalid | The product ID is the 12-character alphanumeric on the Partner Center URL after `/products/` (NOT the Store ID `9NXXXXXXXXXX`); copy from the URL bar |

## Secrets

Stored in repo Settings → Secrets and variables → Actions:

| Secret | Purpose | Rotation cadence |
|---|---|---|
| `GITHUB_TOKEN` | Built-in — covers GitHub Release creation, GHCR push, PR comments | Auto, no action needed |
| `HOMEBREW_TAP_TOKEN` | PAT with write access to `kinostack-app/homebrew-kino` | Yearly; rotate immediately if leaked |
| `WINGET_TOKEN` | PAT for `vedantmgoyal9/winget-releaser` to PR to `microsoft/winget-pkgs` | Yearly |
| `AUR_SSH_PRIVATE_KEY` | SSH key registered with the AUR account | Yearly; rotate immediately if leaked |
| `GPG_PRIVATE_KEY` + `GPG_PASSPHRASE` | Signs release archives + APT/RPM repo metadata | Multi-year; primary cert + 1-year subkey |
| `MSSTORE_TENANT_ID` | Microsoft Entra (Azure AD) tenant ID associated with the Partner Center account | Stable — only changes if the Entra directory itself is replaced |
| `MSSTORE_CLIENT_ID` | Application (client) ID of the Entra app registration that has Manager role on Partner Center | Stable until the app registration is replaced |
| `MSSTORE_CLIENT_SECRET` | Client secret of the Entra app registration | **Max 24 months** — Azure caps the lifetime; rotate ahead of expiry to avoid release-day 401s |
| `MSSTORE_SELLER_ID` | Publisher / seller ID from Partner Center → Account settings → Identifiers | Stable — fixed for the lifetime of the publisher account |

The Store channel also needs one repo **variable** (not a secret —
public, displayed on the Store listing URL):

| Variable | Purpose |
|---|---|
| `MSSTORE_PRODUCT_ID` | The 12-character product ID from the Partner Center URL (`/products/<id>`). NOT the customer-facing Store ID `9NXXXXXXXXXX`. |
| `MSSTORE_IDENTITY_NAME` | The full `<publisher-id>.Kino` Identity/Name string from Partner Center → Product identity. Substituted into `appxmanifest.xml` by the `build-msix` job. |
| `MSSTORE_PUBLISHER_CN` | The full `CN=<…>` Publisher string from Partner Center → Product identity. Must match exactly. |

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

For `MSSTORE_CLIENT_SECRET` (the only one with a hard expiry):

1. Azure Portal → Microsoft Entra ID → App registrations → kino's app
2. Certificates & secrets → New client secret (expiry: 24 months max)
3. Copy the **value** immediately — Azure never displays it again
4. Replace `MSSTORE_CLIENT_SECRET` in repo Secrets
5. Confirm by re-running channels.yml `msstore` job manually
6. Delete the old secret from Azure once the new one is verified

## Microsoft Store: first submission

The Store channel can't be fully automated from day one. Microsoft
requires the first submission of every new product to go through
Partner Center manually (the Submission API only works on products
that already have a published submission).

So the bootstrap flow is:

1. **v0.1.0 ships** through release.yml → channels.yml. The
   `msstore` job soft-skips (no secrets configured yet); the MSIX
   appears as a release asset on the GitHub Release.
2. **Manual upload to Partner Center**:
   - Sign in to [Partner Center](https://partner.microsoft.com/)
   - Apps and games → Kino Media Server → Submission 1 → Packages
   - Upload `kino-0.1.0-x64.msix` from the GitHub Release
   - In submission notes, justify the `broadFileSystemAccess`
     restricted capability: "Self-hosted media server scans
     user-selected library directories outside the app sandbox."
   - Submit → review (1-3 business days for first MSIX, longer if
     the rescap justification needs back-and-forth)
3. **After approval**, set up automation:
   - Register an Entra app in Azure Portal (Microsoft Entra ID →
     App registrations → New registration)
   - Partner Center → Account settings → Users → Microsoft Entra
     applications → Add the new app with **Manager** role
   - Generate a client secret (Certificates & secrets → New)
   - Set the four `MSSTORE_*` repo secrets and three repo variables
     listed above
4. **v0.2.0 onwards**: tag → release.yml → channels.yml `msstore`
   job sees the credentials → submits automatically.

Once the API is in use, **don't mix Partner Center UI edits with
API-driven submissions**: the API loses the ability to update or
publish a submission that was last edited via the UI. If you have to
make a UI edit, follow up with a no-op API submission to re-bind.

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
