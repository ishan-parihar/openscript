//! Multi-backend LLM / vision inference for OpenScript directors.
//!
//! Cascade (first success wins):
//!   1. **Local** — Ollama OpenAI-compatible API (`qwen3.5-4b` / GGUF at
//!      `~/Downloads/Qwen3.5-4B-Q4_K_M.gguf` via `OPENSCRIPT_GGUF_PATH`)
//!   2. **OpenRouter free multimodal** —
//!      `google/gemma-4-31b-it:free` then
//!      `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free`
//!
//! Vision: extract mid-frame JPEG with ffmpeg, send as base64 `image_url` to
//! backends that support multimodal. Local Qwen3.5-4B text model can still
//! score relevance from scene text + search query when vision is unavailable.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("No LLM backend available: {0}")]
    NoBackend(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Inference failed: {0}")]
    Inference(String),
    #[error("Frame extract failed: {0}")]
    Frame(String),
}

/// Ordered model cascade for text / vision.
#[derive(Debug, Clone)]
pub struct LlmCascade {
    pub local_model: String,
    pub local_base_url: String,
    pub openrouter_models: Vec<String>,
    pub openrouter_base_url: String,
    pub gguf_path: Option<PathBuf>,
    pub mmproj_path: Option<PathBuf>,
}

impl Default for LlmCascade {
    fn default() -> Self {
        Self {
            local_model: std::env::var("OPENSCRIPT_LOCAL_MODEL")
                .unwrap_or_else(|_| "qwen3.5-4b".to_string()),
            local_base_url: std::env::var("OPENSCRIPT_LLM_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_string()),
            openrouter_models: vec![
                std::env::var("OPENSCRIPT_OPENROUTER_VISION_MODEL")
                    .unwrap_or_else(|_| "google/gemma-4-31b-it:free".to_string()),
                std::env::var("OPENSCRIPT_OPENROUTER_VISION_FALLBACK")
                    .unwrap_or_else(|_| {
                        "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free".to_string()
                    }),
            ],
            openrouter_base_url: std::env::var("OPENROUTER_BASE_URL")
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string()),
            gguf_path: resolve_gguf_path(),
            mmproj_path: resolve_mmproj_path(),
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolve local GGUF: env → ~/Downloads → mcp/models
pub fn resolve_gguf_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OPENSCRIPT_GGUF_PATH") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let candidates = [
        home_dir().join("Downloads/Qwen3.5-4B-Q4_K_M.gguf"),
        home_dir().join("Downloads/Qwen3.5-4B-Q4_K_S.gguf"),
        PathBuf::from("mcp/models/Qwen3.5-4B-Q4_K_M.gguf"),
        PathBuf::from("mcp/assets/models/Qwen3.5-4B-Q4_K_M.gguf"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

pub fn resolve_mmproj_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OPENSCRIPT_MMPROJ_PATH") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let candidates = [
        home_dir().join("Downloads/mmproj-F16.gguf"),
        home_dir().join("Downloads/mmproj-BF16.gguf"),
        PathBuf::from("mcp/models/mmproj-F16.gguf"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn openrouter_key() -> String {
    std::env::var("OPENROUTER_API_KEY")
        .or_else(|_| std::env::var("OPENROUTER_KEY"))
        .unwrap_or_default()
}

/// Probe which backends are usable.
pub async fn probe_llm_capabilities() -> Value {
    let cascade = LlmCascade::default();
    let local_ok = probe_openai_compatible(&cascade.local_base_url, None).await;
    let or_key = !openrouter_key().is_empty();
    json!({
        "local": {
            "available": local_ok,
            "base_url": cascade.local_base_url,
            "model": cascade.local_model,
            "gguf_path": cascade.gguf_path.as_ref().map(|p| p.display().to_string()),
            "gguf_present": cascade.gguf_path.is_some(),
            "mmproj_path": cascade.mmproj_path.as_ref().map(|p| p.display().to_string()),
            "mmproj_present": cascade.mmproj_path.is_some(),
            "reason": if local_ok {
                Value::Null
            } else {
                Value::String(
                    "Ollama not reachable at OPENSCRIPT_LLM_URL (default http://127.0.0.1:11434/v1). \
                     Start: ollama serve && ollama run qwen3.5-4b. \
                     GGUF: ~/Downloads/Qwen3.5-4B-Q4_K_M.gguf — import with scripts/import_local_gguf.sh"
                        .into(),
                )
            },
        },
        "openrouter": {
            "available": or_key,
            "models": cascade.openrouter_models,
            "base_url": cascade.openrouter_base_url,
            "reason": if or_key {
                Value::Null
            } else {
                Value::String("OPENROUTER_API_KEY not set — free multimodal fallbacks disabled".into())
            },
        },
        "cascade": [
            format!("local:{}", cascade.local_model),
            format!("openrouter:{}", cascade.openrouter_models.first().cloned().unwrap_or_default()),
            format!("openrouter:{}", cascade.openrouter_models.get(1).cloned().unwrap_or_default()),
        ],
    })
}

async fn probe_openai_compatible(base_url: &str, api_key: Option<&str>) -> bool {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut req = client.get(&url);
    if let Some(k) = api_key {
        if !k.is_empty() {
            req = req.bearer_auth(k);
        }
    }
    match req.send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResult {
    pub text: String,
    pub backend: String,
    pub model: String,
}

fn strip_think_tags(s: &str) -> String {
    // Qwen3.5 may emit <think>...</think> before the answer
    let mut out = s.to_string();
    while let Some(start) = out.find("<think>") {
        if let Some(end_rel) = out[start..].find("</think>") {
            let end = start + end_rel + "</think>".len();
            out.replace_range(start..end, "");
        } else {
            break;
        }
    }
    out.trim().to_string()
}

/// Chat completion with cascade. `image_b64` enables multimodal when provided.
pub async fn chat_complete(
    system: &str,
    user: &str,
    image_b64_jpeg: Option<&str>,
) -> Result<ChatResult, LlmError> {
    let cascade = LlmCascade::default();
    let mut errors: Vec<String> = Vec::new();

    // 1) Local Ollama (text always; vision only if image + mmproj-capable model)
    if probe_openai_compatible(&cascade.local_base_url, None).await {
        // Prefer text-only for local Qwen unless OPENSCRIPT_LOCAL_VISION=1
        let use_image = image_b64_jpeg.is_some()
            && std::env::var("OPENSCRIPT_LOCAL_VISION")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
        match openai_chat(
            &cascade.local_base_url,
            &cascade.local_model,
            None,
            system,
            user,
            if use_image { image_b64_jpeg } else { None },
        )
        .await
        {
            Ok(text) => {
                return Ok(ChatResult {
                    text: strip_think_tags(&text),
                    backend: "local_ollama".into(),
                    model: cascade.local_model.clone(),
                });
            }
            Err(e) => errors.push(format!("local:{}", e)),
        }
        // If vision was requested but local failed with image, retry text-only
        if use_image {
            if let Ok(text) = openai_chat(
                &cascade.local_base_url,
                &cascade.local_model,
                None,
                system,
                user,
                None,
            )
            .await
            {
                return Ok(ChatResult {
                    text: strip_think_tags(&text),
                    backend: "local_ollama_text".into(),
                    model: cascade.local_model.clone(),
                });
            }
        }
    } else {
        errors.push("local:ollama unreachable".into());
    }

    // 2) OpenRouter free multimodal cascade
    let key = openrouter_key();
    if !key.is_empty() {
        for model in &cascade.openrouter_models {
            match openai_chat(
                &cascade.openrouter_base_url,
                model,
                Some(&key),
                system,
                user,
                image_b64_jpeg,
            )
            .await
            {
                Ok(text) => {
                    return Ok(ChatResult {
                        text: strip_think_tags(&text),
                        backend: "openrouter".into(),
                        model: model.clone(),
                    });
                }
                Err(e) => errors.push(format!("openrouter/{}: {}", model, e)),
            }
        }
    } else {
        errors.push("openrouter: no OPENROUTER_API_KEY".into());
    }

    Err(LlmError::NoBackend(errors.join(" | ")))
}

async fn openai_chat(
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    system: &str,
    user: &str,
    image_b64_jpeg: Option<&str>,
) -> Result<String, LlmError> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| LlmError::Http(e.to_string()))?;

    let user_content: Value = if let Some(b64) = image_b64_jpeg {
        json!([
            {"type": "text", "text": user},
            {"type": "image_url", "image_url": {
                "url": format!("data:image/jpeg;base64,{}", b64)
            }}
        ])
    } else {
        json!(user)
    };

    // Qwen3.5 (Ollama) may spend tokens on internal reasoning; give headroom
    // and disable think when the backend supports it (Ollama / some OpenAI forks).
    let mut body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user_content}
        ],
        "temperature": 0.2,
        "max_tokens": 1024,
        "think": false,
    });
    // Ollama also accepts think under options for some versions
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "options".into(),
            json!({"temperature": 0.2, "num_predict": 1024}),
        );
    }

    let mut req = client.post(&url).json(&body);
    if let Some(k) = api_key {
        req = req
            .bearer_auth(k)
            .header("HTTP-Referer", "https://github.com/ishan-parihar/openscript")
            .header("X-Title", "OpenScript Vision Director");
    }

    let resp = req
        .send()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status();
    let text_body = resp
        .text()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
    if !status.is_success() {
        return Err(LlmError::Inference(format!(
            "HTTP {} — {}",
            status,
            text_body.chars().take(400).collect::<String>()
        )));
    }
    let v: Value = serde_json::from_str(&text_body)?;
    let msg = v.pointer("/choices/0/message");
    let content = extract_message_text(msg).ok_or_else(|| {
        LlmError::Inference(format!(
            "no content in response: {}",
            text_body.chars().take(200).collect::<String>()
        ))
    })?;
    if content.trim().is_empty() {
        return Err(LlmError::Inference(
            "model returned empty content (try raising max_tokens or disable thinking)".into(),
        ));
    }
    Ok(content)
}

/// Pull assistant text from OpenAI-compatible message objects.
/// Qwen3.5-via-Ollama often puts chain-of-thought in `reasoning` / `reasoning_content`
/// while leaving `content` empty until think finishes — accept either.
fn extract_message_text(msg: Option<&Value>) -> Option<String> {
    let msg = msg?;
    let from_content = |c: &Value| -> Option<String> {
        if let Some(s) = c.as_str() {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
            return None;
        }
        if let Some(arr) = c.as_array() {
            let joined: String = arr
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("");
            if !joined.trim().is_empty() {
                return Some(joined);
            }
        }
        None
    };
    if let Some(c) = msg.get("content") {
        if let Some(s) = from_content(c) {
            return Some(s);
        }
    }
    for key in ["reasoning_content", "reasoning", "thinking"] {
        if let Some(c) = msg.get(key) {
            if let Some(s) = from_content(c) {
                return Some(s);
            }
        }
    }
    None
}

/// Extract a JPEG frame from a video at `at_s` seconds (default midpoint).
pub async fn extract_frame_jpeg(
    video_path: &str,
    at_s: Option<f64>,
    out_jpg: &str,
) -> Result<PathBuf, LlmError> {
    if !Path::new(video_path).exists() {
        return Err(LlmError::Frame(format!("video not found: {}", video_path)));
    }
    let ss = if let Some(t) = at_s {
        t
    } else {
        // midpoint via ffprobe
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                video_path,
            ])
            .output()
            .await?;
        let dur: f64 = String::from_utf8_lossy(&probe.stdout)
            .trim()
            .parse()
            .unwrap_or(3.0);
        (dur * 0.4).max(0.1)
    };
    if let Some(parent) = Path::new(out_jpg).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = Command::new("ffmpeg")
        .args([
            "-y",
            "-ss",
            &ss.to_string(),
            "-i",
            video_path,
            "-frames:v",
            "1",
            "-q:v",
            "3",
            out_jpg,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await?;
    if !out.status.success() || !Path::new(out_jpg).exists() {
        return Err(LlmError::Frame(format!(
            "ffmpeg frame extract failed for {}",
            video_path
        )));
    }
    Ok(PathBuf::from(out_jpg))
}

pub fn jpeg_to_base64(path: &str) -> Result<String, LlmError> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(base64_encode(&buf))
}

fn base64_encode(data: &[u8]) -> String {
    // Minimal base64 without extra crate dependency
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Describe a video clip frame with the vision/text cascade.
pub async fn analyze_clip(
    video_path: &str,
    at_s: Option<f64>,
    prompt: Option<&str>,
) -> Result<Value, LlmError> {
    let frame_path = format!(
        "/tmp/openscript_vision_analyze_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let _ = extract_frame_jpeg(video_path, at_s, &frame_path).await?;
    let b64 = jpeg_to_base64(&frame_path)?;
    let _ = std::fs::remove_file(&frame_path);

    let system = "You are a short-form video vision assistant. \
        /no_think Describe the frame factually for a director choosing B-roll. \
        Reply with ONLY compact JSON (no markdown, no reasoning): \
        {\"description\":\"...\",\"time_of_day\":\"day|night|indoor|unknown\",\
        \"setting\":\"brief\",\"subjects\":[\"...\"],\"ui_or_text_visible\":false}";
    let user = format!(
        "/no_think {}",
        prompt.unwrap_or(
            "Describe this video frame: setting, lighting/time of day, main subjects, and any on-screen UI/text.",
        )
    );

    let result = chat_complete(system, &user, Some(&b64)).await?;
    let parsed = parse_json_loose(&result.text);
    Ok(json!({
        "status": "analyzed",
        "backend": result.backend,
        "model": result.model,
        "video_path": video_path,
        "at_s": at_s,
        "raw": result.text,
        "analysis": parsed,
    }))
}

/// Vision/text score of a clip against scene context. Returns JSON-shaped Value.
pub async fn score_clip_relevance(
    video_path: &str,
    scene_text: &str,
    video_keywords: &[String],
    search_query: Option<&str>,
) -> Result<Value, LlmError> {
    let frame_path = format!(
        "/tmp/openscript_vision_{}.jpg",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let _ = extract_frame_jpeg(video_path, None, &frame_path).await?;
    let b64 = jpeg_to_base64(&frame_path)?;
    let _ = std::fs::remove_file(&frame_path);

    let kw = video_keywords.join(", ");
    let system = "You are a short-form video director's vision assistant. \
        /no_think Judge whether a stock clip matches the spoken scene. \
        Reply with ONLY compact JSON (no markdown, no reasoning): \
        {\"relevance\":0.0-1.0,\"time_of_day\":\"day|night|indoor|unknown\",\
        \"setting\":\"brief\",\"match\":true|false,\"reason\":\"one sentence\"}";
    let user = format!(
        "/no_think Scene dialogue: \"{}\"\nVideo topic keywords: [{}]\nStock search query: \"{}\"\n\
         Does this frame match the scene context? Score relevance 0-1. JSON only.",
        scene_text,
        kw,
        search_query.unwrap_or("")
    );

    let result = chat_complete(system, &user, Some(&b64)).await?;
    let parsed = parse_json_loose(&result.text);
    Ok(json!({
        "status": "scored",
        "backend": result.backend,
        "model": result.model,
        "video_path": video_path,
        "scene_text": scene_text,
        "raw": result.text,
        "score": parsed,
    }))
}

fn parse_json_loose(s: &str) -> Value {
    let trimmed = s.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v;
    }
    // Find first { ... }
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                if let Ok(v) = serde_json::from_str::<Value>(&trimmed[start..=end]) {
                    return v;
                }
            }
        }
    }
    json!({"parse_error": true, "text": trimmed})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_prefix() {
        let s = base64_encode(b"hello");
        assert!(s.starts_with("aGVsbG8"));
    }

    #[test]
    fn strip_think() {
        let s = strip_think_tags("<think>nope</think>\n{\"a\":1}");
        assert!(s.contains("{\"a\":1}"));
        assert!(!s.contains("think"));
    }

    #[test]
    fn parse_loose_json() {
        let v = parse_json_loose("Here you go:\n{\"relevance\": 0.8, \"match\": true}\n");
        assert_eq!(v["relevance"], 0.8);
    }

    #[test]
    fn extract_prefers_content_over_reasoning() {
        let msg = json!({"content": "pong", "reasoning": "thinking..."});
        assert_eq!(extract_message_text(Some(&msg)).as_deref(), Some("pong"));
    }

    #[test]
    fn extract_falls_back_to_reasoning_when_content_empty() {
        let msg = json!({"content": "", "reasoning": "answer is pong"});
        assert_eq!(
            extract_message_text(Some(&msg)).as_deref(),
            Some("answer is pong")
        );
    }
}
