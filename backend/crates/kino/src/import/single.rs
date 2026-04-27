//! Single-file import — fired when `import_download` sees the
//! download's `linked_episodes` set has zero or one entry. Walks the
//! torrent's directory for the video file (or follows the
//! `.extracted/` sibling for archived releases), materialises into
//! the library, runs ffprobe + opensubtitles, and writes the media
//! row + `media_episode` link (for episodes) or `movie_id` (for movies).
//!
//! Sibling to `pack.rs`. Helpers used by both paths live in
//! `trigger.rs` as `pub(crate)`.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::download::{DownloadPhase, TorrentSession};
use crate::events::AppEvent;
use crate::import::naming;
use crate::import::trigger::{
    OldMediaRow, ParsedQuality, episode_naming_context, fetch_subtitles_best_effort,
    find_media_file, load_naming_formats, materialise_into_library, movie_naming_context,
};

#[allow(clippy::too_many_lines)]
pub(crate) async fn do_import(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    torrent: Option<&dyn TorrentSession>,
    torrent_hash: Option<&str>,
    download_id: i64,
    ffprobe_path: &str,
) -> anyhow::Result<()> {
    let now = crate::time::Timestamp::now().to_rfc3339();

    // Get download info
    let download: Option<(i64, String, Option<i64>)> =
        sqlx::query_as("SELECT id, title, release_id FROM download WHERE id = ?")
            .bind(download_id)
            .fetch_optional(pool)
            .await?;

    let Some((_id, title, release_id)) = download else {
        anyhow::bail!("download {download_id} not found");
    };

    // Get linked content
    let linked_movie: Option<i64> = sqlx::query_scalar(
        "SELECT movie_id FROM download_content WHERE download_id = ? AND movie_id IS NOT NULL LIMIT 1",
    )
    .bind(download_id)
    .fetch_optional(pool)
    .await?
    .flatten();

    // Collect every linked episode — one download may link many when
    // grabbed as a season pack (see `grab_episode_release`'s pack
    // multi-link branch). The old code only looked at LIMIT 1, which
    // silently imported one file for a 10-episode pack and left the
    // rest wanted, triggering duplicate pack grabs.
    let linked_episodes: Vec<i64> = sqlx::query_scalar(
        "SELECT episode_id FROM download_content
         WHERE download_id = ? AND episode_id IS NOT NULL",
    )
    .bind(download_id)
    .fetch_all(pool)
    .await?;

    if linked_movie.is_none() && linked_episodes.is_empty() {
        anyhow::bail!("download {download_id} has no linked movie or episode");
    }

    // Get quality info from release
    let (resolution, source, video_codec, audio_codec, hdr_format, release_group) = if let Some(
        rid,
    ) =
        release_id
    {
        sqlx::query_as::<_, (Option<i64>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>(
                "SELECT resolution, source, video_codec, audio_codec, hdr_format, release_group FROM release WHERE id = ?",
            )
            .bind(rid)
            .fetch_optional(pool)
            .await?
            .unwrap_or_default()
    } else {
        Default::default()
    };

    // Get config paths
    let (download_path, library_path, use_hardlinks): (String, String, bool) = sqlx::query_as(
        "SELECT download_path, media_library_path, use_hardlinks FROM config WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;

    // Find the actual downloaded file. Prefer librqbit's own
    // knowledge of the torrent's on-disk name (which is how it names
    // the subdir / file under the session's download folder) over
    // our stored `download.title` — those two often differ because
    // our title is the *release* title from the indexer, while
    // librqbit uses the torrent's `info.name` which frequently carries
    // site prefixes like "www.UIndex.org - ...".
    //
    // Without this, `find_media_file` fell back to "largest video file
    // anywhere in the base download dir", which is a cross-
    // contamination footgun: a 14 GB movie sitting at the root from a
    // previous download shadowed every subsequent smaller episode
    // import, linking the wrong file to the episode's media row.
    let torrent_name = torrent
        .zip(torrent_hash)
        .and_then(|(c, h)| c.torrent_name(h));

    // Scope archive extraction to this torrent's subdirectory, not
    // the full `download_path`. The old behaviour re-extracted stray
    // archives from every sibling download on every import —
    // harmless but wasteful and a real problem if two imports run
    // concurrently. Also track the extraction dir so we can clean it
    // up after the media file is materialised.
    let torrent_root: PathBuf = torrent_name.as_deref().map_or_else(
        || Path::new(&download_path).join(&title),
        |n| Path::new(&download_path).join(n),
    );
    let extraction_root: PathBuf = if torrent_root.is_dir() {
        torrent_root.clone()
    } else {
        Path::new(&download_path).to_path_buf()
    };
    let extracted_dir = match crate::import::archive::extract_all(&extraction_root).await {
        Ok(Some(dir)) => {
            tracing::info!(dir = %dir.display(), "extracted archives into");
            Some(dir)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(
                dir = %extraction_root.display(),
                error = %e,
                "archive extraction failed (continuing without it)"
            );
            None
        }
    };

    // Pack import: this download was grabbed as a season pack (see
    // grab_episode_release's multi-link branch). Walk the torrent's
    // on-disk directory, parse each video file's SxxExx tag, and
    // match it to its episode row — creating one media + media_episode
    // per matched file. An episode linked to the download but absent
    // from the pack (pack says S01 but only ships S01E01..09) stays
    // wanted for an individual grab later.
    if linked_episodes.len() > 1 {
        return crate::import::pack::do_pack_import(
            pool,
            event_tx,
            download_id,
            &title,
            release_id,
            (
                resolution,
                source.clone(),
                video_codec,
                audio_codec,
                hdr_format,
                release_group,
            ),
            &linked_episodes,
            &download_path,
            &library_path,
            use_hardlinks,
            torrent_name.as_deref(),
            &now,
            ffprobe_path,
        )
        .await;
    }

    let linked_episode = linked_episodes.first().copied();

    let source_file = find_media_file(
        &download_path,
        &title,
        torrent_name.as_deref(),
        extracted_dir.as_deref(),
    )?;
    let file_size = std::fs::metadata(&source_file)?.len().cast_signed();

    // Determine container from extension
    let extension = source_file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mkv");
    let container = extension.to_lowercase();

    // Templated library destination — replaces the old "dump files
    // flat under library_path" behaviour. Reads the user's naming
    // format config; falls back to the built-in defaults for missing
    // / empty rows (e.g. first boot before the wizard writes config).
    let formats = load_naming_formats(pool).await?;
    let library_dir = Path::new(&library_path);
    std::fs::create_dir_all(library_dir)?;
    let pq = ParsedQuality {
        resolution,
        source: source.as_deref(),
        video_codec: video_codec.as_deref(),
        audio_codec: audio_codec.as_deref(),
        hdr_format: hdr_format.as_deref(),
        release_group: release_group.as_deref(),
    };
    let dest_path: PathBuf = if let Some(ep_id) = linked_episode {
        let (show_name, ctx) = episode_naming_context(pool, ep_id, &pq, &container).await?;
        naming::episode_path(
            library_dir,
            &formats.episode,
            &formats.season,
            &show_name,
            &ctx,
        )
    } else if let Some(mid) = linked_movie {
        let ctx = movie_naming_context(pool, mid, &pq, &container).await?;
        naming::movie_path(library_dir, &formats.movie, &ctx)
    } else {
        // Shouldn't reach here — the early guard at line 131 bails
        // when both linked_movie and linked_episodes are empty — but
        // belt-and-braces: fall back to the source filename so we
        // don't panic on unexpected states.
        let name = source_file
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("no filename"))?;
        library_dir.join(name)
    };

    tracing::debug!(
        src = %source_file.display(),
        dst = %dest_path.display(),
        container,
        size = file_size,
        resolution = ?resolution,
        source = ?source,
        release_group = ?release_group,
        use_hardlinks,
        "import plan",
    );

    materialise_into_library(&source_file, &dest_path, use_hardlinks).await?;

    let file_path_str = dest_path.to_string_lossy().to_string();
    // relative_path is library-root-relative so the frontend /
    // playback layer can construct URLs without leaking the host's
    // absolute library root.
    let relative_path = dest_path.strip_prefix(library_dir).map_or_else(
        |_| dest_path.to_string_lossy().to_string(),
        |p| p.to_string_lossy().to_string(),
    );

    // Probe the file so the media row carries real codec + runtime
    // info. Parsed release-title codecs are a guess at best (they
    // miss AC3/DD entirely for some naming conventions — "DD 5 1"
    // never matched the parser's pattern), and release parsing has
    // no way to see the actual duration.
    let probed = tokio::task::spawn_blocking({
        let p = dest_path.clone();
        let pp = ffprobe_path.to_owned();
        move || {
            crate::import::ffprobe::probe(&p, &pp)
                .inspect_err(|e| {
                    tracing::warn!(
                        path = %p.display(),
                        ffprobe = %pp,
                        error = %e,
                        "ffprobe failed during import — media row will be missing stream metadata"
                    );
                })
                .ok()
        }
    })
    .await
    .ok()
    .flatten();
    let probed_video_codec = probed
        .as_ref()
        .and_then(|p| p.streams.as_ref())
        .and_then(|s| {
            s.iter()
                .find(|st| st.codec_type.as_deref() == Some("video"))
                .and_then(|st| st.codec_name.clone())
        });
    let probed_audio_codec = probed
        .as_ref()
        .and_then(|p| p.streams.as_ref())
        .and_then(|s| {
            s.iter()
                .find(|st| st.codec_type.as_deref() == Some("audio"))
                .and_then(|st| st.codec_name.clone())
        });

    // Audio-language verification. Compare every audio track's
    // language tag (ISO 639-2/3) against the quality profile's
    // `accepted_languages` (ISO 639-1). Untagged tracks are treated
    // as a match under the same "scene default = first preferred"
    // convention as the scorer. Logged at warn so operators can see
    // drift without spamming the broadcast; webhook / UI
    // surfacing belongs to subsystem #08 once health-event dedup
    // is in (captured in the pre-release tracker).
    check_audio_languages(pool, linked_movie, linked_episode, probed.as_ref(), &title).await;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let probed_runtime_ticks: Option<i64> = probed
        .as_ref()
        .and_then(|p| p.format.as_ref())
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .map(|secs| (secs * 10_000_000.0) as i64);
    // Prefer probed values over the release-parser's guesses — the
    // file on disk is the ground truth.
    let final_video_codec = probed_video_codec.or(video_codec);
    let final_audio_codec = probed_audio_codec.or(audio_codec);

    // Upgrade detection: if a previous media row already serves
    // this movie/episode but at a *different* file path, we've
    // upgraded. Capture the old row so we can delete its file +
    // fire an Upgraded event once the new row is in place. Distinct
    // from the dedup-by-path check below (which matches same path /
    // same file — e.g. re-importing the exact same grab). Struct
    // hoisted to module scope to keep clippy's items-after-
    // statements lint happy.
    let old_media: Option<OldMediaRow> = if let Some(mid) = linked_movie {
        sqlx::query_as(
            "SELECT id, file_path, resolution, source FROM media
             WHERE movie_id = ? AND file_path != ?
             ORDER BY date_added DESC LIMIT 1",
        )
        .bind(mid)
        .bind(&file_path_str)
        .fetch_optional(pool)
        .await?
    } else if let Some(ep_id) = linked_episode {
        sqlx::query_as(
            "SELECT m.id, m.file_path, m.resolution, m.source
             FROM media m JOIN media_episode me ON me.media_id = m.id
             WHERE me.episode_id = ? AND m.file_path != ?
             ORDER BY m.date_added DESC LIMIT 1",
        )
        .bind(ep_id)
        .bind(&file_path_str)
        .fetch_optional(pool)
        .await?
    } else {
        None
    };

    // Dedup guard: a prior import (or a re-follow after delete-then-
    // re-add) can land at the exact same on-disk path. Hardlinks
    // share inodes; a second media row would duplicate the library
    // card and confuse every aggregate query.
    let existing_media: Option<i64> =
        sqlx::query_scalar("SELECT id FROM media WHERE file_path = ?")
            .bind(&file_path_str)
            .fetch_optional(pool)
            .await?;

    let media_id = if let Some(id) = existing_media {
        tracing::info!(
            media_id = id,
            path = %file_path_str,
            "reusing existing media row (dedup)"
        );
        // Refresh the codec/runtime info in case an earlier import
        // missed them (e.g. ffprobe added later, or release title
        // had a sparse codec parse). Leaves movie_id alone since a
        // delete-then-re-add flow might have changed it.
        sqlx::query(
            "UPDATE media SET
               size = ?, container = ?, resolution = COALESCE(?, resolution),
               source = COALESCE(?, source),
               video_codec = COALESCE(?, video_codec),
               audio_codec = COALESCE(?, audio_codec),
               hdr_format = COALESCE(?, hdr_format),
               runtime_ticks = COALESCE(?, runtime_ticks)
             WHERE id = ?",
        )
        .bind(file_size)
        .bind(&container)
        .bind(resolution)
        .bind(source.as_deref())
        .bind(final_video_codec.as_deref())
        .bind(final_audio_codec.as_deref())
        .bind(hdr_format.as_deref())
        .bind(probed_runtime_ticks)
        .bind(id)
        .execute(pool)
        .await?;
        id
    } else {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO media (movie_id, file_path, relative_path, size, container, resolution, source, video_codec, audio_codec, hdr_format, release_group, scene_name, runtime_ticks, date_added) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(linked_movie)
        .bind(&file_path_str)
        .bind(&relative_path)
        .bind(file_size)
        .bind(&container)
        .bind(resolution)
        .bind(source.as_deref())
        .bind(final_video_codec.as_deref())
        .bind(final_audio_codec.as_deref())
        .bind(hdr_format.as_deref())
        .bind(release_group.as_deref())
        .bind(&title)
        .bind(probed_runtime_ticks)
        .bind(&now)
        .fetch_one(pool)
        .await?
    };

    // Link to episode if TV. INSERT OR IGNORE so re-imports of the
    // same file for the same episode don't fail with UNIQUE conflicts.
    if let Some(ep_id) = linked_episode {
        sqlx::query("INSERT OR IGNORE INTO media_episode (media_id, episode_id) VALUES (?, ?)")
            .bind(media_id)
            .bind(ep_id)
            .execute(pool)
            .await?;
    }

    // Persist the ffprobe result into `stream` + `chapter`.
    // The decision engine + playback picker read these rows to
    // choose direct-play / remux / transcode + to build the
    // track lists the frontend shows. Without them a
    // multi-audio EAC-3 MKV would direct-play silently on a
    // browser that can't decode the primary track. Greppable
    // summary at the end so "streams=0 audio=0" jumps out in
    // logs when something's wrong with the probe.
    let (mut stream_count, mut audio_count, mut chapter_count) = (0_usize, 0_usize, 0_usize);
    if let Some(probe) = probed.as_ref() {
        if let Some(streams) = probe.streams.as_ref() {
            for s in streams {
                if s.codec_type.as_deref() == Some("audio") {
                    audio_count += 1;
                }
                match crate::import::pipeline::create_stream_entity(pool, media_id, s).await {
                    Ok(()) => stream_count += 1,
                    Err(e) => tracing::warn!(
                        media_id,
                        stream_index = s.index,
                        codec = ?s.codec_name,
                        codec_type = ?s.codec_type,
                        error = %e,
                        "import_trigger: failed to persist stream row",
                    ),
                }
            }
        }
        if let Some(chapters) = probe.chapters.as_ref() {
            let mut parsed: Vec<(f64, Option<f64>, Option<String>)> = chapters
                .iter()
                .filter_map(|c| {
                    let start = c.start_time.as_deref()?.parse::<f64>().ok()?;
                    let end = c.end_time.as_deref().and_then(|s| s.parse::<f64>().ok());
                    let title = c.tags.as_ref().and_then(|t| t.title.clone());
                    Some((start, end, title))
                })
                .collect();
            parsed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (idx, (start, end, title)) in parsed.into_iter().enumerate() {
                match sqlx::query(
                    "INSERT INTO chapter (media_id, idx, start_secs, end_secs, title) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(media_id)
                .bind(i64::try_from(idx).unwrap_or(i64::MAX))
                .bind(start)
                .bind(end)
                .bind(title)
                .execute(pool)
                .await
                {
                    Ok(_) => chapter_count += 1,
                    Err(e) => tracing::warn!(
                        media_id,
                        idx,
                        error = %e,
                        "import_trigger: chapter insert failed",
                    ),
                }
            }
        }
    }
    tracing::info!(
        media_id,
        streams = stream_count,
        audio_tracks = audio_count,
        chapters = chapter_count,
        probe_present = probed.is_some(),
        "import_trigger: probe persistence summary",
    );

    // Status 'available' now derives from the media row created
    // above — no explicit `UPDATE SET status` needed.
    if let Some(mid) = linked_movie {
        // Best-effort subtitle fetch from OpenSubtitles. Requires creds +
        // imdb_id; silent if either is missing. Errors never fail import.
        fetch_subtitles_best_effort(pool, mid, &dest_path).await;
    }

    // Mark download as imported
    sqlx::query("UPDATE download SET state = ?, output_path = ? WHERE id = ?")
        .bind(DownloadPhase::Imported)
        .bind(&file_path_str)
        .bind(download_id)
        .execute(pool)
        .await?;

    // Emit event
    let quality = match (resolution, source.as_deref()) {
        (Some(r), Some(s)) => Some(format!("{s}-{r}p")),
        (Some(r), None) => Some(format!("{r}p")),
        _ => None,
    };
    // Parent show for episode imports — the hero toast uses this to
    // render the show's poster. One lookup; silent on failure.
    let show_id: Option<i64> = if let Some(ep_id) = linked_episode {
        sqlx::query_scalar("SELECT show_id FROM episode WHERE id = ?")
            .bind(ep_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    // User-facing title — compose "Show · SxxExx · Title" for
    // episodes so toasts / history / notifications don't show the
    // raw release filename. Falls back to the release title for
    // movies or when the episode row is gone.
    let display_title = if let Some(ep_id) = linked_episode {
        let composed = crate::events::display::episode_display_title(pool, ep_id).await;
        if composed.is_empty() { title } else { composed }
    } else {
        title
    };

    // Upgrade-replace: if a previous media row existed for this
    // movie/episode at a different file path, this import is the
    // upgrade target. Delete the old file + its sidecars + its
    // media row (cascades streams + media_episode), then fire the
    // `Upgraded` event instead of `Imported` so webhooks /
    // notifications can distinguish. Errors deleting files are
    // logged but not fatal — the media row update succeeds either
    // way and the user can manually clean up.
    let is_upgrade = old_media.as_ref().is_some_and(|o| o.id != media_id);
    if is_upgrade {
        let old = old_media.as_ref().expect("is_upgrade implies Some");
        let old_quality = match (old.resolution, old.source.as_deref()) {
            (Some(r), Some(s)) => Some(format!("{s}-{r}p")),
            (Some(r), None) => Some(format!("{r}p")),
            _ => None,
        };
        remove_media_artifacts_best_effort(&old.file_path);
        if let Err(e) = sqlx::query("DELETE FROM media WHERE id = ?")
            .bind(old.id)
            .execute(pool)
            .await
        {
            tracing::warn!(
                old_media_id = old.id,
                error = %e,
                "upgrade: failed to delete old media row; new row inserted anyway"
            );
        }
        tracing::info!(
            media_id,
            old_media_id = old.id,
            old_quality = ?old_quality,
            new_quality = ?quality,
            "upgrade replaced previous media"
        );
        let _ = event_tx.send(AppEvent::Upgraded {
            media_id,
            movie_id: linked_movie,
            title: display_title,
            old_quality,
            new_quality: quality,
        });
    } else {
        let _ = event_tx.send(AppEvent::Imported {
            media_id,
            movie_id: linked_movie,
            episode_id: linked_episode,
            show_id,
            title: display_title,
            quality,
        });
    }

    tracing::info!(
        download_id,
        media_id,
        file_path = %file_path_str,
        size = file_size,
        is_upgrade,
        "import completed"
    );

    // Clean up the extraction sibling-dir so archives don't leave
    // junk behind on every import. Best-effort — the source torrent
    // is owned by librqbit / `check_seed_limits`, so removing the
    // `.extracted/` folder is safe regardless of hardlink strategy.
    if let Some(dir) = extracted_dir
        && let Err(e) = tokio::fs::remove_dir_all(&dir).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            dir = %dir.display(),
            error = %e,
            "failed to clean up .extracted dir (non-fatal)"
        );
    }

    Ok(())
}

/// Compare the imported file's audio-track language tags against
/// the applicable `QualityProfile.accepted_languages`. Warns when
/// none of the tracks match so a mistagged foreign release doesn't
/// silently ship. No-op when:
/// - the probe failed (nothing to verify)
/// - no quality profile is linked (movie/episode without one)
/// - the accept list is empty (user opted out of language filtering)
/// - any audio track has a matching ISO 639-1 code
/// - any audio track has NO language tag (scene convention: untagged = first preferred)
async fn check_audio_languages(
    pool: &SqlitePool,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
    probed: Option<&crate::import::ffprobe::ProbeResult>,
    title: &str,
) {
    let Some(probed) = probed else {
        return;
    };
    let Some(streams) = probed.streams.as_deref() else {
        return;
    };
    let audio_langs: Vec<Option<String>> = streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .map(|s| {
            s.tags
                .as_ref()
                .and_then(|t| t.language.as_deref())
                .map(|l| l.trim().to_ascii_lowercase())
                // Normalise common 3-letter codes to 2-letter so
                // comparisons against the profile (ISO 639-1) work.
                // Incomplete mapping — the main "do we have audio
                // in a language we accept" signal is fine with the
                // dozen most common codes.
                .map(|l| match l.as_str() {
                    "eng" => "en".into(),
                    "fra" | "fre" => "fr".into(),
                    "deu" | "ger" => "de".into(),
                    "spa" => "es".into(),
                    "ita" => "it".into(),
                    "por" => "pt".into(),
                    "rus" => "ru".into(),
                    "jpn" => "ja".into(),
                    "kor" => "ko".into(),
                    "zho" | "chi" => "zh".into(),
                    "nld" | "dut" => "nl".into(),
                    "swe" => "sv".into(),
                    "pol" => "pl".into(),
                    "tur" => "tr".into(),
                    "ara" => "ar".into(),
                    "hin" => "hi".into(),
                    _ => l,
                })
        })
        .collect();
    if audio_langs.is_empty() {
        return;
    }
    // Any untagged audio → treat as a match (scene convention).
    if audio_langs.iter().any(Option::is_none) {
        return;
    }

    let accepted_json: Option<String> = if let Some(mid) = movie_id {
        sqlx::query_scalar(
            "SELECT qp.accepted_languages FROM movie m
             JOIN quality_profile qp ON qp.id = m.quality_profile_id
             WHERE m.id = ?",
        )
        .bind(mid)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    } else if let Some(ep_id) = episode_id {
        sqlx::query_scalar(
            "SELECT qp.accepted_languages FROM episode e
             JOIN show s ON s.id = e.show_id
             JOIN quality_profile qp ON qp.id = s.quality_profile_id
             WHERE e.id = ?",
        )
        .bind(ep_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    } else {
        None
    };
    let Some(json) = accepted_json else {
        return;
    };
    let accepted: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
    if accepted.is_empty() {
        return;
    }
    let tagged: Vec<String> = audio_langs.into_iter().flatten().collect();
    let any_match = tagged.iter().any(|t| accepted.iter().any(|a| a == t));
    if !any_match {
        tracing::warn!(
            release = %title,
            audio_languages = ?tagged,
            accepted_languages = ?accepted,
            "imported release has no audio track matching the accepted languages — \
             mistagged foreign release? Scorer's release-title language filter passed \
             but the file's audio disagrees."
        );
    }
}

/// Best-effort delete of a library file plus common sidecars
/// (external SRT / VTT) that sit next to it. Used on upgrade to
/// evict the old quality from disk. Never returns an error: a
/// missing file is fine (user may have deleted it manually) and a
/// permission error is worth logging but not worth failing the
/// import that succeeded.
fn remove_media_artifacts_best_effort(file_path: &str) {
    let path = Path::new(file_path);
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %file_path, error = %e, "failed to delete old media file");
        }
    } else {
        tracing::debug!(path = %file_path, "deleted old media file on upgrade");
    }
    // Sidecars: same stem + `.srt` / `.vtt`. Best-effort only.
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        && let Some(parent) = path.parent()
    {
        for ext in ["srt", "vtt", "en.srt", "en.vtt"] {
            let sidecar = parent.join(format!("{stem}.{ext}"));
            if sidecar.exists()
                && let Err(e) = std::fs::remove_file(&sidecar)
            {
                tracing::debug!(
                    path = %sidecar.display(),
                    error = %e,
                    "sidecar delete failed (continuing)"
                );
            }
        }
    }
}
