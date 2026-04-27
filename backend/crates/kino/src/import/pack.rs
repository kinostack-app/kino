//! Season-pack import — fired when `import_download` sees the
//! download's `linked_episodes` set has more than one entry. Walks the
//! torrent's video files, parses `SxxExx` from each name, matches each
//! file to the right episode, and creates `media` + `media_episode`
//! rows per match. Unmatched files and unmatched linked episodes are
//! both non-fatal — files outside the pack are skipped, episodes the
//! pack didn't ship stay wanted for an individual grab.

use std::path::Path;

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::download::DownloadPhase;
use crate::events::AppEvent;
use crate::import::naming;
use crate::import::trigger::{
    ParsedQuality, ReleaseQuality, collect_video_files, episode_naming_context,
    load_naming_formats, materialise_into_library,
};

/// Import a season-pack download: enumerate the torrent's video files,
/// parse `SxxExx` from each filename, and match to the pre-linked
/// episode rows. One media + `media_episode` row per matched file.
/// Unmatched files and unmatched linked episodes are logged but
/// non-fatal.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn do_pack_import(
    pool: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
    download_id: i64,
    download_title: &str,
    release_id: Option<i64>,
    release_quality: ReleaseQuality,
    linked_episodes: &[i64],
    download_path: &str,
    library_path: &str,
    use_hardlinks: bool,
    torrent_name: Option<&str>,
    now: &str,
    ffprobe_path: &str,
) -> anyhow::Result<()> {
    let (resolution, source, video_codec, audio_codec, hdr_format, release_group) = release_quality;

    // Scope the file walk to this torrent's own subdirectory so we
    // don't accidentally scoop in files from other torrents sitting
    // in the shared download root.
    let dir = torrent_name.map_or_else(
        || Path::new(download_path).join(download_title),
        |n| Path::new(download_path).join(n),
    );
    if !dir.is_dir() {
        anyhow::bail!(
            "pack import: expected torrent subdirectory at {} (torrent_name={:?})",
            dir.display(),
            torrent_name
        );
    }
    let files = collect_video_files(&dir);
    if files.is_empty() {
        anyhow::bail!("pack import: no video files in {}", dir.display());
    }

    // Pull (episode_id, season_number, episode_number) for every
    // linked episode so we can match files to episode rows by the
    // SxxExx tag parsed from filenames.
    let placeholders = std::iter::repeat_n("?", linked_episodes.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, season_number, episode_number FROM episode WHERE id IN ({placeholders})"
    );
    let mut q = sqlx::query_as::<_, (i64, i64, i64)>(&sql);
    for id in linked_episodes {
        q = q.bind(id);
    }
    let ep_rows: Vec<(i64, i64, i64)> = q.fetch_all(pool).await?;

    let library_dir = Path::new(library_path);
    std::fs::create_dir_all(library_dir)?;
    let formats = load_naming_formats(pool).await?;

    let mut imported_any = false;
    let mut matched_episode_ids = std::collections::HashSet::<i64>::new();
    for file in files {
        let Some(file_name_str) = file.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let parsed = crate::parser::parse(file_name_str);
        let (Some(parsed_s), Some(&parsed_e)) = (parsed.season, parsed.episodes.first()) else {
            tracing::debug!(
                file = %file.display(),
                "pack import: skipping file with no SxxExx tag",
            );
            continue;
        };
        let matched_ep = ep_rows
            .iter()
            .find(|(_, s, e)| *s == i64::from(parsed_s) && *e == i64::from(parsed_e));
        let Some(&(ep_id, _, _)) = matched_ep else {
            tracing::debug!(
                file = %file.display(),
                parsed_s, parsed_e,
                "pack import: file doesn't match any linked episode",
            );
            continue;
        };

        let file_size = std::fs::metadata(&file)?.len().cast_signed();
        let container = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mkv")
            .to_lowercase();

        // Templated dest — same helper as the single-file path, so
        // season-packs produce the same `TV/{show}/Season NN/...`
        // structure as individual episode grabs.
        let pq = ParsedQuality {
            resolution,
            source: source.as_deref(),
            video_codec: video_codec.as_deref(),
            audio_codec: audio_codec.as_deref(),
            hdr_format: hdr_format.as_deref(),
            release_group: release_group.as_deref(),
        };
        let (show_name, ctx) = episode_naming_context(pool, ep_id, &pq, &container).await?;
        let dest_path = naming::episode_path(
            library_dir,
            &formats.episode,
            &formats.season,
            &show_name,
            &ctx,
        );

        materialise_into_library(&file, &dest_path, use_hardlinks).await?;

        let file_path_str = dest_path.to_string_lossy().to_string();
        let relative_path = dest_path.strip_prefix(library_dir).map_or_else(
            |_| dest_path.to_string_lossy().to_string(),
            |p| p.to_string_lossy().to_string(),
        );

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
        let final_video_codec = probed_video_codec.or_else(|| video_codec.clone());
        let final_audio_codec = probed_audio_codec.or_else(|| audio_codec.clone());

        let existing_media: Option<i64> =
            sqlx::query_scalar("SELECT id FROM media WHERE file_path = ?")
                .bind(&file_path_str)
                .fetch_optional(pool)
                .await?;
        let media_id = if let Some(id) = existing_media {
            id
        } else {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO media (movie_id, file_path, relative_path, size, container, resolution, source, video_codec, audio_codec, hdr_format, release_group, scene_name, runtime_ticks, date_added) VALUES (NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
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
            .bind(download_title)
            .bind(probed_runtime_ticks)
            .bind(now)
            .fetch_one(pool)
            .await?
        };

        sqlx::query("INSERT OR IGNORE INTO media_episode (media_id, episode_id) VALUES (?, ?)")
            .bind(media_id)
            .bind(ep_id)
            .execute(pool)
            .await?;

        let quality = match (resolution, source.as_deref()) {
            (Some(r), Some(s)) => Some(format!("{s}-{r}p")),
            (Some(r), None) => Some(format!("{r}p")),
            _ => None,
        };
        // Look up the parent show for the hero toast's poster.
        let show_id: Option<i64> = sqlx::query_scalar("SELECT show_id FROM episode WHERE id = ?")
            .bind(ep_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
        // Pretty episode title for the toast / history row.
        let composed = crate::events::display::episode_display_title(pool, ep_id).await;
        let display_title = if composed.is_empty() {
            download_title.to_owned()
        } else {
            composed
        };
        let _ = event_tx.send(AppEvent::Imported {
            media_id,
            movie_id: None,
            episode_id: Some(ep_id),
            show_id,
            title: display_title,
            quality,
        });

        matched_episode_ids.insert(ep_id);
        imported_any = true;
        tracing::info!(
            download_id,
            ep_id,
            file = %dest_path.display(),
            "pack import: linked file to episode"
        );
    }

    if !imported_any {
        anyhow::bail!("pack import: no files matched any linked episode");
    }

    // Episodes we couldn't match stay monitored for a fresh grab.
    for (ep_id, _, _) in &ep_rows {
        if !matched_episode_ids.contains(ep_id) {
            tracing::warn!(
                download_id,
                ep_id,
                "pack import: episode had no matching file — stays wanted"
            );
        }
    }

    // Mark the whole download imported. `output_path` = the directory
    // so the UI/history has a link back to the torrent contents.
    let _ = release_id; // quality pulled above — release_id not needed here
    sqlx::query("UPDATE download SET state = ?, output_path = ? WHERE id = ?")
        .bind(DownloadPhase::Imported)
        .bind(dir.to_string_lossy().as_ref())
        .bind(download_id)
        .execute(pool)
        .await?;

    Ok(())
}
