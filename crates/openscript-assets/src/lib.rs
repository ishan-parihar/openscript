pub mod sfx;
pub mod music;
pub mod pexels;

/// Shared utility: probe media duration in milliseconds via ffprobe.
/// Returns None if ffprobe is unavailable or the file cannot be probed.
pub(crate) fn probe_duration_ms(path: &str) -> Option<i64> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let dur_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let dur_secs: f64 = dur_str.parse().ok()?;
    Some((dur_secs * 1000.0).round() as i64)
}
