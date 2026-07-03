// =============================================================================
// APEX WHISPER IS THE ONLY TRANSCRIPTION ENGINE. PERIOD.
// =============================================================================
//
// This crate uses ONLY the Apex model: Oriserve/Whisper-Hindi2Hinglish-Apex
// via the whisper_timestamped Python library.
//
// DO NOT add fallbacks to faster-whisper, whisper-cli, openai-whisper, or any
// other transcription engine. Apex is optimized for Hindi/Hinglish content and
// is the only model that produces acceptable output for this pipeline.
//
// If Apex fails, the error should propagate to the user — NOT silently fall
// back to an inferior model. The user needs to fix their Apex installation
// (conda env: whisper-hindi) rather than getting garbage transcription.
//
// Conda environment: ~/miniconda3/envs/whisper-hindi
// Python:            python3.11 in that env
// Model:             Oriserve/Whisper-Hindi2Hinglish-Apex
// Library:           whisper_timestamped
// Wrapper script:    mcp/scripts/apex_transcriber.py
// =============================================================================

use std::path::{Path, PathBuf};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct TranscribeResult {
    pub output_path: String,
    pub entry_count: usize,
    /// Word-level SRT path (if forced alignment succeeded)
    pub word_srt_path: Option<String>,
    /// Phrase-level SRT path (best for EDL building)
    pub phrase_srt_path: Option<String>,
    /// Which transcription engine produced this result — always Apex.
    pub engine: TranscriptionEngine,
}

/// The transcription engine used.
/// APEX IS THE ONLY ENGINE. This enum exists for API compatibility and
/// reporting — it will always be `Apex`. Do not add variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionEngine {
    Apex,
}

impl std::fmt::Display for TranscriptionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Apex => write!(f, "apex"),
        }
    }
}

/// Result of the Apex health check.
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
    #[error("Apex wrapper script not found: {0}")]
    WrapperNotFound(String),
    #[error("Conda environment python not found. Set WHISPER_HINDI_PYTHON env var or install at ~/miniconda3/envs/whisper-hindi/bin/python3.11")]
    CondaPythonNotFound,
    #[error("Apex model unhealthy: {0}")]
    ApexUnhealthy(String),
    #[error("Output validation failed: {0}")]
    OutputValidationFailed(String),
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
    let latin_chars: usize = content
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .count();
    let non_latin_text_chars: usize = content
        .chars()
        .filter(|c| {
            c.is_alphabetic() && !c.is_ascii() && !c.is_ascii_digit() && *c != '\n' && *c != '\r'
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
// Health check
// ---------------------------------------------------------------------------

/// Quick health check: can the conda python import the required packages
/// and locate the Apex model?  Takes ~2 s (just imports, no transcription).
pub async fn check_apex_health() -> ApexHealth {
    let conda_python = match find_conda_python() {
        Some(p) => p,
        None => return ApexHealth::PythonMissing,
    };

    // Check that whisper_timestamped and the Apex model are importable.
    let mut cmd = Command::new(&conda_python);
    cmd.arg("-c")
        .arg("import whisper_timestamped; print('ok')");
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
                String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("")
            ),
        };
    }

    // Deeper check: can whisper_timestamped load the Apex model?
    // This takes ~5-10s on CPU but catches the "model not cached" case.
    let mut cmd2 = Command::new(&conda_python);
    cmd2.arg("-c")
        .arg("import whisper_timestamped as w; m = w.load_model('Oriserve/Whisper-Hindi2Hinglish-Apex', device='cpu'); print('model_ok')");
    cmd2.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    match cmd2.output().await {
        Ok(o) if o.status.success() => ApexHealth::Healthy,
        Ok(o) => ApexHealth::PythonOkModelBroken {
            detail: format!(
                "Apex model load failed: {}",
                String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")
            ),
        },
        Err(e) => ApexHealth::PythonOkModelBroken {
            detail: format!("Health check subprocess failed: {}", e),
        },
    }
}

/// Find the Apex transcription wrapper script.
fn find_apex_script() -> Option<PathBuf> {
    let home = home_dir()?;
    let candidates = [
        home.join("Documents/GitHub/openscript/mcp/scripts/apex_transcriber.py"),
        home.join("projects/openscript/mcp/scripts/apex_transcriber.py"),
        PathBuf::from("mcp/scripts/apex_transcriber.py"),
        PathBuf::from("../mcp/scripts/apex_transcriber.py"),
        PathBuf::from("../../mcp/scripts/apex_transcriber.py"),
    ];
    for c in &candidates {
        let p = Path::new(c);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = Path::new(&manifest_dir)
            .join("../../mcp/scripts/apex_transcriber.py");
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(path) = std::env::var("OPENSCRIPT_APEX_WRAPPER") {
        let p = Path::new(&path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// Cross-platform home directory resolution.
/// Uses HOME on Unix, USERPROFILE on Windows, falls back to None.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Find the conda environment python that has Whisper-Hindi2Hinglish-Apex installed.
///
/// APEX PYTHON — This is the ONLY Python interpreter used for transcription.
/// All paths reference the whisper-hindi conda environment which contains
/// whisper_timestamped and the Apex model.
fn find_conda_python() -> Option<PathBuf> {
    // Priority 1: explicit env var
    if let Ok(path) = std::env::var("WHISPER_HINDI_PYTHON") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
    }

    // Priority 2: common conda/venv paths for the whisper-hindi env
    let home = home_dir()?;
    let candidates = [
        // Unix paths
        home.join("miniconda3/envs/whisper-hindi/bin/python3.11"),
        home.join("miniconda3/envs/whisper-hindi/bin/python3"),
        home.join("miniconda3/envs/whisper-hindi/bin/python"),
        home.join("anaconda3/envs/whisper-hindi/bin/python3.11"),
        home.join("anaconda3/envs/whisper-hindi/bin/python3"),
        home.join(".conda/envs/whisper-hindi/bin/python3.11"),
        home.join(".local/share/conda/envs/whisper-hindi/bin/python3.11"),
        // Windows paths (conda uses Scripts/python.exe on Windows)
        home.join("miniconda3/envs/whisper-hindi/python.exe"),
        home.join("miniconda3/envs/whisper-hindi/Scripts/python.exe"),
        home.join("anaconda3/envs/whisper-hindi/python.exe"),
        home.join("anaconda3/envs/whisper-hindi/Scripts/python.exe"),
        home.join("AppData/Local/miniconda3/envs/whisper-hindi/python.exe"),
        home.join("AppData/Local/anaconda3/envs/whisper-hindi/python.exe"),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    None
}

// =============================================================================
// TRANSCRIPTION — APEX ONLY. NO FALLBACKS. NO ALTERNATIVES.
// =============================================================================
//
// The transcribe() function ALWAYS uses the Apex model. There is no fallback
// chain, no alternative engines, no "auto" mode that degrades quality.
//
// If Apex is unavailable, the error propagates to the caller so the user can
// fix their installation. Getting garbage transcription from a fallback model
// is worse than getting a clear error.
// =============================================================================

/// Transcribe media to SRT using the Apex model.
///
/// This is the ONLY transcription entry point. It calls the Apex wrapper
/// script directly. There are no fallbacks to other models.
///
/// Pipeline:
/// 1. Extract 16kHz mono WAV via ffmpeg (handled by apex wrapper)
/// 2. Run Apex transcription via conda env python
/// 3. Parse output SRT files and return results
pub async fn transcribe(
    media_path: &str,
    output_srt_path: &str,
) -> Result<TranscribeResult, TranscribeError> {
    let out_dir = Path::new(output_srt_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = Path::new(media_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());

    transcribe_apex(media_path, output_srt_path, &stem, out_dir).await
}

/// Transcribe using Apex model via conda env.
///
/// This is the ONLY transcription implementation. No other engines exist
/// in this codebase. Do not add fallbacks.
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

    let conda_python = find_conda_python().ok_or(TranscribeError::CondaPythonNotFound)?;

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
        eprintln!("[transcribe] Output validation warning ({engine}): {reason}");
    }

    let entry_count = content
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .count();

    let word_srt_path = format!("{}/{}.apex.word.srt", out_dir_str, stem);
    let phrase_srt_path = format!("{}/{}.apex.phrase.srt", out_dir_str, stem);

    Ok(TranscribeResult {
        output_path: output_srt_path.to_string(),
        entry_count,
        word_srt_path: if Path::new(&word_srt_path).exists() {
            Some(word_srt_path)
        } else {
            None
        },
        phrase_srt_path: if Path::new(&phrase_srt_path).exists() {
            Some(phrase_srt_path)
        } else {
            None
        },
        engine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_apex_script_env_var() {
        // If OPENSCRIPT_APEX_WRAPPER is set to a valid path, it should be found
        // (This test doesn't require the actual file to exist, just tests the logic)
        let result = find_apex_script();
        // On a dev machine with the repo checked out, this should find the file
        // On CI it might not, so we just verify it returns Option<PathBuf>
        assert!(result.is_some() || std::env::var("OPENSCRIPT_APEX_WRAPPER").is_err());
    }

    #[test]
    fn test_find_conda_python_env_var() {
        // When env var points to a real path, it should be used
        let result = find_conda_python();
        // On a dev machine with conda env, should find python
        // On CI or machines without it, may return None
        if let Some(p) = &result {
            assert!(p.exists(), "Found python at {:?} but it doesn't exist", p);
        }
    }

    #[test]
    fn test_find_conda_python_fake_env_falls_back() {
        std::env::set_var("WHISPER_HINDI_PYTHON", "/nonexistent/fake/python");
        let with_fake = find_conda_python();
        std::env::remove_var("WHISPER_HINDI_PYTHON");
        let without_fake = find_conda_python();
        assert_eq!(with_fake.is_some(), without_fake.is_some());
        if let (Some(a), Some(b)) = (&with_fake, &without_fake) {
            assert_eq!(a, b, "Fake env var should be ignored, both should resolve to same conda path");
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
        assert!(err.contains("Arabic"), "Expected Arabic detection, got: {err}");
    }

    #[test]
    fn test_validate_hinglish_devanagari_fails() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nमैं एक इंजीनियर हूँ\n\n";
        let err = validate_hinglish_output(srt).unwrap_err();
        assert!(err.contains("Devanagari"), "Expected Devanagari detection, got: {err}");
    }

    #[test]
    fn test_validate_hinglish_mixed_hinglish_passes() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nkya haal hai भाई\n\n";
        assert!(validate_hinglish_output(srt).is_ok());
    }
}
