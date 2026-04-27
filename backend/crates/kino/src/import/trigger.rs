//! Import trigger — runs when a download completes.
//!
//! Finds the downloaded file, hardlinks or copies it to the media library,
//! creates Media entity with real file info, updates content status.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::download::{DownloadPhase, TorrentSession};
use crate::events::AppEvent;
use crate::import::naming::{self, NamingContext};

/// Previous media row for a movie/episode, captured before the new
/// import lands so the upgrade-replace path can delete the file +
/// fire the `Upgraded` event.
#[derive(sqlx::FromRow)]
pub(crate) struct OldMediaRow {
    pub id: i64,
    pub file_path: String,
    pub resolution: Option<i64>,
    pub source: Option<String>,
}

/// Naming-format fields pulled once per import. Falls back to the
/// built-in defaults if the config row is missing or empty — matches
/// the schema defaults so a first-run install still gets sensible
/// library paths.
pub(crate) struct NamingFormats {
    pub movie: String,
    pub episode: String,
    pub season: String,
}

pub(crate) async fn load_naming_formats(pool: &SqlitePool) -> anyhow::Result<NamingFormats> {
    let (movie, episode, season): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT movie_naming_format, episode_naming_format, season_folder_format
         FROM config WHERE id = 1",
        )
        .fetch_one(pool)
        .await?;
    Ok(NamingFormats {
        movie: movie
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "{title} ({year}) [{quality}]".to_owned()),
        episode: episode.filter(|s| !s.is_empty()).unwrap_or_else(|| {
            "{show} - S{season:00}E{episode:00} - {title} [{quality}]".to_owned()
        }),
        season: season
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Season {season:00}".to_owned()),
    })
}

/// Build a naming context for a movie import. Pulls the movie row's
/// canonical title / year / external IDs so the library filename
/// reads the user's intended name even if the torrent's filename was
/// noisy (sites, prefixes, encoder tags).
/// Release-level parsed quality bundle, passed by reference so the
/// naming-context builders don't balloon into 9-argument signatures
/// (clippy is strict about that).
pub(crate) struct ParsedQuality<'a> {
    pub resolution: Option<i64>,
    pub source: Option<&'a str>,
    pub video_codec: Option<&'a str>,
    pub audio_codec: Option<&'a str>,
    pub hdr_format: Option<&'a str>,
    pub release_group: Option<&'a str>,
}

pub(crate) async fn movie_naming_context(
    pool: &SqlitePool,
    movie_id: i64,
    pq: &ParsedQuality<'_>,
    container: &str,
) -> anyhow::Result<NamingContext> {
    let (title, year, imdb_id, tmdb_id): (String, Option<i64>, Option<String>, i64) =
        sqlx::query_as("SELECT title, year, imdb_id, tmdb_id FROM movie WHERE id = ?")
            .bind(movie_id)
            .fetch_one(pool)
            .await?;
    let resolution_s = pq.resolution.map(|r| r.to_string());
    Ok(NamingContext {
        title,
        show: None,
        year,
        season: None,
        episode: None,
        episode_title: None,
        quality: naming::quality_string(pq.source, resolution_s.as_deref()),
        resolution: resolution_s,
        source: pq.source.map(str::to_owned),
        codec: pq.video_codec.map(str::to_owned),
        hdr: pq.hdr_format.map(str::to_owned),
        audio: pq.audio_codec.map(str::to_owned),
        group: pq.release_group.map(str::to_owned),
        imdb_id,
        tmdb_id: Some(tmdb_id),
        container: container.to_owned(),
    })
}

/// Build a naming context for an episode. Pulls the show title (for
/// the folder + `{show}` token) and the episode's own title / season
/// / episode numbers (for the filename).
pub(crate) async fn episode_naming_context(
    pool: &SqlitePool,
    episode_id: i64,
    pq: &ParsedQuality<'_>,
    container: &str,
) -> anyhow::Result<(String, NamingContext)> {
    let row: (
        String,
        Option<i64>,
        Option<String>,
        i64,
        i64,
        Option<String>,
        i64,
    ) = sqlx::query_as(
        "SELECT s.title, s.year, s.imdb_id, s.tmdb_id,
                e.season_number, e.title, e.episode_number
         FROM episode e
         JOIN show s ON s.id = e.show_id
         WHERE e.id = ?",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;
    let (show_title, year, imdb_id, show_tmdb_id, season_number, episode_title, episode_number) =
        row;
    let resolution_s = pq.resolution.map(|r| r.to_string());
    let ctx = NamingContext {
        title: episode_title
            .clone()
            .unwrap_or_else(|| format!("Episode {episode_number}")),
        show: Some(show_title.clone()),
        year,
        season: Some(season_number),
        episode: Some(episode_number),
        episode_title,
        quality: naming::quality_string(pq.source, resolution_s.as_deref()),
        resolution: resolution_s,
        source: pq.source.map(str::to_owned),
        codec: pq.video_codec.map(str::to_owned),
        hdr: pq.hdr_format.map(str::to_owned),
        audio: pq.audio_codec.map(str::to_owned),
        group: pq.release_group.map(str::to_owned),
        imdb_id,
        tmdb_id: Some(show_tmdb_id),
        container: container.to_owned(),
    };
    Ok((show_title, ctx))
}

/// Place `source_file` at `dest_path`, creating parent directories
/// and picking hardlink vs copy per config. Returns nothing on
/// success; errors bubble up so the caller reverts the download
/// state.
///
/// Behaviour when `dest_path` already exists:
/// * If it points to the **same file** as `source_file` (same dev +
///   inode — meaning a previous import already hardlinked here),
///   idempotent: leave it and return.
/// * Otherwise it's a collision — a redownload, an upgrade, or two
///   releases that resolved to the same templated path. The new file
///   is written to a sibling staging path and renamed atomically over
///   the existing destination so the swap is crash-safe and playback
///   only ever sees a complete file.
///
/// Runs the actual filesystem work on `spawn_blocking` — the cross-FS
/// fallback path can copy multi-GB media, which would otherwise pin
/// a tokio worker.
pub(crate) async fn materialise_into_library(
    source_file: &Path,
    dest_path: &Path,
    use_hardlinks: bool,
) -> anyhow::Result<()> {
    let source = source_file.to_owned();
    let dest = dest_path.to_owned();
    tokio::task::spawn_blocking(move || materialise_blocking(&source, &dest, use_hardlinks))
        .await
        .map_err(|e| anyhow::anyhow!("materialise task join: {e}"))?
}

/// Compare `a` and `b` as filesystem entries. Returns `true` when
/// they're the same physical file (same device + inode), which is
/// what a hardlinked re-import looks like. Falls back to `false` on
/// any metadata error since "we couldn't tell" means we shouldn't
/// silently skip — the caller will replace via the staged-rename
/// path.
#[cfg(unix)]
fn paths_point_to_same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(am) = std::fs::metadata(a) else {
        return false;
    };
    let Ok(bm) = std::fs::metadata(b) else {
        return false;
    };
    am.dev() == bm.dev() && am.ino() == bm.ino()
}

#[cfg(not(unix))]
fn paths_point_to_same_file(_a: &Path, _b: &Path) -> bool {
    false
}

fn materialise_blocking(
    source_file: &Path,
    dest_path: &Path,
    use_hardlinks: bool,
) -> anyhow::Result<()> {
    if dest_path.exists() {
        if paths_point_to_same_file(source_file, dest_path) {
            tracing::debug!(
                dst = %dest_path.display(),
                "library destination is already the source file (hardlink); skipping",
            );
            return Ok(());
        }
        tracing::warn!(
            src = %source_file.display(),
            dst = %dest_path.display(),
            "library destination exists and differs from source — replacing atomically",
        );
    }
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Stage as a sibling so the atomic rename stays on the same
    // filesystem. The PID + nanos suffix keeps two concurrent
    // materialises (different downloads racing onto the same templated
    // dest) from clobbering each other's staging file.
    let staging = staging_path(dest_path);
    let _ = std::fs::remove_file(&staging); // best-effort cleanup of any prior crash
    if use_hardlinks {
        match std::fs::hard_link(source_file, &staging) {
            Ok(()) => {
                std::fs::rename(&staging, dest_path)?;
                tracing::info!(
                    src = %source_file.display(),
                    dst = %dest_path.display(),
                    "hardlinked to library"
                );
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(
                    src = %source_file.display(),
                    dst = %dest_path.display(),
                    error = %e,
                    "hardlink failed — falling back to copy"
                );
                let _ = std::fs::remove_file(&staging);
            }
        }
    }
    if let Err(e) = std::fs::copy(source_file, &staging) {
        let _ = std::fs::remove_file(&staging);
        return Err(e.into());
    }
    if let Err(e) = std::fs::rename(&staging, dest_path) {
        let _ = std::fs::remove_file(&staging);
        return Err(e.into());
    }
    tracing::info!(src = %source_file.display(), dst = %dest_path.display(), "copied to library");
    Ok(())
}

/// Sibling staging path for an atomic rename. Carries the PID + nanos
/// so concurrent materialises onto the same templated dest don't
/// share a staging file.
fn staging_path(dest_path: &Path) -> PathBuf {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let suffix = format!(".kino-staging-{pid}-{nanos}");
    let mut name = dest_path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(suffix);
    dest_path.with_file_name(name)
}

/// Import a completed download — move file to library, create Media entity.
/// If import fails at any point, the download is reverted to "failed" state.
///
/// `torrent` + `torrent_hash`, when present, are used to ask librqbit
/// for the *specific* files owned by this download — a hard guarantee
/// that we don't accidentally import an orphan file from a previous
/// download sitting in the same base directory (which was a real bug
/// before: a 14 GB movie at the download-root would shadow every
/// subsequent torrent's smaller completed episode).
#[tracing::instrument(skip(pool, event_tx, torrent), fields(download_id))]
pub async fn import_download(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    torrent: Option<&dyn TorrentSession>,
    torrent_hash: Option<&str>,
    download_id: i64,
    ffprobe_path: &str,
) -> anyhow::Result<()> {
    // Atomic claim: only proceed if *this* call transitioned the
    // state. Two monitor ticks can land on the same completed row
    // (scheduler + on-complete callback, or two scheduler passes
    // racing); without this guard both fire do_import, both try to
    // create media_episode rows, and the second one silently fails
    // halfway — leaving the download state inconsistent.
    let claimed = sqlx::query("UPDATE download SET state = ? WHERE id = ? AND state = ?")
        .bind(DownloadPhase::Importing)
        .bind(download_id)
        .bind(DownloadPhase::Completed)
        .execute(pool)
        .await?
        .rows_affected();
    if claimed == 0 {
        tracing::debug!(
            download_id,
            "import already claimed by another ticker; skipping"
        );
        return Ok(());
    }

    match crate::import::single::do_import(
        pool,
        event_tx,
        torrent,
        torrent_hash,
        download_id,
        ffprobe_path,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::error!(download_id, error = %e, "import failed, reverting to failed state");
            // Stop librqbit seeding this torrent: import failed so we
            // shouldn't be announcing/uploading stale data that's about
            // to be retried via a different release. Pass
            // `delete_files = false` — the file on disk is left for the
            // user to inspect/recover if the failure was transient
            // (disk full, permission). Non-fatal if remove errors; the
            // torrent may already be gone from the session.
            if let (Some(client), Some(hash)) = (torrent, torrent_hash) {
                let outcome = crate::cleanup::CleanupTracker::new(pool.clone())
                    .try_remove(crate::cleanup::ResourceKind::Torrent, hash, || async {
                        client.remove(hash, false).await
                    })
                    .await?;
                if !outcome.is_removed() {
                    tracing::warn!(
                        download_id,
                        torrent_hash = %hash,
                        ?outcome,
                        "torrent removal queued for retry after import failure",
                    );
                }
            }
            let _ = sqlx::query("UPDATE download SET state = ?, error_message = ? WHERE id = ?")
                .bind(DownloadPhase::Failed)
                .bind(format!("Import failed: {e}"))
                .bind(download_id)
                .execute(pool)
                .await;
            let _ = event_tx.send(AppEvent::DownloadFailed {
                download_id,
                title: String::new(),
                error: format!("Import failed: {e}"),
            });
            Err(e)
        }
    }
}

/// (resolution, source, `video_codec`, `audio_codec`, `hdr_format`, `release_group`)
pub(crate) type ReleaseQuality = (
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

const MIN_PACK_FILE_BYTES: u64 = 50 * 1024 * 1024;

fn walk_video_files(dir: &Path, out: &mut Vec<PathBuf>, min: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_video_files(&p, out, min);
        } else if p.is_file()
            && let Some(ext) = p.extension().and_then(|e| e.to_str())
            && MEDIA_EXTENSIONS.contains(&ext.to_lowercase().as_str())
            && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) >= min
        {
            out.push(p);
        }
    }
}

pub(crate) fn collect_video_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_video_files(dir, &mut out, MIN_PACK_FILE_BYTES);
    out
}

/// Find the downloaded file for a completed torrent. Safe against
/// cross-torrent contamination: the search is scoped to the specific
/// entry that librqbit (or the caller's stored title) says belongs to
/// this torrent — never a blind "largest video file in the base dir."
///
/// `torrent_name` is librqbit's authoritative `info.name`, which is
/// the actual subdir / file name on disk. `title` is our stored
/// release title, used as a fallback when librqbit isn't available.
///
/// `extracted_dir` (when set) is the `.extracted` sibling that
/// `archive::extract_all` writes into. It's checked **first** because
/// archived single-file releases ship the video buried inside the
/// archive — without this branch the importer would fail with "no
/// media file found" even though the extracted file is on disk. The
/// search inside the extracted dir is recursive (rar/zip can produce
/// nested structure) and picks the largest video file.
pub(crate) fn find_media_file(
    download_dir: &str,
    title: &str,
    torrent_name: Option<&str>,
    extracted_dir: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let download_path = Path::new(download_dir);

    // Extracted-archive output wins over the torrent payload itself —
    // when both exist, the extracted file is what the user actually
    // wants to play (the on-disk archive bytes aren't watchable).
    if let Some(dir) = extracted_dir
        && dir.is_dir()
        && let Some((f, size, considered)) = find_largest_media_recursive_logged(dir)
    {
        tracing::debug!(
            source = "extracted-dir",
            dir = %dir.display(),
            picked = %f.display(),
            picked_size = size,
            considered,
            "file-pick resolved",
        );
        return Ok(f);
    }

    // Try each candidate name in order. The torrent's actual `info.name`
    // wins when available — our stored `title` often differs (e.g.
    // "www.UIndex.org - ..." prefixes).
    let candidates: Vec<&str> = torrent_name
        .into_iter()
        .chain(std::iter::once(title))
        .collect();

    for name in &candidates {
        let path = download_path.join(name);
        if path.is_file() {
            tracing::debug!(
                source = "candidate-file",
                candidate = name,
                path = %path.display(),
                "file-pick resolved",
            );
            return Ok(path);
        }
        if path.is_dir() {
            if let Some((f, size, considered)) = find_largest_media_in_dir_logged(&path) {
                tracing::debug!(
                    source = "largest-in-dir",
                    candidate = name,
                    dir = %path.display(),
                    picked = %f.display(),
                    picked_size = size,
                    considered,
                    "file-pick resolved",
                );
                return Ok(f);
            }
            tracing::debug!(
                candidate = name,
                dir = %path.display(),
                "file-pick: candidate dir contained no media — trying next candidate",
            );
        }
    }

    tracing::warn!(
        download_dir,
        title,
        ?torrent_name,
        ?extracted_dir,
        candidates = ?candidates,
        "file-pick FAILED — no media file under any candidate",
    );
    anyhow::bail!(
        "no media file found for '{title}' (torrent_name {torrent_name:?}) in {download_dir}"
    )
}

/// Recursive variant of `find_largest_media_in_dir_logged`. The
/// extracted-archive search has to walk into nested subdirs (rar
/// volumes for episode packs sometimes write `Extras/` and the like),
/// not just the top of `.extracted/`.
fn find_largest_media_recursive_logged(dir: &Path) -> Option<(PathBuf, u64, usize)> {
    let mut files: Vec<PathBuf> = Vec::new();
    walk_video_files(dir, &mut files, 0);
    if files.is_empty() {
        return None;
    }
    let considered = files.len();
    let mut best: Option<(PathBuf, u64)> = None;
    for path in files {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if best.as_ref().is_none_or(|(_, s)| size > *s) {
            best = Some((path, size));
        }
    }
    best.map(|(p, s)| (p, s, considered))
}

const MEDIA_EXTENSIONS: &[&str] = &["mkv", "mp4", "avi", "m4v", "webm", "ts", "wmv"];

/// Return the largest-by-bytes media file under `dir`, plus its size
/// and the total number of media candidates considered. The candidate
/// count is used by the caller to log "picked 1 of N" so a silent
/// best-pick in a pack is inspectable after the fact.
fn find_largest_media_in_dir_logged(dir: &Path) -> Option<(PathBuf, u64, usize)> {
    let mut best: Option<(PathBuf, u64)> = None;
    let mut considered: usize = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && let Some(ext) = path.extension().and_then(|e| e.to_str())
                && MEDIA_EXTENSIONS.contains(&ext.to_lowercase().as_str())
            {
                considered += 1;
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if best.as_ref().is_none_or(|(_, s)| size > *s) {
                    best = Some((path, size));
                }
            }
        }
    }
    best.map(|(p, s)| (p, s, considered))
}

/// Look up `OpenSubtitles` creds and the movie's `imdb_id`, then fetch + save
/// one subtitle per accepted language next to the media file. Fully
/// best-effort: any error is logged and swallowed.
pub(crate) async fn fetch_subtitles_best_effort(
    pool: &SqlitePool,
    movie_id: i64,
    media_path: &Path,
) {
    use crate::integrations::opensubtitles::{OpenSubtitlesClient, OsCredentials};

    let creds: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT opensubtitles_api_key, opensubtitles_username, opensubtitles_password FROM config WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some((Some(api_key), Some(user), Some(pass))) = creds else {
        return;
    };
    if api_key.is_empty() || user.is_empty() || pass.is_empty() {
        return;
    }

    let imdb: Option<Option<String>> = sqlx::query_scalar("SELECT imdb_id FROM movie WHERE id = ?")
        .bind(movie_id)
        .fetch_optional(pool)
        .await
        .ok();
    let Some(Some(imdb_id)) = imdb else {
        return;
    };

    // Use the movie's quality profile to decide target languages.
    let langs: Option<String> = sqlx::query_scalar(
        "SELECT qp.accepted_languages FROM movie m
         JOIN quality_profile qp ON qp.id = m.quality_profile_id
         WHERE m.id = ?",
    )
    .bind(movie_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let languages: Vec<String> = langs
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_else(|| vec!["en".into()]);
    let langs_csv = languages.join(",");

    let client = OpenSubtitlesClient::new(OsCredentials {
        api_key,
        username: user,
        password: pass,
    });

    let hits = match client.search_by_imdb(&imdb_id, &langs_csv).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(movie_id, error = %e, "opensubtitles search failed");
            return;
        }
    };

    // Pick one subtitle per requested language (first match wins).
    let mut saved = 0;
    for lang in &languages {
        let Some(hit) = hits.iter().find(|h| h.language.eq_ignore_ascii_case(lang)) else {
            continue;
        };
        match client.download_to(hit, media_path).await {
            Ok(path) => {
                tracing::info!(lang = %lang, path = %path.display(), "fetched subtitle");
                saved += 1;
            }
            Err(e) => {
                tracing::warn!(lang = %lang, error = %e, "opensubtitles download failed");
            }
        }
    }
    if saved > 0 {
        tracing::info!(movie_id, saved, "fetched subtitles from OpenSubtitles");
    }
}

#[cfg(test)]
mod materialise_tests {
    use super::{materialise_blocking, paths_point_to_same_file, staging_path};
    use std::fs;

    fn write_file(path: &std::path::Path, bytes: u8) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![bytes; 16]).unwrap();
    }

    #[test]
    fn replaces_existing_dest_with_different_content() {
        // Bug #16: when `dest_path` already existed, the importer
        // returned Ok without copying — so a redownload / upgrade /
        // template-collision left the OLD bytes in place and Kino
        // reported the import as successful.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("new.mkv");
        let dst = tmp.path().join("library").join("movie.mkv");
        write_file(&src, 0xBB);
        write_file(&dst, 0xAA);

        materialise_blocking(&src, &dst, false).expect("materialise replaces");
        let after = fs::read(&dst).unwrap();
        assert_eq!(
            after,
            vec![0xBB; 16],
            "destination bytes replaced with the new file"
        );
    }

    #[test]
    fn idempotent_when_dest_is_existing_hardlink_to_source() {
        // Re-running materialise on a destination that's *already* a
        // hardlink to the source must skip — that's the normal
        // re-import path, and replacing it would be wasted I/O.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.mkv");
        let dst = tmp.path().join("dst.mkv");
        write_file(&src, 0xCC);
        std::fs::hard_link(&src, &dst).unwrap();

        let before_meta = fs::metadata(&dst).unwrap();
        materialise_blocking(&src, &dst, true).expect("idempotent skip");
        let after_meta = fs::metadata(&dst).unwrap();

        // Same dev+inode after — no replacement happened.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(before_meta.ino(), after_meta.ino());
            assert_eq!(before_meta.dev(), after_meta.dev());
        }
        #[cfg(not(unix))]
        let _ = (before_meta, after_meta);
    }

    #[test]
    fn copies_when_dest_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.mkv");
        let dst = tmp.path().join("library").join("movie.mkv");
        write_file(&src, 0xDD);

        materialise_blocking(&src, &dst, false).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), vec![0xDD; 16]);
    }

    #[test]
    fn paths_point_to_same_file_detects_hardlink() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let c = tmp.path().join("c");
        write_file(&a, 1);
        std::fs::hard_link(&a, &b).unwrap();
        write_file(&c, 1);

        // a and b share an inode (hardlink); a and c don't.
        #[cfg(unix)]
        {
            assert!(paths_point_to_same_file(&a, &b));
            assert!(!paths_point_to_same_file(&a, &c));
        }
        // Non-unix builds: helper returns false unconditionally; the
        // collision branch will then replace, which is also safe.
        #[cfg(not(unix))]
        assert!(!paths_point_to_same_file(&a, &b));
    }

    #[test]
    fn staging_path_is_a_sibling() {
        let dst = std::path::Path::new("/lib/Movies/Some Movie.mkv");
        let staging = staging_path(dst);
        assert_eq!(staging.parent(), dst.parent());
        assert!(
            staging
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Some Movie.mkv.kino-staging-")
        );
    }
}

#[cfg(test)]
mod find_media_file_tests {
    use super::find_media_file;
    use std::fs;

    fn write_file(path: &std::path::Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn picks_extracted_video_over_missing_torrent_file() {
        // Bug #15: archived single-file releases extracted into
        // `<torrent>/.extracted/<file>.mkv` weren't found because the
        // file-pick walked only the top of the candidate dir. Without
        // this branch the importer failed with "no media file found"
        // even though the watchable file was on disk.
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path();
        let torrent_dir = dl.join("Some.Movie.2024.1080p.WEB-DL");
        let extracted = torrent_dir.join(".extracted");
        // Only the extracted video exists — the candidate dir is empty
        // because the archive itself was the payload.
        fs::create_dir_all(&extracted).unwrap();
        write_file(&extracted.join("Some.Movie.2024.1080p.WEB-DL.mkv"), 4096);

        let picked = find_media_file(
            dl.to_str().unwrap(),
            "Some.Movie.2024.1080p.WEB-DL",
            Some("Some.Movie.2024.1080p.WEB-DL"),
            Some(&extracted),
        )
        .expect("picks the extracted file");
        assert!(
            picked.starts_with(&extracted),
            "picked from extracted dir: {}",
            picked.display()
        );
    }

    #[test]
    fn extracted_dir_walks_recursively() {
        // RAR volumes for series sometimes nest the video one level
        // deep (e.g. `.extracted/Some.Show/episode.mkv`). The recursive
        // walk has to find it, otherwise the importer falls back to
        // the candidate dir and misses the file.
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path();
        let extracted = dl.join("torrent").join(".extracted");
        write_file(&extracted.join("nested").join("ep01.mkv"), 8192);

        let picked = find_media_file(
            dl.to_str().unwrap(),
            "torrent",
            Some("torrent"),
            Some(&extracted),
        )
        .expect("recursive walk finds nested file");
        assert!(
            picked.ends_with("nested/ep01.mkv"),
            "picked nested file: {}",
            picked.display()
        );
    }

    #[test]
    fn extracted_dir_picks_largest_when_multiple() {
        // Multi-file archives (e.g. movie + extras) — pick the largest
        // video so the import lands the feature, not the bonus content.
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path();
        let extracted = dl.join("torrent").join(".extracted");
        write_file(&extracted.join("featurette.mkv"), 1024);
        write_file(&extracted.join("movie.mkv"), 1024 * 1024);
        write_file(&extracted.join("trailer.mkv"), 8192);

        let picked = find_media_file(
            dl.to_str().unwrap(),
            "torrent",
            Some("torrent"),
            Some(&extracted),
        )
        .unwrap();
        assert!(
            picked.ends_with("movie.mkv"),
            "picked largest: {}",
            picked.display()
        );
    }

    #[test]
    fn falls_back_to_candidate_dir_when_no_extraction() {
        // The classic non-archived path still works: no extracted_dir,
        // file lives directly in the torrent's candidate dir.
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path();
        let torrent_dir = dl.join("Some.Show.S01E01");
        write_file(&torrent_dir.join("show.s01e01.mkv"), 4096);

        let picked = find_media_file(
            dl.to_str().unwrap(),
            "Some.Show.S01E01",
            Some("Some.Show.S01E01"),
            None,
        )
        .unwrap();
        assert!(
            picked.ends_with("show.s01e01.mkv"),
            "candidate-dir fallback works: {}",
            picked.display()
        );
    }

    #[test]
    fn empty_extracted_dir_falls_through_to_candidate() {
        // Extraction failure / empty .extracted/ shouldn't poison the
        // pick — fall through to the candidate-dir search.
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path();
        let torrent_dir = dl.join("torrent");
        let extracted = torrent_dir.join(".extracted");
        fs::create_dir_all(&extracted).unwrap();
        write_file(&torrent_dir.join("movie.mkv"), 4096);

        let picked = find_media_file(
            dl.to_str().unwrap(),
            "torrent",
            Some("torrent"),
            Some(&extracted),
        )
        .unwrap();
        assert!(
            picked.ends_with("movie.mkv") && !picked.starts_with(&extracted),
            "fell back to candidate dir: {}",
            picked.display()
        );
    }
}
