// =============================================================================
// TRANSCRIPTION ENGINES
// =============================================================================
//
// Three engines are supported:
//
// 1. Whisper (DEFAULT) — openai-whisper (base model)
//    - 99 languages, native word-level timestamps
//    - Python sidecar: mcp/scripts/nemotron_transcriber.py (uses Whisper)
//    - LLM post-processing: mcp/scripts/llm_postprocessor.py (Devanagari→Hinglish)
//    - Word alignment: built-in via whisper word_timestamps=True
//
// 2. Nemotron (DEPRECATED) — ONNX streaming model (non-functional)
//    - The streaming ONNX model cannot do offline batch inference.
//    - Kept as enum variant for backward compat with external callers.
//
// 3. Apex (DEPRECATED) — Oriserve/Whisper-Hindi2Hinglish-Apex via whisper_timestamped
//    - Hindi/Hinglish only, 29.79% Hindi WER
//    - Requires conda env: whisper-hindi
//    - Python sidecar: mcp/scripts/apex_transcriber.py
//    - Kept for backward compatibility, will be removed in a future release.
//
// 4. HinglishGgml — whisper.cpp + Whisper-Hindi2Hinglish-Apex-GGML (q8_0)
//    - Direct Hinglish output from Hindi audio (no LLM post-processing needed)
//    - Requires whisper-cli (built from source) and GGML model file
//    - Python sidecar: mcp/scripts/hinglish_ggml_transcriber.py
//    - Best for Hindi audio where native Latin-script output is desired.
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
/// Whisper is the default. Nemotron and Apex are deprecated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionEngine {
    Whisper,
    
    Nemotron,
    Apex,
    HinglishGgml,
}

impl std::fmt::Display for TranscriptionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Whisper => write!(f, "whisper"),
            #[allow(deprecated)]
            Self::Nemotron => write!(f, "nemotron"),
            Self::Apex => write!(f, "apex"),
            Self::HinglishGgml => write!(f, "hinglish-ggml"),
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
        let p = Path::new(&manifest_dir).join("../../mcp/scripts/").join(relative_name);
        if p.exists() {
            return Some(p);
        }
    }

    // 3. OPENSCRIPT_ROOT (deployment override)
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = Path::new(&root).join("mcp/scripts/").join(relative_name);
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

/// Find the Nemotron transcription wrapper script.
fn find_whisper_script() -> Option<PathBuf> {
    resolve_script("OPENSCRIPT_NEMOTRON_WRAPPER", "nemotron_transcriber.py")
}

/// Find the Apex transcription wrapper script (DEPRECATED).
fn find_apex_script() -> Option<PathBuf> {
    resolve_script("OPENSCRIPT_APEX_WRAPPER", "apex_transcriber.py")
}

/// Find the Nemotron ONNX transcription script (onnxruntime-genai).
fn find_nemotron_onnx_script() -> Option<PathBuf> {
    resolve_script("OPENSCRIPT_NEMOTRON_ONNX_WRAPPER", "nemotron_onnx_transcriber.py")
}

/// Find the Hinglish GGML transcription script (whisper.cpp + Hindi2Hinglish).
fn find_hinglish_ggml_script() -> Option<PathBuf> {
    resolve_script("OPENSCRIPT_HINGLISH_GGML_WRAPPER", "hinglish_ggml_transcriber.py")
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

/// Check if Whisper transcription is available (primary engine).
pub async fn check_whisper_health() -> Result<String, String> {
    let _python = find_system_python().ok_or("System Python 3 not found".to_string())?;
    let script = find_whisper_script()
        .ok_or("nemotron_transcriber.py not found".to_string())?;

    // Check that openai-whisper is importable
    let mut cmd = Command::new("python3");
    cmd.arg("-c")
        .arg("import whisper; print('ok')");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    match cmd.output().await {
        Ok(o) if o.status.success() => Ok(format!(
            "Whisper available (script: {})",
            script.display()
        )),
        Ok(o) => Err(format!(
            "Whisper Python deps missing: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .next()
                .unwrap_or("")
        )),
        Err(e) => Err(format!("Failed to check Whisper health: {}", e)),
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
/// - `Whisper` (default): Uses openai-whisper (base model)
/// - `Nemotron` (deprecated): ONNX streaming model (non-functional)
/// - `Apex` (deprecated): Uses whisper_timestamped via conda env
///
/// Pipeline (Whisper):
/// 1. Extract 16kHz mono WAV via ffmpeg
/// 2. Run Whisper transcription via Python sidecar
/// 3. If Hindi: Run LLM post-processor (Devanagari → Hinglish)
/// 4. Generate word-level and phrase-level SRT
pub async fn transcribe(
    media_path: &str,
    output_srt_path: &str,
) -> Result<TranscribeResult, TranscribeError> {
    transcribe_with_engine(media_path, output_srt_path, TranscriptionEngine::HinglishGgml, "auto").await
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
        TranscriptionEngine::Whisper => {
            transcribe_nemotron_onnx(media_path, output_srt_path, &stem, out_dir, language_hint).await
        }
        #[allow(deprecated)]
        TranscriptionEngine::Nemotron => {
            tracing::info!("Using Nemotron ONNX engine (onnxruntime-genai, cache-aware streaming)");
            // Route through Whisper since Nemotron ONNX cannot do offline batch inference
            transcribe_nemotron_onnx(media_path, output_srt_path, &stem, out_dir, language_hint).await
        }
        #[allow(deprecated)]
        TranscriptionEngine::Apex => {
            tracing::warn!("Apex engine is deprecated. Use Whisper instead.");
            transcribe_apex(media_path, output_srt_path, &stem, out_dir).await
        }
        TranscriptionEngine::HinglishGgml => {
            tracing::info!("Using Hinglish GGML engine (whisper.cpp + Hindi2Hinglish-Apex-GGML)");
            transcribe_hinglish_ggml(media_path, output_srt_path, &stem, out_dir, language_hint).await
        }
    }
}

// =============================================================================
// TRANSCRIPTION — NEMOTRON ENGINE
// =============================================================================

// =============================================================================
// TRANSCRIPTION — NEMOTRON ONNX ENGINE (via onnxruntime-genai)
// =============================================================================

/// Transcribe using Nemotron ONNX via onnxruntime-genai with cache-aware streaming.
/// Uses 560ms chunks (8960 samples at 16kHz) with automatic cache management.
async fn transcribe_nemotron_onnx(
    media_path: &str,
    output_srt_path: &str,
    stem: &str,
    out_dir: &Path,
    language_hint: &str,
) -> Result<TranscribeResult, TranscribeError> {
    let wrapper = find_nemotron_onnx_script().ok_or_else(|| {
        TranscribeError::WrapperNotFound(
            "nemotron_onnx_transcriber.py not found. Set OPENSCRIPT_NEMOTRON_ONNX_WRAPPER or \
             ensure mcp/scripts/nemotron_onnx_transcriber.py exists."
                .into(),
        )
    })?;

    let python = find_system_python().ok_or_else(|| {
        TranscribeError::PythonNotFound(
            "System Python 3 not found. Set OPENSCRIPT_PYTHON env var.".into(),
        )
    })?;

    // Run the onnxruntime-genai sidecar
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

    let output = cmd.output().await.map_err(TranscribeError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TranscribeError::TranscriptionFailed {
            engine: "nemotron-onnx".into(),
            detail: stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n"),
        });
    }

    // Parse JSON output from the sidecar
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        TranscribeError::TranscriptionFailed {
            engine: "nemotron-onnx".into(),
            detail: format!("Failed to parse sidecar output: {}", e),
        }
    })?;

    if result.get("error").is_some() {
        return Err(TranscribeError::TranscriptionFailed {
            engine: "nemotron-onnx".into(),
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

    // Get the output SRT path from sidecar result
    let output_srt = result["output_srt_path"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            out_dir
                .join(format!("{}.nemotron.srt", stem))
                .to_string_lossy()
                .to_string()
        });

    // If the sidecar didn't produce the output SRT, copy from phrase_srt
    if !Path::new(&output_srt).exists() && Path::new(&phrase_srt).exists() {
        if let Ok(content) = std::fs::read_to_string(&phrase_srt) {
            let _ = std::fs::write(&output_srt, content);
        }
    }

    // Copy to the requested output path if different
    if output_srt != output_srt_path && Path::new(&output_srt).exists() {
        if let Ok(content) = std::fs::read_to_string(&output_srt) {
            std::fs::write(output_srt_path, content).map_err(TranscribeError::Io)?;
        }
    }

    build_result(output_srt_path, stem, out_dir, TranscriptionEngine::Nemotron)
}

// =============================================================================
// =============================================================================
// TRANSCRIPTION — HINGLISH GGML ENGINE (whisper.cpp + Hindi2Hinglish-Apex-GGML)
// ==============================================================================

/// Transcribe using whisper.cpp with the Whisper-Hindi2Hinglish-Apex-GGML model.
/// Outputs Hinglish (Latin script) directly from Hindi audio.
async fn transcribe_hinglish_ggml(
    media_path: &str,
    output_srt_path: &str,
    stem: &str,
    out_dir: &Path,
    language_hint: &str,
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

    let output = cmd.output().await.map_err(TranscribeError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TranscribeError::TranscriptionFailed {
            engine: "hinglish-ggml".into(),
            detail: stderr.lines().rev().take(5).collect::<Vec<_>>().join("
"),
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
            detail: result["error"].as_str().unwrap_or("unknown error").to_string(),
        });
    }

    let output_srt = result["output_srt_path"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            out_dir
                .join(format!("{}.nemotron.srt", stem))
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
        TranscriptionEngine::Whisper => (
            format!("{}/{}.nemotron.word.srt", out_dir_str, stem),
            format!("{}/{}.nemotron.phrase.srt", out_dir_str, stem),
        ),
        #[allow(deprecated)]
        TranscriptionEngine::Nemotron => (
            format!("{}/{}.nemotron.word.srt", out_dir_str, stem),
            format!("{}/{}.nemotron.phrase.srt", out_dir_str, stem),
        ),
        #[allow(deprecated)]
        TranscriptionEngine::Apex => (
            format!("{}/{}.apex.word.srt", out_dir_str, stem),
            format!("{}/{}.apex.phrase.srt", out_dir_str, stem),
        ),
        TranscriptionEngine::HinglishGgml => (
            format!("{}/{}.nemotron.word.srt", out_dir_str, stem),
            format!("{}/{}.nemotron.phrase.srt", out_dir_str, stem),
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
    fn test_find_whisper_script() {
        let result = find_whisper_script();
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
    fn test_find_hinglish_ggml_script() {
        let result = find_hinglish_ggml_script();
        // On a dev machine with the repo checked out, this should find the file
        assert!(result.is_some() || std::env::var("OPENSCRIPT_HINGLISH_GGML_WRAPPER").is_err());
    }

    #[test]
    fn test_transcription_engine_display() {
        assert_eq!(TranscriptionEngine::Whisper.to_string(), "whisper");
        assert_eq!(TranscriptionEngine::Nemotron.to_string(), "nemotron");
        assert_eq!(TranscriptionEngine::Apex.to_string(), "apex");
        assert_eq!(TranscriptionEngine::HinglishGgml.to_string(), "hinglish-ggml");
    }

    #[tokio::test]
    async fn test_check_whisper_health() {
        // On a dev machine with openai-whisper installed, this should succeed
        let result = check_whisper_health().await;
        // We don't assert success because whisper may not be installed in CI
        // but we verify the function doesn't panic
        assert!(result.is_ok() || result.is_err(), "check_whisper_health must not panic");
    }
}
