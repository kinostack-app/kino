# ADR 0007 — No paid code signing at launch

**Status:** Accepted (2026-04-26)

## Context

Distributing macOS and Windows binaries through any direct-download
channel (`.dmg`, `.msi`, raw `.tar.gz`) puts users in front of OS
gatekeepers — Apple Gatekeeper warns about unsigned developers,
Windows SmartScreen warns about unrecognised binaries. The standard
fix is paid code-signing certificates:

- **Apple Developer Program** — $99/year, enables Developer ID
  signing + notarisation (no Gatekeeper warning)
- **Windows Authenticode** — EV cert (~$300-500/year, instant
  SmartScreen reputation) or OV cert (~$100-300/year, builds
  reputation over weeks)

The cross-platform audit examined what other Rust projects do
(starship signs Windows + notarises macOS; uv ships unsigned with
Astral's distribution muscle compensating; bottom signs Windows
only) and the user experience consequences of skipping.

## Decision

**Don't pay for code-signing certificates at launch.** Steer users
toward package-manager channels (Homebrew, winget, AUR, `.deb`,
`.rpm`, Docker) which sign on our behalf. Document the
direct-download Gatekeeper / SmartScreen click-through clearly.

Re-evaluate when one of:

1. **User volume justifies the spend** (no specific threshold yet —
   "we hear about the warning every week" is the rough trigger)
2. **A trust signal becomes load-bearing** (e.g. enterprise users
   ask "is this signed?" as a procurement check)

When we do start, **Apple Developer Program first** — Gatekeeper is
a scarier dialog than SmartScreen and the $99/year is a third the
cost of even an OV Authenticode cert.

## Why

**Package channels do the work for us.** The matrix in subsystem 21
shows Tier 1 channels — Homebrew Cask, winget, AUR, `.deb`, `.rpm`,
Docker, Pi image — all bypass OS gatekeepers entirely:

| Channel | Direct-download friction | Channel-install friction |
|---|---|---|
| macOS Homebrew Cask | Gatekeeper right-click | None |
| macOS direct `.dmg` | Gatekeeper right-click | n/a |
| Windows winget | SmartScreen "More info" | None |
| Windows direct `.msi` | SmartScreen "More info" | n/a |
| Linux `.deb` / `.rpm` | none | None |

The marketing site + install docs lead with the package-manager
commands. The direct-download path is the "advanced users" route,
and they expect a one-time click-through.

**Cost vs benefit at our stage.** $99 (Apple) + $200-500 (Windows
EV/OV) = ~$300-600/year recurring. Pre-revenue, pre-launch, with no
known users complaining about the warning, that spend is hard to
justify. Better to revisit when the project has the funding model
to support it (donations, sponsorship, paid hosted version — none
of which exist today).

**Reference projects vary.** uv (Astral) ships unsigned and relies
on `astral.sh` brand recognition. starship signs Windows
(SignPath community programme) and notarises macOS — both via
sponsorship rather than out-of-pocket. We have neither at launch.

## Consequences

**Direct-download users see a warning the first time:**

- **macOS**: "Kino can't be opened because Apple cannot verify it
  is free of malware" → right-click → Open. Once accepted, future
  launches don't prompt
- **Windows**: "Windows protected your PC" → "More info" → "Run
  anyway"
- **Linux**: nothing

**Documentation must explicitly cover the click-through.** Per the
audit's Phase 2 recommendation, the per-platform install guides
(task #529) include screenshots of the dialogs and the workaround.
Without docs, the warning looks like "kino is malware."

**SmartScreen reputation never builds without signing.** Even after
many downloads, unsigned binaries will continue to trigger the
SmartScreen prompt. Only the EV cert provides instant reputation; OV
builds reputation over weeks of accumulated installs.

**Defender false-positive risk.** Fresh unsigned binaries
occasionally get flagged as "suspicious." Mitigation: submit each
release to the Microsoft Defender analysis portal (free) before
announcing.

**No supply-chain provenance signal at the binary level.** GPG-sign
release archives + checksums (cargo-dist does this); GitHub
attestations now enabled (per Phase 1) provide build-provenance
SLSA signal. These are weaker than code-signing for end-user trust
but stronger for sysadmin verification.

## Alternatives considered

- **Apple Developer Program now ($99/year).** The cheaper of the
  two; biggest UX win. Rejected for v0 launch but earmarked as
  first investment when we revisit
- **Windows EV cert ($300-500/year).** More expensive, less urgent
  (SmartScreen click-through is less scary than Gatekeeper).
  Defer further than the Apple cert
- **Apply to SignPath community programme** (free Authenticode for
  OSS projects). Worth doing in parallel with the Apple decision —
  no cost, builds Windows reputation
- **Self-sign with a non-trusted CA.** No — pretends to be signed
  but doesn't help with OS gatekeepers; just confuses users

## Related ADRs

- ADR 0001 — Single-binary architecture (cross-platform shipping
  motivates this discussion)
- subsystem 21 — Cross-platform deployment §6 documents the
  unsigned-binary posture in operational terms
