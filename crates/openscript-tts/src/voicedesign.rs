//! VoiceDesign TTS sidecar — Qwen3-TTS-12Hz-1.7B-VoiceDesign via ONNX Runtime.
//!
//! Designs novel character voices from a natural-language description
//! (`instruct`) plus a sample line (`text`) — no reference audio required.
//! This powers the `voice.design` MCP tool: an agent describes a persona
//! ("grumpy detective, low gravelly voice") and gets back a 24 kHz WAV of a
//! brand-new voice matching that description, which can then be registered as
//! a cloned-voice profile for reuse across a script.
//!
//! Model: wavekat/Qwen3-TTS-1.7B-VoiceDesign-ONNX (Apache-2.0), int4,
//! ~4.3 GB. Inference is NumPy + ONNX Runtime only (no torch), so the venv is
//! light: onnxruntime-gpu + numpy + soundfile + transformers, provisioned by
//! `scripts/setup_voicedesign.sh`. The sidecar is
//! `mcp/scripts/voicedesign_tts_sidecar.py`, a long-lived stdin/stdout JSON
//! process mirroring the audio8/higgs sidecar pattern:
//!
//! ```text
//! → {"op":"design","instruct":"Speak in a warm friendly female voice",
//!    "text":"Give every small business the voice of a big one.",
//!    "output_path":"/tmp/persona.wav","language":"english","seed":42}
//! ← {"status":"ok","output_path":"...","duration_ms":1234,"sample_rate":24000}
//! ```
//!
//! On error: `{"status":"error","error":"..."}`. The sidecar prints
//! `{"ready":true}` on startup and loads the four ONNX sessions lazily on the
//! first design request (MCP server startup stays fast).

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

/// A long-lived VoiceDesign sidecar process.
pub struct Sidecar {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    request_count: u64,
}

#[derive(Serialize)]
struct DesignRequest<'a> {
    op: &'static str,
    instruct: &'a str,
    text: &'a str,
    output_path: &'a str,
    language: &'a str,
    seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
}

#[derive(Deserialize)]
struct SidecarResponse {
    status: String,
    #[serde(default)]
    duration_ms: i64,
    #[serde(default)]
    sample_rate: u32,
    #[serde(default)]
    output_path: String,
    #[serde(default)]
    error: String,
}

impl Sidecar {
    /// Spawn the sidecar under the resolved venv python and wait for the
    /// `{"ready":true}` handshake.
    pub fn start(sidecar_script: &str) -> Result<Self, String> {
        let python = resolve_voicedesign_python();
        let mut child = std::process::Command::new(&python)
            .arg(sidecar_script)
            .arg("--serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn voicedesign sidecar: {}", e))?;

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
                "VoiceDesign sidecar did not signal ready. stdout={:?} stderr={}",
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
            .map_err(|e| SidecarFailure::Transport(format!("Failed to write to voicedesign sidecar stdin: {}", e)))?;
        self.stdin
            .flush()
            .map_err(|e| SidecarFailure::Transport(format!("Failed to flush voicedesign sidecar stdin: {}", e)))?;

        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to read voicedesign sidecar response: {}", e))
        })?;
        if n == 0 {
            return Err(SidecarFailure::Transport(
                "VoiceDesign sidecar closed stdout (process died)".to_string(),
            ));
        }
        let resp: SidecarResponse = serde_json::from_str(resp_line.trim()).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to parse voicedesign sidecar response: {}", e))
        })?;
        if resp.status != "ok" {
            return Err(SidecarFailure::Response(if resp.error.is_empty() {
                format!("VoiceDesign sidecar returned status={}", resp.status)
            } else {
                resp.error
            }));
        }
        Ok(resp)
    }

    /// Design a novel voice from an instruction + sample text, writing WAV to
    /// `output_path`. Returns (duration_ms, sample_rate). Generation knobs
    /// (max_tokens / temperature / top_k) are optional — None sends the
    /// sidecar defaults (2048 / 0.9 / 50, matching the reference config).
    pub fn design(
        &mut self,
        instruct: &str,
        text: &str,
        output_path: &str,
        language: &str,
        seed: Option<i64>,
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        top_k: Option<u32>,
    ) -> Result<(i64, u32, String), SidecarFailure> {
        let req = DesignRequest {
            op: "design",
            instruct,
            text,
            output_path,
            language,
            seed,
            max_tokens,
            temperature,
            top_k,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize voicedesign design request: {}", e)))?;
        let resp = self.roundtrip(json)?;
        Ok((resp.duration_ms, resp.sample_rate, resp.output_path))
    }

    /// Synthesize a LINE directly with the Qwen3 VoiceDesign model (op
    /// `synth` — a protocol alias of `design`). `instruct` is the voice
    /// description: the character's personality PLUS the line's emotion/tone,
    /// so the same character voice stays consistent across scenes while each
    /// line is attuned to its required delivery. This is the script.to_video
    /// synthesis path for `voicedesign`-provider profiles — audio is generated
    /// BY the voice-design model, never re-cloned through a cloning engine.
    pub fn synth(
        &mut self,
        instruct: &str,
        text: &str,
        output_path: &str,
        language: &str,
        seed: Option<i64>,
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        top_k: Option<u32>,
    ) -> Result<(i64, u32, String), SidecarFailure> {
        let req = DesignRequest {
            op: "synth",
            instruct,
            text,
            output_path,
            language,
            seed,
            max_tokens,
            temperature,
            top_k,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize voicedesign synth request: {}", e)))?;
        let resp = self.roundtrip(json)?;
        Ok((resp.duration_ms, resp.sample_rate, resp.output_path))
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
        .map_err(|e| format!("voicedesign sidecar mutex poisoned: {}", e))?;
    if guard.is_none() {
        let script = resolve_voicedesign_sidecar();
        match Sidecar::start(&script.to_string_lossy()) {
            Ok(s) => {
                tracing::info!("VoiceDesign TTS sidecar started (voice design engine)");
                *guard = Some(s);
            }
            Err(e) => {
                tracing::warn!(
                    "VoiceDesign TTS sidecar failed to start ({}); voice.design unavailable. \
                     Run scripts/setup_voicedesign.sh to provision the model + venv.",
                    e
                );
                return Err(e);
            }
        }
    }
    let sidecar = guard.as_mut().ok_or("voicedesign sidecar slot empty")?;
    match f(sidecar) {
        Ok(v) => Ok(v),
        Err(SidecarFailure::Transport(e)) => {
            tracing::warn!("VoiceDesign sidecar died ({}); resetting pool so it respawns.", e);
            *guard = None;
            Err(e)
        }
        Err(SidecarFailure::Response(e)) => {
            // Point at the setup script when the error smells like a missing
            // model or dependency — the sidecar starts fine (prints ready) but
            // the lazy model load fails when the model/venv isn't provisioned.
            let hint = if e.contains("model not found")
                || e.contains("setup_voicedesign")
                || e.contains("No module")
                || e.contains("onnxruntime")
            {
                " (run scripts/setup_voicedesign.sh to download the model + build the venv)"
            } else {
                ""
            };
            Err(format!("{}{}", e, hint))
        }
    }
}

/// Design a novel voice from an instruction + sample text.
/// Returns (duration_ms, sample_rate, written_output_path). Generation knobs
/// default to the sidecar defaults when None.
pub fn voicedesign_design(
    instruct: &str,
    text: &str,
    output_path: &str,
    language: &str,
    seed: Option<i64>,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    top_k: Option<u32>,
) -> Result<(i64, u32, String), String> {
    with_sidecar(|s| s.design(instruct, text, output_path, language, seed, max_tokens, temperature, top_k))
}

/// Synthesize a line DIRECTLY with the Qwen3 VoiceDesign model (op `synth`).
/// Same generation as `voicedesign_design`, but named for the synthesis
/// boundary: `instruct` carries the character personality + per-line
/// emotion/tone, and `text` is the scene line. Returns
/// (duration_ms, sample_rate, written_output_path).
pub fn voicedesign_synthesize(
    instruct: &str,
    text: &str,
    output_path: &str,
    language: &str,
    seed: Option<i64>,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    top_k: Option<u32>,
) -> Result<(i64, u32, String), String> {
    with_sidecar(|s| s.synth(instruct, text, output_path, language, seed, max_tokens, temperature, top_k))
}

// ---------------------------------------------------------------------------
// Path / interpreter resolution (AGENTS.md §9 priority chain)
// ---------------------------------------------------------------------------

/// Resolve the sidecar script path:
///   1. `VOICEDESIGN_SIDECAR` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/scripts/voicedesign_tts_sidecar.py`
///   3. `OPENSCRIPT_ROOT/mcp/scripts/voicedesign_tts_sidecar.py`
///   4. Relative `mcp/scripts/voicedesign_tts_sidecar.py` (last resort)
pub fn resolve_voicedesign_sidecar() -> PathBuf {
    if let Ok(s) = std::env::var("VOICEDESIGN_SIDECAR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/scripts/voicedesign_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/scripts/voicedesign_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("mcp/scripts/voicedesign_tts_sidecar.py")
}

/// Resolve the venv Python interpreter for the sidecar:
///   1. `VOICEDESIGN_PYTHON` env var (explicit override)
///   2. `<repo root>/.venv-voicedesign/bin/python` (auto-discovered after
///      `scripts/setup_voicedesign.sh`)
///   3. `python3.12` on PATH
///   4. `python3` (last resort — system python has onnxruntime-gpu)
///
/// Returns a candidate whose file actually exists (or a bare command on PATH).
pub fn resolve_voicedesign_python() -> String {
    if let Ok(p) = std::env::var("VOICEDESIGN_PYTHON") {
        if !p.is_empty() {
            tracing::debug!(python = %p, "VoiceDesign Python: using VOICEDESIGN_PYTHON override");
            return p;
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        if !root.is_empty() {
            candidates.push(PathBuf::from(&root).join(".venv-voicedesign/bin/python"));
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(Path::new(d).join("../../.venv-voicedesign/bin/python"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".venv-voicedesign/bin/python"));
    }
    for candidate in candidates {
        if candidate.exists() {
            tracing::info!(python = %candidate.display(), "VoiceDesign Python: found .venv-voicedesign venv");
            return candidate.to_string_lossy().into_owned();
        }
    }
    for cand in ["python3.12", "python3"] {
        if which_exists(cand) {
            tracing::debug!(python = cand, "VoiceDesign Python: PATH fallback");
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

/// Best-effort check that the sidecar AND a real interpreter are present.
/// `system.capabilities` uses this so it doesn't advertise an engine that
/// can't run. Note: the model download itself is checked by the sidecar on
/// first design (it errors with a setup hint); this reports `false` until the
/// venv/interpreter exists.
pub fn voicedesign_available() -> bool {
    resolve_voicedesign_sidecar().exists() && python_candidate_resolves(&resolve_voicedesign_python())
}

/// The int4 ONNX external-weight files that must all exist for the model to
/// actually run (config.json downloads first and is NOT sufficient — the
/// session fails at load if any `.onnx.data` weight file is missing).
const INT4_REQUIRED_DATA_FILES: [&str; 4] = [
    "int4/code_predictor.onnx.data",
    "int4/talker_decode.onnx.data",
    "int4/talker_prefill.onnx.data",
    "int4/vocoder.onnx.data",
];

/// Check whether the model directory is fully provisioned — used by
/// system.capabilities to report model_present separately from venv presence.
/// Requires config.json AND all four int4 `.onnx.data` weight files so a
/// partial download is never advertised as ready.
pub fn voicedesign_model_present() -> bool {
    let dirs = [
        std::env::var("VOICEDESIGN_MODEL_DIR").ok().map(PathBuf::from),
        std::env::var("OPENSCRIPT_ROOT")
            .ok()
            .map(|r| PathBuf::from(&r).join("mcp/assets/voicedesign")),
        option_env!("CARGO_MANIFEST_DIR")
            .map(|d| Path::new(d).join("../../mcp/assets/voicedesign")),
        Some(PathBuf::from("mcp/assets/voicedesign")),
    ];
    dirs.iter().flatten().any(|d| {
        d.join("config.json").exists()
            && INT4_REQUIRED_DATA_FILES
                .iter()
                .all(|rel| d.join(rel).exists())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidecar_script_resolution_returns_a_path() {
        std::env::remove_var("VOICEDESIGN_SIDECAR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_voicedesign_sidecar();
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn test_python_resolution_returns_a_path() {
        std::env::remove_var("VOICEDESIGN_PYTHON");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_voicedesign_python();
        assert!(!p.is_empty());
    }

    #[test]
    fn test_shared_slot_starts_empty() {
        let shared: SharedSidecar = Arc::new(Mutex::new(None));
        let g = shared.lock().unwrap();
        assert!(g.is_none());
    }

    #[test]
    fn test_available_false_without_model() {
        // In CI the sidecar may be absent — must not panic, just report.
        let _ = voicedesign_available();
        let _ = voicedesign_model_present();
    }
}
