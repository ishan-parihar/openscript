pub mod client;
pub mod profiles;

/// Native Kokoro TTS backend (preset-voice, ONNX-based).
///
/// Enabled via the `kokoro` feature flag. The default build is sidecar-only
/// (drives the `faster-qwen3-tts` Python sidecar via `client.rs`).
#[cfg(feature = "kokoro")]
pub mod kokoro;
