// =============================================================================
// TRANSCRIPTION ENGINES
// =============================================================================
//
// Two engines are supported:
//
// 1. Nemotron (DEFAULT) — nvidia/nemotron-3.5-asr-streaming-0.6b via ONNX Runtime
//    - 40 languages, 6.81% Hindi WER, single unified model
//    - Python sidecar: mcp/scripts/nemotron_transcriber.py
//    - LLM post-processing: mcp/scripts/llm_postprocessor.py (Devanagari→Hinglish)
//    - Word alignment: mcp/scripts/whisper_align.py (openai-whisper)
//
// 2. Apex (DEPRECATED) — Oriserve/Whisper-Hindi2Hinglish-Apex via whisper_timestamped
//    - Hindi/Hinglish only, 29.79% Hindi WER
//    - Requires conda env: whisper-hindi
//    - Python sidecar: mcp/scripts/apex_transcriber.py
//    - Kept for backward compatibility, will be removed in a future release.
//
// Nemotron is the recommended engine. Apex is kept as a deprecated fallback.
// =============================================================================

use std::path::{Path, PathBuf};
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

/// The transcription engine used.
/// Nemotron is the default; Apex is deprecated but kept for backward compat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionEngine {
    Nemotron,
    Apex,
}

impl std::fmt::Display for TranscriptionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nemotron => write!(f, "nemotron"),
            Self::Apex => write!(f, "apex"),
        }
    }
}

/// Result of the Apex health check (deprecated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApexHealth {
    /// Apex model loads and transcribes correctly.
    Healthy,
    /// Conda python exists but Apex model fails to load.
    PythonOkModelBroken { detail: String },
    /// Conda python not found at any known path.
    PythonMissing,
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
// Script resolution — Nemotron
// ---------------------------------------------------------------------------

/// Find the Nemotron transcription wrapper script.
///
/// Resolution order:
///   1. `OPENSCRIPT_NEMOTRON_WRAPPER` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/scripts/nemotron_transcriber.py`
///   3. `OPENSCRIPT_ROOT/mcp/scripts/nemotron_transcriber.py`
///   4. Relative paths (works if CWD is repo root or a crate dir)
fn find_nemotron_script() -> Option<PathBuf> {
    // 1. Explicit env var override
    if let Ok(path) = std::env::var("OPENSCRIPT_NEMOTRON_WRAPPER") {
        let p = Path::new(&path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    // 2. CARGO_MANIFEST_DIR
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = Path::new(&manifest_dir).join("../../mcp/scripts/nemotron_transcriber.py");
        if p.exists() {
            return Some(p);
        }
    }

    // 3. OPENSCRIPT_ROOT
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = Path::new(&root).join("mcp/scripts/nemotron_transcriber.py");
        if p.exists() {
            return Some(p);
        }
    }

    // 4. Relative fallbacks
    let relative_candidates = [
        PathBuf::from("mcp/scripts/nemotron_transcriber.py"),
        PathBuf::from("../mcp/scripts/nemotron_transcriber.py"),
        PathBuf::from("../../mcp/scripts/nemotron_transcriber.py"),
    ];
    for c in &relative_candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    None
}

/// Find the whisper alignment script (word-level timestamps).
fn find_whisper_align_script() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPENSCRIPT_WHISPER_ALIGN") {
        let p = Path::new(&path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = Path::new(&manifest_dir).join("../../mcp/scripts/whisper_align.py");
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = Path::new(&root).join("mcp/scripts/whisper_align.py");
        if p.exists() {
            return Some(p);
        }
    }

    let relative_candidates = [
        PathBuf::from("mcp/scripts/whisper_align.py"),
        PathBuf::from("../mcp/scripts/whisper_align.py"),
        PathBuf::from("../../mcp/scripts/whisper_align.py"),
    ];
    for c in &relative_candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    None
}

/// Find the LLM post-processor script (Devanagari → Hinglish).
fn find_llm_postprocessor_script() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPENSCRIPT_LLM_POSTPROCESSOR") {
        let p = Path::new(&path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = Path::new(&manifest_dir).join("../../mcp/scripts/llm_postprocessor.py");
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = Path::new(&root).join("mcp/scripts/llm_postprocessor.py");
        if p.exists() {
            return Some(p);
        }
    }

    let relative_candidates = [
        PathBuf::from("mcp/scripts/llm_postprocessor.py"),
        PathBuf::from("../mcp/scripts/llm_postprocessor.py"),
        PathBuf::from("../../mcp/scripts/llm_postprocessor.py"),
    ];
    for c in &relative_candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    None
}



// ---------------------------------------------------------------------------
// Script resolution — Apex (DEPRECATED)
// ---------------------------------------------------------------------------

/// Find the Apex transcription wrapper script (DEPRECATED).
fn find_apex_script() -> Option<PathBuf> {
    // 1. Explicit env var override
    if let Ok(path) = std::env::var("OPENSCRIPT_APEX_WRAPPER") {
        let p = Path::new(&path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    // 2. CARGO_MANIFEST_DIR
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = Path::new(&manifest_dir).join("../../mcp/scripts/apex_transcriber.py");
        if p.exists() {
            return Some(p);
        }
    }

    // 3. OPENSCRIPT_ROOT
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = Path::new(&root).join("mcp/scripts/apex_transcriber.py");
        if p.exists() {
            return Some(p);
        }
    }

    // 4-5. Relative fallbacks
    let relative_candidates = [
        PathBuf::from("mcp/scripts/apex_transcriber.py"),
        PathBuf::from("../mcp/scripts/apex_transcriber.py"),
        PathBuf::from("../../mcp/scripts/apex_transcriber.py"),
    ];
    for c in &relative_candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    None
}

/// Cross-platform home directory resolution.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Find the conda environment python (DEPRECATED — used by Apex only).
fn find_conda_python() -> Option<PathBuf> {
    // Priority 1: explicit env var
    if let Ok(path) = std::env::var("WHISPER_HINDI_PYTHON") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
    }

    // Priority 2: common conda/venv paths
    let home = home_dir()?;
    let candidates = [
        home.join("miniconda3/envs/whisper-hindi/bin/python3.11"),
        home.join("miniconda3/envs/whisper-hindi/bin/python3"),
        home.join("miniconda3/envs/whisper-hindi/bin/python"),
        home.join("anaconda3/envs/whisper-hindi/bin/python3.11"),
        home.join("anaconda3/envs/whisper-hindi/bin/python3"),
        home.join(".conda/envs/whisper-hindi/bin/python3.11"),
        home.join(".local/share/conda/envs/whisper-hindi/bin/python3.11"),
        home.join("miniconda3/envs/whisper-hindi/python.exe"),
        home.join("miniconda3/envs/whisper-hindi/Scripts/python.exe"),
        home.join("anaconda3/envs/whisper-hindi/python.exe"),
        home.join("anaconda3/envs/whisper-hindi/Scripts/python.exe"),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    None
}

/// Find system Python 3 (for Nemotron sidecar).
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
    if let Ok(output) = std::process::Command::new("which").arg("python3").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let p = PathBuf::from(&path);
            if p.exists() {
                return Some(p);
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Health check — Nemotron
// ---------------------------------------------------------------------------

/// Check if Nemotron transcription is available.
pub async fn check_nemotron_health() -> Result<String, String> {
    let _python = find_system_python().ok_or("System Python 3 not found".to_string())?;
    let script = find_nemotron_script()
        .ok_or("nemotron_transcriber.py not found".to_string())?;

    // Check that onnxruntime and sentencepiece are importable
    let mut cmd = Command::new("python3");
    cmd.arg("-c")
        .arg("import onnxruntime; import sentencepiece; print('ok')");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    match cmd.output().await {
        Ok(o) if o.status.success() => Ok(format!(
            "Nemotron available (script: {})",
            script.display()
        )),
        Ok(o) => Err(format!(
            "Nemotron Python deps missing: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .next()
                .unwrap_or("")
        )),
        Err(e) => Err(format!("Failed to check Nemotron health: {}", e)),
    }
}

/// Quick health check for Apex (DEPRECATED).
pub async fn check_apex_health() -> ApexHealth {
    let conda_python = match find_conda_python() {
        Some(p) => p,
        None => return ApexHealth::PythonMissing,
    };

    let mut cmd = Command::new(&conda_python);
    cmd.arg("-c").arg("import whisper_timestamped; print('ok')");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => {
            return ApexHealth::PythonOkModelBroken {
                detail: format!("Failed to run conda python: {}", e),
            }
        }
    };

    if !output.status.success() {
        return ApexHealth::PythonOkModelBroken {
            detail: format!(
                "whisper_timestamped import failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
            ),
        };
    }

    ApexHealth::Healthy
}

// =============================================================================
// TRANSCRIPTION — MAIN ENTRY POINT
// =============================================================================

/// Transcribe media to SRT.
///
/// This is the main transcription entry point. It routes to the appropriate
/// engine based on the `engine` parameter:
/// - `Nemotron` (default): Uses ONNX Runtime direct inference
/// - `Apex` (deprecated): Uses whisper_timestamped via conda env
///
/// Pipeline (Nemotron):
/// 1. Extract 16kHz mono WAV via ffmpeg
/// 2. Run Nemotron ASR via Python sidecar
/// 3. If Hindi: Run LLM post-processor (Devanagari → Hinglish)
/// 4. Generate word-level and phrase-level SRT
pub async fn transcribe(
    media_path: &str,
    output_srt_path: &str,
) -> Result<TranscribeResult, TranscribeError> {
    transcribe_with_engine(media_path, output_srt_path, TranscriptionEngine::Nemotron, "auto").await
}

/// Transcribe with a specific engine and language hint.
pub async fn transcribe_with_engine(
    media_path: &str,
    output_srt_path: &str,
    engine: TranscriptionEngine,
    language_hint: &str,
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
        TranscriptionEngine::Nemotron => {
            transcribe_nemotron(media_path, output_srt_path, &stem, out_dir, language_hint).await
        }
        #[allow(deprecated)]
        TranscriptionEngine::Apex => {
            tracing::warn!("Apex engine is deprecated. Use Nemotron instead.");
            transcribe_apex(media_path, output_srt_path, &stem, out_dir).await
        }
    }
}

// =============================================================================
// TRANSCRIPTION — NEMOTRON ENGINE
// =============================================================================

/// Transcribe using Nemotron 3.5 ASR via ONNX Runtime.
async fn transcribe_nemotron(
    media_path: &str,
    output_srt_path: &str,
    stem: &str,
    out_dir: &Path,
    language_hint: &str,
) -> Result<TranscribeResult, TranscribeError> {
    let wrapper = find_nemotron_script().ok_or_else(|| {
        TranscribeError::WrapperNotFound(
            "nemotron_transcriber.py not found. Set OPENSCRIPT_NEMOTRON_WRAPPER or \
             ensure mcp/scripts/nemotron_transcriber.py exists."
                .into(),
        )
    })?;

    let python = find_system_python().ok_or_else(|| {
        TranscribeError::PythonNotFound(
            "System Python 3 not found. Set OPENSCRIPT_PYTHON env var.".into(),
        )
    })?;

    // Step 1: Run Nemotron transcription
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

    let output = cmd.output().await.map_err(|e| TranscribeError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TranscribeError::TranscriptionFailed {
            engine: "nemotron".into(),
            detail: stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n"),
        });
    }

    // Parse JSON output from the sidecar
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        TranscribeError::TranscriptionFailed {
            engine: "nemotron".into(),
            detail: format!("Failed to parse sidecar output: {}", e),
        }
    })?;

    if result.get("error").is_some() {
        return Err(TranscribeError::TranscriptionFailed {
            engine: "nemotron".into(),
            detail: result["error"].as_str().unwrap_or("unknown error").to_string(),
        });
    }

    // Get output paths from sidecar result
    let phrase_srt = result["phrase_srt_path"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            out_dir
                .join(format!("{}.nemotron.phrase.srt", stem))
                .to_string_lossy()
                .to_string()
        });

    // Step 2: Run whisper_align.py for real word-level timestamps
    // The nemotron sidecar produces estimated word timings (evenly distributed).
    // whisper_align.py uses openai-whisper to get actual frame-accurate timestamps.
    let word_srt = result["word_srt_path"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            out_dir
                .join(format!("{}.nemotron.word.srt", stem))
                .to_string_lossy()
                .to_string()
        });

    // Read the transcript text for alignment — strip SRT metadata, keep only text lines
    let transcript_text = if Path::new(output_srt_path).exists() {
        let srt_content = std::fs::read_to_string(output_srt_path).unwrap_or_default();
        // SRT format: index, timestamp, text, blank line. Extract only text lines.
        let text_lines: Vec<&str> = srt_content
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty()
                    && !trimmed.parse::<usize>().is_ok()  // skip index numbers
                    && !trimmed.contains("-->")           // skip timestamps
            })
            .collect();
        text_lines.join(" ")
    } else {
        String::new()
    };

    if !transcript_text.is_empty() && Path::new(media_path).exists() {
        if let Some(align_script) = find_whisper_align_script() {
            tracing::info!("Running whisper_align.py for word-level alignment");
            let align_output_dir = out_dir.to_string_lossy().to_string();
            let mut align_cmd = Command::new(&python);
            align_cmd
                .arg(&align_script)
                .arg("--wav")
                .arg(media_path)
                .arg("--text")
                .arg(&transcript_text)
                .arg("--language")
                .arg(language_hint)
                .arg("--model")
                .arg("base")
                .arg("--out-dir")
                .arg(&align_output_dir)
                .kill_on_drop(true);

            match align_cmd.output().await {
                Ok(o) if o.status.success() => {
                    tracing::info!("Whisper alignment complete");
                    // Use the aligned phrase SRT from whisper_align.py
                    let aligned_phrase = out_dir
                        .join(format!("{}.phrase.srt", stem));
                    if aligned_phrase.exists() {
                        tracing::info!("Using whisper-aligned phrase SRT");
                        // Copy aligned phrase to output path
                        if let Ok(content) = std::fs::read_to_string(&aligned_phrase) {
                            let _ = std::fs::write(output_srt_path, content);
                        }
                    }
                }
                Ok(o) => {
                    tracing::warn!(
                        "Whisper alignment failed (falling back to estimated timings): {}",
                        String::from_utf8_lossy(&o.stderr)
                            .lines()
                            .last()
                            .unwrap_or("")
                    );
                }
                Err(e) => {
                    tracing::warn!("Whisper alignment subprocess failed: {}", e);
                }
            }
        } else {
            tracing::warn!("whisper_align.py not found, using estimated word timings");
        }
    }

    // Copy phrase SRT to output path if it exists
    if Path::new(&phrase_srt).exists() {
        let content = std::fs::read_to_string(&phrase_srt).map_err(|e| TranscribeError::Io(e))?;
        std::fs::write(output_srt_path, content).map_err(|e| TranscribeError::Io(e))?;
    }

    // Step 2: If Hindi, run LLM post-processor (Devanagari → Hinglish)
    let is_hindi = language_hint.starts_with("hi")
        || result
            .get("language")
            .and_then(|v: &serde_json::Value| v.as_str())
            .map_or(false, |lang: &str| lang.starts_with("hi"));

    if is_hindi && Path::new(output_srt_path).exists() {
        if let Some(llm_script) = find_llm_postprocessor_script() {
            tracing::info!("Running LLM post-processor (Devanagari → Hinglish)");
            let mut llm_cmd = Command::new(&python);
            llm_cmd
                .arg(&llm_script)
                .arg("file")
                .arg("--input")
                .arg(output_srt_path)
                .arg("--output")
                .arg(output_srt_path)
                .kill_on_drop(true);

            match llm_cmd.output().await {
                Ok(o) if o.status.success() => {
                    tracing::info!("LLM post-processing complete");
                }
                Ok(o) => {
                    tracing::warn!(
                        "LLM post-processing failed: {}",
                        String::from_utf8_lossy(&o.stderr)
                            .lines()
                            .last()
                            .unwrap_or("")
                    );
                }
                Err(e) => {
                    tracing::warn!("LLM post-processing subprocess failed: {}", e);
                }
            }
        } else {
            tracing::warn!("LLM post-processor not found, skipping Devanagari → Hinglish conversion");
        }
    }

    build_result(output_srt_path, stem, out_dir, TranscriptionEngine::Nemotron)
}

// =============================================================================
// TRANSCRIPTION — APEX ENGINE (DEPRECATED)
// =============================================================================

/// Transcribe using Apex model via conda env (DEPRECATED).
#[allow(deprecated)]
async fn transcribe_apex(
    media_path: &str,
    output_srt_path: &str,
    stem: &str,
    out_dir: &Path,
) -> Result<TranscribeResult, TranscribeError> {
    let wrapper = find_apex_script().ok_or_else(|| {
        TranscribeError::WrapperNotFound(
            "apex_transcriber.py not found. Set OPENSCRIPT_APEX_WRAPPER or \
             ensure mcp/scripts/apex_transcriber.py exists."
                .into(),
        )
    })?;

    let conda_python = find_conda_python().ok_or_else(|| {
        TranscribeError::PythonNotFound(
            "Conda environment python not found. Set WHISPER_HINDI_PYTHON env var \
             or install at ~/miniconda3/envs/whisper-hindi/bin/python3.11"
                .into(),
        )
    })?;

    let mut cmd = Command::new(&conda_python);
    cmd.arg(&wrapper)
        .arg("run")
        .arg("--video")
        .arg(media_path)
        .arg("--out-dir")
        .arg(out_dir)
        .kill_on_drop(true);

    let output = cmd.output().await.map_err(|e| TranscribeError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TranscribeError::TranscriptionFailed {
            engine: "apex".into(),
            detail: stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n"),
        });
    }

    // Apex creates {stem}.apex.phrase.srt — copy to expected output path
    let phrase_srt = out_dir.join(format!("{}.apex.phrase.srt", stem));
    if phrase_srt.exists() {
        if let Ok(content) = std::fs::read_to_string(&phrase_srt) {
            std::fs::write(output_srt_path, content).map_err(|e| TranscribeError::Io(e))?;
        }
    }

    build_result(output_srt_path, stem, out_dir, TranscriptionEngine::Apex)
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

    let content = std::fs::read_to_string(output_srt_path).map_err(|e| TranscribeError::Io(e))?;

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

    // Find word/phrase SRT files for the appropriate engine
    let (word_srt_pattern, phrase_srt_pattern) = match engine {
        TranscriptionEngine::Nemotron => (
            format!("{}/{}.nemotron.word.srt", out_dir_str, stem),
            format!("{}/{}.nemotron.phrase.srt", out_dir_str, stem),
        ),
        #[allow(deprecated)]
        TranscriptionEngine::Apex => (
            format!("{}/{}.apex.word.srt", out_dir_str, stem),
            format!("{}/{}.apex.phrase.srt", out_dir_str, stem),
        ),
    };

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
    fn test_find_nemotron_script() {
        let result = find_nemotron_script();
        // On a dev machine with the repo checked out, this should find the file
        assert!(result.is_some() || std::env::var("OPENSCRIPT_NEMOTRON_WRAPPER").is_err());
    }

    #[test]
    fn test_find_apex_script() {
        let result = find_apex_script();
        assert!(result.is_some() || std::env::var("OPENSCRIPT_APEX_WRAPPER").is_err());
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
        assert_eq!(TranscriptionEngine::Nemotron.to_string(), "nemotron");
        assert_eq!(TranscriptionEngine::Apex.to_string(), "apex");
    }
}
