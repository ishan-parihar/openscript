//! Previewer — single-frame still rendering via Remotion `still` command.
//!
//! Renders one frame of a motion composition as a 1080×1920 PNG.
//! 2-5 second feedback loop vs 30-120s for full video render.
//!
//! Usage: `preview_motion_frame(tsx_code, frame_number, output_path?)`

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::process::Command;

/// Arguments for a single-frame preview render.
#[derive(Debug)]
pub struct PreviewArgs {
    /// Complete TSX composition code.
    pub tsx_code: String,
    /// Frame number to render (0-based).
    pub frame_number: u32,
    /// Optional output PNG path. Auto-generated if None.
    pub output_path: Option<String>,
}

/// Result of a successful preview render.
#[derive(Debug)]
pub struct PreviewResult {
    pub output_path: String,
    pub frame_number: u32,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
}

/// Find the remotion/ root directory (same logic as renderer).
fn find_remotion_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let candidate = PathBuf::from(&root).join("remotion");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let mut current = std::env::current_dir().ok()?;
    loop {
        let candidate = current.join("remotion");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Find the openscript project root.
fn find_openscript_root(remotion_root: &Path) -> PathBuf {
    remotion_root.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

/// Render a single frame of a motion composition as PNG.
///
/// This is the fast feedback path: write TSX → remotion still → return PNG path.
/// Takes 2-5 seconds vs 30-120 seconds for full video render.
pub async fn preview_motion_frame(args: PreviewArgs) -> Result<PreviewResult, String> {
    let remotion_root = find_remotion_root()
        .ok_or_else(|| "Could not find remotion/ directory. Set OPENSCRIPT_ROOT or ensure remotion/ exists in an ancestor directory.".to_string())?;

    // Write the TSX composition to the hot-composition slot
    let composition_path = remotion_root.join("src/compositions/hot-composition.tsx");
    if let Some(parent) = composition_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create composition directory: {e}"))?;
    }
    fs::write(&composition_path, &args.tsx_code)
        .await
        .map_err(|e| format!("Failed to write TSX composition: {e}"))?;

    // Determine output path
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let output_path = match args.output_path {
        Some(path) => PathBuf::from(path),
        None => {
            let project_root = find_openscript_root(&remotion_root);
            let artifacts_dir = project_root.join("artifacts");
            fs::create_dir_all(&artifacts_dir)
                .await
                .map_err(|e| format!("Failed to create artifacts directory: {e}"))?;
            artifacts_dir.join(format!("preview_frame{}_{}.png", args.frame_number, timestamp))
        }
    };

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create output directory: {e}"))?;
    }

    let output_str = output_path
        .to_str()
        .ok_or("Output path contains invalid UTF-8")?
        .to_string();

    // Run remotion still command
    let frame_arg = args.frame_number.to_string();
    let still_output = Command::new("npx")
        .args([
            "remotion",
            "still",
            "HotMotion",
            &output_str,
            "--frame",
            &frame_arg,
            "--log-level=error",
        ])
        .current_dir(&remotion_root)
        .output()
        .await
        .map_err(|e| format!("Failed to execute remotion still: {e}"))?;

    if !still_output.status.success() {
        let stderr = String::from_utf8_lossy(&still_output.stderr);
        let stdout = String::from_utf8_lossy(&still_output.stdout);
        let error_detail = if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!("Remotion still exited with status: {}", still_output.status)
        };
        return Err(format!("Remotion still failed: {error_detail}"));
    }

    let metadata = fs::metadata(&output_path)
        .await
        .map_err(|e| format!("Preview succeeded but cannot read output file: {e}"))?;

    // PNG is always 1080×1920 for our compositions
    Ok(PreviewResult {
        output_path: output_str,
        frame_number: args.frame_number,
        width: 1080,
        height: 1920,
        file_size: metadata.len(),
    })
}
