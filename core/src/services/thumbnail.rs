//! Thumbnail generation cached under application data — never beside the
//! originals (TECHNICAL_SPEC §16). Unsupported formats get no thumbnail but
//! stay fully indexed with a clear status.

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ThumbService {
    pub cache_dir: PathBuf,
    max_edge: u32,
}

impl ThumbService {
    pub fn new(cache_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(cache_dir);
        Self {
            cache_dir: cache_dir.to_path_buf(),
            max_edge: 320,
        }
    }

    fn cache_path(&self, normalized_path: &str) -> PathBuf {
        let mut h = Sha256::new();
        h.update(normalized_path.as_bytes());
        let key: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        self.cache_dir.join(format!("{key}.jpg"))
    }

    pub fn thumbnail_for(&self, conn: &Connection, media_id: &str) -> Option<PathBuf> {
        let row: Option<(String, String, String)> = conn
            .query_row(
                "SELECT path, normalized_path, thumbnail_path FROM media_item WHERE id=?1",
                rusqlite::params![media_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                },
            )
            .ok();
        let (src, np, cached) = row?;
        let out = self.cache_path(&np);
        if std::fs::metadata(&out).is_ok()
            || (!cached.is_empty() && std::fs::metadata(&cached).is_ok())
        {
            let p = if std::fs::metadata(&out).is_ok() {
                out
            } else {
                PathBuf::from(cached)
            };
            return Some(p);
        }
        if self.render(Path::new(&src), &out) {
            let _ = conn.execute(
                "UPDATE media_item SET thumbnail_path=?1 WHERE id=?2",
                rusqlite::params![out.to_string_lossy(), media_id],
            );
            Some(out)
        } else {
            None
        }
    }

    fn render(&self, src: &Path, out: &Path) -> bool {
        // 1. native decode when the `image` crate supports the format
        let decoded = (|| -> std::io::Result<image::DynamicImage> {
            let r = image::ImageReader::open(src)?;
            let r = r.with_guessed_format()?;
            r.decode().map_err(std::io::Error::other)
        })();
        if let Ok(img) = decoded {
            let small = img.thumbnail(self.max_edge, self.max_edge);
            if small.save(out).is_ok() {
                return true;
            }
        }
        // 2. embedded preview via ExifTool (HEIC/RAW often carry one)
        if let Some(bytes) = exiftool_preview(src) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                let small = img.thumbnail(self.max_edge, self.max_edge);
                if small.save(out).is_ok() {
                    return true;
                }
            }
        }
        false
    }

    /// Generate thumbnails for a page of items; returns count generated.
    pub fn generate_pending(
        &self,
        conn: &Connection,
        cancel: &AtomicBool,
        limit: i64,
        mut on_done: impl FnMut(&str, Option<PathBuf>),
    ) -> usize {
        let ids: Vec<(String, String, String)> = conn
            .prepare(
                "SELECT id, path, normalized_path FROM media_item
                 WHERE media_kind != 'sidecar' AND (thumbnail_path IS NULL OR thumbnail_path = '')
                 ORDER BY COALESCE(captured_at_local,'9999') DESC LIMIT ?1",
            )
            .and_then(|mut st| {
                st.query_map(rusqlite::params![limit], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default();
        let mut made = 0;
        for (id, path, np) in ids {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let out = self.cache_path(&np);
            if self.render(Path::new(&path), &out) {
                let _ = conn.execute(
                    "UPDATE media_item SET thumbnail_path=?1 WHERE id=?2",
                    rusqlite::params![out.to_string_lossy(), id],
                );
                made += 1;
                on_done(&id, Some(out));
            } else {
                on_done(&id, None);
            }
        }
        made
    }
}

fn exiftool_preview(path: &Path) -> Option<Vec<u8>> {
    let bin = crate::services::metadata::ExifTool::new().bin?;
    let mut cmd = Command::new(bin);
    crate::services::metadata::no_window(&mut cmd);
    cmd.args(["-b", "-ThumbnailImage", "-EmbeddedImage", "-PreviewImage"])
        .arg(path);
    let (status, stdout, _) =
        crate::services::metadata::run_captured(&mut cmd, std::time::Duration::from_secs(20))
            .ok()?;
    if !status.map(|s| s.success()).unwrap_or(false) {
        return None;
    }
    if stdout.is_empty() {
        return None;
    }
    Some(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::db::{add_source_root, open_db, query_media, MediaFilter};
    use crate::services::scanner::{scan_root, ScanOptions};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn thumb_generated_for_png_none_for_fake() {
        let dir = std::env::temp_dir().join(format!("kadr-thumb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // real 8x8 png
        let img = image::DynamicImage::new_rgb8(8, 8);
        img.save(dir.join("real.png")).unwrap();
        std::fs::write(dir.join("fake.jpg"), b"not an image").unwrap();

        let conn = open_db(":memory:").unwrap();
        let root = add_source_root(&conn, dir.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        scan_root(
            &conn,
            &root,
            &ScanOptions {
                compute_hashes: false,
                ..Default::default()
            },
            "j",
            &cancel,
            &|_| {},
        )
        .unwrap();
        let cache = dir.join("thumbs");
        let svc = ThumbService::new(&cache);
        let made = svc.generate_pending(&conn, &cancel, 50, |_, _| {});
        assert_eq!(made, 1, "png thumb generated, fake jpg honestly has none");
        let (items, _) = query_media(&conn, &MediaFilter::default()).unwrap();
        let png = items.iter().find(|i| i.extension == "png").unwrap();
        assert!(png.thumbnail_path.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
