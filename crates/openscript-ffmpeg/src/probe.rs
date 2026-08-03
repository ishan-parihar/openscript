use tokio::process::Command;

use crate::FfmpegError;

pub struct MediaMetrics {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pix_fmt: Option<String>,
    pub codec: Option<String>,
    pub fps: f64,
    pub duration: f64,
    pub size_bytes: u64,
}

pub async fn probe(path: &str) -> Result<MediaMetrics, FfmpegError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            path,
        ])
        .output()
        .await?;

    let j: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let streams = j
        .get("streams")
        .and_then(|v| v.as_array())
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let fmt = j.get("format").and_then(|v| v.as_object());
    let v_stream = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("video"));

    let fps = parse_fps(v_stream);

    Ok(MediaMetrics {
        width: v_stream
            .and_then(|s| s.get("width").and_then(as_u64))
            .map(|v| v as u32),
        height: v_stream
            .and_then(|s| s.get("height").and_then(as_u64))
            .map(|v| v as u32),
        pix_fmt: v_stream
            .and_then(|s| s.get("pix_fmt").and_then(|v| v.as_str()))
            .map(String::from),
        codec: v_stream
            .and_then(|s| s.get("codec_name").and_then(|v| v.as_str()))
            .map(String::from),
        fps,
        // ffprobe emits `format.duration` as a STRING (e.g. "18.233333")
        // in JSON output, not a number. Using `.as_f64()` directly silently
        // yields None → duration 0.0 → every probe appears to fail, and the
        // renderer falls back to the conservative unknown-duration loop=3
        // path (which also caps seek offsets wrongly). Parse both shapes.
        duration: fmt
            .and_then(|f| f.get("duration"))
            .and_then(as_f64)
            .unwrap_or(0.0),
        // `format.size` is likewise a string ("18035565").
        size_bytes: fmt
            .and_then(|f| f.get("size"))
            .and_then(as_u64)
            .unwrap_or(0),
    })
}

/// Parse a JSON value as f64, accepting both JSON numbers and numeric strings
/// (ffprobe emits most numeric fields as strings in its JSON output).
fn as_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Parse a JSON value as u64, accepting both JSON numbers and numeric strings.
fn as_u64(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn parse_fps(stream: Option<&serde_json::Value>) -> f64 {
    stream
        .and_then(|s| s.get("r_frame_rate").and_then(|v| v.as_str()))
        .and_then(|r| {
            let parts: Vec<&str> = r.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = parts[0].parse().ok()?;
                let den: f64 = parts[1].parse().ok()?;
                if den != 0.0 {
                    Some(num / den)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_f64_handles_number_and_string() {
        // ffprobe emits `format.duration` as a JSON string ("18.233333")
        assert!((as_f64(&serde_json::json!("18.233333")).unwrap() - 18.233333).abs() < 1e-9);
        assert!((as_f64(&serde_json::json!(18.233333)).unwrap() - 18.233333).abs() < 1e-9);
        assert_eq!(as_f64(&serde_json::json!(0.0)), Some(0.0));
        assert_eq!(as_f64(&serde_json::json!("abc")), None);
        assert_eq!(as_f64(&serde_json::json!(null)), None);
    }

    #[test]
    fn test_as_u64_handles_number_and_string() {
        // ffprobe emits `format.size` as a JSON string ("18035565")
        assert_eq!(as_u64(&serde_json::json!("18035565")), Some(18035565));
        assert_eq!(as_u64(&serde_json::json!(18035565)), Some(18035565));
        assert_eq!(as_u64(&serde_json::json!("18.5")), None);
        assert_eq!(as_u64(&serde_json::json!("-3")), None);
        assert_eq!(as_u64(&serde_json::json!(null)), None);
    }

    #[test]
    fn test_parse_fps_valid() {
        let val = serde_json::json!({ "r_frame_rate": "30/1" });
        assert!((parse_fps(Some(&val)) - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_fps_fractional() {
        let val = serde_json::json!({ "r_frame_rate": "30000/1001" });
        let fps = parse_fps(Some(&val));
        assert!((fps - 29.97002997).abs() < 0.001);
    }

    #[test]
    fn test_parse_fps_missing() {
        let val = serde_json::json!({ "codec_name": "h264" });
        assert!((parse_fps(Some(&val)) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_fps_none() {
        assert!((parse_fps(None) - 0.0).abs() < 0.001);
    }
}
