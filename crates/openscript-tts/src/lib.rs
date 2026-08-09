pub mod audio8;
pub mod client;
pub mod gepard;
pub mod profiles;
pub mod voicedesign;

/// Native Kokoro TTS backend (preset-voice, ONNX-based).
///
/// Enabled via the `kokoro` feature flag. The default build is sidecar-only
/// (drives the `faster-qwen3-tts` Python sidecar via `client.rs`).
#[cfg(feature = "kokoro")]
pub mod kokoro;

/// Long-lived Kokoro sidecar pool (stdin/stdout JSON protocol).
///
/// Eliminates the ~360ms per-chunk cold-start that the fresh-process
/// path pays (Python startup + kokoro_onnx import + ONNX model load +
/// voices load). For a 20-scene script with 2 chunks per scene, this
/// saves ~14 seconds of pure overhead.
#[cfg(feature = "kokoro")]
pub mod kokoro_sidecar;

/// Evict oldest cache entries (LRU by mtime) if the cache directory exceeds
/// the configured max size.
///
/// Reads `OPENSCRIPT_TTS_CACHE_MAX_MB` env var (default: 500 MB). If the
/// total size of `cache_dir` exceeds this, oldest files by mtime are
/// deleted until the total is under the limit.
///
/// This is best-effort: errors are logged to stderr but don't propagate,
/// since cache eviction is a housekeeping task, not a functional requirement.
pub fn evict_cache_if_needed(cache_dir: &std::path::Path) {
    let max_mb: u64 = std::env::var("OPENSCRIPT_TTS_CACHE_MAX_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(500);
    let max_bytes = max_mb * 1024 * 1024;
    evict_cache_to_limit(cache_dir, max_bytes);
}

/// Internal: evict oldest files by mtime until total size <= max_bytes.
pub(crate) fn evict_cache_to_limit(cache_dir: &std::path::Path, max_bytes: u64) {
    // Collect all files in the cache dir with their sizes and mtimes
    let entries: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> =
        match std::fs::read_dir(cache_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let path = e.path();
                    let meta = std::fs::metadata(&path).ok()?;
                    if !meta.is_file() {
                        return None;
                    }
                    let mtime = meta.modified().ok()?;
                    Some((path, meta.len(), mtime))
                })
                .collect(),
            Err(_) => return,
        };

    // Calculate total size
    let total: u64 = entries.iter().map(|(_, size, _)| *size).sum();
    if total <= max_bytes {
        return;
    }

    // Sort by mtime ascending (oldest first)
    let mut sorted = entries;
    sorted.sort_by(|a, b| a.2.cmp(&b.2));

    // Delete oldest until under limit
    let mut current = total;
    let mut evicted = 0;
    for (path, size, _) in &sorted {
        if current <= max_bytes {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            current -= size;
            evicted += 1;
        }
    }

    if evicted > 0 {
        eprintln!(
            "[tts-cache] Evicted {} files, cache now {} MB (limit {} MB)",
            evicted,
            current / 1024 / 1024,
            max_bytes / 1024 / 1024
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::SystemTime;

    #[test]
    fn test_evict_cache_under_limit() {
        let dir = std::env::temp_dir().join(format!("tts_cache_under_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.wav"), b"small").unwrap();
        // 1MB limit, file is 5 bytes — no eviction
        evict_cache_to_limit(&dir, 1024 * 1024);
        assert!(dir.join("a.wav").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_evict_cache_over_limit() {
        let dir = std::env::temp_dir().join(format!("tts_cache_over_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        // Write 3 files, 1KB each. Set old mtimes to control eviction order.
        fs::write(dir.join("old.wav"), vec![0u8; 1024]).unwrap();
        let old_time = SystemTime::now() - std::time::Duration::from_secs(3600);
        let _ = filetime::set_file_mtime(
            dir.join("old.wav"),
            filetime::FileTime::from_system_time(old_time),
        );

        fs::write(dir.join("mid.wav"), vec![0u8; 1024]).unwrap();
        let mid_time = SystemTime::now() - std::time::Duration::from_secs(1800);
        let _ = filetime::set_file_mtime(
            dir.join("mid.wav"),
            filetime::FileTime::from_system_time(mid_time),
        );

        fs::write(dir.join("new.wav"), vec![0u8; 1024]).unwrap();
        // new.wav has the newest mtime (now)

        // Total = 3KB. Limit = 1.5KB -> should evict old.wav (1KB) and mid.wav (1KB),
        // leaving new.wav (1KB) = 1KB <= 1.5KB
        evict_cache_to_limit(&dir, 1536);

        assert!(!dir.join("old.wav").exists(), "old.wav should be evicted");
        assert!(!dir.join("mid.wav").exists(), "mid.wav should be evicted");
        assert!(dir.join("new.wav").exists(), "new.wav should survive");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_evict_cache_empty_dir() {
        let dir = std::env::temp_dir().join(format!("tts_cache_empty_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Empty dir, tiny limit - should not panic
        evict_cache_to_limit(&dir, 100);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_evict_cache_nonexistent_dir() {
        let dir = std::env::temp_dir().join(format!("tts_cache_nonexist_{}", std::process::id()));
        // Don't create the dir - should not panic
        evict_cache_to_limit(&dir, 100);
    }
}
