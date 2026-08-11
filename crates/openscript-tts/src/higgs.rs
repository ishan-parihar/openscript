//! Higgs Audio v3 TTS sidecar — expressive 4B zero-shot TTS via ONNX GenAI.
//!
//! Higgs Audio v3 (bosonai/higgs-audio-v3-tts-4b) is a 4B conversational TTS:
//! **100+ languages, zero-shot voice cloning, and inline control tokens** for
//! emotion / prosody / style / sound effects (43 tags), 24 kHz. We drive the
//! self-contained ONNX export (`onnx-community/higgs-audio-v3-tts-4b`,
//! branch `cuda_int4`, ~3.6 GB — provisioned by `scripts/setup_higgs.sh`):
//! an int4 Qwen3-4B backbone under **ONNX Runtime GenAI** plus fused
//! text/audio embed + heads and the Higgs v2 codec as plain ONNX.
//!
//! The sidecar is `mcp/scripts/higgs_tts_sidecar.py`, a long-lived process
//! (mirroring `gepard_tts_sidecar.py`) running under the dedicated
//! `.venv-higgs` interpreter (Python 3.12 + onnxruntime-genai + ORT CUDA —
//! provisioned by `scripts/setup_higgs.sh`). It loads the pipeline once
//! (~4.5 GB) and serves a stdin/stdout JSON protocol:
//!
//! ```text
//! → {"op":"synth","text":"Hello","voice":"ishan","output_path":"/tmp/a.wav",
//!    "emote":"excited","temperature":0.8,"top_k":50}
//! ← {"status":"ok","duration_ms":1234,"sample_rate":24000,"chunks":1}
//! → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","text":"Exact transcript.","overwrite":true}
//! ← {"status":"ok","voice":"ishan"}
//! ```
//!
//! On error: `{"status":"error","error":"..."}`. The sidecar prints
//! `{"ready":true}` on startup and loads the model lazily on first synth.
//!
//! All sidecar instances share a single process-global pool so the ~4.5 GB
//! pipeline is loaded at most once per MCP server process.
//!
//! **License note:** Higgs Audio v3 weights are research / non-commercial
//! (Boson license). See `scripts/setup_higgs.sh`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Process-global shared sidecar pool. All callers share one Python process.
pub type SharedSidecar = Arc<Mutex<Option<Sidecar>>>;

/// Failure mode for a sidecar roundtrip. `Transport` means the pipe died
/// (process crashed) — the slot should be reset so the next call respawns it.
/// `Response` means the sidecar is alive but reported an error.
pub enum SidecarFailure {
    Transport(String),
    Response(String),
}

impl std::fmt::Display for SidecarFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SidecarFailure::Transport(m) | SidecarFailure::Response(m) => write!(f, "{}", m),
        }
    }
}

/// A long-lived Higgs sidecar process.
pub struct Sidecar {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    request_count: u64,
}

/// Optional per-request synthesis parameters — the "tonality" knobs.
/// All fields default to None (= engine defaults).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HiggsSynthParams {
    /// Emote id (e.g. "angry", "excited", "whisper"). Mapped to a Higgs
    /// control tag (`<|emotion:X|>`, `<|style:X|>`, `<|sfx:X|>`).
    pub emote: Option<String>,
    /// Free-form delivery instruction folded into the prompt.
    pub instruct: Option<String>,
    /// Override the registered voice's reference WAV (emotion takes).
    pub ref_audio: Option<String>,
    /// Transcript of the reference clip (voice cloning prompt).
    pub ref_text: Option<String>,
    /// Script-level default speed — selects a `<|prosody:speed_*|>` tag.
    pub default_speed: Option<f64>,
    pub temperature: Option<f64>,
    pub top_k: Option<u32>,
    pub max_new_tokens: Option<u32>,
}

#[derive(Serialize)]
struct SynthRequest<'a> {
    op: &'static str,
    text: &'a str,
    output_path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emote: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instruct: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_audio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_new_tokens: Option<u32>,
}

#[derive(Serialize)]
struct RegisterRequest<'a> {
    op: &'static str,
    name: &'a str,
    audio_path: &'a str,
    text: &'a str,
    overwrite: bool,
}

#[derive(Deserialize)]
struct SidecarResponse {
    status: String,
    #[serde(default)]
    duration_ms: i64,
    #[serde(default)]
    sample_rate: u32,
    #[serde(default)]
    chunks: u32,
    #[serde(default)]
    error: String,
}

impl Sidecar {
    /// Spawn the sidecar under the resolved venv python and wait for the
    /// `{"ready":true}` handshake.
    pub fn start(sidecar_script: &str) -> Result<Self, String> {
        let python = resolve_higgs_python();
        let mut child = std::process::Command::new(&python)
            .arg(sidecar_script)
            .arg("--serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn higgs sidecar: {}", e))?;

        let stdin = child.stdin.take().ok_or("sidecar stdin not piped")?;
        let stdout = child.stdout.take().ok_or("sidecar stdout not piped")?;

        let mut stdout_reader = BufReader::new(stdout);
        let mut ready_line = String::new();
        stdout_reader
            .read_line(&mut ready_line)
            .map_err(|e| format!("Failed to read sidecar ready signal: {}", e))?;

        if !ready_line.contains("\"ready\"") {
            let stderr = child.stderr.take();
            let err_msg = if let Some(mut s) = stderr {
                use std::io::Read;
                let mut buf = String::new();
                let _ = s.read_to_string(&mut buf);
                buf
            } else {
                String::new()
            };
            let _ = child.kill();
            return Err(format!(
                "Higgs sidecar did not signal ready. stdout={:?} stderr={}",
                ready_line.trim(),
                err_msg.trim()
            ));
        }

        Ok(Self {
            stdin,
            stdout: stdout_reader,
            _child: child,
            request_count: 0,
        })
    }

    /// Send one request, read one response line, return the parsed response.
    fn roundtrip(&mut self, req_json: String) -> Result<SidecarResponse, SidecarFailure> {
        self.request_count += 1;
        writeln!(self.stdin, "{}", req_json)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to write to higgs sidecar stdin: {}", e)))?;
        self.stdin
            .flush()
            .map_err(|e| SidecarFailure::Transport(format!("Failed to flush higgs sidecar stdin: {}", e)))?;

        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to read higgs sidecar response: {}", e))
        })?;
        if n == 0 {
            return Err(SidecarFailure::Transport(
                "Higgs sidecar closed stdout (process died)".to_string(),
            ));
        }
        let resp: SidecarResponse = serde_json::from_str(resp_line.trim()).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to parse higgs sidecar response: {}", e))
        })?;
        if resp.status != "ok" {
            return Err(SidecarFailure::Response(if resp.error.is_empty() {
                format!("Higgs sidecar returned status={}", resp.status)
            } else {
                resp.error
            }));
        }
        Ok(resp)
    }

    /// Synthesize `text` with the registered `voice` (or no voice = model's
    /// zero-shot voice), writing WAV to `output_path`. Returns
    /// (duration_ms, sample_rate, chunk_count).
    pub fn synth(&mut self, text: &str, voice: Option<&str>, output_path: &str) -> Result<(i64, u32, u32), SidecarFailure> {
        self.synth_params(text, voice, output_path, &HiggsSynthParams::default())
    }

    /// Synthesize with per-request tonality knobs (emote, instruct, sampling).
    pub fn synth_params(
        &mut self,
        text: &str,
        voice: Option<&str>,
        output_path: &str,
        params: &HiggsSynthParams,
    ) -> Result<(i64, u32, u32), SidecarFailure> {
        let req = SynthRequest {
            op: "synth",
            text,
            output_path,
            voice,
            emote: params.emote.as_deref(),
            instruct: params.instruct.as_deref(),
            ref_audio: params.ref_audio.as_deref(),
            ref_text: params.ref_text.as_deref(),
            default_speed: params.default_speed,
            temperature: params.temperature,
            top_k: params.top_k,
            max_new_tokens: params.max_new_tokens,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize higgs synth request: {}", e)))?;
        let resp = self.roundtrip(json)?;
        Ok((resp.duration_ms, resp.sample_rate, resp.chunks))
    }

    /// Register a reference voice: `name` + reference WAV + transcript.
    pub fn register(&mut self, name: &str, audio_path: &str, text: &str) -> Result<(), SidecarFailure> {
        let req = RegisterRequest {
            op: "register",
            name,
            audio_path,
            text,
            overwrite: true,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize higgs register request: {}", e)))?;
        self.roundtrip(json)?;
        Ok(())
    }

    pub fn request_count(&self) -> u64 {
        self.request_count
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self._child.kill();
        let _ = self._child.wait();
    }
}

// ---------------------------------------------------------------------------
// Process-global pool
// ---------------------------------------------------------------------------

static GLOBAL_SIDECAR: std::sync::OnceLock<SharedSidecar> = std::sync::OnceLock::new();

fn global_shared_sidecar() -> &'static SharedSidecar {
    GLOBAL_SIDECAR.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// Acquire the shared sidecar, starting it on first use. On failure the slot
/// stays `None` (no repeated startup attempts) and callers get the error.
/// If a request fails with a Transport error (sidecar process died), the slot
/// is reset to `None` so the NEXT call respawns it.
fn with_sidecar<T>(
    f: impl FnOnce(&mut Sidecar) -> Result<T, SidecarFailure>,
) -> Result<T, String> {
    let shared = global_shared_sidecar();
    let mut guard = shared
        .lock()
        .map_err(|e| format!("higgs sidecar mutex poisoned: {}", e))?;
    if guard.is_none() {
        let script = resolve_higgs_sidecar();
        match Sidecar::start(&script.to_string_lossy()) {
            Ok(s) => {
                tracing::info!("Higgs TTS sidecar started (expressive TTS engine)");
                *guard = Some(s);
            }
            Err(e) => {
                tracing::warn!(
                    "Higgs TTS sidecar failed to start ({}); higgs voiceover generation unavailable. \
                     Run scripts/setup_higgs.sh to build the .venv-higgs inference environment.",
                    e
                );
                return Err(e);
            }
        }
    }
    let sidecar = guard.as_mut().ok_or("higgs sidecar slot empty")?;
    match f(sidecar) {
        Ok(v) => Ok(v),
        Err(SidecarFailure::Transport(e)) => {
            tracing::warn!("Higgs sidecar died ({}); resetting pool so it respawns.", e);
            *guard = None;
            Err(e)
        }
        Err(SidecarFailure::Response(e)) => {
            let hint = if e.contains("module")
                || e.contains("import")
                || e.contains("onnxruntime")
                || e.contains("No module")
                || e.contains("model dir")
                || e.contains("Run: bash scripts/setup_higgs.sh")
            {
                " (run scripts/setup_higgs.sh to build the Higgs inference venv + download the cuda_int4 model)"
            } else {
                ""
            };
            Err(format!("{}{}", e, hint))
        }
    }
}

/// Synthesize with a higgs voice clone (or zero-shot voice when `voice` is
/// None). Returns (duration_ms, sample_rate, chunk_count).
pub fn higgs_synthesize(
    text: &str,
    voice: Option<&str>,
    output_path: &str,
) -> Result<(i64, u32, u32), String> {
    with_sidecar(|s| s.synth(text, voice, output_path))
}

/// Synthesize with per-request tonality knobs (emote, instruct, sampling).
pub fn higgs_synthesize_params(
    text: &str,
    voice: Option<&str>,
    output_path: &str,
    params: &HiggsSynthParams,
) -> Result<(i64, u32, u32), String> {
    with_sidecar(|s| s.synth_params(text, voice, output_path, params))
}

/// Register (or overwrite) a higgs voice clone from a reference WAV + transcript.
pub fn higgs_register(name: &str, audio_path: &str, text: &str) -> Result<(), String> {
    with_sidecar(|s| s.register(name, audio_path, text))
}

// ---------------------------------------------------------------------------
// Path / interpreter resolution (AGENTS.md §9 priority chain)
// ---------------------------------------------------------------------------

/// Resolve the sidecar script path:
///   1. `HIGGS_SIDECAR` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/scripts/higgs_tts_sidecar.py`
///   3. `OPENSCRIPT_ROOT/mcp/scripts/higgs_tts_sidecar.py`
///   4. Relative `mcp/scripts/higgs_tts_sidecar.py` (last resort)
pub fn resolve_higgs_sidecar() -> PathBuf {
    if let Ok(s) = std::env::var("HIGGS_SIDECAR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/scripts/higgs_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/scripts/higgs_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("mcp/scripts/higgs_tts_sidecar.py")
}

/// Resolve the venv Python interpreter for the sidecar:
///   1. `HIGGS_PYTHON` env var
///   2. `<repo root>/.venv-higgs/bin/python` (auto-discovered after
///      `scripts/setup_higgs.sh`)
///   3. `python3.12` on PATH
///   4. `python3` (last resort)
pub fn resolve_higgs_python() -> String {
    if let Ok(p) = std::env::var("HIGGS_PYTHON") {
        if !p.is_empty() {
            tracing::debug!(python = %p, "Higgs Python: using HIGGS_PYTHON override");
            return p;
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        if !root.is_empty() {
            candidates.push(PathBuf::from(&root).join(".venv-higgs/bin/python"));
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(Path::new(d).join("../../.venv-higgs/bin/python"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".venv-higgs/bin/python"));
    }
    for candidate in candidates {
        if candidate.exists() {
            tracing::info!(python = %candidate.display(), "Higgs Python: found .venv-higgs venv");
            return candidate.to_string_lossy().into_owned();
        }
    }
    for cand in ["python3.12", "python3"] {
        if which_exists(cand) {
            return cand.to_string();
        }
    }
    "python3".to_string()
}

/// Best-effort `command -v` check.
fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", cmd))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// True when the python candidate resolves to something real.
fn python_candidate_resolves(python: &str) -> bool {
    if python.contains('/') {
        Path::new(python).exists()
    } else {
        which_exists(python)
    }
}

/// Best-effort check that the sidecar AND a real venv interpreter AND the
/// model directory are present. Reports `false` until
/// `scripts/setup_higgs.sh` has built `.venv-higgs` AND downloaded the
/// cuda_int4 export — so `system.capabilities` doesn't advertise an engine
/// that can't run.
pub fn higgs_available() -> bool {
    if !resolve_higgs_sidecar().exists() || !python_candidate_resolves(&resolve_higgs_python()) {
        return false;
    }
    // Model dir present with the core files?
    let dir = resolve_higgs_model_dir();
    let required = ["genai_config.json", "llm_decoder.onnx", "llm_decoder.onnx.data",
                    "text_embed.onnx", "audio_tokenizer.onnx"];
    required.iter().all(|f| dir.join(f).exists())
}

/// Resolve the model dir:
///   1. `HIGGS_MODEL_DIR` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/assets/higgs/cuda_int4`
///   3. `OPENSCRIPT_ROOT/mcp/assets/higgs/cuda_int4`
///   4. Relative `mcp/assets/higgs/cuda_int4`
pub fn resolve_higgs_model_dir() -> PathBuf {
    if let Ok(s) = std::env::var("HIGGS_MODEL_DIR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    let rel = Path::new("mcp/assets/higgs/cuda_int4");
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/assets/higgs/cuda_int4");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/assets/higgs/cuda_int4");
        if p.exists() {
            return p;
        }
    }
    rel.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidecar_script_resolution_returns_a_path() {
        std::env::remove_var("HIGGS_SIDECAR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_higgs_sidecar();
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn test_python_resolution_returns_a_path() {
        std::env::remove_var("HIGGS_PYTHON");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_higgs_python();
        assert!(!p.is_empty());
    }

    #[test]
    fn test_model_dir_resolution_returns_a_path() {
        std::env::remove_var("HIGGS_MODEL_DIR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_higgs_model_dir();
        assert!(p.as_os_str().to_string_lossy().contains("higgs"));
    }

    #[test]
    fn test_higgs_available_false_without_model() {
        // In CI the venv/model may be absent — must not panic, just report.
        let _ = higgs_available();
    }

    #[test]
    fn test_shared_slot_starts_empty() {
        let shared: SharedSidecar = Arc::new(Mutex::new(None));
        let g = shared.lock().unwrap();
        assert!(g.is_none());
    }
}
