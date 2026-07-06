//! Long-lived Kokoro sidecar — eliminates per-call cold-start penalty.
//!
//! ## Why this exists
//!
//! The legacy `synth_one()` path in `kokoro.rs` spawns a fresh Python process
//! per TTS chunk. Each spawn pays:
//!
//!   - Python interpreter startup: ~80ms
//!   - `kokoro_onnx` import: ~50ms
//!   - ONNX model load (310MB): ~200ms
//!   - Voices file load (27MB): ~30ms
//!
//! Total: ~360ms **per chunk**. A 20-scene script with 2 chunks per scene
//! pays 40 × 360ms = **14.4 seconds** of pure overhead — wasted on every
//! run, even for cached text.
//!
//! This module spawns ONE long-lived Python process that loads the model
//! once and serves requests via a stdin/stdout JSON protocol. Subsequent
//! synth calls pay only the inference cost (~150ms for a short chunk).
//!
//! ## Protocol
//!
//! One JSON request per line on stdin, one JSON response per line on
//! stdout. The Python side loads the model on startup and prints `{"ready":true}`
//! before reading the first request.
//!
//! ```text
//! → {"text":"Hello world","voice":"af_heart","speed":1.0,"output_path":"/tmp/foo.wav"}
//! ← {"status":"ok","duration_ms":1234,"sample_rate":24000}
//! ```
//!
//! On error:
//!
//! ```text
//! ← {"status":"error","error":"synthesis failed: ..."}
//! ```
//!
//! ## Fallback
//!
//! If the long-lived sidecar fails to start (Python missing, imports
//! missing, model missing), `acquire()` returns `None` and callers fall
//! back to the fresh-process path in `synth_one`. The fallback is logged
//! via `tracing::warn!` so the operator knows why perf regressed.
//!
//! ## Thread safety
//!
//! The sidecar is shared across all `KokoroClient` clones via an
//! `Arc<Mutex<Option<Sidecar>>>`. The Mutex serialises requests — which
//! is fine because Kokoro inference is CPU-bound and a single ONNX
//! session is not thread-safe anyway. Concurrent callers block on the
//! Mutex; this is acceptable for the typical 20-scene sequential
//! synthesis workload.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// One entry point for the long-lived sidecar. Wrapped in Arc<Mutex> so
/// `KokoroClient` clones can share a single Python process. `None` means
/// "tried to start, failed, do not retry" — we don't want to attempt the
/// ~200ms startup on every call after the first failure.
pub type SharedSidecar = Arc<Mutex<Option<Sidecar>>>;

/// A long-lived Kokoro sidecar process. Created once, used many times.
///
/// The inner `Child` is held by `SidecarHandle` so that dropping the
/// handle kills the process. The stdin/stdout pipes are owned by the
/// handle and accessed via `&mut` (single-threaded inside the Mutex).
pub struct Sidecar {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    /// Monotonic request counter, for diagnostics.
    request_count: u64,
}

/// One synth request, serialised to JSON and written to the sidecar's stdin.
#[derive(Serialize)]
struct SynthRequest<'a> {
    text: &'a str,
    voice: &'a str,
    speed: f32,
    output_path: &'a str,
}

/// One synth response, deserialised from the sidecar's stdout.
#[derive(Deserialize)]
struct SynthResponse {
    status: String,
    #[serde(default)]
    duration_ms: i64,
    #[serde(default)]
    sample_rate: u32,
    #[serde(default)]
    error: String,
}

/// Result of a successful synth call.
pub struct SidecarSynthResult {
    pub duration_ms: i64,
    pub sample_rate: u32,
}

impl Sidecar {
    /// Try to start the long-lived sidecar. Returns `Ok(Sidecar)` on
    /// success, `Err(message)` on failure. The caller should cache the
    /// result (success or failure) so we don't repeatedly attempt the
    /// startup cost.
    pub fn start(
        model_path: &Path,
        voices_path: &Path,
        sidecar_script: &str,
    ) -> Result<Self, String> {
        let mut child = std::process::Command::new("python3")
            .arg(sidecar_script)
            .arg("--serve")
            .arg("--model")
            .arg(model_path)
            .arg("--voices")
            .arg(voices_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn kokoro sidecar: {}", e))?;

        let stdin = child.stdin.take().ok_or("sidecar stdin not piped")?;
        let stdout = child.stdout.take().ok_or("sidecar stdout not piped")?;

        // Wait for the ready signal before returning. The sidecar prints
        // `{"ready":true}` once the ONNX model is loaded. If we don't wait,
        // the first synth request will race the model load.
        let mut stdout_reader = BufReader::new(stdout);
        let mut ready_line = String::new();
        stdout_reader
            .read_line(&mut ready_line)
            .map_err(|e| format!("Failed to read sidecar ready signal: {}", e))?;

        if !ready_line.contains("\"ready\"") {
            // Sidecar printed an error instead of the ready signal.
            // Read stderr for context.
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
                "Sidecar did not signal ready. stdout={:?} stderr={}",
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

    /// Synthesise one chunk via the long-lived sidecar. Writes the request
    /// JSON to stdin, reads the response JSON from stdout, and reads the
    /// resulting WAV file from `output_path` (which the sidecar wrote).
    pub fn synth(
        &mut self,
        text: &str,
        voice: &str,
        speed: f32,
        output_path: &str,
    ) -> Result<SidecarSynthResult, String> {
        self.request_count += 1;

        let req = SynthRequest {
            text,
            voice,
            speed,
            output_path,
        };
        let req_json = serde_json::to_string(&req)
            .map_err(|e| format!("Failed to serialise synth request: {}", e))?;

        // Write request line + flush. If the write fails the sidecar has
        // likely died — surface the error so the caller can fall back.
        writeln!(self.stdin, "{}", req_json)
            .map_err(|e| format!("Failed to write to sidecar stdin: {}", e))?;
        self.stdin
            .flush()
            .map_err(|e| format!("Failed to flush sidecar stdin: {}", e))?;

        // Read one response line. If the line is empty, the sidecar closed
        // its stdout (process died).
        let mut resp_line = String::new();
        let n = self
            .stdout
            .read_line(&mut resp_line)
            .map_err(|e| format!("Failed to read sidecar response: {}", e))?;
        if n == 0 {
            return Err("Sidecar closed stdout (process died)".to_string());
        }

        let resp: SynthResponse = serde_json::from_str(resp_line.trim())
            .map_err(|e| format!("Failed to parse sidecar response: {}", e))?;

        if resp.status != "ok" {
            return Err(if resp.error.is_empty() {
                format!("Sidecar returned status={}", resp.status)
            } else {
                resp.error
            });
        }

        Ok(SidecarSynthResult {
            duration_ms: resp.duration_ms,
            sample_rate: resp.sample_rate,
        })
    }

    /// Number of synth requests served so far. For diagnostics.
    pub fn request_count(&self) -> u64 {
        self.request_count
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        // Best-effort kill. If the child is already dead, kill() returns
        // an error we ignore.
        let _ = self._child.kill();
        let _ = self._child.wait();
    }
}

/// Lazily acquire the shared sidecar. Returns `Ok(&mut Sidecar)` if the
/// sidecar is running (or was just started), or `Err(message)` if it
/// could not be started. On error, the shared slot is set to `None` so
/// subsequent callers do not retry the startup cost.
///
/// Callers should hold the returned guard for the shortest possible scope
/// — the Mutex serialises all synth calls.
pub fn acquire_or_init<'a>(
    shared: &'a SharedSidecar,
    model_path: &Path,
    voices_path: &Path,
    sidecar_script: &str,
) -> Result<std::sync::MutexGuard<'a, Option<Sidecar>>, String> {
    let mut guard = shared.lock().map_err(|e| format!("sidecar mutex poisoned: {}", e))?;
    if guard.is_none() {
        match Sidecar::start(model_path, voices_path, sidecar_script) {
            Ok(s) => {
                tracing::info!(
                    "Kokoro long-lived sidecar started (eliminates per-call cold-start)"
                );
                *guard = Some(s);
            }
            Err(e) => {
                tracing::warn!(
                    "Kokoro long-lived sidecar failed to start ({}); falling back to \
                     fresh-process-per-call mode. This is fine but slower for multi-scene scripts.",
                    e
                );
                // Leave the slot as None so we don't retry on every call.
                // The startup cost (~200ms) is too high to pay repeatedly.
            }
        }
    }
    Ok(guard)
}

// ---------------------------------------------------------------------------
// Process-global sidecar pool
// ---------------------------------------------------------------------------

/// Process-global sidecar pool. All `KokoroClient` instances share this
/// single `SharedSidecar`, so the ONNX model is loaded exactly once per
/// process — even if `KokoroClient::new()` is called per-request (which
/// is what `tts_generate_routed` does in `tools.rs`).
///
/// Without this global, each `KokoroClient` got its own `SharedSidecar`
/// (via `KokoroEngine::sidecar`), which meant each `script.generate_voices`
/// scene started a fresh sidecar — 5 scenes × ~7s cold-start = ~35s wasted.
/// (UX audit GAP #6 fix.)
static GLOBAL_SIDECAR: std::sync::OnceLock<SharedSidecar> = std::sync::OnceLock::new();

/// Get the process-global shared sidecar pool. All callers should use this
/// instead of constructing their own `SharedSidecar`. The pool is lazily
/// initialized on first use and lives for the lifetime of the process.
pub fn global_shared_sidecar() -> &'static SharedSidecar {
    GLOBAL_SIDECAR.get_or_init(|| {
        std::sync::Arc::new(std::sync::Mutex::new(None))
    })
}

/// Resolve the Kokoro sidecar script path using the same priority chain
/// as the legacy `synth_one` in kokoro.rs. Duplicated here so this module
/// is self-contained.
///
/// Priority:
///   1. `KOKORO_SIDECAR` env var (explicit override)
///   2. `CARGO_MANIFEST_DIR/../../mcp/scripts/kokoro_tts_sidecar.py`
///   3. `OPENSCRIPT_ROOT/mcp/scripts/kokoro_tts_sidecar.py`
///   4. Relative `"mcp/scripts/kokoro_tts_sidecar.py"` (last resort)
pub fn resolve_sidecar_script() -> PathBuf {
    if let Ok(s) = std::env::var("KOKORO_SIDECAR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let p = Path::new(d).join("../../mcp/scripts/kokoro_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let p = PathBuf::from(&root).join("mcp/scripts/kokoro_tts_sidecar.py");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("mcp/scripts/kokoro_tts_sidecar.py")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the shared sidecar slot starts empty and acquire_or_init
    /// gracefully degrades to None when Python is unavailable.
    #[test]
    fn test_shared_sidecar_acquire_returns_guard_even_on_failure() {
        let shared: SharedSidecar = Arc::new(Mutex::new(None));
        // Use a deliberately-bogus script path so startup fails fast.
        let guard = acquire_or_init(
            &shared,
            Path::new("/nonexistent/model.onnx"),
            Path::new("/nonexistent/voices.bin"),
            "/nonexistent/sidecar.py",
        );
        // acquire_or_init returns Ok(guard) regardless of whether the sidecar
        // started — the guard wraps Option<Sidecar>. Caller checks guard.is_some().
        assert!(guard.is_ok(), "acquire should not hard-fail");
        let g = guard.unwrap();
        assert!(
            g.is_none(),
            "Sidecar should not have started with bogus paths"
        );
    }

    /// Verify script resolution falls back to the relative path when no
    /// env vars are set.
    #[test]
    fn test_resolve_sidecar_script_returns_a_path() {
        std::env::remove_var("KOKORO_SIDECAR");
        std::env::remove_var("OPENSCRIPT_ROOT");
        let p = resolve_sidecar_script();
        assert!(
            !p.as_os_str().is_empty(),
            "resolve_sidecar_script must return a non-empty path"
        );
    }
}
