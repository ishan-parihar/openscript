//! Kokoro TTS backend (preset-voice, ONNX-based).
//!
//! Wired into `lib.rs` behind the `kokoro` feature flag. The default build
//! (sidecar-only) is unaffected.
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
//!
//! # Chunking
//!
//! Kokoro caps input at 510 phoneme tokens (~one paragraph). Long-form
//! narration is split on sentence boundaries via `chunk_text()`, synthesised
//! per-chunk, and the PCM samples are concatenated. Silence between chunks
//! is not inserted — the Kokoro model already produces natural inter-sentence
//! pauses.

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
    /// Directory containing `onnx/kokoro-v1.0.onnx` and `voices/voices-v1.0.bin`.
    pub model_dir: PathBuf,
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

// The engine shells out to the Python `kokoro-onnx` sidecar
// (mcp/scripts/kokoro_tts_sidecar.py) for ONNX inference. This avoids
// the need for a native Rust ONNX runtime dependency.
struct KokoroEngine {
    cfg: KokoroConfig,
    /// Long-lived sidecar pool. Shared across all synth_one calls so we
    /// pay the ~200ms ONNX model load once instead of per-chunk. The
    /// Option is None until first use; once a start attempt fails we
    /// leave it as None so subsequent calls don't retry (and fall back
    /// to fresh-process mode instead).
    sidecar: crate::kokoro_sidecar::SharedSidecar,
}

impl KokoroEngine {
    fn load(cfg: &KokoroConfig) -> Result<Self, KokoroError> {
        let model_path = cfg.model_dir.join("onnx").join("kokoro-v1.0.onnx");
        if !model_path.exists() {
            return Err(KokoroError::AssetMissing(format!(
                "Kokoro model not found at {}. Download from \
                 https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx",
                model_path.display()
            )));
        }
        let voices_file = cfg.model_dir.join("voices").join("voices-v1.0.bin");
        if !voices_file.exists() {
            return Err(KokoroError::AssetMissing(format!(
                "Kokoro voices file not found at {}. Download from \
                 https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin",
                voices_file.display()
            )));
        }
        Ok(Self {
            cfg: cfg.clone(),
            sidecar: std::sync::Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// Synthesise `text` with `voice` at `speed` (1.0 = normal).
    /// Returns mono f32 PCM at 24 kHz — the native Kokoro sample rate.
    ///
    /// Long text is chunked via `chunk_text()` (510-token Kokoro limit) and
    /// the per-chunk PCM is concatenated.
    fn synth(&self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, KokoroError> {
        let chunks = chunk_text(text, MAX_TOKENS_PER_CHUNK);
        let mut all_samples = Vec::new();
        for chunk in chunks {
            let samples = self.synth_one(&chunk, voice, speed)?;
            all_samples.extend(samples);
        }
        Ok(all_samples)
    }

    /// Synthesise a single chunk (must be ≤510 phoneme tokens).
    ///
    /// Tries the long-lived sidecar first (loads the ONNX model once,
    /// serves requests via stdin/stdout JSON). If the sidecar is
    /// unavailable (Python missing, imports missing, model missing, or
    /// the sidecar died mid-request), falls back to the legacy
    /// fresh-process-per-call path.
    ///
    /// The fallback path pays ~360ms cold-start per chunk (Python startup
    /// + kokoro_onnx import + ONNX model load + voices load). For a
    /// 20-scene script with 2 chunks per scene, that's 40 × 360ms =
    /// 14.4s of pure overhead. The long-lived path pays it once.
    fn synth_one(&self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, KokoroError> {
        let model_path = self.cfg.model_dir.join("onnx").join("kokoro-v1.0.onnx");
        let voices_file = self.cfg.model_dir.join("voices").join("voices-v1.0.bin");

        // Use a temp file for the WAV output
        let tmp = std::env::temp_dir().join(format!(
            "kokoro_{}_{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        // Resolve the Kokoro sidecar script path via the shared helper in
        // kokoro_sidecar.rs (priority: KOKORO_SIDECAR env > CARGO_MANIFEST_DIR
        // > OPENSCRIPT_ROOT > relative path). Prior versions inlined this
        // resolution; we now share it between the long-lived and
        // fresh-process paths so they always agree on the script location.
        let sidecar_script = crate::kokoro_sidecar::resolve_sidecar_script();

        // === PATH 1: Long-lived sidecar (preferred) ===
        // Acquire the shared sidecar (starting it on first call). If the
        // sidecar is running, send the synth request and read the WAV
        // result. On any error, fall through to PATH 2 (fresh-process mode).
        let sidecar_ok = match crate::kokoro_sidecar::acquire_or_init(
            &self.sidecar,
            &model_path,
            &voices_file,
            &sidecar_script.to_string_lossy(),
        ) {
            Ok(mut guard) => {
                if let Some(ref mut sidecar) = *guard {
                    match sidecar.synth(text, voice, speed, &tmp.to_string_lossy()) {
                        Ok(_result) => true,
                        Err(e) => {
                            // Sidecar was running but the request failed. The
                            // process may have died; reset the slot so the next
                            // call attempts a fresh start.
                            tracing::warn!(
                                "Kokoro long-lived sidecar request failed ({}); \
                                 falling back to fresh-process mode for this chunk.",
                                e
                            );
                            *guard = None;
                            false
                        }
                    }
                } else {
                    // Sidecar failed to start earlier; fall back to fresh mode.
                    false
                }
            }
            Err(e) => {
                tracing::debug!("Sidecar acquire failed ({}); using fresh-process mode", e);
                false
            }
        };

        // === PATH 2: Fresh-process-per-call (fallback) ===
        // Spawns a new Python process for this one chunk. Pays the full
        // ~360ms cold-start. Used when the long-lived sidecar is unavailable.
        if !sidecar_ok {
            let output = std::process::Command::new("python3")
                .arg(&sidecar_script)
                .arg("--text")
                .arg(text)
                .arg("--voice")
                .arg(voice)
                .arg("--speed")
                .arg(speed.to_string())
                .arg("--model")
                .arg(&model_path)
                .arg("--voices")
                .arg(&voices_file)
                .arg("--output")
                .arg(&tmp)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .map_err(|e| {
                    KokoroError::Engine(format!("Failed to spawn kokoro sidecar: {}", e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let _ = std::fs::remove_file(&tmp);
                return Err(KokoroError::Engine(format!(
                    "Kokoro sidecar failed: {}",
                    stderr
                )));
            }
        }

        // Read the WAV file via hound (chunk-aware, supports 16/24/32-bit + float).
        // Prior versions hand-parsed the WAV header assuming `data` starts at
        // byte 44, which breaks on WAVs with LIST/fact/bext chunks and only
        // supports 16-bit PCM.
        let samples = match hound::WavReader::open(&tmp) {
            Ok(reader) => {
                let spec = reader.spec();
                match spec.sample_format {
                    hound::SampleFormat::Int => {
                        let max_val = (1u64 << (spec.bits_per_sample - 1)) as f32;
                        reader
                            .into_samples::<i32>()
                            .map(|s| s.map(|v| v as f32 / max_val).unwrap_or(0.0))
                            .collect::<Vec<f32>>()
                    }
                    hound::SampleFormat::Float => reader
                        .into_samples::<f32>()
                        .map(|s| s.unwrap_or(0.0))
                        .collect::<Vec<f32>>(),
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(KokoroError::Engine(format!(
                    "Failed to parse sidecar WAV output: {}",
                    e
                )));
            }
        };
        let _ = std::fs::remove_file(&tmp);
        Ok(samples)
    }
}

/// Kokoro's hard limit on input token count. We approximate 1 token ≈ 0.75 word
/// (phoneme tokens are denser than word tokens) and use 400 as a safe ceiling
/// below the 510-token model limit.
const MAX_TOKENS_PER_CHUNK: usize = 400;

/// Split `text` into chunks that fit within Kokoro's 510-token input limit.
/// Splits on sentence boundaries (`.`, `!`, `?`, `。`, `！`, `？`) first, then
/// on commas / semicolons if a single sentence exceeds the limit, then on
/// word boundaries as a last resort.
///
/// For CJK text (no spaces between words), uses a character budget instead
/// of a word budget — each CJK character is approximately 1 token.
pub fn chunk_text(text: &str, max_words: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    // Detect CJK-heavy text: if >30% of characters are CJK, use char-based chunking
    let cjk_count = text.chars().filter(|&c| is_cjk(c)).count();
    let total_chars = text.chars().count();
    let is_cjk_heavy = total_chars > 0 && cjk_count as f64 / total_chars as f64 > 0.3;

    if is_cjk_heavy {
        // Use character budget: ~1.5 chars per token, so max_chars = max_words * 1.5
        let max_chars = (max_words as f64 * 1.5) as usize;
        return chunk_by_chars(text, max_chars);
    }

    let sentences = split_sentences(text);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_words = 0usize;

    for sentence in sentences {
        let sentence_words = sentence.split_whitespace().count();

        if sentence_words > max_words {
            // Flush current chunk first
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current_words = 0;
            }
            // Split the long sentence on commas
            for part in split_on_punctuation(&sentence, max_words) {
                chunks.push(part);
            }
            continue;
        }

        if current_words + sentence_words > max_words {
            chunks.push(std::mem::take(&mut current));
            current_words = 0;
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
        current_words += sentence_words;
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Returns true if the character is CJK (Chinese/Japanese/Korean).
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF |   // CJK Unified Ideographs
        0x3400..=0x4DBF |   // CJK Extension A
        0x3040..=0x309F |   // Hiragana
        0x30A0..=0x30FF |   // Katakana
        0xAC00..=0xD7AF |   // Hangul Syllables
        0xFF00..=0xFFEF     // Halfwidth/Fullwidth Forms
    )
}

/// Chunk text by character count (for CJK text).
/// Splits on sentence boundaries first, then hard-splits on character count.
fn chunk_by_chars(text: &str, max_chars: usize) -> Vec<String> {
    let sentences = split_sentences(text);
    let mut chunks = Vec::new();
    let mut current = String::new();

    for sentence in sentences {
        let sentence_chars = sentence.chars().count();

        if sentence_chars > max_chars {
            // Flush current chunk
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            // Hard-split the long sentence by character count
            let chars: Vec<char> = sentence.chars().collect();
            for chunk in chars.chunks(max_chars) {
                let s: String = chunk.iter().collect();
                let s = s.trim();
                if !s.is_empty() {
                    chunks.push(s.to_string());
                }
            }
            continue;
        }

        if current.chars().count() + sentence_chars > max_chars && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Split text into sentences on `.`, `!`, `?`, `。`, `！`, `？`.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？') {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }
    sentences
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split a too-long sentence on `,`, `;`, `、`, `，` to stay under `max_words`.
fn split_on_punctuation(sentence: &str, max_words: usize) -> Vec<String> {
    let parts: Vec<&str> = sentence
        .split(|c: char| matches!(c, ',' | ';' | '、' | '，'))
        .collect();
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_words = 0usize;

    for part in parts {
        let part = part.trim();
        let words = part.split_whitespace().count();
        if current_words + words > max_words && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_words = 0;
        }
        if !current.is_empty() {
            current.push_str(", ");
        }
        current.push_str(part);
        current_words += words;
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    // Last resort: if a single comma-split part still exceeds max_words,
    // hard-split on word boundaries.
    chunks
        .into_iter()
        .flat_map(|chunk| {
            if chunk.split_whitespace().count() > max_words {
                hard_split_words(&chunk, max_words)
            } else {
                vec![chunk]
            }
        })
        .collect()
}

/// Hard-split `text` on word boundaries, each chunk ≤ `max_words`.
fn hard_split_words(text: &str, max_words: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words
        .chunks(max_words)
        .map(|chunk| chunk.join(" "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_short_text() {
        let chunks = chunk_text("Hello world.", 400);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world.");
    }

    #[test]
    fn test_chunk_empty() {
        let chunks = chunk_text("", 400);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_multiple_sentences() {
        let text = "First sentence. Second sentence! Third? Fourth.";
        let chunks = chunk_text(text, 400);
        assert_eq!(chunks.len(), 1); // all fit in one chunk
        assert!(chunks[0].contains("First sentence"));
        assert!(chunks[0].contains("Fourth"));
    }

    #[test]
    fn test_chunk_splits_on_sentence_boundary() {
        // 5 sentences, each 3 words = 15 words. With max_words=6 we expect
        // at least 3 chunks (6+6+3).
        let text = "one two three. four five six. seven eight nine. ten eleven twelve. thirteen fourteen fifteen.";
        let chunks = chunk_text(text, 6);
        assert!(chunks.len() >= 3);
        for chunk in &chunks {
            assert!(chunk.split_whitespace().count() <= 6);
        }
    }

    #[test]
    fn test_chunk_long_single_sentence() {
        // A single sentence with many words — must be split on commas/words.
        let words: Vec<String> = (0..50).map(|i| format!("word{}", i)).collect();
        let text = words.join(", ") + ".";
        let chunks = chunk_text(&text, 10);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.split_whitespace().count() <= 10);
        }
    }

    #[test]
    fn test_wav_encoder_roundtrip() {
        let samples = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let wav = encode_wav_pcm16(&samples, 24_000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // data_len = 5 samples * 2 bytes = 10
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 10);
    }

    #[test]
    fn test_wav_duration_ms() {
        // 24000 Hz, 24000 samples = 1 second = 1000ms
        let samples = vec![0.0f32; 24_000];
        let wav = encode_wav_pcm16(&samples, 24_000);
        let tmp = std::env::temp_dir().join("kokoro_test.wav");
        std::fs::write(&tmp, &wav).unwrap();
        let dur = wav_duration_ms(&tmp).unwrap();
        assert_eq!(dur, 1000);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_chunk_cjk_text() {
        // CJK text without spaces — must use character-based chunking
        // 100 CJK characters, max_words=10 → max_chars=15 → ~7 chunks
        let text: String = "你好世界。".repeat(20);
        let chunks = chunk_text(&text, 10);
        assert!(
            chunks.len() > 1,
            "CJK text should be split into multiple chunks"
        );
        for chunk in &chunks {
            // Each chunk should be under the char budget (with some tolerance)
            assert!(
                chunk.chars().count() <= 20,
                "Chunk too long: {} chars",
                chunk.chars().count()
            );
        }
    }

    #[test]
    fn test_chunk_mixed_cjk_latin() {
        // Mixed CJK + Latin text
        let text = "Hello world. 你好世界. This is a test. 这是测试.";
        let chunks = chunk_text(text, 400);
        // Should produce at least one chunk without panicking
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_is_cjk() {
        assert!(is_cjk('中'));
        assert!(is_cjk('日'));
        assert!(is_cjk('한'));
        assert!(!is_cjk('a'));
        assert!(!is_cjk(' '));
        assert!(!is_cjk('1'));
    }
}

impl KokoroClient {
    pub fn new(cfg: KokoroConfig) -> Self {
        Self {
            cfg,
            engine: tokio::sync::OnceCell::new(),
        }
    }

    /// Cheap liveness check — confirms the model file + voices file are present.
    pub fn health_check(&self) -> Result<bool, KokoroError> {
        let model = self.cfg.model_dir.join("onnx").join("kokoro-v1.0.onnx");
        let voices = self.cfg.model_dir.join("voices").join("voices-v1.0.bin");
        Ok(model.exists() && voices.exists())
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
            profile
                .model
                .split(':')
                .nth(1)
                .unwrap_or(&self.cfg.default_voice)
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
        //    We use a blocking init path because ONNX session creation is sync.
        let cfg_clone = self.cfg.clone();
        let engine = self
            .engine
            .get_or_try_init(|| async {
                let cfg = cfg_clone;
                let eng = tokio::task::spawn_blocking(move || KokoroEngine::load(&cfg))
                    .await
                    .map_err(|e| KokoroError::Engine(e.to_string()))??;
                Ok::<_, KokoroError>(Arc::new(eng))
            })
            .await?
            .clone();

        // 3. Synthesise on a blocking thread (ONNX inference is CPU-bound).
        let text = text.to_string();
        let voice = voice.to_string();
        let samples =
            tokio::task::spawn_blocking(move || engine.synth(&text, &voice, speed as f32))
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

        // Best-effort cache eviction — prevents unbounded growth
        crate::evict_cache_if_needed(&self.cfg.cache_dir);

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
        let input = format!(
            "kokoro|{}|{}|{}|{}|{}",
            voice_id, text, speed, pitch, volume
        );
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
}

// ---- WAV helpers via hound (replaces hand-rolled encoder + duration parser) ----

/// Encode f32 samples (range [-1.0, 1.0]) as a 16-bit PCM mono WAV file.
fn encode_wav_pcm16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // hound::WavWriter requires Seek, which Vec<u8> doesn't implement.
        // Use std::io::Cursor<Vec<u8>> as a seekable buffer.
        let mut cursor = std::io::Cursor::new(std::mem::take(&mut out));
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec)
                .expect("hound WavWriter init on Cursor<Vec<u8>> cannot fail");
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                writer
                    .write_sample(v)
                    .expect("write_sample on Cursor cannot fail");
            }
            writer.finalize().expect("finalize on Cursor cannot fail");
        }
        out = cursor.into_inner();
    }
    out
}

/// Read a WAV file's duration in milliseconds via hound (chunk-aware).
fn wav_duration_ms(path: &Path) -> Option<i64> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let duration = reader.duration();
    Some(((duration as f64 / spec.sample_rate as f64) * 1000.0).round() as i64)
}
