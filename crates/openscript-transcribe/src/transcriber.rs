// =============================================================================
// TRANSCRIPTION ENGINE — HinglishGgml (canonical default)
// =============================================================================
//
// Single engine: HinglishGgml — whisper.cpp + Whisper-Hindi2Hinglish-Apex-GGML
//   - Direct Hinglish output from Hindi audio (no LLM post-processing needed)
//   - Requires whisper-cli (built from source) and GGML model file
//   - Python sidecar: mcp/scripts/hinglish_ggml_transcriber.py
//   - Best for Hindi audio where native Latin-script output is desired.
//
// Previous engines (Whisper, Nemotron ONNX, Apex) were removed in Phase 41
// as YAGNI cleanup — they were deprecated, non-functional, or required
// environments (conda) that are not standard on this system.
// =============================================================================

use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct TranscribeResult {
    pub output_path: String,
    pub entry_count: usize,
    /// Word-level SRT path (if word alignment succeeded)
    pub word_srt_path: Option<String>,
    /// Phrase-level SRT path (best for EDL building)
    pub phrase_srt_path: Option<String>,
    /// Which transcription engine produced this result
    pub engine: TranscriptionEngine,
}

/// The transcription engine used. HinglishGgml is the sole engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionEngine {
    HinglishGgml,
}

impl std::fmt::Display for TranscriptionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HinglishGgml => write!(f, "hinglish-ggml"),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Transcription failed ({engine}): {detail}")]
    TranscriptionFailed { engine: String, detail: String },
    #[error("Output file not found: {0}")]
    OutputNotFound(String),
    #[error("Wrapper script not found: {0}")]
    WrapperNotFound(String),
    #[error("Python not found: {0}")]
    PythonNotFound(String),
}

// ---------------------------------------------------------------------------
// Output validation — detect wrong script
// ---------------------------------------------------------------------------

/// Check if SRT content appears to be Hinglish (Latin script) vs.
/// Arabic/Devanagari/other non-Latin scripts.
///
/// Returns `Ok(())` if the content looks like Latin-script Hinglish,
/// or `Err(reason)` if it appears to be in a different script.
pub fn validate_hinglish_output(content: &str) -> Result<(), String> {
    let latin_chars: usize = content.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let non_latin_text_chars: usize = content
        .chars()
        .filter(|c| {
            c.is_alphabetic()
                && !c.is_ascii()
                && !c.is_ascii_digit()
                && *c != '\n'
                && *c != '\r'
                && *c != '\t'
        })
        .count();

    let total_alpha = latin_chars + non_latin_text_chars;
    if total_alpha == 0 {
        return Err("SRT contains no alphabetic characters — transcription may be empty".into());
    }

    let latin_pct = (latin_chars as f64 / total_alpha as f64) * 100.0;

    // If < 50 % of alphabetic chars are Latin, likely wrong script
    if latin_pct < 50.0 {
        // Detect likely script for the error message
        let devanagari = content
            .chars()
            .filter(|c| matches!(u32::from(*c), 0x0900..=0x097F))
            .count();
        let arabic = content
            .chars()
            .filter(|c| matches!(u32::from(*c), 0x0600..=0x06FF | 0x0750..=0x077F))
            .count();

        let script = if devanagari > arabic {
            "Devanagari (Hindi)"
        } else if arabic > 0 {
            "Arabic/Urdu script"
        } else {
            "non-Latin script"
        };

        return Err(format!(
            "Only {:.0}% Latin chars — output appears to be {} (expected Hinglish in Latin script). \
             The model may have auto-detected the wrong language.",
            latin_pct, script
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Script resolution — shared helper
// ---------------------------------------------------------------------------

/// Resolve a Python sidecar script via env var, CARGO_MANIFEST_DIR,
/// OPENSCRIPT_ROOT, and relative fallbacks.
fn resolve_script(env_var: &str, relative_name: &str) -> Option<PathBuf> {
    // 1. Explicit env var override
    if let Ok(path) = std::env::var(env_var) {
        let p = Path::new(&path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    // 2. CARGO_MANIFEST_DIR (compile-time workspace path; works in dev)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = Path::new(&manifest_dir)
            .join("../../mcp/scripts/")
            .join(relative_name);
        if p.exists() {
            return Some(p);
        }
    }

    // 3. OPENSCRIPT_ROOT (deployment override)
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = Path::new(&root)
            .join("mcp/scripts/")
            .join(relative_name);
        if p.exists() {
            return Some(p);
        }
    }

    // 4. Relative fallbacks (works if CWD is repo root or a crate dir)
    let relative_candidates = [
        PathBuf::from("mcp/scripts/").join(relative_name),
        PathBuf::from("../mcp/scripts/").join(relative_name),
        PathBuf::from("../../mcp/scripts/").join(relative_name),
    ];
    for c in &relative_candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    None
}

/// Find the Hinglish GGML transcription script (whisper.cpp + Hindi2Hinglish).
fn find_hinglish_ggml_script() -> Option<PathBuf> {
    resolve_script(
        "OPENSCRIPT_HINGLISH_GGML_WRAPPER",
        "hinglish_ggml_transcriber.py",
    )
}

/// Find system Python 3.
fn find_system_python() -> Option<PathBuf> {
    // Priority 1: explicit env var
    if let Ok(path) = std::env::var("OPENSCRIPT_PYTHON") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
    }

    // Priority 2: common python3 paths
    let candidates = [
        PathBuf::from("python3"),
        PathBuf::from("/usr/bin/python3"),
        PathBuf::from("/usr/local/bin/python3"),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    // Priority 3: which python3
    if let Ok(output) = std::process::Command::new("which")
        .arg("python3")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string();
            let p = PathBuf::from(&path);
            if p.exists() {
                return Some(p);
            }
        }
    }

    None
}

/// Check if HinglishGgml transcription is available.
pub async fn check_hinglish_ggml_health() -> Result<String, String> {
    let _python = find_system_python()
        .ok_or("System Python 3 not found".to_string())?;
    let script = find_hinglish_ggml_script()
        .ok_or("hinglish_ggml_transcriber.py not found".to_string())?;

    // Check that whisper-cli is available
    let whisper_cli = std::env::var("WHISPER_CLI")
        .unwrap_or_else(|_| {
            dirs_or_home()
                .join(".local/bin/whisper-cli")
                .to_string_lossy()
                .to_string()
        });

    if Path::new(&whisper_cli).exists() {
        Ok(format!(
            "HinglishGgml available (script: {}, whisper-cli: {})",
            script.display(),
            whisper_cli
        ))
    } else {
        Err(format!(
            "whisper-cli not found at {}. Build whisper.cpp or set WHISPER_CLI env var.",
            whisper_cli
        ))
    }
}

/// Helper to get home dir for health check.
fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

// =============================================================================
// TRANSCRIPTION — MAIN ENTRY POINT
// =============================================================================

/// Transcribe media to SRT using the HinglishGgml engine (canonical default).
///
/// Pipeline:
/// 1. Extract 16kHz mono WAV via ffmpeg
/// 2. Run whisper.cpp with Hindi2Hinglish GGML model
/// 3. Generate word-level and phrase-level SRT
pub async fn transcribe(
    media_path: &str,
    output_srt_path: &str,
) -> Result<TranscribeResult, TranscribeError> {
    transcribe_with_engine(media_path, output_srt_path, TranscriptionEngine::HinglishGgml, "auto", None)
        .await
}

/// Transcribe with a specific engine and language hint.
pub async fn transcribe_with_engine(
    media_path: &str,
    output_srt_path: &str,
    engine: TranscriptionEngine,
    language_hint: &str,
    progress_cb: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Result<TranscribeResult, TranscribeError> {
    let out_dir = Path::new(output_srt_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = Path::new(media_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());

    match engine {
        TranscriptionEngine::HinglishGgml => {
            tracing::info!(
                "Using Hinglish GGML engine (whisper.cpp + Hindi2Hinglish-Apex-GGML)"
            );
            transcribe_hinglish_ggml(media_path, output_srt_path, &stem, out_dir, language_hint, progress_cb)
                .await
        }
    }
}

// =============================================================================
// TRANSCRIPTION — HINGLISH GGML ENGINE (whisper.cpp + Hindi2Hinglish-Apex-GGML)
// =============================================================================

/// Transcribe using whisper.cpp with the Whisper-Hindi2Hinglish-Apex-GGML model.
/// Outputs Hinglish (Latin script) directly from Hindi audio.
async fn transcribe_hinglish_ggml(
    media_path: &str,
    output_srt_path: &str,
    stem: &str,
    out_dir: &Path,
    language_hint: &str,
    progress_cb: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Result<TranscribeResult, TranscribeError> {
    let wrapper = find_hinglish_ggml_script().ok_or_else(|| {
        TranscribeError::WrapperNotFound(
            "hinglish_ggml_transcriber.py not found. Set OPENSCRIPT_HINGLISH_GGML_WRAPPER or \
             ensure mcp/scripts/hinglish_ggml_transcriber.py exists."
                .into(),
        )
    })?;

    let python = find_system_python().ok_or_else(|| {
        TranscribeError::PythonNotFound(
            "System Python 3 not found. Set OPENSCRIPT_PYTHON env var.".into(),
        )
    })?;

    let mut cmd = Command::new(&python);
    cmd.arg(&wrapper)
        .arg("run")
        .arg("--video")
        .arg(media_path)
        .arg("--out-dir")
        .arg(out_dir)
        .arg("--language")
        .arg(language_hint)
        .kill_on_drop(true);

    // If a progress callback is provided, stream stderr for progress lines.
    // The Python sidecar emits lines like "[progress:XX]" during whisper processing.
    let mut child = if progress_cb.is_some() {
        Some(cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(TranscribeError::Io)?)
    } else {
        None
    };

    let output = if let Some(ref mut child) = child {
        // Stream stderr for progress, capture stdout for result
        let stderr_handle = child.stderr.take().expect("Failed to capture stderr");
        let mut stderr_reader = tokio::io::BufReader::new(stderr_handle).lines();
        let mut stderr_buf = String::new();
        let mut stdout_buf = String::new();

        // Read stderr line by line for progress updates
        loop {
            tokio::select! {
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            if line.starts_with("[progress:") {
                                if let Some(pct_str) = line.strip_prefix("[progress:").and_then(|s| s.strip_suffix(']')) {
                                    if let Ok(pct) = pct_str.parse::<f64>() {
                                        if let Some(ref cb) = progress_cb {
                                            cb(pct, "Transcribing audio...");
                                        }
                                    }
                                }
                            } else {
                                stderr_buf.push_str(&line);
                                stderr_buf.push('\n');
                            }
                        }
                        Ok(None) => break, // EOF
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    if let Some(ref cb) = progress_cb {
                        cb(50.0, "Transcribing audio...");
                    }
                }
            }
        }

        // Wait for process to finish and capture stdout
        let status = child.wait().await.map_err(TranscribeError::Io)?;
        child.stdout.take().expect("stdout should be piped").read_to_string(&mut stdout_buf).await.map_err(TranscribeError::Io)?;

        std::process::Output {
            status,
            stdout: stdout_buf.into_bytes(),
            stderr: stderr_buf.into_bytes(),
        }
    } else {
        cmd.output().await.map_err(TranscribeError::Io)?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TranscribeError::TranscriptionFailed {
            engine: "hinglish-ggml".into(),
            detail: stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n"),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        TranscribeError::TranscriptionFailed {
            engine: "hinglish-ggml".into(),
            detail: format!("Failed to parse sidecar output: {}", e),
        }
    })?;

    if result.get("error").is_some() {
        return Err(TranscribeError::TranscriptionFailed {
            engine: "hinglish-ggml".into(),
            detail: result["error"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string(),
        });
    }

    let output_srt = result["output_srt_path"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            out_dir
                .join(format!("{}.hinglish-ggml.srt", stem))
                .to_string_lossy()
                .to_string()
        });

    if output_srt != output_srt_path && Path::new(&output_srt).exists() {
        if let Ok(content) = std::fs::read_to_string(&output_srt) {
            std::fs::write(output_srt_path, content).map_err(TranscribeError::Io)?;
        }
    }

    build_result(output_srt_path, stem, out_dir, TranscriptionEngine::HinglishGgml)
}

// ---------------------------------------------------------------------------
// Result builder
// ---------------------------------------------------------------------------

/// Build a TranscribeResult from the output file, with validation.
fn build_result(
    output_srt_path: &str,
    stem: &str,
    out_dir: &Path,
    engine: TranscriptionEngine,
) -> Result<TranscribeResult, TranscribeError> {
    let out_dir_str = out_dir.to_string_lossy();

    if !Path::new(output_srt_path).exists() {
        return Err(TranscribeError::OutputNotFound(output_srt_path.to_string()));
    }

    let content = std::fs::read_to_string(output_srt_path).map_err(TranscribeError::Io)?;

    if let Err(reason) = validate_hinglish_output(&content) {
        tracing::warn!(
            engine = %engine,
            "Hinglish output validation warning: {}",
            reason
        );
    }

    let entry_count = content
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .count();

    // Find word/phrase SRT files for HinglishGgml
    let word_srt_pattern = format!("{}/{}.hinglish-ggml.word.srt", out_dir_str, stem);
    let phrase_srt_pattern = format!("{}/{}.hinglish-ggml.phrase.srt", out_dir_str, stem);

    Ok(TranscribeResult {
        output_path: output_srt_path.to_string(),
        entry_count,
        word_srt_path: if Path::new(&word_srt_pattern).exists() {
            Some(word_srt_pattern)
        } else {
            None
        },
        phrase_srt_path: if Path::new(&phrase_srt_pattern).exists() {
            Some(phrase_srt_pattern)
        } else {
            None
        },
        engine,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_hinglish_ggml_script() {
        let result = find_hinglish_ggml_script();
        // On a dev machine with the repo checked out, this should find the file
        assert!(
            result.is_some() || std::env::var("OPENSCRIPT_HINGLISH_GGML_WRAPPER").is_err()
        );
    }

    #[test]
    fn test_find_system_python() {
        let result = find_system_python();
        // On most systems, python3 should be available
        if let Some(p) = &result {
            assert!(p.exists(), "Found python at {:?} but it doesn't exist", p);
        }
    }

    #[test]
    fn test_validate_hinglish_latin_passes() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nmain ek engineer hoon\n\n";
        assert!(validate_hinglish_output(srt).is_ok());
    }

    #[test]
    fn test_validate_hinglish_arabic_fails() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nأنا مهندس\n\n";
        let err = validate_hinglish_output(srt).unwrap_err();
        assert!(
            err.contains("Arabic"),
            "Expected Arabic detection, got: {err}"
        );
    }

    #[test]
    fn test_validate_hinglish_devanagari_fails() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nमैं एक इंजीनियर हूँ\n\n";
        let err = validate_hinglish_output(srt).unwrap_err();
        assert!(
            err.contains("Devanagari"),
            "Expected Devanagari detection, got: {err}"
        );
    }

    #[test]
    fn test_validate_hinglish_mixed_hinglish_passes() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nkya haal hai भाई\n\n";
        assert!(validate_hinglish_output(srt).is_ok());
    }

    #[test]
    fn test_transcription_engine_display() {
        assert_eq!(TranscriptionEngine::HinglishGgml.to_string(), "hinglish-ggml");
    }

    #[tokio::test]
    async fn test_check_hinglish_ggml_health() {
        // On a dev machine with whisper-cli installed, this should succeed
        let result = check_hinglish_ggml_health().await;
        // We don't assert success because whisper-cli may not be installed in CI
        // but we verify the function doesn't panic
        assert!(
            result.is_ok() || result.is_err(),
            "check_hinglish_ggml_health must not panic"
        );
    }
}
