//! PNG bytes -> `RenderImage` cache for the library grid.
//!
//! In the Tauri build every thumbnail crossed the IPC bridge as a base64 data
//! URL, so the gallery could inline it in an `<img>`. With no webview, the
//! natural primitive is a decoded texture, and the expensive step (Media
//! Foundation decode + PNG encode) has to be paid at most once per clip per
//! session.
//!
//! Two things make the cache correct rather than merely fast:
//!
//! * **Keyed by `(path, mtime)`** — trimming or renaming a clip changes the
//!   file, and a path-only key would serve a stale frame forever.
//! * **Bounded** — a library of thousands of clips must not hold thousands of
//!   decoded textures; the oldest insertions are evicted past
//!   [`MAX_ENTRIES`], LRU-ish in insertion order because that is enough for a
//!   list that scrolls forward.
//!
//! Decoding must never happen on the render path: [`load`] is blocking and the
//! library view calls it from `background_spawn`, then hands the resulting
//! `Arc<RenderImage>` to `img(ImageSource::Image(..))`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::RenderImage;

use crate::media::library;

/// Plenty for a gallery that shows ~30 rows at a time, small enough that the
/// decoded textures stay well under 100 MB at 400 px wide.
pub const MAX_ENTRIES: usize = 240;

#[derive(Debug, Default)]
pub struct ThumbnailCache {
    entries: HashMap<Key, Arc<RenderImage>>,
    /// Insertion order, used to evict the oldest when over [`MAX_ENTRIES`].
    order: Vec<Key>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    /// Milliseconds since the epoch. A file whose mtime moved is a different
    /// clip as far as the cache is concerned.
    modified_ms: i64,
}

impl ThumbnailCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Decodes a thumbnail without touching any cache. Blocking; used from
    /// `background_spawn` by the library rows.
    pub fn load(path: &Path, at_seconds: f32) -> Option<Arc<RenderImage>> {
        let png = library::extract_thumbnail_bytes(path, at_seconds).ok()?;
        decode_png(&png)
    }

    /// Returns the cached texture, or decodes it.
    ///
    /// Blocking (Media Foundation). Call from `background_spawn`.
    pub fn get_or_load(&mut self, path: &Path, at_seconds: f32) -> Option<Arc<RenderImage>> {
        let key = Key {
            path: path.to_path_buf(),
            modified_ms: modified_ms(path)?,
        };
        if let Some(img) = self.entries.get(&key) {
            return Some(Arc::clone(img));
        }

        let png = library::extract_thumbnail_bytes(path, at_seconds).ok()?;
        let image = decode_png(&png)?;

        self.insert(key, Arc::clone(&image));
        Some(image)
    }

    /// Looks up a texture without decoding, for the render path.
    ///
    /// Returns `None` for a path with a stale mtime, so the row re-decodes
    /// rather than showing the pre-trim frame.
    pub fn get_cached(&self, path: &str) -> Option<Arc<RenderImage>> {
        let path = Path::new(path);
        let now = modified_ms(path)?;
        self.entries
            .get(&Key {
                path: path.to_path_buf(),
                modified_ms: now,
            })
            .map(Arc::clone)
    }

    /// Stores a texture a background task already decoded.
    pub fn insert_shared(&mut self, path: String, image: Arc<RenderImage>) {
        let path = PathBuf::from(path);
        let Some(modified) = modified_ms(&path) else {
            return;
        };
        self.insert(
            Key {
                path,
                modified_ms: modified,
            },
            image,
        );
    }

    /// Empties the cache (delete-all, output folder change).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    /// Drops an entry (delete, rename, trim).
    pub fn forget(&mut self, path: &Path) {
        self.entries.retain(|k, _| k.path != path);
        self.order.retain(|k| k.path != path);
    }

    fn insert(&mut self, key: Key, image: Arc<RenderImage>) {
        if self.entries.insert(key.clone(), image).is_none() {
            self.order.push(key);
        }
        while self.order.len() > MAX_ENTRIES {
            let oldest = self.order.remove(0);
            self.entries.remove(&oldest);
        }
    }
}

/// Decodes a PNG into a texture.
///
/// **`RenderImage` wants BGRA, not RGBA.** Both the DirectX atlas
/// (`DXGI_FORMAT_B8G8R8A8_UNORM`) and Metal's upload path read the buffer as
/// premultiplied BGRA, so the red and blue channels are swapped here. This is
/// the same transform gpui's own `decode_static_image` performs; that function
/// is `pub(crate)`, so it is reproduced rather than called.
///
/// `image` is pinned to the same major as gpui's own dependency (`0.25`) in
/// `Cargo.toml`, so `Frame::new` accepts our buffer without an interop crate.
pub fn decode_png(png: &[u8]) -> Option<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(rgba)])))
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real, decodable 2x2 PNG. Generating it keeps the test honest: it
    /// exercises the same `decode_static_image` path the app uses instead of
    /// stubbing the decode away.
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn a_missing_file_is_a_cache_miss_not_a_panic() {
        let mut cache = ThumbnailCache::new();
        assert!(cache.get_or_load(Path::new("/no/such/clip.mp4"), 1.0).is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn a_garbage_png_does_not_decode() {
        assert!(decode_png(b"not a png at all").is_none());
    }

    #[test]
    fn decodes_a_real_png() {
        let image = decode_png(&tiny_png()).expect("2x2 png decodes");
        let size = image.size(0);
        assert_eq!(size.width.0, 2);
        assert_eq!(size.height.0, 2);
    }

    #[test]
    fn channels_are_swapped_to_bgra() {
        // The texture pipeline reads BGRA; if this regresses every thumbnail
        // renders with red and blue exchanged.
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        let decoded = decode_png(&out.into_inner()).unwrap();
        let bytes = decoded.as_bytes(0).unwrap();
        assert_eq!(&bytes[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn the_cache_is_bounded_and_evicts_oldest_first() {
        let mut cache = ThumbnailCache::new();
        let png = tiny_png();
        let image = decode_png(&png).unwrap();
        // Bypass the filesystem: feed synthetic keys with a real texture.
        for i in 0..(MAX_ENTRIES + 10) {
            cache.insert(
                Key {
                    path: PathBuf::from(format!("/clips/{i}.mp4")),
                    modified_ms: i as i64,
                },
                Arc::clone(&image),
            );
        }
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert_eq!(cache.order.len(), MAX_ENTRIES);
        // The oldest keys were evicted, the newest survived.
        assert!(!cache.order.iter().any(|k| k.path.ends_with("0.mp4")));
        assert!(cache.order.iter().any(|k| k.path.ends_with("249.mp4")));
    }

    #[test]
    fn reinserting_the_same_key_does_not_grow_the_order() {
        let mut cache = ThumbnailCache::new();
        let image = decode_png(&tiny_png()).unwrap();
        let key = Key {
            path: PathBuf::from("/clips/a.mp4"),
            modified_ms: 1,
        };
        cache.insert(key.clone(), Arc::clone(&image));
        cache.insert(key, image);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.order.len(), 1);
    }

    #[test]
    fn forgetting_a_path_removes_it_from_both_indexes() {
        let mut cache = ThumbnailCache::new();
        let image = decode_png(&tiny_png()).unwrap();
        cache.insert(
            Key {
                path: PathBuf::from("/clips/a.mp4"),
                modified_ms: 1,
            },
            image,
        );
        cache.forget(Path::new("/clips/a.mp4"));
        assert!(cache.is_empty());
        assert!(cache.order.is_empty());
    }

    #[test]
    fn a_touched_file_is_a_different_cache_entry() {
        // The key includes mtime, so a trimmed/renamed clip re-decodes instead
        // of serving a stale frame. Assert the key shape directly.
        let a = Key {
            path: PathBuf::from("/clips/a.mp4"),
            modified_ms: 100,
        };
        let b = Key {
            path: PathBuf::from("/clips/a.mp4"),
            modified_ms: 200,
        };
        assert_ne!(a, b);
    }
}
