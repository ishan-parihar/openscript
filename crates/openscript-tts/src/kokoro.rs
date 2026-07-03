//! Kokoro TTS backend (preset-voice, ONNX-based).
//!
//! PROPOSAL / SKETCH — not wired into `lib.rs` yet. See research report.
//!
//! Unlike `client.rs` (which drives the `faster-qwen3-tts` Python sidecar and is
//! built around *voice cloning* via ref_audio + ref_text + xvec), this backend
//! drives the Kokoro-82M model, which is a **preset-voice** TTS: you pick one of
//! 54 bundled voice packs (e.g. `af_heart`, `am_michael`) and it synthesises from
//! text alone. There is no reference-audio cloning path in stock Kokoro.
//!
//! Recommended engine crate: `kokoro-tts` (mzdk100, v0.3.x, ONNX/ort 2.x, Apache-2.0).
//! Fallback engine crate: `any-tts` (TM9657, v0.1.x, Candle, unifies Kokoro + Qwen3-TTS).
//!
//! Model assets (download once, Apache-2.0):
//!   onnx-community/Kokoro-82M-v1.0-ONNX  ->  onnx/model_q8f16.onnx (86 MB) + voices/*.bin
//!   mzdk100/kokoro releases V1.0 / V1.1   ->  bundled model + voice packs

use crate::profiles::VoiceProfile;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Kokoro preset voice identifier, e.g. `af_heart` (American-female "heart").
/// See hexgrad/Kokoro-82M VOICES.md for the full 54-voice list (v1.0).
pub type KokoroVoiceId = str;

/// Configuration for the Kokoro backend. Cheap to clone; the heavy state
/// (ONNX session + loaded voice packs) lives in the `Arc<KokoroEngine>`.
#[derive(Clone)]
pub struct KokoroConfig {
    /// Directory containing `model.onnx` (+ quantised variants) and `voices/*.bin`.
    pub model_dir: PathBuf,
    /// Which ONNX variant to load: `model.onnx` | `model_q8f16.onnx` | `model_fp16.onnx` ...
    pub model_variant: String,
    /// Default preset voice when a profile doesn't name one.
    pub default_voice: String,
    /// Where to persist generated WAVs (content-addressed by hash).
    pub cache_dir: PathBuf,
}

/// Native Kokoro backend. The engine is lazily initialised on first `generate`
/// so that simply registering the backend doesn't pay the ONNX load cost.
pub struct KokoroClient {
    cfg: KokoroConfig,
    // Lazily-built engine. `Arc` keeps it cheap to clone if the Tauri state
    // needs shared ownership. The inner `Box<dyn>` hides the engine crate so
    // the rest of openscript-tts doesn't need to depend on `ort`/`kokoro-tts`
    // at the type level.
    engine: tokio::sync::OnceCell<Arc<KokoroEngine>>,
}

// In the real implementation this wraps `kokoro_tts::Kokoro` (or an `ort::Session`
// + phonemizer). Kept opaque here so the sketch compiles without the engine dep.
struct KokoroEngine {
    // e.g. kokoro_tts::Kokoro  — loaded model + voice registry
    _inner: (),
}

impl KokoroEngine {
    fn load(cfg: &KokoroConfig) -> Result<Self, KokoroError> {
        let model_path = cfg.model_dir.join("onnx").join(&cfg.model_variant);
        if !model_path.exists() {
            return Err(KokoroError::AssetMissing(format!(
                "Kokoro model not found at {}. Download from \
                 https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX",
                model_path.display()
            )));
        }
        // ---- real wiring (illustrative, against kokoro-tts 0.3.x API) ----
        // use kokoro_tts::{Kokoro, Config};
        // let config = Config::builder()
        //     .model_path(&model_path)
        //     .voices_dir(cfg.model_dir.join("voices"))
        //     .sample_rate(24_000)
        //     .build()?;
        // let engine = Kokoro::new(config)?;
        // Ok(Self { inner: engine })
        Ok(Self { _inner: () })
    }

    /// Synthesise `text` with `voice` at `speed` (1.0 = normal).
    /// Returns mono f32 PCM at 24 kHz — the native Kokoro sample rate.
    fn synth(&self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, KokoroError> {
        let _ = (text, voice, speed);
        // engine.inner.synth(text, voice, speed)
        Err(KokoroError::NotWired(
            "engine synth() not connected to kokoro-tts crate yet".into(),
        ))
    }
}

impl KokoroClient {
    pub fn new(cfg: KokoroConfig) -> Self {
        Self {
            cfg,
            engine: tokio::sync::OnceCell::new(),
        }
    }

    /// Cheap liveness check — confirms the model dir + at least the default
    /// voice pack are present. Does NOT load the ONNX session.
    pub fn health_check(&self) -> Result<bool, KokoroError> {
        let model = self.cfg.model_dir.join("onnx").join(&self.cfg.model_variant);
        let voice = self.cfg.model_dir.join("voices").join(format!(
            "{}.bin",
            self.cfg.default_voice.split('_').next().unwrap_or("af")
        ));
        Ok(model.exists() && voice.exists())
    }

    /// Generate speech into `output_path`. Mirrors `TtsClient::generate` so the
    /// two backends are interchangeable from the Tauri command layer.
    ///
    /// NOTE on the `profile` arg: Kokoro ignores `ref_audio` / `ref_text` (those
    /// are clone-only fields). It reads the preset voice from `profile.model`
    /// (we reuse that field to carry e.g. `af_heart`) falling back to the
    /// config default. `speed`/`pitch`/`volume` map to Kokoro's `speed` knob
    /// (pitch & volume are post-processed in FFmpeg, as today).
    pub async fn generate(
        &self,
        voice_profile_id: &str,
        text: &str,
        output_path: &str,
        speed: f64,
        pitch: f64,
        volume: f64,
        _format: &str,
        profile: &VoiceProfile,
    ) -> Result<KokoroResult, KokoroError> {
        let voice = if !profile.model.is_empty() && profile.model.starts_with("kokoro") {
            // model field convention: "kokoro:af_heart"
            profile.model.split(':').nth(1).unwrap_or(&self.cfg.default_voice)
        } else {
            &self.cfg.default_voice
        };

        // 1. Content-addressed cache (same scheme as client.rs so caches don't collide).
        let key = Self::cache_key(voice_profile_id, text, speed, pitch, volume);
        if let Some(cached) = Self::cached_path(&self.cfg.cache_dir, &key, "wav") {
            if cached.exists() {
                std::fs::create_dir_all(Path::new(output_path).parent().unwrap_or(Path::new(".")))?;
                std::fs::copy(&cached, output_path)?;
                return Ok(KokoroResult {
                    output_path: output_path.to_string(),
                    duration_ms: wav_duration_ms(&cached).unwrap_or(0),
                    cached: true,
                });
            }
        }

        // 2. Lazily build the ONNX engine on first use (heavy: ~200ms cold start).
        let engine = self
            .engine
            .get_or_try_init(|| {
                std::thread::scope::<_, Result<Arc<KokoroEngine>, KokoroError>, _>(|s| {
                    let eng = s.spawn(|| KokoroEngine::load(&self.cfg)).join().unwrap()?;
                    Ok(Arc::new(eng))
                })
            })?
            .clone();

        // 3. Synthesise on a blocking thread (ONNX inference is CPU-bound).
        let text = text.to_string();
        let voice = voice.to_string();
        let samples = tokio::task::spawn_blocking(move || {
            engine.synth(&text, &voice, speed as f32)
        })
        .await
        .map_err(|e| KokoroError::Engine(e.to_string()))??;

        // 4. Encode f32 PCM -> 16-bit PCM WAV at 24 kHz.
        let wav_bytes = encode_wav_pcm16(&samples, 24_000);

        // 5. Write outputs (final + cache).
        let out = PathBuf::from(output_path);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&out, &wav_bytes)?;
        let cache = self.cfg.cache_dir.join(format!("{}.wav", key));
        if let Some(p) = cache.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&cache, &wav_bytes)?;

        Ok(KokoroResult {
            output_path: output_path.to_string(),
            duration_ms: ((samples.len() as f64 / 24_000.0) * 1000.0).round() as i64,
            cached: false,
        })
    }

    /// Reuses the exact estimate formula from `client.rs` so timelines line up
    /// regardless of which backend produced a clip.
    pub fn estimate_duration(text: &str, speed: f64) -> i64 {
        let words = text.split_whitespace().count() as f64;
        let base_rate = 2.5;
        let adjusted = base_rate * speed;
        if adjusted <= 0.0 {
            return 0;
        }
        ((words / adjusted) * 1000.0) as i64
    }

    fn cache_key(voice_id: &str, text: &str, speed: f64, pitch: f64, volume: f64) -> String {
        let input = format!("kokoro|{}|{}|{}|{}|{}", voice_id, text, speed, pitch, volume);
        let mut h = Sha256::new();
        h.update(input.as_bytes());
        hex::encode(h.finalize())[..16].to_string()
    }

    fn cached_path(cache_dir: &Path, key: &str, fmt: &str) -> Option<PathBuf> {
        let p = cache_dir.join(format!("{}.{}", key, fmt));
        p.exists().then_some(p)
    }
}

pub struct KokoroResult {
    pub output_path: String,
    pub duration_ms: i64,
    pub cached: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum KokoroError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Kokoro asset missing: {0}")]
    AssetMissing(String),
    #[error("Kokoro engine error: {0}")]
    Engine(String),
    #[error("Backend not yet wired: {0}")]
    NotWired(String),
}

// ---- tiny WAV encoder so we don't pull in `hound` for a one-shot write ----
fn encode_wav_pcm16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    let fmt_len: u32 = 16;
    let audio_fmt: u16 = 1; // PCM
    let channels: u16 = 1;
    let bits: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * (bits / 2);
    let block_align = channels * (bits / 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    out.extend_from_slice(&audio_fmt.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn wav_duration_ms(path: &Path) -> Option<i64> {
    // Prefer the embedded duration; fall back to ffprobe (same as client.rs).
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" {
        return None;
    }
    let sample_rate = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    let data_len = u32::from_le_bytes(bytes[40..44].try_into().ok()?) as f64;
    if sample_rate == 0 {
        return None;
    }
    Some(((data_len / 2.0 / sample_rate as f64) * 1000.0).round() as i64)
}
