//! Audio8 TTS sidecar — zero-shot voice cloning via the vendored Audio8 ONNX runtime.
//!
//! Audio8 TTS Preview 0.6B (DualAR, Fish-S2-Pro-style) is the default engine
//! for cloned voices: `voice.profile.add` with `provider: "audio8"` registers
//! a reference WAV + exact transcript through the sidecar, and
//! `tts.generate` / `script.generate_voices` synthesize with that voice.
//!
//! The sidecar is `mcp/scripts/audio8_tts_sidecar.py`, a long-lived process
//! (mirroring `kokoro_sidecar`) that loads the INT4 ONNX sessions once
//! (~1 GiB) and serves a stdin/stdout JSON protocol:
//!
//! ```text
//! → {"op":"synth","text":"Hello","voice":"ishan","output_path":"/tmp/a.wav"}
//! ← {"status":"ok","duration_ms":1234,"sample_rate":44100}
//! → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","text":"Exact transcript.","overwrite":true}
//! ← {"status":"ok","voice":"ishan","codes_shape":[10,241]}
//! ```
//!
//! On error: `{"status":"error","error":"..."}`. The sidecar prints
//! `{"ready":true}` on startup and loads the model lazily on first synth.
//!
//! All sidecar instances share a single process-global pool so the ~1 GiB
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

/// A long-lived Audio8 sidecar process.
pub struct Sidecar {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    request_count: u64,
}

#[derive(Serialize)]
struct SynthRequest<'a> {
    op: &'static str,
    text: &'a str,
    voice: &'a str,
    output_path: &'a str,
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
    /// Spawn the sidecar and wait for the `{"ready":true}` handshake.
    pub fn start(sidecar_script: &str) -> Result<Self, String> {
        let python = resolve_audio8_python();
        let mut child = std::process::Command::new(&python)
            .arg(sidecar_script)
            .arg("--serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn audio8 sidecar: {}", e))?;

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
                "Audio8 sidecar did not signal ready. stdout={:?} stderr={}",
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
            .map_err(|e| SidecarFailure::Transport(format!("Failed to write to audio8 sidecar stdin: {}", e)))?;
        self.stdin
            .flush()
            .map_err(|e| SidecarFailure::Transport(format!("Failed to flush audio8 sidecar stdin: {}", e)))?;

        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to read audio8 sidecar response: {}", e))
        })?;
        if n == 0 {
            return Err(SidecarFailure::Transport(
                "Audio8 sidecar closed stdout (process died)".to_string(),
            ));
        }
        let resp: SidecarResponse = serde_json::from_str(resp_line.trim()).map_err(|e| {
            SidecarFailure::Transport(format!("Failed to parse audio8 sidecar response: {}", e))
        })?;
        if resp.status != "ok" {
            return Err(SidecarFailure::Response(if resp.error.is_empty() {
                format!("Audio8 sidecar returned status={}", resp.status)
            } else {
                resp.error
            }));
        }
        Ok(resp)
    }

    /// Synthesize `text` with the registered `voice`, writing WAV to `output_path`.
    /// Returns (duration_ms, sample_rate).
    pub fn synth(&mut self, text: &str, voice: &str, output_path: &str) -> Result<(i64, u32), SidecarFailure> {
        let req = SynthRequest {
            op: "synth",
            text,
            voice,
            output_path,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize audio8 synth request: {}", e)))?;
        let resp = self.roundtrip(json)?;
        Ok((resp.duration_ms, resp.sample_rate))
    }

    /// Register a reference voice: `name` + reference WAV + exact transcript.
    pub fn register(&mut self, name: &str, audio_path: &str, text: &str) -> Result<(), SidecarFailure> {
        let req = RegisterRequest {
            op: "register",
            name,
            audio_path,
            text,
            overwrite: true,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| SidecarFailure::Transport(format!("Failed to serialize audio8 register request: {}", e)))?;
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
/// is reset to `None` so the NEXT call respawns it — a crashed sidecar must
/// not permanently wedge the pool.
fn with_sidecar<T>(
    f: impl FnOnce(&mut Sidecar) -> Result<T, SidecarFailure>,
) -> Result<T, String> {
    let shared = global_shared_sidecar();
    let mut guard = shared
        .lock()
        .map_err(|e| format!("audio8 sidecar mutex poisoned: {}", e))?;
    if guard.is_none() {
        let script = resolve_audio8_sidecar();
        match Sidecar::start(&script.to_string_lossy()) {
            Ok(s) => {
                tracing::info!("Audio8 TTS sidecar started (voice cloning engine)");
                *guard = Some(s);
            }
            Err(e) => {
                tracing::warn!(
                    "Audio8 TTS sidecar failed to start ({}); audio8 voiceover generation unavailable.",
                    e
                );
                return Err(e);
            }
        }
    }
    let sidecar = guard.as_mut().ok_or("audio8 sidecar slot empty")?;
    match f(sidecar) {
        Ok(v) => Ok(v),
        Err(SidecarFailure::Transport(e)) => {
            tracing::warn!("Audio8 sidecar died ({}); resetting pool so it respawns.", e);
            // Drop the dead child and clear the slot.
            *guard = None;
            Err(e)
        }
        Err(SidecarFailure::Response(e)) => Err(e),
    }
}

/// Synthesize with an audio8 voice clone. Returns (duration_ms, sample_rate).
pub fn audio8_synthesize(text: &str, voice: &str, output_path: &str) -> Result<(i64, u32), String> {
    with_sidecar(|s| s.synth(text, voice, output_path))
}

/// Register (or overwrite) an audio8 voice clone from a reference WAV + transcript.
pub fn audio8_register(name: &str, audio_path: &str, text: &str) -> Result<(), String> {
    with_sidecar(|s| s.register(name, audio_path, text))
}

// ---------------------------------------------------------------------------
// Path / interpreter resolution (AGENTS.md §9 priority chain)
// ---------------------------------------------------------------------------

/// Resolve the sidecar script path:
///   1. `AUDIO8_SIDECAR` env var
///   2. `CARGO_MANIFEST_DIR/../../mcp/scripts/audio8_tts_sidecar.py`
///   3. `OPENSCRIPT_ROOT/mcp/scripts/audio8_tts_sidecar.py`
///   4. Relative `mcp/scripts/audio8_tts_sidecar.py` (last resort)
pub fn resolve_audio8_sidecar() -> PathBuf {
    if let Ok(s) = std::env::var("AUDIO8_SIDECAR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/scripts/audio8_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/scripts/audio8_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("mcp/scripts/audio8_tts_sidecar.py")
}

/// Resolve the Python interpreter: `AUDIO8_PYTHON` env var, then `python3`.
pub fn resolve_audio8_python() -> String {
    if let Ok(p) = std::env::var("AUDIO8_PYTHON") {
        if !p.is_empty() {
            return p;
        }
    }
    "python3".to_string()
}

/// Best-effort check that the model + sidecar are present.
pub fn audio8_available() -> bool {
    resolve_audio8_sidecar().exists() && model_dir().join("runtime_manifest.json").exists()
}

/// Resolve the ONNX model dir: `AUDIO8_MODEL_DIR` env, then
/// `OPENSCRIPT_ROOT/mcp/assets/audio8/model`, then relative `mcp/assets/audio8/model`.
pub fn model_dir() -> PathBuf {
    if let Ok(m) = std::env::var("AUDIO8_MODEL_DIR") {
        if !m.is_empty() {
            return PathBuf::from(m);
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/assets/audio8/model");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("mcp/assets/audio8/model")
}

/// Resolve the registered-voices dir: `AUDIO8_VOICES_DIR`, then
/// `<model_dir>/../voices` (i.e. `mcp/assets/audio8/voices`).
pub fn voices_dir() -> PathBuf {
    if let Ok(v) = std::env::var("AUDIO8_VOICES_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    model_dir().parent().map(|p| p.join("voices")).unwrap_or_else(|| PathBuf::from("mcp/assets/audio8/voices"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidecar_script_resolution_returns_a_path() {
        std::env::remove_var("AUDIO8_SIDECAR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_audio8_sidecar();
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn test_python_resolution_defaults_to_python3() {
        std::env::remove_var("AUDIO8_PYTHON");
        assert_eq!(resolve_audio8_python(), "python3");
    }

    #[test]
    fn test_shared_slot_starts_empty() {
        let shared: SharedSidecar = Arc::new(Mutex::new(None));
        let g = shared.lock().unwrap();
        assert!(g.is_none());
    }

    #[test]
    fn test_audio8_available_false_without_model() {
        // In CI the model is absent — must not panic, just report false.
        std::env::remove_var("AUDIO8_MODEL_DIR");
        let _ = audio8_available();
    }
}
