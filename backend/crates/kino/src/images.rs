use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{GenericImageView, ImageReader};
use reqwest::Client;
use sha2::{Digest, Sha256};

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

/// Image cache manager.
#[derive(Debug, Clone)]
pub struct ImageCache {
    data_path: PathBuf,
    http: Client,
}

impl ImageCache {
    pub fn new(data_path: &str) -> Self {
        Self {
            data_path: PathBuf::from(data_path).join("images"),
            http: Client::new(),
        }
    }

    /// Get the path to an original image, downloading it if not cached.
    pub async fn get_original(
        &self,
        content_type: &str,
        tmdb_id: i64,
        image_type: &str,
        tmdb_path: Option<&str>,
    ) -> Result<Option<PathBuf>, ImageError> {
        let dir = self.originals_dir(content_type, tmdb_id);
        let file_path = dir.join(format!("{image_type}.jpg"));

        if file_path.exists() {
            return Ok(Some(file_path));
        }

        // Need to download from TMDB
        let Some(tmdb_path) = tmdb_path else {
            return Ok(None);
        };

        let url = format!("{TMDB_IMAGE_BASE}{tmdb_path}");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ImageError::Download(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ImageError::Download(format!(
                "TMDB returned {}",
                resp.status()
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ImageError::Download(e.to_string()))?;

        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| ImageError::Io(e.to_string()))?;
        tokio::fs::write(&file_path, &bytes)
            .await
            .map_err(|e| ImageError::Io(e.to_string()))?;

        Ok(Some(file_path))
    }

    /// Get a resized variant, generating it on demand from the original.
    pub async fn get_resized(
        &self,
        original_path: &Path,
        width: Option<u32>,
        height: Option<u32>,
        quality: Option<u8>,
    ) -> Result<PathBuf, ImageError> {
        let width = width.unwrap_or(0);
        let height = height.unwrap_or(0);
        let quality = quality.unwrap_or(85);

        // If no resize requested, return original
        if width == 0 && height == 0 {
            return Ok(original_path.to_owned());
        }

        // Cache key from params
        let cache_key = {
            let input = format!(
                "{}:{}:{}:{}",
                original_path.display(),
                width,
                height,
                quality
            );
            let hash = Sha256::digest(input.as_bytes());
            hex::encode(&hash[..16]) // 128-bit prefix is plenty
        };

        let resized_dir = self.data_path.join("resized");
        let resized_path = resized_dir.join(format!("{cache_key}.jpg"));

        if resized_path.exists() {
            return Ok(resized_path);
        }

        // Resize from original
        let original_path_owned = original_path.to_owned();
        let resized_path_clone = resized_path.clone();

        tokio::task::spawn_blocking(move || {
            let img = ImageReader::open(&original_path_owned)
                .map_err(|e| ImageError::Io(e.to_string()))?
                .decode()
                .map_err(|e| ImageError::Resize(e.to_string()))?;

            let (orig_w, orig_h) = (img.width(), img.height());

            // Don't upscale
            let target_w = if width > 0 && width < orig_w {
                width
            } else {
                orig_w
            };
            let target_h = if height > 0 && height < orig_h {
                height
            } else {
                orig_h
            };

            // If no actual resize needed, return original
            if target_w == orig_w && target_h == orig_h {
                return Err(ImageError::NoResize);
            }

            let resized = img.resize(target_w, target_h, FilterType::Lanczos3);

            std::fs::create_dir_all(
                resized_path_clone
                    .parent()
                    .expect("resized path has parent"),
            )
            .map_err(|e| ImageError::Io(e.to_string()))?;

            let mut output = std::io::BufWriter::new(
                std::fs::File::create(&resized_path_clone)
                    .map_err(|e| ImageError::Io(e.to_string()))?,
            );

            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality);
            resized
                .write_with_encoder(encoder)
                .map_err(|e| ImageError::Resize(e.to_string()))?;

            Ok(resized_path_clone)
        })
        .await
        .map_err(|e| ImageError::Resize(e.to_string()))?
        .or_else(|e| match e {
            ImageError::NoResize => Ok(original_path.to_owned()),
            other => Err(other),
        })
    }

    /// Compute a 4x3 `BlurHash` string from an image file. Returns None on
    /// decode errors. Runs on a blocking thread since image decode + resize
    /// is CPU-bound.
    pub async fn compute_blurhash(&self, path: &Path) -> Option<String> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            let img = ImageReader::open(&path).ok()?.decode().ok()?;
            // Downscale to a small size for fast encode — blurhash doesn't
            // need pixel-level detail. 64x48 is a good tradeoff.
            let small = img.resize(64, 48, FilterType::Triangle);
            let (w, h) = small.dimensions();
            let rgba = small.to_rgba8().into_raw();
            blurhash::encode(4, 3, w, h, &rgba).ok()
        })
        .await
        .ok()
        .flatten()
    }

    /// Delete all cached images for a content item.
    #[allow(dead_code)] // Used by cleanup subsystem (Phase 8)
    pub async fn delete_content(&self, content_type: &str, tmdb_id: i64) -> Result<(), ImageError> {
        let dir = self.originals_dir(content_type, tmdb_id);
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir)
                .await
                .map_err(|e| ImageError::Io(e.to_string()))?;
        }
        Ok(())
    }

    fn originals_dir(&self, content_type: &str, tmdb_id: i64) -> PathBuf {
        self.data_path
            .join("originals")
            .join(content_type)
            .join(tmdb_id.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("download failed: {0}")]
    Download(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("resize failed: {0}")]
    Resize(String),
    #[error("no resize needed")]
    NoResize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[tokio::test]
    async fn blurhash_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("red.jpg");
        let img = ImageBuffer::from_pixel(32, 32, Rgb::<u8>([200, 30, 30]));
        img.save(&path).unwrap();

        let cache = ImageCache::new(tmp.path().to_str().unwrap());
        let hash = cache
            .compute_blurhash(&path)
            .await
            .expect("expected a blurhash");
        // BlurHash first char encodes 4x3 components: (Y-1)*9 + (X-1) = 20 → 'L'.
        assert!(hash.starts_with('L'), "unexpected blurhash prefix: {hash}");
        // Length for 4x3 is 1 + 1 + 4 + (4*3 - 1)*2 = 28.
        assert_eq!(hash.len(), 28, "unexpected blurhash length: {hash}");
    }

    #[tokio::test]
    async fn blurhash_returns_none_on_bad_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = ImageCache::new(tmp.path().to_str().unwrap());
        let hash = cache
            .compute_blurhash(&tmp.path().join("does-not-exist.jpg"))
            .await;
        assert!(hash.is_none());
    }
}
