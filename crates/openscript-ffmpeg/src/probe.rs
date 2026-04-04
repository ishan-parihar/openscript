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
            .and_then(|s| s.get("width").and_then(|v| v.as_u64()))
            .map(|v| v as u32),
        height: v_stream
            .and_then(|s| s.get("height").and_then(|v| v.as_u64()))
            .map(|v| v as u32),
        pix_fmt: v_stream
            .and_then(|s| s.get("pix_fmt").and_then(|v| v.as_str()))
            .map(String::from),
        codec: v_stream
            .and_then(|s| s.get("codec_name").and_then(|v| v.as_str()))
            .map(String::from),
        fps,
        duration: fmt
            .and_then(|f| f.get("duration").and_then(|v| v.as_f64()))
            .unwrap_or(0.0),
        size_bytes: fmt
            .and_then(|f| f.get("size").and_then(|v| v.as_u64()))
            .unwrap_or(0),
    })
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
