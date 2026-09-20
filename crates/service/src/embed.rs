//! The embedded web assets.
//!
//! `web/dist` is compiled into the binary, so a deployment is one file and the
//! page can never be out of step with the API it talks to. The build script
//! guarantees the directory exists, writing a placeholder page when the front-end
//! was not built, which is what [`is_placeholder`] detects.

use std::collections::HashMap;
use std::sync::LazyLock;

use include_dir::{Dir, include_dir};

/// The front-end build output.
static DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../web/dist");

/// Index of every embedded file, keyed by its request path without a leading
/// slash, with its MIME type and ETag precomputed.
static INDEX: LazyLock<HashMap<String, Asset>> = LazyLock::new(|| {
    let mut assets = HashMap::new();
    collect(&DIST, "", &mut assets);
    assets
});

/// One embedded file.
#[derive(Debug, Clone, Copy)]
pub struct Asset {
    /// File contents.
    pub bytes: &'static [u8],
    /// MIME type derived from the extension.
    pub mime: &'static str,
    /// Content hash used for `ETag` and `304` responses.
    pub etag: u64,
}

impl Asset {
    /// `ETag` header value, quoted as the HTTP specification requires.
    pub fn etag_header(&self) -> String {
        format!("\"{:016x}\"", self.etag)
    }
}

/// Number of embedded files.
pub fn count() -> usize {
    INDEX.len()
}

/// Total size of the embedded assets, bytes.
pub fn total_bytes() -> usize {
    INDEX.values().map(|asset| asset.bytes.len()).sum()
}

/// Looks up an embedded file by request path.
pub fn get(path: &str) -> Option<&'static Asset> {
    let key = path.trim_start_matches('/');
    let key = if key.is_empty() { "index.html" } else { key };
    INDEX.get(key)
}

/// The single-page application entry point.
pub fn index() -> Option<&'static Asset> {
    get("index.html")
}

/// Whether the embedded page is the build script's placeholder rather than a real
/// front-end build.
///
/// The placeholder is a complete HTML document that names the marker in a meta tag,
/// so the check survives minification of a real build.
pub fn is_placeholder() -> bool {
    index()
        .map(|asset| {
            let text = String::from_utf8_lossy(asset.bytes);
            text.contains("Web assets were not built")
        })
        .unwrap_or(true)
}

fn collect(dir: &'static Dir<'static>, prefix: &str, out: &mut HashMap<String, Asset>) {
    for file in dir.files() {
        let name = file
            .path()
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let key = format!("{prefix}{name}");
        let bytes = file.contents();
        out.insert(
            key,
            Asset {
                bytes,
                mime: mime_of(name),
                etag: hash(bytes),
            },
        );
    }
    for child in dir.dirs() {
        let name = child
            .path()
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        collect(child, &format!("{prefix}{name}/"), out);
    }
}

/// Content hash used as the ETag.
///
/// A fast non-cryptographic hash is the right tool: the value only has to change
/// when the bytes do, and both sides of the comparison come from the same file.
fn hash(bytes: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = twox_hash_of();
    hasher.write(bytes);
    hasher.finish()
}

/// TwoX hash with a fixed seed, so an ETag is stable across restarts.
fn twox_hash_of() -> twox_hash::XxHash64 {
    twox_hash::XxHash64::with_seed(0x0DDB_1A5E_5EED_1234)
}

/// MIME type of a file name.
fn mime_of(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") | Some("md") => "text/plain; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_tree_holds_an_entry_point() {
        // The build script guarantees `web/dist/index.html` exists, so this can
        // only fail if the embed path or the build order broke.
        assert!(index().is_some(), "no embedded index.html");
        assert!(count() >= 1, "the embedded tree is empty");
    }

    #[test]
    fn mime_types_follow_the_extension() {
        assert_eq!(mime_of("a/b/app.js"), "text/javascript; charset=utf-8");
        assert_eq!(mime_of("index.html"), "text/html; charset=utf-8");
        assert_eq!(mime_of("style.woff2"), "font/woff2");
        assert_eq!(mime_of("unknown.bin"), "application/octet-stream");
    }

    #[test]
    fn an_etag_changes_with_the_bytes() {
        let first = hash(b"abc");
        let second = hash(b"abd");
        assert_ne!(first, second);
        assert_eq!(first, hash(b"abc"));
    }
}
