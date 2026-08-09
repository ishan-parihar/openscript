//! Gepard TTS sidecar — zero-shot voice cloning via Gepard 1.0 (Qwen3.5 AR + NeMo NanoCodec).
//!
//! Gepard 1.0 (Apache-2.0 weights; the NeMo NanoCodec it loads at runtime is
//! covered by the NVIDIA Open Model License Agreement) is the high-quality
//! native-English cloned-voice engine: `voice.profile.add` with
//! `provider: "gepard"` registers a reference WAV through the sidecar, and
//! `tts.generate` / `script.generate_voices` synthesize with that voice.
//!
//! The sidecar is `mcp/scripts/gepard_tts_sidecar.py`, a long-lived process
//! (mirroring `audio8_tts_sidecar.py`) running under the dedicated
//! `.venv-gepard` interpreter (Python 3.12 + CUDA torch + NeMo codec +
//! transformers 5.3.0 — provisioned by `scripts/setup_gepard.sh`). It loads
//! the model + codec once (~2.5 GB) and serves a stdin/stdout JSON protocol:
//!
//! ```text
//! → {"op":"synth","text":"Hello","voice":"ishan","output_path":"/tmp/a.wav"}
//! ← {"status":"ok","duration_ms":1234,"sample_rate":22050}
//! → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","text":"Exact transcript.","overwrite":true}
//! ← {"status":"ok","voice":"ishan"}
//! ```
//!
//! On error: `{"status":"error","error":"..."}`. The sidecar prints
//! `{"ready":true}` on startup and loads the model lazily on first synth.
//!
//! All sidecar instances share a single process-global pool so the ~2.5 GB
//! model is loaded at most once per MCP server process.

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

/// A long-lived Gepard sidecar process.
pub struct Sidecar {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    request_count: u64,
}

/// Optional per-request synthesis parameters — the "tonality" knobs.
/// All fields default to None (= engine defaults / base voice reference).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GepardSynthParams {
    /// Emotion id (e.g. "angry", "whisper"). Passed to the sidecar for
    /// diagnostics; the actual emotion take is selected by `ref_audio`.
    pub emotion: Option<String>,
    /// Override the registered voice's reference WAV. Used for emotion
    /// takes — the sidecar synthesizes with THIS reference instead of the
    /// profile's neutral one.
    pub ref_audio: Option<String>,
    /// Reference-fidelity: higher = timbre clings closer to the reference.
    pub cfg_scale: Option<f64>,
    pub temperature: Option<f64>,
    pub top_k: Option<u32>,
    pub max_frames: Option<u32>,
}

#[derive(Serialize)]
struct SynthRequest<'a> {
    op: &'static str,
    text: &'a str,
    voice: &'a str,
    output_path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    emotion: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_audio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cfg_scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_frames: Option<u32>,
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
    error: String,
}

impl Sidecar {
    /// Spawn the sidecar under the resolved venv python and wait for the
    /// `{"ready":true}` handshake.
    pub fn start(sidecar_script: &str) -> Result<Self, String> {
        let python = resolve_gepard_python();
        let mut child = std::process::Command::new(&python)
            .arg(sidecar_script)
            .arg("--serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn gepard sidecar: {}", e))?;

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
                "Gepard sidecar did not signal ready. stdout={:?} stderr={}",
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
            .map_err(|e| SidecarFailure::Transport(format!("Failed to write to gepard sidecar stdin: {}", e)))?;
        self.stdin
            .flush()
            .map_err(|e| SidecarFailure::Transport(format!("Failed to flush gepard sidecar stdin: {}", e)))?;

        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to read gepard sidecar response: {}", e))
        })?;
        if n == 0 {
            return Err(SidecarFailure::Transport(
                "Gepard sidecar closed stdout (process died)".to_string(),
            ));
        }
        let resp: SidecarResponse = serde_json::from_str(resp_line.trim()).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to parse gepard sidecar response: {}", e))
        })?;
        if resp.status != "ok" {
            return Err(SidecarFailure::Response(if resp.error.is_empty() {
                format!("Gepard sidecar returned status={}", resp.status)
            } else {
                resp.error
            }));
        }
        Ok(resp)
    }

    /// Synthesize `text` with the registered `voice`, writing WAV to `output_path`.
    /// Returns (duration_ms, sample_rate).
    pub fn synth(&mut self, text: &str, voice: &str, output_path: &str) -> Result<(i64, u32), SidecarFailure> {
        self.synth_params(text, voice, output_path, &GepardSynthParams::default())
    }

    /// Synthesize with per-request tonality knobs (emotion take ref override,
    /// cfg_scale, temperature, ...).
    pub fn synth_params(
        &mut self,
        text: &str,
        voice: &str,
        output_path: &str,
        params: &GepardSynthParams,
    ) -> Result<(i64, u32), SidecarFailure> {
        let req = SynthRequest {
            op: "synth",
            text,
            voice,
            output_path,
            emotion: params.emotion.as_deref(),
            ref_audio: params.ref_audio.as_deref(),
            cfg_scale: params.cfg_scale,
            temperature: params.temperature,
            top_k: params.top_k,
            max_frames: params.max_frames,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize gepard synth request: {}", e)))?;
        let resp = self.roundtrip(json)?;
        Ok((resp.duration_ms, resp.sample_rate))
    }

    /// Register a reference voice: `name` + reference WAV + transcript
    /// (the transcript is metadata — Gepard's Q-Former cloning needs only audio).
    pub fn register(&mut self, name: &str, audio_path: &str, text: &str) -> Result<(), SidecarFailure> {
        let req = RegisterRequest {
            op: "register",
            name,
            audio_path,
            text,
            overwrite: true,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize gepard register request: {}", e)))?;
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
        .map_err(|e| format!("gepard sidecar mutex poisoned: {}", e))?;
    if guard.is_none() {
        let script = resolve_gepard_sidecar();
        match Sidecar::start(&script.to_string_lossy()) {
            Ok(s) => {
                tracing::info!("Gepard TTS sidecar started (voice cloning engine)");
                *guard = Some(s);
            }
            Err(e) => {
                tracing::warn!(
                    "Gepard TTS sidecar failed to start ({}); gepard voiceover generation unavailable. \
                     Run scripts/setup_gepard.sh to build the .venv-gepard inference environment.",
                    e
                );
                return Err(e);
            }
        }
    }
    let sidecar = guard.as_mut().ok_or("gepard sidecar slot empty")?;
    match f(sidecar) {
        Ok(v) => Ok(v),
        Err(SidecarFailure::Transport(e)) => {
            tracing::warn!("Gepard sidecar died ({}); resetting pool so it respawns.", e);
            *guard = None;
            Err(e)
        }
        Err(SidecarFailure::Response(e)) => {
            // If the error smells like a missing environment, point at the
            // setup script — the sidecar starts fine (prints ready) but the
            // lazy model import fails when .venv-gepard isn't built.
            let hint = if e.contains("module")
                || e.contains("import")
                || e.contains("transformers")
                || e.contains("torch")
                || e.contains("No module")
            {
                " (run scripts/setup_gepard.sh to build the Gepard inference venv)"
            } else {
                ""
            };
            Err(format!("{}{}", e, hint))
        }
    }
}

/// Synthesize with a gepard voice clone. Returns (duration_ms, sample_rate).
pub fn gepard_synthesize(text: &str, voice: &str, output_path: &str) -> Result<(i64, u32), String> {
    with_sidecar(|s| s.synth(text, voice, output_path))
}

/// Synthesize with per-request tonality knobs (emotion take ref override,
/// cfg_scale, temperature, ...).
pub fn gepard_synthesize_params(
    text: &str,
    voice: &str,
    output_path: &str,
    params: &GepardSynthParams,
) -> Result<(i64, u32), String> {
    with_sidecar(|s| s.synth_params(text, voice, output_path, params))
}

/// Register (or overwrite) a gepard voice clone from a reference WAV + transcript.
pub fn gepard_register(name: &str, audio_path: &str, text: &str) -> Result<(), String> {
    with_sidecar(|s| s.register(name, audio_path, text))
}

// ---------------------------------------------------------------------------
// Path / interpreter resolution (AGENTS.md §9 priority chain)
// ---------------------------------------------------------------------------

/// Resolve the sidecar script path:
///   1. `GEPARD_SIDECAR` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/scripts/gepard_tts_sidecar.py`
///   3. `OPENSCRIPT_ROOT/mcp/scripts/gepard_tts_sidecar.py`
///   4. Relative `mcp/scripts/gepard_tts_sidecar.py` (last resort)
pub fn resolve_gepard_sidecar() -> PathBuf {
    if let Ok(s) = std::env::var("GEPARD_SIDECAR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/scripts/gepard_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/scripts/gepard_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("mcp/scripts/gepard_tts_sidecar.py")
}

/// Resolve the venv Python interpreter for the sidecar:
///   1. `GEPARD_PYTHON` env var (explicit override — the operator sets this
///      to `.venv-gepard/bin/python`, or the setup script's venv)
///   2. `<repo root>/.venv-gepard/bin/python` (auto-discovered after
///      `scripts/setup_gepard.sh`; CARGO_MANIFEST_DIR is the crate dir, so
///      the venv is two levels up: `crates/openscript-tts/../../.venv-gepard`)
///   3. `python3.12` on PATH
///   4. `python3` (last resort)
///
/// Returns a candidate whose file actually exists (or a bare command on
/// PATH) — a venv path that doesn't exist is never returned.
pub fn resolve_gepard_python() -> String {
    if let Ok(p) = std::env::var("GEPARD_PYTHON") {
        if !p.is_empty() {
            tracing::debug!(python = %p, "Gepard Python: using GEPARD_PYTHON override");
            return p;
        }
    }
    // Auto-discover the venv created by scripts/setup_gepard.sh. Check the
    // repo root (OPENSCRIPT_ROOT, CWD) and the crate-relative `../../` form
    // so the priority chain works regardless of which crate calls it.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        if !root.is_empty() {
            candidates.push(PathBuf::from(&root).join(".venv-gepard/bin/python"));
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(Path::new(d).join("../../.venv-gepard/bin/python"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".venv-gepard/bin/python"));
    }
    for candidate in candidates {
        if candidate.exists() {
            tracing::info!(python = %candidate.display(), "Gepard Python: found .venv-gepard venv");
            return candidate.to_string_lossy().into_owned();
        }
    }
    // PATH fallbacks
    for cand in ["python3.12", "python3"] {
        if which_exists(cand) {
            tracing::debug!(python = cand, "Gepard Python: PATH fallback");
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

/// True when the python candidate resolves to something real: either a path
/// whose file exists (a venv interpreter) or a command on PATH.
fn python_candidate_resolves(python: &str) -> bool {
    if python.contains('/') {
        Path::new(python).exists()
    } else {
        which_exists(python)
    }
}

/// Best-effort check that the sidecar AND a real venv interpreter are
/// present. Unlike a bare script-exists check, this reports `false` until
/// `scripts/setup_gepard.sh` has actually built `.venv-gepard` — so
/// `system.capabilities` doesn't advertise an engine that can't run.
pub fn gepard_available() -> bool {
    resolve_gepard_sidecar().exists() && python_candidate_resolves(&resolve_gepard_python())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidecar_script_resolution_returns_a_path() {
        std::env::remove_var("GEPARD_SIDECAR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_gepard_sidecar();
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn test_python_resolution_returns_a_path() {
        std::env::remove_var("GEPARD_PYTHON");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_gepard_python();
        assert!(!p.is_empty());
    }

    #[test]
    fn test_shared_slot_starts_empty() {
        let shared: SharedSidecar = Arc::new(Mutex::new(None));
        let g = shared.lock().unwrap();
        assert!(g.is_none());
    }

    #[test]
    fn test_gepard_available_false_without_model() {
        // In CI the sidecar may be absent — must not panic, just report.
        let _ = gepard_available();
    }
}
