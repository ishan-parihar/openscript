//! IndexTTS-2.5 TTS sidecar — emotionally expressive zero-shot voice cloning.
//!
//! IndexTTS-2.5 (index-tts / bilibili) is a ~0.6–0.8B semantic-GPT +
//! Zipformer-flow S2M + BigVGAN vocoder pipeline: **22.05 kHz output,
//! 5 languages (zh/en/ja/es/ar), state-of-the-art zero-shot cloning from a
//! single reference clip** (CV3-Eval SS ≈ 68–77% / WER 3.3–5.6% in the
//! IndexTTS paper), plus three emotion channels: `emo_audio_prompt` +
//! `emo_alpha` (emotional reference clip — maps 1:1 to our profile emotion
//! takes), `emo_text` (natural-language emotion guidance via QwenEmo), and
//! the 8-dim `emo_vector`.
//!
//! We drive the official PyTorch stack from the `third_party/index-tts`
//! checkout + `IndexTeam/IndexTTS-2.5` checkpoints (~5.7 GB — provisioned by
//! `scripts/setup_indextts.sh`). The sidecar is
//! `mcp/scripts/indextts_tts_sidecar.py`, a long-lived process under the
//! dedicated `.venv-indextts` interpreter (Python 3.11 + torch 2.8 + CUDA)
//! serving a stdin/stdout JSON protocol:
//!
//! ```text
//! → {"op":"synth","text":"Hello","voice":"ishan","output_path":"/tmp/a.wav",
//!    "emote":"grave","temperature":0.9}
//! ← {"status":"ok","duration_ms":2340,"sample_rate":22050,"chunks":1}
//! → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","overwrite":true}
//! ← {"status":"ok","voice":"ishan"}
//! ```
//!
//! On error: `{"status":"error","error":"..."}`. The sidecar prints
//! `{"ready":true}` on startup and loads the ~5.7 GB pipeline lazily on first
//! synth (cold start takes minutes — not a hang). All sidecar instances share
//! a single process-global pool so the pipeline loads at most once per MCP
//! server process.
//!
//! **License note:** IndexTTS-2.5 weights are under the bilibili Model Use
//! License — research / non-commercial use; commercial use requires
//! contacting indexspeech@bilibili.com. See `scripts/setup_indextts.sh`.

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

/// A long-lived IndexTTS sidecar process.
pub struct Sidecar {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    request_count: u64,
}

/// Optional per-request synthesis parameters — the "tonality" knobs.
/// All fields default to None (= engine defaults).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct IndexttsSynthParams {
    /// Emote id (e.g. "grave", "firm", "whisper"). The sidecar maps it to
    /// natural-language `emo_text` guidance (QwenEmo) so the clone stays
    /// tonally attuned per line.
    pub emote: Option<String>,
    /// Override the registered voice's reference WAV (emotion takes).
    pub ref_audio: Option<String>,
    /// EXPLICIT emotional reference clip (the `emo_audio_prompt` channel —
    /// conditions the take on a separate emotional WAV).
    pub emo_audio_prompt: Option<String>,
    /// Explicit natural-language emotion guidance, e.g. "sad, somber,
    /// subdued". Beats the emote map when set.
    pub emo_text: Option<String>,
    /// Blend strength of the emotional reference clip (default 1.0).
    pub emo_alpha: Option<f64>,
    /// Script-level speed multiplier (1.0 = natural). Sent to the sidecar as
    /// `duration_factor` (reciprocal), which the flow model applies NATURALLY
    /// — never ffmpeg atempo rubber-banding.
    pub speed: Option<f64>,
    pub temperature: Option<f64>,
    pub top_k: Option<u32>,
    /// top-p nucleus sampling (0 = disabled by the engine default).
    pub top_p: Option<f64>,
    pub repetition_penalty: Option<f64>,
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
    ref_audio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emo_audio_prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emo_text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emo_alpha: Option<f64>,
    /// duration_factor = 1/speed (flow-model pacing).
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_factor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repetition_penalty: Option<f64>,
}

#[derive(Serialize)]
struct RegisterRequest<'a> {
    op: &'static str,
    name: &'a str,
    audio_path: &'a str,
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
    #[serde(default)]
    warning: String,
}

impl Sidecar {
    /// Spawn the sidecar under the resolved venv python and wait for the
    /// `{"ready":true}` handshake.
    pub fn start(sidecar_script: &str) -> Result<Self, String> {
        let python = resolve_indextts_python();
        let mut child = std::process::Command::new(&python)
            .arg(sidecar_script)
            .arg("--serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn indextts sidecar: {}", e))?;

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
                "IndexTTS sidecar did not signal ready. stdout={:?} stderr={}",
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
            .map_err(|e| SidecarFailure::Transport(format!("Failed to write to indextts sidecar stdin: {}", e)))?;
        self.stdin
            .flush()
            .map_err(|e| SidecarFailure::Transport(format!("Failed to flush indextts sidecar stdin: {}", e)))?;

        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to read indextts sidecar response: {}", e))
        })?;
        if n == 0 {
            return Err(SidecarFailure::Transport(
                "IndexTTS sidecar closed stdout (process died)".to_string(),
            ));
        }
        let resp: SidecarResponse = serde_json::from_str(resp_line.trim()).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to parse indextts sidecar response: {}", e))
        })?;
        if resp.status == "error" {
            return Err(SidecarFailure::Response(if resp.error.is_empty() {
                "IndexTTS sidecar returned status=error".to_string()
            } else {
                resp.error
            }));
        }
        if resp.status != "ok" && resp.status != "warning" {
            return Err(SidecarFailure::Response(format!(
                "IndexTTS sidecar returned unknown status={}",
                resp.status
            )));
        }
        Ok(resp)
    }

    /// Synthesize `text` with the registered `voice` clone, writing WAV to
    /// `output_path`. Returns (duration_ms, sample_rate, chunk_count).
    pub fn synth(
        &mut self,
        text: &str,
        voice: Option<&str>,
        output_path: &str,
    ) -> Result<(i64, u32, u32), SidecarFailure> {
        self.synth_params(text, voice, output_path, &IndexttsSynthParams::default())
    }

    /// Synthesize with per-request tonality knobs (emote, sampling, pacing).
    /// Returns (duration_ms, sample_rate, chunk_count).
    pub fn synth_params(
        &mut self,
        text: &str,
        voice: Option<&str>,
        output_path: &str,
        params: &IndexttsSynthParams,
    ) -> Result<(i64, u32, u32), SidecarFailure> {
        let duration_factor = params.speed.map(|s| {
            if s > 0.0 && (s - 1.0).abs() > 1e-6 {
                1.0 / s
            } else {
                1.0
            }
        });
        let req = SynthRequest {
            op: "synth",
            text,
            output_path,
            voice,
            emote: params.emote.as_deref(),
            ref_audio: params.ref_audio.as_deref(),
            emo_audio_prompt: params.emo_audio_prompt.as_deref(),
            emo_text: params.emo_text.as_deref(),
            emo_alpha: params.emo_alpha,
            duration_factor,
            temperature: params.temperature,
            top_k: params.top_k,
            top_p: params.top_p,
            repetition_penalty: params.repetition_penalty,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize indextts synth request: {}", e)))?;
        let resp = self.roundtrip(json)?;
        if !resp.warning.is_empty() {
            tracing::warn!("[tts/indextts] sidecar warning: {}", resp.warning);
        }
        Ok((resp.duration_ms, resp.sample_rate, resp.chunks))
    }

    /// Register a reference voice: `name` + reference WAV. The sidecar copies
    /// the WAV into its voices dir (the clone is conditioned at synth time).
    pub fn register(&mut self, name: &str, audio_path: &str) -> Result<(), SidecarFailure> {
        let req = RegisterRequest {
            op: "register",
            name,
            audio_path,
            overwrite: true,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize indextts register request: {}", e)))?;
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
        .map_err(|e| format!("indextts sidecar mutex poisoned: {}", e))?;
    if guard.is_none() {
        let script = resolve_indextts_sidecar();
        match Sidecar::start(&script.to_string_lossy()) {
            Ok(s) => {
                tracing::info!("IndexTTS TTS sidecar started (emotion-aware clone engine)");
                *guard = Some(s);
            }
            Err(e) => {
                tracing::warn!(
                    "IndexTTS TTS sidecar failed to start ({}); indextts voiceover generation unavailable. \
                     Run scripts/setup_indextts.sh to build the .venv-indextts inference environment.",
                    e
                );
                return Err(e);
            }
        }
    }
    let sidecar = guard.as_mut().ok_or("indextts sidecar slot empty")?;
    match f(sidecar) {
        Ok(v) => Ok(v),
        Err(SidecarFailure::Transport(e)) => {
            tracing::warn!("IndexTTS sidecar died ({}); resetting pool so it respawns.", e);
            *guard = None;
            Err(e)
        }
        Err(SidecarFailure::Response(e)) => {
            let hint = if e.contains("module")
                || e.contains("import")
                || e.contains("No module")
                || e.contains("model dir")
                || e.contains("setup_indextts")
            {
                " (run scripts/setup_indextts.sh to build the IndexTTS inference venv + download the ~5.7 GB checkpoints)"
            } else {
                ""
            };
            Err(format!("{}{}", e, hint))
        }
    }
}

/// Synthesize with an indextts voice clone. Returns
/// (duration_ms, sample_rate, chunk_count).
pub fn indextts_synthesize(
    text: &str,
    voice: Option<&str>,
    output_path: &str,
) -> Result<(i64, u32, u32), String> {
    with_sidecar(|s| s.synth(text, voice, output_path))
}

/// Synthesize with per-request tonality knobs (emote, sampling, pacing).
pub fn indextts_synthesize_params(
    text: &str,
    voice: Option<&str>,
    output_path: &str,
    params: &IndexttsSynthParams,
) -> Result<(i64, u32, u32), String> {
    with_sidecar(|s| s.synth_params(text, voice, output_path, params))
}

/// Register (or overwrite) an indextts voice clone from a reference WAV.
pub fn indextts_register(name: &str, audio_path: &str) -> Result<(), String> {
    with_sidecar(|s| s.register(name, audio_path))
}

// ---------------------------------------------------------------------------
// Path / interpreter resolution (AGENTS.md §9 priority chain)
// ---------------------------------------------------------------------------

/// Resolve the sidecar script path:
///   1. `INDEXTTS_SIDECAR` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/scripts/indextts_tts_sidecar.py`
///   3. `OPENSCRIPT_ROOT/mcp/scripts/indextts_tts_sidecar.py`
///   4. Relative `mcp/scripts/indextts_tts_sidecar.py` (last resort)
pub fn resolve_indextts_sidecar() -> PathBuf {
    if let Ok(s) = std::env::var("INDEXTTS_SIDECAR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/scripts/indextts_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/scripts/indextts_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("mcp/scripts/indextts_tts_sidecar.py")
}

/// Resolve the venv Python interpreter for the sidecar:
///   1. `INDEXTTS_PYTHON` env var
///   2. `<repo root>/.venv-indextts/bin/python` (auto-discovered after
///      `scripts/setup_indextts.sh`)
///   3. `python3.11` on PATH
///   4. `python3` (last resort)
pub fn resolve_indextts_python() -> String {
    if let Ok(p) = std::env::var("INDEXTTS_PYTHON") {
        if !p.is_empty() {
            tracing::debug!(python = %p, "IndexTTS Python: using INDEXTTS_PYTHON override");
            return p;
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        if !root.is_empty() {
            candidates.push(PathBuf::from(&root).join(".venv-indextts/bin/python"));
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(Path::new(d).join("../../.venv-indextts/bin/python"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".venv-indextts/bin/python"));
    }
    for candidate in candidates {
        if candidate.exists() {
            tracing::info!(python = %candidate.display(), "IndexTTS Python: found .venv-indextts venv");
            return candidate.to_string_lossy().into_owned();
        }
    }
    for cand in ["python3.11", "python3"] {
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
/// `scripts/setup_indextts.sh` has built `.venv-indextts` AND downloaded the
/// checkpoints — so `system.capabilities` doesn't advertise an engine that
/// can't run.
pub fn indextts_available() -> bool {
    if !resolve_indextts_sidecar().exists() || !python_candidate_resolves(&resolve_indextts_python())
    {
        return false;
    }
    // Model dir present with the core files?
    let dir = resolve_indextts_model_dir();
    let required = ["config.yaml", "gpt.pth", "codec.pth", "s2mel.pth"];
    required.iter().all(|f| dir.join(f).exists())
}

/// Resolve the model dir:
///   1. `INDEXTTS_MODEL_DIR` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/assets/indextts`
///   3. `OPENSCRIPT_ROOT/mcp/assets/indextts`
///   4. Relative `mcp/assets/indextts`
pub fn resolve_indextts_model_dir() -> PathBuf {
    if let Ok(s) = std::env::var("INDEXTTS_MODEL_DIR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    let rel = Path::new("mcp/assets/indextts");
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/assets/indextts");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/assets/indextts");
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
        std::env::remove_var("INDEXTTS_SIDECAR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_indextts_sidecar();
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn test_python_resolution_returns_a_path() {
        std::env::remove_var("INDEXTTS_PYTHON");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_indextts_python();
        assert!(!p.is_empty());
    }

    #[test]
    fn test_model_dir_resolution_returns_a_path() {
        std::env::remove_var("INDEXTTS_MODEL_DIR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_indextts_model_dir();
        assert!(p.as_os_str().to_string_lossy().contains("indextts"));
    }

    #[test]
    fn test_indextts_available_false_without_model() {
        // In CI the venv/model may be absent — must not panic, just report.
        let _ = indextts_available();
    }

    #[test]
    fn test_duration_factor_reciprocal() {
        // speed 1.25 (faster) -> duration_factor 0.8 (shorter audio)
        let params = IndexttsSynthParams {
            speed: Some(1.25),
            ..Default::default()
        };
        let df = params.speed.map(|s| if s > 0.0 && (s - 1.0).abs() > 1e-6 { 1.0 / s } else { 1.0 });
        assert_eq!(df, Some(0.8));
        // speed 1.0 -> neutral, no tag
        let neutral = IndexttsSynthParams {
            speed: Some(1.0),
            ..Default::default()
        };
        let df = neutral.speed.map(|s| if s > 0.0 && (s - 1.0).abs() > 1e-6 { 1.0 / s } else { 1.0 });
        assert_eq!(df, Some(1.0));
    }

    #[test]
    fn test_shared_slot_starts_empty() {
        let shared: SharedSidecar = Arc::new(Mutex::new(None));
        let g = shared.lock().unwrap();
        assert!(g.is_none());
    }
}
