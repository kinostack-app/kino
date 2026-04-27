# ADR 0009 — Cross-platform filesystem invariants

**Status:** Accepted (2026-04-26)

## Context

Kino is a media-server that scans, hardlinks, transcodes, and renames
hundreds of media files per import. Cross-platform filesystem
behaviour differs in ways that silently corrupt data if ignored:

- Windows MAX_PATH limits (260 unless long-path support is enabled)
- Reserved filenames (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`,
  `LPT1-9` — media titles can hit these)
- Case-sensitivity differences (Linux ext4: case-sensitive; Windows
  NTFS: case-insensitive; macOS APFS: case-insensitive by default,
  case-sensitive if formatted that way)
- Unicode normalisation (Linux/Windows: NFC; macOS HFS+: NFD; APFS:
  preserves what was written) — episode titles imported from one OS
  may not match library scans on another
- `rename()` semantics: POSIX overwrites if dest exists; Windows
  fails. Atomic-replace differs
- File-watching APIs (inotify / FSEvents / ReadDirectoryChangesW) —
  the `notify` crate abstracts but each has gotchas
- Network filesystems (SMB, NFS, FUSE) — locking semantics differ;
  SQLite WAL is officially unsupported on NFS

The cross-platform audit (Phase 3) catalogued these. This ADR
records the invariants we commit to and where in code they're
enforced.

## Decision

**Adopt eight cross-platform filesystem invariants. Enforce each at
a single chokepoint.** When a new code path needs filesystem
operations, it goes through the existing helper rather than rolling
its own.

### 1. All paths are `PathBuf`, never `String`

Why: `Path` / `PathBuf` understand per-OS separators and edge cases
(UNC paths on Windows, etc.). String concatenation with `/` works
on Linux/macOS, breaks on Windows.

Enforce: clippy lint `path_buf_push_overwrite` is on by default;
keep it that way. Code review for `format!("{}/{}", base, name)`
patterns — flag as bug.

### 2. Filenames sanitised against Windows reserved names + chars

Why: a TV show titled "Aux" or an episode named `Coordinator (con)`
will silently fail to write on Windows.

Reserved names (case-insensitive, with or without extension): `CON`,
`PRN`, `AUX`, `NUL`, `COM0-9`, `LPT0-9`.
Reserved chars: `< > : " / \ | ? *` and ASCII control chars 0-31.

Enforce: a single `sanitize_filename(s: &str) -> String` helper in
`backend/crates/kino/src/conventions/` (or similar) that handles
both. Every path-construction site for media filenames goes through
it. Existing release-parser sanitization should be checked against
these rules.

### 3. Path-length limit checked against Windows MAX_PATH (260)

Why: long episode titles + deep season folders can exceed 260 chars
on Windows even with long-path support disabled. Some users have it
enabled, many don't.

Enforce: a `check_path_length(path: &Path) -> Result<()>` helper
that returns a typed error if the *full* path (including media root
prefix) exceeds 240 chars (giving 20 chars of headroom). Called at
the destination-templating step in subsystem 06 (cleanup) /
subsystem 04 (import). Surfaces as a user-facing error: "filename
too long for Windows; rename source to be shorter."

Doesn't apply when running on Linux/macOS (no MAX_PATH limit). Cfg
or runtime check against the host OS is fine.

### 4. Unicode normalisation: NFC everywhere

Why: NFC is the default on Linux/Windows; NFD is the default on
macOS HFS+ (less common now that APFS preserves what's written).
Filenames roundtripping between OSes can fail string equality
checks.

Enforce: when reading filenames from the filesystem (library scan),
normalise to NFC before comparing or storing. When writing files,
write whatever the title metadata gave us (already NFC since TMDB
returns NFC). Helper in `conventions/` that exposes
`normalize_for_compare(s: &str) -> String`.

### 5. Atomic file replacement: use `tempfile` + `persist`

Why: `std::fs::rename(src, dst)` on Windows fails if `dst` exists.
On POSIX it overwrites atomically. Cross-platform atomic replace
needs the `tempfile` crate's `persist`/`persist_noclobber`.

Enforce: any code that writes a file in two phases ("write to temp,
move to final") uses the `tempfile` crate. We already have it as a
dep (subsystem 19 backup uses it). Audit existing callsites for
naked `std::fs::rename`.

### 6. Case sensitivity: store filenames case-preserved, compare
   case-insensitively only when explicit

Why: a Linux user's library has `Severance` and `severance` as two
folders; a Windows user can't. Imports from both OSes need to
agree.

Enforce: store paths verbatim as the filesystem reported them. When
matching ("does this filename already exist?"), use case-sensitive
comparison on the OS string (the kernel's view), NOT a manual
lowercase fold. The OS filesystem layer handles
case-fold-or-not-fold per-volume.

### 7. SQLite WAL on local disk only

Why: SQLite's WAL mode (which we use) is officially unsupported on
network filesystems (NFS, SMB). Locking semantics break.

Enforce: documentation in subsystem 13 (startup) — kino's
`data_path` must be on local disk. Trying to put `kino.db` on a NAS
share is unsupported. We don't programmatically detect this; we
document and fail loudly when locks misbehave.

The *media library* path can absolutely be on a NAS — different
concern.

### 8. File-watching uses the `notify` crate, never platform-specific
   directly

Why: inotify / FSEvents / ReadDirectoryChangesW have semantically
different event coalescing. The `notify` crate normalises (mostly).
Per-platform code would multiply our test surface.

Enforce: any new file-watcher reaches for `notify`. Today this is
relevant for: future "watch media library for new files" work
(roadmap, not implemented).

## Consequences

- **A `conventions/filesystem.rs` (or similar) module** holds the
  helpers (`sanitize_filename`, `check_path_length`,
  `normalize_for_compare`). Refactor existing scattered logic into
  it as we touch the surrounding code. Don't do a big-bang refactor
- **Test-first for the helpers.** Each invariant gets unit tests
  with platform-specific edge cases (a `CON.mkv` test, a 280-char
  path test, a "Café" vs "Café" NFC/NFD test)
- **CI matrix when we have one.** v0 runs Linux-only CI per
  subsystem 21 — these invariants will rot if untested. Phase 4
  testing-strategy ADR (next) covers this gap
- **Documentation.** The release-parser, import, cleanup specs each
  reference the invariants relevant to them. No central "filesystem
  rules" doc beyond this ADR — let each subsystem's spec note its
  filesystem dependencies inline
- **Migration: do nothing today.** These invariants are forward-
  looking. Existing code may not enforce them all yet. Sweep
  individual subsystems as they get touched; don't gate the audit
  on a top-down rewrite

## Alternatives considered

- **Be Linux-only and tell Windows users to use WSL2.** Rejected —
  subsystem 21's "single binary every platform" is a load-bearing
  decision (ADR 0001)
- **Rely on the `path-clean` / `slug` crates instead of writing
  helpers.** Reasonable; we'd evaluate when sweeping the existing
  parsing code. Adding a crate for `sanitize_filename` is fine if
  the crate is well-maintained
- **Reject media filenames with non-ASCII characters at import.**
  Rejected — TMDB returns Unicode titles; we'd lose metadata
  fidelity for international content

## Related ADRs

- ADR 0009 (this) feeds requirements into subsystems 04 (import),
  06 (cleanup), 14 (indexer engine — title parsing)
- subsystem 19 backup-restore — tar archive must roundtrip
  filenames cross-OS; relies on invariants 4 + 6
