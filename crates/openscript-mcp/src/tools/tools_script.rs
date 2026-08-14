// ---------------------------------------------------------------------------
// tools_script — Script-to-video handlers (script.*, background integration)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

/// Apply user-level TTS config defaults to a script JSON string BEFORE parse:
/// when the script omits `tts.backend` / `tts.voice`, inject the resolved
/// config values (env `OPENSCRIPT_TTS_BACKEND`/`OPENSCRIPT_TTS_VOICE` →
/// `~/.openscript/config.json` `tts.default_backend`/`default_voice` →
/// built-in). Explicit script fields always win. This makes the configured
/// audio model the default for script→video without forcing every script to
/// repeat it — the "config-like" engine selection layer.
pub(crate) fn apply_tts_config_defaults(json_str: &str) -> String {
    let mut root: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return json_str.to_string(), // leave malformed JSON to parse_script's error
    };
    let obj = match root.as_object_mut() {
        Some(o) => o,
        None => return json_str.to_string(),
    };
    let tts_obj = obj
        .entry("tts")
        .or_insert_with(|| json!({}));
    let tts = match tts_obj.as_object_mut() {
        Some(t) => t,
        None => return json_str.to_string(),
    };
    if !tts.contains_key("backend") {
        let backend = crate::config::resolve_tts_default_backend();
        if !backend.is_empty() {
            tts.insert("backend".to_string(), json!(backend));
        }
    }
    if !tts.contains_key("voice") {
        if let Some(voice) = crate::config::resolve_tts_default_voice() {
            if !voice.is_empty() {
                tts.insert("voice".to_string(), json!(voice));
            }
        }
    }
    serde_json::to_string(&root).unwrap_or_else(|_| json_str.to_string())
}

/// Handle script.schema: return the full JSON schema for ScriptSpec.
/// WARNING: dual-maintenance — update this handler when ScriptSpec/SceneSpec/SpeakerSpec/BackgroundSpec fields change.
/// Agents call this to discover the correct format before writing a script.
pub(crate) async fn handle_script_schema(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "OpenScript Video Creation Script",
        "description": "Complete specification for AI-agent-driven video creation. All fields have sensible defaults — only 'speakers' and 'scenes' are required.",
        "type": "object",
        "required": ["speakers", "scenes"],
        "properties": {
            "schema": {"type": "string", "default": "openscript-video/v1", "description": "Schema version. Always use 'openscript-video/v1'."},
            "title": {"type": "string", "description": "Human-readable video title. If omitted, video_keywords are auto-extracted from this."},
            "video_keywords": {"type": "array", "items": {"type": "string"}, "description": "Topic keywords for the WHOLE video (3-5 words). Used to bias stock footage search. Auto-extracted from title if omitted."},
            "meta": {
                "type": "object",
                "description": "Output video metadata.",
                "properties": {
                    "aspect": {"type": "string", "default": "9:16", "enum": ["9:16", "16:9", "1:1"], "description": "Aspect ratio."},
                    "fps": {"type": "integer", "default": 30, "enum": [24, 30, 60], "description": "Frames per second."},
                    "width": {"type": "integer", "default": 1080},
                    "height": {"type": "integer", "default": 1920},
                    "resolution": {"type": "string", "default": "1080p"}
                }
            },
            "tts": {
                "type": "object",
                "description": "TTS engine configuration. Backend selects the audio model engine; a speaker voice of \"default\" resolves to tts.voice (or the user's ~/.openscript/config.json tts.default_voice, or OPENSCRIPT_TTS_VOICE).",
                "properties": {
                    "backend": {"type": "string", "default": "kokoro", "enum": ["kokoro", "audio8", "voicedesign", "higgs", "indextts", "sidecar"], "description": "Audio model engine: kokoro (presets), audio8 (zero-shot clone, ONNX INT4), voicedesign (Qwen3 VoiceDesign — direct NL-instruction synthesis with per-line emotion/tonality, NO cloning), higgs (Higgs Audio v3 4B — zero-shot clone + inline emotion/prosody control tags, 100+ languages), indextts (IndexTTS-2.5 — emotion-aware zero-shot clone, 22.05kHz, en/zh/ja/es/ar), sidecar (faster-qwen3-tts)."},
                    "voice": {"anyOf": [{"type": "string"}, {"type": "null"}], "default": null, "description": "Default voice profile id (e.g. 'ishan'). Speakers whose voice is the literal string 'default' use this profile."},
                    "default_speed": {"type": "number", "default": 1.0, "description": "Speech speed multiplier."},
                    "default_pitch": {"type": "number", "default": 1.0}
                }
            },
            "speakers": {
                "description": "Speaker definitions. Accepts BOTH formats: map (canonical) {\"narrator\": {\"voice\": \"kokoro:af_heart\"}} OR array (agent-friendly) [{\"id\": \"narrator\", \"voice\": \"kokoro:af_heart\"}].",
                "oneOf": [
                    {
                        "type": "object",
                        "description": "Map format (canonical): speaker_id → SpeakerSpec",
                        "additionalProperties": {
                            "$ref": "#/definitions/SpeakerSpec"
                        }
                    },
                    {
                        "type": "array",
                        "description": "Array format (agent-friendly): each entry needs 'id' and 'voice'.",
                        "items": {
                            "type": "object",
                            "required": ["voice"],
                            "properties": {
                                "id": {"type": "string", "description": "Speaker ID referenced by scenes."},
                                "voice": {"type": "string", "description": "Voice ID: 'kokoro:af_heart', 'kokoro:am_michael', or bare 'af_heart'. Use tts.voices to list all."}
                            }
                        }
                    }
                ]
            },
            "background": {
                "description": "Background config. Accepts BOTH: object (canonical) or string (agent-friendly). String = shorthand for {type: value}.",
                "oneOf": [
                    {
                        "$ref": "#/definitions/BackgroundSpec"
                    },
                    {
                        "type": "string",
                        "enum": ["procedural", "gameplay", "static"],
                        "description": "String shorthand: procedural, gameplay, or static."
                    }
                ]
            },
            "music": {
                "type": ["object", "null"],
                "properties": {
                    "path": {"type": ["string", "null"], "description": "Music file path. Omit to auto-select from library by mood."},
                    "gain_db": {"type": "number", "default": -10.0, "description": "Music volume in dB. Recommended: -8 to -14. Above -8 overpowers voice."},
                    "ducking": {"type": "boolean", "default": true, "description": "Auto-lower music during speech."},
                    "ducking_depth_db": {"type": "number", "default": 12.0},
                    "mood": {"type": ["string", "null"], "default": null, "description": "Music mood hint for auto-select (neutral | calm | energetic). Set by the content format's music_mood when the agent didn't pick a track."}
                }
            },
            "format": {
                "type": "object",
                "description": "Content-format configuration: correlated defaults + scene-structure playbook (presentation | podcast | dialogue | comedy_sketch | romcom | meme_reel | documentary | how_to | listicle | storytime | debate | newsflash | review) plus the speaker alternation strategy. Accepts string shorthand: \"format\": \"podcast\".",
                "properties": {
                    "type": {"type": "string", "default": "presentation", "enum": ["presentation", "podcast", "dialogue", "comedy_sketch", "romcom", "meme_reel", "documentary", "how_to", "listicle", "storytime", "debate", "newsflash", "review"], "description": "Format kind. presentation = linear single-narrator (default)."},
                    "alternation": {"type": "string", "default": "none", "enum": ["none", "male_female", "auto"], "description": "Speaker alternation strategy. male_female alternates male/female speakers every scene and requires >=2 distinct genders (validated by script.format.validate)."},
                    "min_speakers": {"type": "integer", "default": 0, "description": "Minimum speakers required (0 = none)."},
                    "max_speakers": {"type": "integer", "default": 0, "description": "Maximum speakers allowed (0 = none)."},
                    "min_scenes": {"type": "integer", "default": 0, "description": "Minimum scenes required (0 = none)."},
                    "max_scenes": {"type": "integer", "default": 0, "description": "Maximum scenes allowed (0 = none)."},
                    "default_speed": {"type": "number", "default": 0.0, "description": "Correlated default speech speed (0 = no override)."},
                    "default_temperature": {"type": ["number", "null"], "default": null, "description": "Correlated default synthesis temperature."},
                    "reaction_memes": {"type": "boolean", "default": false, "description": "Enable GIPHY reaction meme pop-ins by default."},
                    "sticker_mode": {"type": "string", "default": "character", "enum": ["character", "reaction", "none"], "description": "Correlated sticker behavior."},
                    "music_mood": {"type": ["string", "null"], "default": null, "description": "Correlated music mood hint (neutral | calm | energetic)."}
                }
            },
            "captions": {
                "type": "object",
                "properties": {
                    "style": {"type": "string", "default": "word_highlight", "enum": ["word_highlight", "sentence_fade", "karaoke_fill", "subtitle_rail"], "description": "Caption style. word_highlight = TikTok-style word sync (default)."},
                    "font": {"type": "string", "default": "Bebas Neue"},
                    "font_size": {"type": "integer", "default": 72},
                    "color": {"type": "string", "default": "#ffffff"},
                    "highlight_color": {"type": "string", "default": "#00ff88"},
                    "position": {"type": "string", "default": "bottom", "enum": ["bottom", "top", "center"]},
                    "safe_zone": {"type": "number", "default": 0.85},
                    "max_words_per_line": {"type": "integer", "default": 5}
                }
            },
            "stickers": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "default": true},
                    "lip_sync": {"type": "string", "default": "amplitude", "enum": ["amplitude", "viseme", "none"]},
                    "blink": {"type": "boolean", "default": true},
                    "idle_bob": {"type": "boolean", "default": true}
                }
            },
            "meme_brolls": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "default": false},
                    "position": {"type": "string", "default": "center-bottom"},
                    "scale": {"type": "number", "default": 0.35},
                    "duration_s": {"type": "number", "default": 2.5},
                    "offset_s": {"type": "number", "default": 0.3}
                }
            },
            "scenes": {
                "type": "array",
                "description": "Ordered list of scenes (script content). Each scene is one speaker's line.",
                "items": {
                    "$ref": "#/definitions/SceneSpec"
                },
                "minItems": 1
            },
            "sfx": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "at_ms": {"type": ["integer", "null"], "description": "Absolute time in ms."},
                        "role": {"type": "string", "enum": ["intro", "transition", "highlight", "outro"]},
                        "trigger": {"type": ["string", "null"], "enum": ["scene_change", "speaker_change", null]}
                    }
                }
            },
            "output": {
                "type": "object",
                "properties": {
                    "format": {"type": "string", "default": "mp4"},
                    "codec": {"type": "string", "default": "h264"},
                    "crf": {"type": "integer", "default": 18, "description": "Video quality. Lower = higher quality. 18-28 typical."},
                    "preset": {"type": "string", "default": "slow", "description": "FFmpeg preset."},
                    "render_engine": {"type": "string", "default": "ffmpeg", "enum": ["ffmpeg", "hyperframes"]},
                    "theme": {"type": "string", "default": "neutral", "enum": ["neutral", "calm", "energetic"], "description": "Theme preset. calm = warm-gold captions for healing content. energetic = neon-green for edu/gaming."}
                }
            }
        },
        "definitions": {
            "SpeakerSpec": {
                "type": "object",
                "required": ["voice"],
                "properties": {
                    "voice": {"type": "string", "description": "Voice ID: 'kokoro:af_heart', 'kokoro:am_michael', bare 'af_heart', a registered clone profile (e.g. 'ishan'), or the literal 'default' to use tts.voice / the user's configured default voice."},
                    "preset": {"type": "string", "default": "default_person", "description": "SVG preset: default_person, robot, cat, etc."},
                    "position": {"type": "string", "default": "top-left", "enum": ["top-left", "top-right", "top-center", "center", "bottom-left", "bottom-right", "bottom-center"]},
                    "scale": {"type": "number", "default": 0.35, "description": "Sticker scale as fraction of canvas width (0.0-1.0)."},
                    "gender": {"type": "string", "default": "auto", "enum": ["male", "female", "nonbinary", "auto"], "description": "Speaker gender. 'auto' infers from the Kokoro voice prefix (af_/am_/bf_/bm_) or free text. Drives format.alternation=male_female; script.format.validate resolves it against the voice profile registry."}
                }
            },
            "BackgroundSpec": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "default": "procedural", "enum": ["gameplay", "procedural", "static"], "description": "procedural = generated motion backgrounds (default). gameplay = YouTube auto-download. static = solid color/gradient."},
                    "source": {"type": "string", "default": "youtube"},
                    "query": {"type": "string", "description": "YouTube search query for gameplay type."},
                    "fallback_pool": {"type": "array", "items": {"type": "string"}, "description": "Local fallback clip paths."},
                    "crop_mode": {"type": "string", "default": "center"},
                    "loop": {"type": "boolean", "default": true},
                    "volume_db": {"type": "number", "default": -20.0},
                    "change_cadence": {"type": "string", "default": "scene", "enum": ["scene", "speaker", "fixed"]}
                }
            },
            "SceneSpec": {
                "type": "object",
                "required": ["speaker", "text"],
                "properties": {
                    "id": {"type": "string", "description": "Unique scene ID. Auto-generated if omitted."},
                    "speaker": {"type": "string", "description": "Speaker ID (must match a key in speakers)."},
                    "text": {"type": "string", "description": "The spoken text for this scene. Engines with inline control tokens (higgs) read `<|emotion|>`/`<|style|>`/`<|sfx|>`/`<|prosody|>` tags from this text — they are AUDIO-ONLY directives and are automatically stripped from captions/timeline previews (never displayed)."},                    "emote": {"type": ["string", "null"], "description": "Emotion/emote for this scene (e.g. 'happy', 'angry', 'whisper', 'thinking'). Selects the speaker's emotion-take (tonality template) at synthesis when the voice profile registered one; also feeds sticker/GIPHY reaction search. For higgs voices, maps to inline emotion/style/sfx control tags. Free-form; falls back to the base voice when no take matches."},
                    "tone": {"type": ["string", "null"], "description": "Natural-language delivery direction for this line (e.g. 'low gravelly whisper, slow deliberate pace'). VoiceDesign receives it verbatim; higgs scans it for delivery keywords (whisper/shout/sing/expressive/flat) and maps them to style/prosody control tags."},
                    "control_tags": {"type": ["string", "null"], "description": "RAW control-tag passthrough for engines with inline control tokens (higgs: emotion/style/sfx/prosody, 43 tags). Prepended verbatim to the line, e.g. \"<|prosody:pause|> mid, <|sfx:laughter|>Haha\". Only the engine's recognized tags are valid."},
                    "speed": {"type": ["number", "null"], "description": "Per-scene speech speed multiplier (overrides tts.default_speed). For higgs, values >=1.08 / <=0.92 emit prosody speed tags (natural pacing); neutral-band values fall back to ffmpeg."},
                    "pitch": {"type": ["number", "null"], "description": "Per-scene pitch multiplier (overrides tts.default_pitch). For higgs, <=0.9 / >=1.1 emit prosody pitch tags."},
                    "background": {"type": ["string", "null"], "description": "Override background for this scene (preset name or null for auto)."},
                    "duration_override_ms": {"type": ["integer", "null"], "description": "Override scene duration in milliseconds. Null = use TTS duration."},
                    "duration_seconds": {"type": ["number", "null"], "description": "Override scene duration in SECONDS. Null = use TTS duration. If both this and duration_override_ms are set, duration_override_ms wins."},
                    "pause_ms": {"type": ["integer", "null"], "description": "Pause in ms after this scene's voiceover (breath beat)."},
                    "stock_query": {"type": ["string", "null"], "description": "Per-scene stock footage search query override. When set, this query is used directly for Pexels search instead of the auto-generated query from scene text + video_keywords. Gives you explicit control over what footage each scene gets."}
                }
            }
        },
        "examples": [{
            "title": "The History of Coffee",
            "video_keywords": ["coffee", "beans", "roasting", "brewing", "cafe"],
            "speakers": {"narrator": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {"speaker": "narrator", "text": "Coffee is one of the most beloved beverages in the world.", "stock_query": "coffee beans roasting closeup", "duration_seconds": 8},
                {"speaker": "narrator", "text": "The story begins in Ethiopia, where a goat herder discovered the energizing effects.", "stock_query": "ethiopian landscape nature", "duration_seconds": 8}
            ],
            "output": {"theme": "neutral"}
        }]
    }))
}

pub(crate) async fn handle_script_parse(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let validate_only = default_bool(&args, "validate_only", false);

    // Determine if input is inline JSON or a file path
    let json_str: String = if script_input.trim_start().starts_with('{') {
        // Inline JSON
        script_input.to_string()
    } else {
        // File path
        let path = sanitize_input_path(script_input)?;
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Script file not found: {}",
                path.display()
            )));
        }
        std::fs::read_to_string(&path)?
    };

    // Parse the script (config defaults for tts applied first)
    let spec = parse_script(&apply_tts_config_defaults(&json_str))
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse script JSON: {}", e)))?;

    // Validate
    let errors = validate_script(&spec);

    if !errors.is_empty() {
        // Return validation errors
        return Ok(json!({
            "status": "invalid",
            "error_count": errors.len(),
            "errors": errors,
            "spec": if validate_only { serde_json::Value::Null } else { json!(spec) },
        }));
    }

    // Valid
    Ok(json!({
        "status": "valid",
        "error_count": 0,
        "spec": if validate_only { serde_json::Value::Null } else { json!(spec) },
        "summary": {
            "title": spec.title,
            "scene_count": spec.scenes.len(),
            "speaker_count": spec.speakers.len(),
            "aspect": spec.meta.aspect,
            "fps": spec.meta.fps,
            "tts_backend": spec.tts.backend,
            "tts_voice": spec.tts.voice,
            "caption_style": spec.captions.style,
            "background_type": spec.background.r#type,
            "stickers_enabled": spec.stickers.enabled,
            "lip_sync_mode": spec.stickers.lip_sync,
        },
    }))
}

/// Validate a script against its DECLARED content format: format-type
/// validity, speaker-count range, male/female alternation (resolving speaker
/// genders from voice-profile registry gender fields + Kokoro prefixes +
/// description free-text), and scene-speaker alternation pattern (dialogic
/// formats warn on 3+ consecutive scenes from one speaker). Returns issues,
/// suggestions, and the alternation diagnostics.
pub(crate) async fn handle_script_format_validate(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use std::collections::HashMap;

    let script_input = extract_str(&args, "script")?;
    let json_str: String = if script_input.trim_start().starts_with('{') {
        script_input.to_string()
    } else {
        let path = sanitize_input_path(script_input)?;
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Script file not found: {}",
                path.display()
            )));
        }
        std::fs::read_to_string(&path)?
    };
    let mut spec = parse_script(&apply_tts_config_defaults(&json_str))
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse script JSON: {}", e)))?;

    // Backfill missing format COUNT constraints from the canonical playbook
    // defaults so hand-written format blocks (e.g. `format: {type: podcast}`)
    // enforce the same min/max speaker+scene contract as scaffolded drafts
    // from director.run / openscript video new. Only keys the agent did NOT
    // write are filled — explicit agent values always win.
    let raw_fmt_keys: std::collections::HashSet<String> = serde_json::from_str::<serde_json::Value>(&json_str)
        .ok()
        .and_then(|v| v.get("format").and_then(|f| f.as_object()).cloned())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let fmt_type = spec.format.r#type.clone();
    if crate::content_formats::is_valid_format(&fmt_type) {
        if let Some(d) = crate::content_formats::playbook(&fmt_type, "").get("defaults") {
            if !raw_fmt_keys.contains("min_speakers") && spec.format.min_speakers == 0 {
                spec.format.min_speakers = d["min_speakers"].as_u64().unwrap_or(0) as u32;
            }
            if !raw_fmt_keys.contains("max_speakers") && spec.format.max_speakers == 0 {
                spec.format.max_speakers = d["max_speakers"].as_u64().unwrap_or(0) as u32;
            }
            if !raw_fmt_keys.contains("min_scenes") && spec.format.min_scenes == 0 {
                spec.format.min_scenes = d["min_scenes"].as_u64().unwrap_or(0) as u32;
            }
            if !raw_fmt_keys.contains("max_scenes") && spec.format.max_scenes == 0 {
                spec.format.max_scenes = d["max_scenes"].as_u64().unwrap_or(0) as u32;
            }
        }
    }

    // Base validation (includes format type / alternation / count checks).
    let errors = validate_script(&spec);
    let mut issues: Vec<String> = errors
        .iter()
        .map(|e| format!("{}: {}", e.field, e.message))
        .collect();
    let mut suggestions: Vec<String> = Vec::new();

    // Resolve each speaker's gender: explicit > voice-profile gender field >
    // Kokoro-prefix inference > profile-description free-text inference.
    let profiles = load_voice_profiles().unwrap_or_else(|_| json!({}));
    let mut gender_by_speaker: HashMap<String, String> = HashMap::new();
    for (id, spk) in &spec.speakers {
        let mut g = spk.gender.clone();
        if let Some(p) = profiles.get(&spk.voice) {
            if let Some(pg) = p.get("gender").and_then(|x| x.as_str()) {
                if !pg.is_empty() && pg != "auto" {
                    g = pg.to_string();
                }
            }
            if g == "auto" || g == "unknown" {
                let desc = p.get("description").and_then(|x| x.as_str()).unwrap_or("");
                let inferred = openscript_core::script::infer_gender(&spk.voice, desc);
                if inferred != "unknown" {
                    g = inferred;
                }
            }
        }
        gender_by_speaker.insert(id.clone(), g);
    }

    // Scene speaker alternation pattern.
    let pattern: Vec<String> = spec.scenes.iter().map(|s| s.speaker.clone()).collect();
    let mut max_run = 1usize;
    let mut run = 1usize;
    for w in pattern.windows(2) {
        if w[0] == w[1] {
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 1;
        }
    }

    let fmt_type = spec.format.r#type.clone();
    let dialogic = matches!(
        fmt_type.as_str(),
        "podcast" | "dialogue" | "comedy_sketch" | "romcom"
    );
    let speaker_count = spec.speakers.len();

    // Dialogic formats need ≥2 speakers and alternating turns.
    if dialogic && speaker_count < 2 {
        suggestions.push(format!(
            "Format '{}' is a dialogue format — add a second speaker (see director.format for the speaker blueprint with gender + voice.design instructs).",
            fmt_type
        ));
    }
    if dialogic && speaker_count >= 2 && max_run >= 3 {
        suggestions.push(format!(
            "{} consecutive scenes from the same speaker (max run {}) — alternate speakers every scene for engagement.",
            max_run, max_run
        ));
    }
    if dialogic && speaker_count >= 2 && max_run >= 2 && spec.scenes.len() >= 6 {
        suggestions.push(
            "No speaker should hold more than 2 consecutive scenes in a dialogue format — swap turns.".into(),
        );
    }

    // Alternation gender check (mirrors validate_script but with resolved
    // genders so voicedesign profiles count too).
    let distinct: Vec<&String> = gender_by_speaker
        .values()
        .filter(|g| **g != "unknown" && **g != "auto")
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let alternation_ok = if spec.format.alternation == "male_female" {
        let ok = distinct.len() >= 2;
        // Drop the PRE-resolution alternation error from validate_script (it
        // ran on unknown/auto genders); this profile-aware resolution is the
        // authoritative check and would otherwise contradict itself.
        issues.retain(|i| !i.starts_with("format.alternation:"));
        if !ok {
            issues.push(format!(
                "format.alternation: male_female requested but only {} distinct speaker gender(s) resolved ({:?}). Design a counterpart voice with voice.design (e.g. a female host) or set speaker gender explicitly.",
                distinct.len(),
                gender_by_speaker
            ));
        }
        ok
    } else {
        true
    };

    // Pacing suggestion: format prescribes a speed the script hasn't adopted.
    // Compare against the format's OWN speed (not 1.0) — a format whose
    // default_speed IS 1.0 and was correctly adopted must not warn.
    if spec.format.default_speed > 0.0
        && (spec.tts.default_speed - spec.format.default_speed).abs() > 1e-3
    {
        suggestions.push(format!(
            "Format '{}' correlates default_speed {} — set tts.default_speed for that pacing.",
            fmt_type, spec.format.default_speed
        ));
    }

    let status = if !issues.is_empty() {
        "fail"
    } else if !suggestions.is_empty() {
        "warning"
    } else {
        "pass"
    };

    Ok(json!({
        "status": status,
        "format": spec.format,
        "issues": issues,
        "suggestions": suggestions,
        "alternation": {
            "strategy": spec.format.alternation,
            "distinct_genders": distinct,
            "gender_by_speaker": gender_by_speaker,
            "scene_speaker_pattern": pattern,
            "max_consecutive_same_speaker": max_run,
            "ok": alternation_ok,
        },
        "next_steps": "Fix the issues, then re-run script.format.validate before script.to_video.",
    }))
}

pub(crate) async fn handle_script_generate_voices(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts/voices");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&apply_tts_config_defaults(&json_str))
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;
    let errors = validate_script(&spec);
    if !errors.is_empty() {
        return Err(ToolError::InvalidArg(format!(
            "Script validation failed: {} errors",
            errors.len()
        )));
    }

    // Create output directory
    std::fs::create_dir_all(&output_dir)?;

    report_progress(0.0, 100.0, "Generating voices...")
        .await
        .ok();

    let total_scenes = spec.scenes.len();
    let mut segments = Vec::new();
    let mut current_ms = 0i64;
    // Collect per-scene warnings (e.g. Parakeet alignment failure) so callers
    // (script.to_timeline → script.to_video) can surface them in their own
    // response. Without this, whisper failures were only visible via
    // tracing::warn! to stderr — the JSON response said "warnings: null"
    // even when 5/5 scenes had fallen back to estimated word timings.
    // (UX audit GAP #1 fix.)
    let mut voice_warnings: Vec<String> = Vec::new();

    for (i, scene) in spec.scenes.iter().enumerate() {
        report_progress(
            (i as f64 / total_scenes as f64) * 100.0,
            100.0,
            &format!("Voice {}/{}: {}", i + 1, total_scenes, scene.speaker),
        )
        .await
        .ok();

        // Get speaker's voice profile
        let speaker = spec
            .speakers
            .get(&scene.speaker)
            .ok_or_else(|| ToolError::NotFound(format!("Speaker not found: {}", scene.speaker)))?;

        // Load voice profile from registry
        let profiles_path = ".openscript/voice_profiles.json";
        let registry = openscript_tts::profiles::VoiceProfileRegistry::new(profiles_path)
            .map_err(|e| ToolError::Tts(e.to_string()))?;

        // Try to find the voice profile by ID or by voice field.
        // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
        // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
        // The literal voice id "default" resolves to the script-level
        // tts.voice, then the user config default voice, then a backend
        // built-in — the config-like engine/voice selection layer.
        let resolved_voice: String = if speaker.voice == "default" {
            let cfg_voice = spec
                .tts
                .voice
                .clone()
                .or_else(crate::config::resolve_tts_default_voice);
            match cfg_voice {
                Some(v) => v,
                None if spec.tts.backend == "audio8"
                    || spec.tts.backend == "voicedesign"
                    || spec.tts.backend == "higgs"
                    || spec.tts.backend == "indextts" => {
                    // Clone engines cannot fall back to a built-in preset — a
                    // speaker voice "default" with no configured voice profile
                    // is a config gap, not a lookup miss. Error clearly instead
                    // of fabricating a "{backend}:default" profile id that would
                    // fail the registry lookup with a misleading message.
                    return Err(ToolError::InvalidArg(format!(
                        "Speaker '{}' uses voice \"default\" but tts.backend '{}' requires a \
                         registered clone profile — set tts.voice, OPENSCRIPT_TTS_VOICE, or \
                         ~/.openscript/config.json tts.default_voice.",
                        scene.speaker, spec.tts.backend
                    )));
                }
                None => "kokoro:af_heart".to_string(),
            }
        } else {
            speaker.voice.clone()
        };
        let voice_lookup = &resolved_voice;
        // Bare IDs resolve as-is first (audio8 clones are stored by bare name);
        // only kokoro presets fall back to the "kokoro:" prefix form.
        let normalized_voice = if !voice_lookup.starts_with("kokoro:")
            && !voice_lookup.starts_with("faster-qwen")
            && !voice_lookup.starts_with("audio8:")
            && !voice_lookup.starts_with("voicedesign:")
            && !voice_lookup.starts_with("higgs:")
            && !voice_lookup.starts_with("indextts:")
        {
            format!("kokoro:{}", voice_lookup)
        } else {
            voice_lookup.clone()
        };
        let profile = registry
            .get(voice_lookup)
            .or_else(|| registry.get(&normalized_voice))
            .or_else(|| {
                // If voice is "kokoro:af_heart", try to find a profile with that model
                registry
                    .list()
                    .iter()
                    .find(|p| p.model == *voice_lookup || p.model == normalized_voice)
                    .cloned()
            }).cloned()
            .ok_or_else(|| {
                ToolError::NotFound(format!(
                    "Voice profile '{}' not found in registry. Try '{}' or add it via voice.profile.add.",
                    voice_lookup, normalized_voice
                ))
            })?;

        // Generate TTS for this scene. Per-scene emotion/tone/speed/pitch are
        // the line-level "performance direction": emote selects the profile's
        // emotion-take (tonality template) when registered; tone is a
        // natural-language refinement; speed/pitch override the script
        // defaults (previously silently dropped for clone engines).
        let wav_path = format!("{}/{}_{}.wav", output_dir, scene.id, scene.speaker);
        if let Some(parent) = Path::new(&wav_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let scene_speed = scene.speed.unwrap_or(spec.tts.default_speed);
        let scene_pitch = scene.pitch.unwrap_or(spec.tts.default_pitch);
        // Expression knobs: scene-level temperature wins, else the script tts
        // default, else None (engine default — expressive 0.7 for clones).
        let scene_temperature = scene
            .temperature
            .or(spec.tts.default_temperature);
        // cfg_scale: only apply the script-level default when the scene does
        // NOT use an emotion take — an emotion take carries its own tuned
        // cfg_scale (set at design time), and a global default would silently
        // clobber it. Explicit scene-level temperature still flows above.
        let scene_cfg_scale = if scene.emote.is_some() {
            None // emotion take's own cfg_scale wins
        } else {
            spec.tts.default_cfg_scale
        };
        let result = tts_generate_routed(
            &speaker.voice,
            &scene.text,
            &wav_path,
            scene_speed,
            scene_pitch,
            1.0, // volume
            "wav",
            scene.emote.as_deref(),
            scene.tone.as_deref(),
            scene.control_tags.as_deref(),
            scene_temperature,
            spec.tts.default_top_k,
            None, // top_p
            scene_cfg_scale,
            &profile,
        )
        .await?;

        // Calculate word timings for this scene, routing the alignment engine
        // by script language: Hinglish/Hindi → Whisper (multilingual, `hi`),
        // English → Parakeet TDT. Parakeet is English-only; on Hinglish its
        // word counts drift and remap_words_to_script collapses to even-spacing
        // estimates (caption-sync gap). Both engines' timings are text-remapped
        // to the script's ground-truth words below.
        //
        // DISPLAY TEXT: TTS engines with inline control tags (higgs
        // `<|emotion|>`/`<|sfx|>`/`<|prosody|>`, etc.) consume the RAW
        // `scene.text` above — but the tags are audio-only directives, not
        // spoken words. The manifest `text` + word list must be the STRIPPED
        // copy, or tags bleed into captions.ass / timeline previews. Tags also
        // inflate the word count, breaking remap_words_to_script's count match.
        let display_text = openscript_core::control_tags::strip_control_tags(&scene.text);
        let scene_end_ms = current_ms + result.duration_ms;
        let lang = spec.language.to_lowercase();
        let hinglish = lang.starts_with("hi") || lang.contains("hinglish");
        let words = if hinglish {
            match run_whisper_alignment(&result.output_path, &display_text, "hi", current_ms, scene_end_ms).await {
                Ok(timed) => remap_words_to_script(&display_text, timed, current_ms, scene_end_ms),
                Err(e) => {
                    let msg = format!(
                        "Scene {}: Whisper alignment failed ({}), falling back to Parakeet.",
                        i + 1,
                        e
                    );
                    tracing::warn!("[script.generate_voices] {}", msg);
                    voice_warnings.push(msg);
                    match run_parakeet_alignment(&result.output_path, current_ms, scene_end_ms).await {
                        Ok(timed) => remap_words_to_script(&display_text, timed, current_ms, scene_end_ms),
                        Err(e2) => {
                            let msg2 = format!(
                                "Scene {}: Parakeet fallback failed ({}), using estimated word timings. Caption sync will be approximate.",
                                i + 1,
                                e2
                            );
                            tracing::warn!("[script.generate_voices] {}", msg2);
                            voice_warnings.push(msg2);
                            estimate_word_timings(&display_text, current_ms, scene_end_ms)
                        }
                    }
                }
            }
        } else {
            match run_parakeet_alignment(&result.output_path, current_ms, scene_end_ms).await {
                Ok(timed) => remap_words_to_script(&display_text, timed, current_ms, scene_end_ms),
                Err(e) => {
                    let msg = format!(
                        "Scene {}: Parakeet force-alignment failed ({}), using estimated word timings. Caption sync will be approximate.",
                        i + 1,
                        e
                    );
                    tracing::warn!("[script.generate_voices] {}", msg);
                    voice_warnings.push(msg);
                    estimate_word_timings(&display_text, current_ms, scene_end_ms)
                }
            }
        };

        segments.push(serde_json::json!({
            "scene_id": scene.id,
            "speaker": scene.speaker,
            "text": display_text,
            "start_ms": current_ms,
            "end_ms": scene_end_ms,
            "duration_ms": result.duration_ms,
            "wav_path": result.output_path,
            "cached": result.cached,
            "backend": result.backend,
            "words": words,
        }));

        current_ms = scene_end_ms;
    }

    // Write manifest
    let manifest_path = format!("{}/manifest.json", output_dir);
    let manifest = json!({
        "segments": segments,
        "total_duration_ms": current_ms,
        "total_scenes": total_scenes,
    });
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    report_progress(100.0, 100.0, "Voices generated").await.ok();

    Ok(json!({
        "status": "generated",
        "manifest_path": manifest_path,
        "total_duration_ms": current_ms,
        "total_scenes": total_scenes,
        "segments": segments,
        "warnings": if voice_warnings.is_empty() { serde_json::Value::Null } else { json!(voice_warnings) },
    }))
}

pub(crate) async fn handle_script_build_captions(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let manifest_path = extract_str(&args, "voiceover_manifest")?;
    let output_path = default_str(&args, "output_path", "artifacts/captions.ass");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&apply_tts_config_defaults(&json_str))
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Build CaptionSegments from manifest
    let mut segments = Vec::new();
    if let Some(segs) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segs {
            // Display boundary: strip TTS control tags (higgs `<|...|>`)
            // from the caption text — they are audio-only directives and must
            // never appear in the rendered captions. Defense-in-depth for
            // manifests written before the generate_voices strip (stale
            // manifests on disk still carry inline tags).
            let text = openscript_core::control_tags::strip_control_tags(
                seg.get("text").and_then(|v| v.as_str()).unwrap_or(""),
            );
            let start_ms = seg.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let end_ms = seg.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);

            // Convert word timings from manifest. Caption TEXT must be the
            // SCRIPT's words — never the ASR transcription of the TTS audio
            // (Parakeet mis-hears cloned voices: "bias" → "pie"). Keep the
            // alignment's real timing windows when the word count matches;
            // otherwise fall back to char-proportional estimation.
            // Stale (pre-fix) manifests also carry `<|...|>` control tags as
            // word entries — strip them so the count matches the stripped text
            // and the real ASR timings survive (instead of collapsing to
            // even-spacing estimates).
            let timed_words: Vec<WordTiming> = seg
                .get("words")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|w| {
                            let word = openscript_core::control_tags::strip_control_tags(
                                w.get("word")?.as_str()?,
                            );
                            if word.is_empty() {
                                return None; // pure-tag token → not a spoken word
                            }
                            Some(WordTiming {
                                word,
                                start_ms: w.get("start_ms")?.as_i64()?,
                                end_ms: w.get("end_ms")?.as_i64()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let words = remap_words_to_script(&text, timed_words, start_ms, end_ms);

            segments.push(CaptionSegment {
                text,
                start_ms,
                end_ms,
                words,
            });
        }
    }

    // Generate ASS
    let ass_content = generate_ass(&segments, &spec.captions, spec.meta.width, spec.meta.height);

    // Write output
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, ass_content)?;

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "caption_style": spec.captions.style,
        "segment_count": segments.len(),
    }))
}

pub(crate) async fn handle_script_to_timeline(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts");
    let skip_background = default_bool(&args, "skip_background", false);
    let skip_stickers = default_bool(&args, "skip_stickers", false);
    let voiceover_manifest_path = default_opt_str(&args, "voiceover_manifest_path");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&apply_tts_config_defaults(&json_str))
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;
    let errors = validate_script(&spec);
    if !errors.is_empty() {
        return Err(ToolError::InvalidArg(format!(
            "Script validation failed: {} errors",
            errors.len()
        )));
    }

    let voices_dir = format!("{}/voices", output_dir);
    let stickers_dir = format!("{}/stickers", output_dir);
    std::fs::create_dir_all(&voices_dir)?;
    std::fs::create_dir_all(&stickers_dir)?;

    let mut warnings = Vec::new();

    // Step 1: Generate voices (or use pre-supplied manifest)
    let (manifest_path, total_duration_ms) = if let Some(ref path) = voiceover_manifest_path {
        // Bring-your-own-audio mode: skip TTS, use the supplied manifest
        if !std::path::Path::new(path).exists() {
            return Err(ToolError::NotFound(format!(
                "voiceover_manifest_path not found: {}",
                path
            )));
        }
        warnings.push(format!(
            "Using pre-supplied voiceover manifest: {} (skipping TTS generation)",
            path
        ));
        // Read total_duration_ms from the manifest
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).unwrap_or_default()
        ).unwrap_or(json!({}));
        let dur = manifest.get("total_duration_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| {
                // Sum segment durations if total not present
                manifest.get("segments")
                    .and_then(|v| v.as_array())
                    .map(|segs| segs.iter()
                        .filter_map(|s| s.get("duration_ms").and_then(|v| v.as_i64()))
                        .sum::<i64>())
                    .unwrap_or(0)
            });
        (path.clone(), dur)
    } else {
        report_progress(0.0, 100.0, "Step 1/5: Generating voices...")
            .await
            .ok();
        let voices_result = handle_script_generate_voices(json!({
            "script": script_input,
            "output_dir": voices_dir,
        }))
        .await?;

        // Collect voice-generation warnings (e.g. Parakeet alignment failure)
        // into our own warnings array so they propagate to script.to_video's
        // final response. Without this, the warnings were returned in
        // voices_result but never read by the caller. (UX audit GAP #1 fix.)
        if let Some(voice_warns) = voices_result.get("warnings").and_then(|v| v.as_array()) {
            for w in voice_warns {
                if let Some(s) = w.as_str() {
                    warnings.push(s.to_string());
                }
            }
        }

        let mp = voices_result
            .get("manifest_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArg("No manifest_path in voices result".into()))?
            .to_string();
        let dur = voices_result
            .get("total_duration_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        (mp, dur)
    };

    // Step 2: Build captions
    report_progress(20.0, 100.0, "Step 2/5: Building captions...")
        .await
        .ok();
    let captions_path = format!("{}/captions.ass", output_dir);
    let _captions_result = handle_script_build_captions(json!({
        "script": script_input,
        "voiceover_manifest": manifest_path,
        "output_path": captions_path,
    }))
    .await?;

    // Step 3: Fetch + assign backgrounds
    report_progress(40.0, 100.0, "Step 3/5: Fetching backgrounds...")
        .await
        .ok();
    let mut background_pool: Vec<String> = spec.background.fallback_pool.clone();

    if !skip_background && spec.background.r#type == "gameplay" && !spec.background.query.is_empty()
    {
        // Fetch a background clip
        let fetch_result = handle_background_fetch(json!({
            "query": spec.background.query,
            "duration_s": total_duration_ms as f64 / 1000.0,
            "aspect": spec.meta.aspect,
            "fallback_pool": spec.background.fallback_pool,
        }))
        .await;

        match fetch_result {
            Ok(r) => {
                if let Some(path) = r.get("clip_path").and_then(|v| v.as_str()) {
                    background_pool.insert(0, path.to_string());
                }
            }
            Err(e) => {
                warnings.push(format!("Background fetch failed: {}", e));
            }
        }
    }

    // Assign backgrounds
    let bg_assignments_path = format!("{}/background_assignments.json", output_dir);
    if !background_pool.is_empty() {
        let _bg_result = handle_background_assign(json!({
            "script": script_input,
            "voiceover_manifest": manifest_path,
            "background_pool": background_pool,
            "output_path": bg_assignments_path,
        }))
        .await?;
    }

    // Step 4: Render stickers (if enabled)
    report_progress(60.0, 100.0, "Step 4/5: Rendering stickers...")
        .await
        .ok();
    let mut sticker_paths: Vec<serde_json::Value> = Vec::new();

    if !skip_stickers && spec.stickers.enabled {
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;

        if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
            for seg in segments {
                let speaker = seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
                let wav_path = seg.get("wav_path").and_then(|v| v.as_str()).unwrap_or("");
                let start_ms = seg.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);

                if wav_path.is_empty() {
                    continue;
                }

                // Get speaker's preset and position
                let speaker_spec = spec.speakers.get(speaker);
                let preset_name = speaker_spec
                    .map(|s| s.preset.clone())
                    .unwrap_or_else(|| "default_person".to_string());
                let position = speaker_spec
                    .map(|s| s.position.clone())
                    .unwrap_or_else(|| "top-left".to_string());
                let scale = speaker_spec.map(|s| s.scale).unwrap_or(0.25);

                let sticker_output = format!("{}/sticker_{}.html", stickers_dir, speaker);

                let sticker_result = handle_sticker_render(json!({
                    "wav_path": wav_path,
                    "preset_name": preset_name,
                    "position": position,
                    "scale": scale,
                    "canvas_width": spec.meta.width,
                    "canvas_height": spec.meta.height,
                    "fps": spec.meta.fps,
                    "output_path": sticker_output,
                    "render_to_video": false,  // HTML only — script.to_video uses GIPHY/PNG stickers for rendering
                }))
                .await;

                match sticker_result {
                    Ok(r) => {
                        sticker_paths.push(json!({
                            "speaker": speaker,
                            "start_ms": start_ms,
                            "html_path": r.get("output_path").and_then(|v| v.as_str()).unwrap_or(""),
                            "video_path": r.get("video_path").and_then(|v| v.as_str()).unwrap_or(""),
                            "frame_count": r.get("frame_count").and_then(|v| v.as_u64()).unwrap_or(0),
                        }));
                    }
                    Err(e) => {
                        warnings.push(format!("Sticker render failed for {}: {}", speaker, e));
                    }
                }
            }
        }
    }

    // Step 5: Assemble timeline using the proper Timeline struct
    report_progress(80.0, 100.0, "Step 5/5: Assembling timeline...")
        .await
        .ok();
    let timeline_path = format!("{}/timeline.json", output_dir);

    // Build a proper Timeline struct — use the first background as the "source" video
    // (for from-scratch videos, the background IS the source)
    let bg_source = background_pool
        .first()
        .cloned()
        .or_else(|| spec.background.fallback_pool.first().cloned())
        .unwrap_or_else(|| "mcp/assets/backgrounds/procedural_01.mp4".to_string());

    // If background pool is empty, use the procedural fallback
    let mut background_pool = background_pool;
    if background_pool.is_empty() {
        background_pool.push(bg_source.clone());
    }

    let mut timeline = Timeline::new(
        std::path::PathBuf::from(&bg_source),
        &spec.meta.aspect,
        spec.meta.fps,
        None,
    );

    // Add segments from the voiceover manifest
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    let mut current_ms = 0i64;
    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            let scene_id = seg.get("scene_id").and_then(|v| v.as_str()).unwrap_or("");
            // Display boundary: strip TTS control tags from the segment caption
            // and voiceover event text (timeline preview must show clean prose).
            let text = openscript_core::control_tags::strip_control_tags(
                seg.get("text").and_then(|v| v.as_str()).unwrap_or(""),
            );
            let dur_ms = seg
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(3000);
            let wav_path = seg.get("wav_path").and_then(|v| v.as_str()).unwrap_or("");

            // Add segment
            let segment = openscript_core::timeline::Segment {
                id: scene_id.to_string(),
                start: current_ms as f64 / 1000.0,
                end: (current_ms + dur_ms) as f64 / 1000.0,
                caption: text.to_string(),
                crossfade_ms: 0,
                semantic_role: None,
            };
            timeline.segments.push(segment);

            // Add voiceover event
            let vo_event = openscript_core::timeline::TimelineEvent {
                id: format!("vo_{}", scene_id),
                asset_id: scene_id.to_string(),
                start_ms: current_ms,
                end_ms: current_ms + dur_ms,
                offset_ms: 0,
                gain_db: -6.0,
                fade_in_ms: 50,
                fade_out_ms: 50,
                tags: vec![],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "script.to_timeline".into(),
                    editorial_role: None,
                    concept: None,
                }),
                kind: openscript_core::timeline::EventKind::Voiceover {
                    voice_profile_id: seg
                        .get("speaker")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    text: text.to_string(),
                    estimated_duration_ms: dur_ms,
                },
            };
            timeline.add_track_event(TrackType::Voiceover, vo_event);

            // Register voiceover asset
            timeline.add_asset("voices", scene_id.to_string(), json!({"path": wav_path}));

            current_ms += dur_ms;
        }
    }

    // Add background as broll
    if let Some(bg_path) = background_pool.first() {
        let broll_event = openscript_core::timeline::TimelineEvent {
            id: "broll_bg".to_string(),
            asset_id: "broll_bg".to_string(),
            start_ms: 0,
            end_ms: total_duration_ms,
            offset_ms: 0,
            gain_db: spec.background.volume_db,
            fade_in_ms: 0,
            fade_out_ms: 0,
            tags: vec![],
            provenance: Some(openscript_core::timeline::Provenance {
                tool: "script.to_timeline".into(),
                editorial_role: None,
                concept: Some("background".to_string()),
            }),
            kind: openscript_core::timeline::EventKind::Broll {
                concept: "background".to_string(),
                source_provider: "youtube".to_string(),
                transition_style: "cut".to_string(),
                crop_mode: spec.background.crop_mode.clone(),
                orientation: spec.meta.aspect.clone(),
                motion_intensity: "medium".to_string(),
            },
        };
        timeline.add_track_event(TrackType::Broll, broll_event);
        timeline.add_asset("broll", "broll_bg".to_string(), json!({"path": bg_path}));
    }

    // Add captions asset
    timeline.add_asset(
        "captions",
        "ass".to_string(),
        json!({"path": captions_path}),
    );

    // Add music if specified
    if let Some(ref music) = spec.music {
        if let Some(ref path) = music.path {
            if !path.is_empty() {
                let music_event = openscript_core::timeline::TimelineEvent {
                    id: "music_bg".to_string(),
                    asset_id: "music_bg".to_string(),
                    start_ms: 0,
                    end_ms: total_duration_ms,
                    offset_ms: 0,
                    gain_db: music.gain_db,
                    fade_in_ms: 500,
                    fade_out_ms: 500,
                    tags: vec![],
                    provenance: Some(openscript_core::timeline::Provenance {
                        tool: "script.to_timeline".into(),
                        editorial_role: None,
                        concept: None,
                    }),
                    kind: openscript_core::timeline::EventKind::Music {
                        mood: "neutral".to_string(),
                        energy: "medium".to_string(),
                        bpm: None,
                        loopability: true,
                        intro_friendly: true,
                        cta_friendly: false,
                        loudness_target_lufs: -14.0,
                        loop_mode: "loop".to_string(),
                        ducking_policy: if music.ducking {
                            "auto".to_string()
                        } else {
                            "none".to_string()
                        },
                    },
                };
                timeline.add_track_event(TrackType::Music, music_event);
                timeline.add_asset("music", "music_bg".to_string(), json!({"path": path}));

                // Add ducking directive
                if music.ducking {
                    timeline.add_ducking_directive(
                        "voiceover",
                        "music",
                        music.ducking_depth_db,
                        50,
                        200,
                    );
                }
            }
        }
    }

    // Save timeline
    timeline.save(&timeline_path)?;

    report_progress(100.0, 100.0, "Timeline assembled")
        .await
        .ok();

    Ok(json!({
        "status": "assembled",
        "timeline_path": timeline_path,
        "voiceover_manifest": manifest_path,
        "captions_path": captions_path,
        "background_assignments": bg_assignments_path,
        "total_duration_ms": total_duration_ms,
        "scene_count": spec.scenes.len(),
        "speaker_count": spec.speakers.len(),
        "background_pool_size": background_pool.len(),
        "sticker_count": sticker_paths.len(),
        "warnings": if warnings.is_empty() { serde_json::Value::Null } else { json!(warnings) },
    }))
}

/// Phase A: auto-design missing speaker voices from the format's speaker
/// blueprint so scaffolded drafts (director.format worked examples) are
/// immediately renderable without manual voice.design. Only voices that are
/// BOTH missing from voice_profiles.json AND match a speaker id in the format
/// playbook blueprint are designed — typo'd ids never produce accidental
/// voices, and existing profiles are never touched.
pub(crate) async fn ensure_speaker_voices(
    spec: &openscript_core::script::ScriptSpec,
) -> Result<Vec<String>, ToolError> {
    let profiles = load_voice_profiles().unwrap_or_else(|_| json!({}));
    let fmt_type = spec.format.r#type.clone();
    if !crate::content_formats::is_valid_format(&fmt_type) {
        return Ok(Vec::new());
    }
    let blueprint = crate::content_formats::playbook(&fmt_type, "")
        .get("speaker_blueprint")
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();
    let mut designed: Vec<String> = Vec::new();
    for (speaker_id, spk) in &spec.speakers {
        if profiles.get(&spk.voice).is_some() {
            continue;
        }
        let Some(entry) = blueprint
            .iter()
            .find(|b| b.get("id").and_then(|i| i.as_str()) == Some(speaker_id.as_str()))
        else {
            continue;
        };
        let Some(instruct) = entry.get("voice_design_instruct").and_then(|i| i.as_str()) else {
            continue;
        };
        let sample = spec
            .scenes
            .iter()
            .find(|s| s.speaker == *speaker_id && !s.text.trim().is_empty())
            .map(|s| s.text.clone())
            .unwrap_or_else(|| {
                format!(
                    "Hello, this is the {} voice.",
                    entry.get("role").and_then(|r| r.as_str()).unwrap_or("speaker")
                )
            });
        // Infer the design language from the sample text (Devanagari → hi),
        // defaulting to en — never hardcode an English persona for hinglish.
        let language = if sample.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) {
            "hi"
        } else {
            "en"
        };
        tracing::info!(
            "Auto-designing missing voice profile '{}' (speaker '{}') from format blueprint (language {})",
            spk.voice,
            speaker_id,
            language
        );
        route_tool(
            "voice.design",
            json!({
                "instruct": instruct,
                "text": sample,
                "profile_id": spk.voice.clone(),
                "language": language,
            }),
        )
        .await
        .map_err(|e| {
            ToolError::Asset(format!(
                "Voice profile '{}' (speaker '{}') is not registered and auto-design from the format blueprint failed: {}. Register the voice with voice.design / voice.profile.add, or pass auto_design_voices=false to script.to_video for a hard fail.",
                spk.voice, speaker_id, e
            ))
        })?;
        designed.push(spk.voice.clone());
    }
    Ok(designed)
}

pub(crate) async fn handle_script_to_video(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let mut output_path = default_str(&args, "output_path", "output.mp4");
    // P0 FIX: Resolve output_path to absolute so ffmpeg writes to a predictable location.
    // Without this, relative paths like "output.mp4" resolve against the MCP server's CWD,
    // which may differ from the agent's expected working directory.
    if !std::path::Path::new(&output_path).is_absolute() {
        match std::env::current_dir() {
            Ok(cwd) => { output_path = cwd.join(&output_path).to_string_lossy().to_string(); }
            Err(e) => {
                return Err(ToolError::InvalidArg(format!(
                    "Cannot resolve output_path '{}' to absolute: {}", output_path, e
                )));
            }
        }
    }
    let output_dir = default_str(&args, "output_dir", "artifacts");
    let skip_background = default_bool(&args, "skip_background", false);
    let skip_stickers = default_bool(&args, "skip_stickers", false);
    let preview_mode = default_bool(&args, "preview_mode", false);
    // Opt-out for auto-designing missing speaker voices: an agent iterating on
    // a script with a genuinely missing voice may want the hard fail instead.
    let auto_design_voices = default_bool(&args, "auto_design_voices", true);
    let voiceover_manifest_path = default_opt_str(&args, "voiceover_manifest_path");

    // Parse script for render config
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&apply_tts_config_defaults(&json_str))
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Phase A: auto-design missing speaker voices from the format blueprint so
    // scaffolded drafts render out of the box (no manual voice.design needed).
    // Newly designed profiles persist in voice_profiles.json (reusable).
    let designed_voices = if auto_design_voices {
        ensure_speaker_voices(&spec).await?
    } else {
        Vec::new()
    };

    report_progress(0.0, 100.0, "Phase 1/3: Building timeline...")
        .await
        .ok();

    // Step 1: Build the timeline
    // ponytail: skip_background=true for timeline handler — this function does
    // its own per-scene multi-broll fetch below. The timeline handler only
    // fetched ONE clip for the whole video (inconsistent with multi-scene).
    let mut timeline_args = json!({
        "script": script_input,
        "output_dir": output_dir,
        "skip_background": true,
        "skip_stickers": skip_stickers,
    });
    if let Some(ref path) = voiceover_manifest_path {
        timeline_args["voiceover_manifest_path"] = json!(path);
    }
    let timeline_result = handle_script_to_timeline(timeline_args).await?;

    let timeline_path = timeline_result
        .get("timeline_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArg("No timeline_path in result".into()))?
        .to_string();
    let warnings = timeline_result
        .get("warnings")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    // Collect additional warnings from the render phase (procedural fallbacks, etc.)
    let mut render_warnings: Vec<String> = Vec::new();

    report_progress(40.0, 100.0, "Phase 2/3: Building layered composition...")
        .await
        .ok();

    // Load manifest
    let manifest_path = timeline_result
        .get("voiceover_manifest")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let captions_path = timeline_result
        .get("captions_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let total_duration_ms = timeline_result
        .get("total_duration_ms")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let total_duration_s = total_duration_ms as f64 / 1000.0;

    let manifest: serde_json::Value = if !manifest_path.is_empty() {
        serde_json::from_str(&std::fs::read_to_string(manifest_path)?)?
    } else {
        json!({"segments": []})
    };

    // Extract voiceover paths and per-scene durations
    let mut voiceover_paths: Vec<String> = Vec::new();
    let mut scene_durations: Vec<f64> = Vec::new();
    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            if let Some(path) = seg.get("wav_path").and_then(|v| v.as_str()) {
                voiceover_paths.push(path.to_string());
            }
            let dur_ms = seg
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(3000);
            scene_durations.push(dur_ms as f64 / 1000.0);
        }
    }

    // Phase 5: Add pause_ms from SceneSpec to scene durations (breath beats).
    for (i, dur) in scene_durations.iter_mut().enumerate() {
        if let Some(scene) = spec.scenes.get(i) {
            if let Some(pause) = scene.pause_ms {
                if pause > 0 {
                    *dur += pause as f64 / 1000.0;
                }
            }
        }
    }

    // === MULTI-BROLL: Download a DIFFERENT stock clip per scene ===
    // Instead of looping one short clip, download a unique stock video
    // for each scene based on keywords extracted from the scene text.
    let mut per_scene_backgrounds: Vec<String> = Vec::new();
    // (video_id, content_hash, search_query) per scene for variance KPIs
    // Per-scene stock provenance for KPI (id, hash, query, lex, title)
    // id, hash, q, lex, title, vision_score, vision_reason
    let mut scene_stock_meta: Vec<Option<(String, String, String, f64, String, f64, Option<String>)>> =
        Vec::new();

    // The final stock query per scene — the sticker stage reuses these SAME
    // keywords so b-roll and stickers are driven by one keyword source
    // (sticker/broll pipeline unification).
    let mut scene_stock_queries: Vec<String> = Vec::new();
    // Set when ANY scene fell to the procedural last resort (drives the
    // final delivery_status downgrade below).
    let mut fell_to_procedural_any = false;

    // Multi-broll stock footage: unique clip per scene.
    // Priority: Pexels (if key) → YouTube via yt-dlp (no key) → procedural (last resort).
    // type:"static" is the explicit opt-out. type:"procedural" still TRIES stock first
    // so agents do not silently ship gradient-only videos when stock is reachable.
    // (Phase CF: production quality upgrade — never treat procedural as success.)
    // Reactions from the unified keyword draft — populated inside the
    // multi-broll block below, consumed by the per-scene sticker stage
    // (declared here at function scope so both sibling blocks share it).
    let mut llm_scene_reactions: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    if !skip_background && spec.background.r#type != "static" {
        report_progress(
            35.0,
            60.0,
            "Fetching multi-broll stock backgrounds (Pexels → YouTube → procedural)...",
        )
        .await
        .ok();


        // YouTube tier is OPT-IN for generation (user decision). Default false:
        // the chain stops at Pexels → Pixabay → fallback_pool. YouTube stays
        // always-on for asset-development workflows (asset.probe / broll.probe).
        let yt_enabled = spec.background.enable_youtube
            || std::env::var("OPENSCRIPT_YT_FOR_GENERATION")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

        // Per-render cache scoping: each render owns its clips under
        // {output_dir}/broll_cache. The old global mcp/assets/background_cache
        // with index-based scene_{:03} names meant parallel or sequential
        // renders clobbered each other's files, and timelines only stored the
        // shared paths — so a later render silently REPLACED earlier videos'
        // b-roll references (the "same crowd clip 4-7 times" / wrong-clip bugs).
        let cache_dir = format!("{}/broll_cache", output_dir);
        std::fs::create_dir_all(&cache_dir).ok();

        // === Agentic keyword unification (keywords module) ===
        // script.to_video drafts per-scene stock keywords through the SAME
        // unified keywords module as broll.keywords / broll.auto (A2V) and
        // sticker.keywords. One batched LLM call emits visual + reaction
        // keywords per scene; the topic-aware salience heuristic is the
        // LLM-down fallback. `effective_video_keywords` implements the
        // documented auto-extraction from the title when the script omitted
        // them (previously a silent Lifestyle collapse).
        let effective_video_keywords: Vec<String> = if spec.video_keywords.is_empty() {
            let derived = crate::keywords::auto_extract_video_keywords(&spec.title);
            if !derived.is_empty() {
                tracing::info!(
                    "[script.to_video] video_keywords omitted — auto-extracted from title: {:?}",
                    derived
                );
                derived
            } else {
                spec.video_keywords.clone()
            }
        } else {
            spec.video_keywords.clone()
        };

        let mut llm_scene_keywords: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        {
            let mut segs: Vec<crate::keywords::SegmentInput> = manifest
                .get("segments")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(i, s)| crate::keywords::SegmentInput {
                    segment_id: s
                        .get("scene_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    caption: s
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    language_hint: Some(spec.language.clone()),
                    duration_s: 0.0,
                    scene_idx: i,
                    total_scenes: manifest
                        .get("segments")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0),
                    video_title: spec.title.clone(),
                    video_keywords: effective_video_keywords.clone(),
                    covered_concepts: Vec::new(),
                })
                .collect();
            segs.retain(|s| !s.segment_id.is_empty());
            if !segs.is_empty() {
                let drafted = crate::keywords::draft_scene_keywords(&segs).await;
                for d in drafted {
                    if !d.segment_id.is_empty() && !d.visual.is_empty() {
                        llm_scene_keywords.insert(d.segment_id.clone(), d.visual);
                    }
                    // Reactions feed the sticker stage (G8): stickers search
                    // reaction/emotion/intent terms, never visual b-roll nouns.
                    if !d.segment_id.is_empty() && !d.reactions.is_empty() {
                        llm_scene_reactions.insert(d.segment_id, d.reactions);
                    }
                }
            }
        }

        // Track Pexels video IDs that have already been used to prevent
        // the same clip appearing in multiple scenes.
        // (Round-16: "There are repeating video-cuts between the GIFs.
        // Ensure that the entire video timeline has unique videos, not
        // repeated video that might reduce the attention-hooking.")
        let mut used_pexels_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut used_yt_queries: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Content-level uniqueness (Phase CI): ytsearch1 often returns the SAME
        // viral video for similar queries — track video IDs + file fingerprints.
        let mut used_video_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut used_content_hashes: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (scene_idx, &dur) in scene_durations.iter().enumerate() {
            // Extract keywords from scene text for the search query
            let scene_text = manifest
                .get("segments")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(scene_idx))
                .and_then(|s| s.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Phase CM signal/noise query: strip listicle noise, bias to visual
            // nouns + video_keywords, attach context-matched visual anchor.
            // Per-scene stock_query override: if the agent specified a
            // stock_query in the scene, use it directly instead of auto-generating.
            // (UX audit GAP #1 fix: agents now have explicit control over
            // per-scene footage search queries.)
            let stock_q = if let Some(ref custom_q) = spec.scenes.get(scene_idx).and_then(|s| s.stock_query.as_ref()).filter(|q| !q.trim().is_empty()) {
                crate::stock_signal::SceneStockQuery {
                    query: custom_q.to_string(),
                    signal_tokens: crate::stock_signal::tokenize(custom_q),
                    visual_anchor: custom_q.to_string(),
                    scene_idx,
                }
            } else {
                // Agentic-first: prefer the LLM-drafted visual keywords (the
                // same broll.keywords pipeline the A2V path uses) when the
                // draft produced at least 2 searchable terms. The topic-aware
                // heuristic is the fallback for LLM-down or abstract scenes.
                let llm_q: Vec<String> = spec
                    .scenes
                    .get(scene_idx)
                    .and_then(|s| llm_scene_keywords.get(&s.id))
                    .cloned()
                    .unwrap_or_default();
                if llm_q.len() >= 2 {
                    let joined = llm_q.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
                    crate::stock_signal::SceneStockQuery {
                        query: joined.clone(),
                        signal_tokens: llm_q,
                        visual_anchor: joined,
                        scene_idx,
                    }
                } else {
                    // De-biased fallback: content-derived salience keywords +
                    // topic keywords (no Lifestyle collapse, no position rotation).
                    let (q, sig) = crate::keywords::heuristic_scene_query(
                        scene_text,
                        &effective_video_keywords,
                        &spec.output.theme,
                        &spec.meta.aspect,
                        scene_idx,
                    );
                    crate::stock_signal::SceneStockQuery {
                        query: q.clone(),
                        signal_tokens: sig,
                        visual_anchor: q,
                        scene_idx,
                    }
                }
            };
            // Keep unsafe-keyword rewrite for edge terms (blood → calm nature)
            let query = crate::keywords::sanitize_query(&stock_q.query, &spec.output.theme);
            scene_stock_queries.push(query.clone());
            let signal_tokens = stock_q.signal_tokens.clone();
            tracing::info!(
                "[script.to_video] stock query scene {}: signal={:?} anchor='{}' → query='{}'",
                scene_idx + 1,
                signal_tokens.iter().take(6).collect::<Vec<_>>(),
                stock_q.visual_anchor,
                query
            );

            let progress_pct = 35.0 + (scene_idx as f64 / scene_durations.len() as f64) * 25.0;
            report_progress(
                progress_pct,
                100.0,
                &format!(
                    "Scene {}/{}: {}",
                    scene_idx + 1,
                    scene_durations.len(),
                    query
                ),
            )
            .await
            .ok();

            // --- Unified acquisition (scene_media): user_library → Pexels →
            //     Pixabay → YouTube (opt-in) → fallback_pool → procedural.
            //     Every tier is attempted; `outcome.exhausted` records why each
            //     tier failed so "why procedural" is answerable per scene. ---
            let mut yt_q = query.clone();
            if used_yt_queries.contains(&yt_q) {
                yt_q = format!("{} scene{}", query, scene_idx + 1);
            }
            used_yt_queries.insert(yt_q.clone());
            let outcome = crate::scene_media::fetch_scene_background(
                crate::scene_media::SceneMediaRequest {
                    query: yt_q,
                    signal_tokens: signal_tokens.clone(),
                    scene_text: scene_text.to_string(),
                    duration_s: dur,
                    min_duration_s: dur,
                    max_duration_s: 0.0,
                    aspect: spec.meta.aspect.clone(),
                    cache_dir: cache_dir.to_string(),
                    out_stem: format!("scene_{:03}", scene_idx + 1),
                    scene_idx,
                    enable_youtube: yt_enabled,
                    fallback_pool: spec.background.fallback_pool.clone(),
                    used_video_ids: &mut used_video_ids,
                    used_content_hashes: &mut used_content_hashes,
                    used_pexels_ids: &mut used_pexels_ids,
                },
            )
            .await?;
            if outcome.fell_to_procedural {
                fell_to_procedural_any = true;
            }
            tracing::info!(
                "[script.to_video] Scene {} source={} exhausted={:?}",
                scene_idx + 1,
                outcome.source,
                outcome.exhausted
            );
            if outcome.fell_to_procedural {
                let allow_proc = std::env::var("OPENSCRIPT_ALLOW_PROCEDURAL")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let warn = if !allow_proc {
                    format!(
                        "⚠️ PRODUCTION_FAIL stock_visuals scene {}: no relevant unique stock after exhausting {:?}. Using procedural {}. Set PEXELS_API_KEY/PIXABAY_API_KEY or OPENSCRIPT_ALLOW_PROCEDURAL=1.",
                        scene_idx + 1,
                        outcome.exhausted,
                        outcome.clip_path
                    )
                } else {
                    format!(
                        "PRODUCTION_FAIL stock_visuals scene {}: synthetic procedural ({})",
                        scene_idx + 1,
                        outcome.clip_path
                    )
                };
                render_warnings.push(warn);
            }
            // Manifest provenance (render manifest source_hint) keys off the
            // `pexels_<id>` prefix — preserve it for the pexels tier.
            let meta_id = match outcome.source.as_str() {
                "pexels" => format!("pexels_{}", outcome.provider_id.clone().unwrap_or_default()),
                _ => outcome.provider_id.clone().unwrap_or_default(),
            };
            scene_stock_meta.push(Some((
                meta_id,
                outcome.content_hash.clone(),
                outcome.search_query.clone(),
                outcome.lexical_score,
                outcome.source_title.clone(),
                outcome.vision_score.unwrap_or(outcome.lexical_score),
                outcome.vision_reason.clone(),
            )));
            per_scene_backgrounds.push(outcome.clip_path);
        }
        let proc_n = per_scene_backgrounds
            .iter()
            .filter(|p| is_procedural_media_path(p))
            .count();
        if !per_scene_backgrounds.is_empty()
            && proc_n * 2 >= per_scene_backgrounds.len()
        {
            render_warnings.push(format!(
                "HARD: majority procedural multi-broll ({}/{}) — visual hooks missing. Configure Pexels or widen YT stock queries.",
                proc_n,
                per_scene_backgrounds.len()
            ));
            let allow_proc = std::env::var("OPENSCRIPT_ALLOW_PROCEDURAL")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !allow_proc {
                // Fail-closed: never present gradient-majority as a production final.
                let draft = if output_path.ends_with(".draft.mp4") {
                    output_path.clone()
                } else if let Some(stripped) = output_path.strip_suffix(".mp4") {
                    format!("{}.draft.mp4", stripped)
                } else {
                    format!("{}.draft.mp4", output_path)
                };
                tracing::warn!(
                    "[script.to_video] fail-closed stock: rewriting output {} → {} (set OPENSCRIPT_ALLOW_PROCEDURAL=1 to override)",
                    output_path,
                    draft
                );
                render_warnings.push(format!(
                    "FAIL_CLOSED: stock_ratio < 0.5 ({}/{} procedural). Writing draft output {} — not a production final. Set PEXELS_API_KEY or OPENSCRIPT_ALLOW_PROCEDURAL=1.",
                    proc_n,
                    per_scene_backgrounds.len(),
                    draft
                ));
                output_path = draft;
            }
        }
    }

    // Build per-scene background clips
    let fallback_pool = if !per_scene_backgrounds.is_empty() {
        per_scene_backgrounds
    } else if !spec.background.fallback_pool.is_empty() {
        spec.background.fallback_pool.clone()
    } else {
        let mut pool = Vec::new();
        if let Ok(entries) = std::fs::read_dir("mcp/assets/backgrounds") {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".mp4") {
                    pool.push(format!("mcp/assets/backgrounds/{}", name));
                }
            }
        }
        if pool.is_empty() {
            pool.push("mcp/assets/backgrounds/procedural_01.mp4".to_string());
        }
        pool
    };

    // Assign backgrounds — one per scene (multi-broll)
    let mut backgrounds: Vec<openscript_ffmpeg::multilayer_render::BackgroundClip> = Vec::new();

    for (i, &dur) in scene_durations.iter().enumerate() {
        // Use the per-scene downloaded background if available, otherwise cycle through pool
        let bg_path = if i < fallback_pool.len() {
            fallback_pool[i].clone()
        } else {
            fallback_pool[i % fallback_pool.len()].clone()
        };

        backgrounds.push(openscript_ffmpeg::multilayer_render::BackgroundClip {
            path: bg_path,
            duration_s: dur,
            // Loop per-scene trims: Pexels source clips are often SHORTER than
            // the scene (e.g. a 6s clip for a 12s scene). Without -stream_loop
            // the concat runs out early and the render holds the last frame
            // for the remaining seconds (frozen tail). select(lte(n,N)) keeps
            // exactly the scene frame count from the looped stream, so an
            // exact-size trim is unaffected.
            looped: true,
        });
    }

    // Build sticker overlays — download GIPHY stickers per speaker
    let mut stickers: Vec<openscript_ffmpeg::multilayer_render::StickerOverlay> = Vec::new();
    if !skip_stickers && spec.stickers.enabled {
        // Fix: prior versions called env::var("GIPHY_API_KEY") twice in
        // unwrap_or_else (the inner call shadowed the outer). Simplify to a
        // single lookup.
        let giphy_key_val = giphy_key();

        // Download stickers: one per speaker by default, but per-scene when
        // a single speaker has multiple scenes (Phase 4: per-scene variation).
        let mut speaker_stickers: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        // Phase 4: when single speaker has 3+ scenes, store per-scene stickers.
        // Key = scene index, Value = sticker path. Falls back to speaker_stickers.
        let mut scene_sticker_map: std::collections::HashMap<usize, String> =
            std::collections::HashMap::new();
        let single_speaker_multi_scene =
            spec.speakers.len() == 1 && spec.scenes.len() >= 3;
        // Track queries used across speakers/scenes so we don't re-search the
        // same term (query-level dedup).
        let mut used_sticker_queries: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Track GIPHY sticker IDs already downloaded — the definitive
        // no-duplicate-sticker guarantee (two different queries can return the
        // same top sticker).
        let mut used_sticker_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        if !giphy_key_val.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

            let stickers_dir = "mcp/assets/stickers";
            std::fs::create_dir_all(stickers_dir).ok();

            for (speaker_name, speaker_spec) in &spec.speakers {
                // Build a mood-aware, scene-text-aware GIPHY search query.
                // Round-5 audit: the old hardcoded "{speaker_name} talking"
                // produced irrelevant stickers (speaker names are abstract IDs
                // like "alice" or "narrator", not GIPHY-indexed content).
                // New priority: theme keyword > scene emote > scene-text noun >
                // speaker preset > trending fallback.
                // Build a topic-aware GIPHY sticker search query.
                // (Round-13: topic-aware video search upgrade.)
                // If video_keywords are available, use the first one as
                // the sticker query (e.g. "brain" for a brain video) so
                // the sticker is topically relevant, not just theme-based.
                let search_query = if !spec.video_keywords.is_empty() {
                    let topic_kw = &spec.video_keywords[0];
                    if !used_sticker_queries.contains(topic_kw) {
                        used_sticker_queries.insert(topic_kw.clone());
                        topic_kw.clone()
                    } else {
                        build_sticker_query(
                            speaker_name,
                            speaker_spec,
                            &spec.scenes,
                            &spec.output.theme,
                            &mut used_sticker_queries,
                        )
                    }
                } else {
                    build_sticker_query(
                        speaker_name,
                        speaker_spec,
                        &spec.scenes,
                        &spec.output.theme,
                        &mut used_sticker_queries,
                    )
                };
                tracing::info!(
                    "[script.to_video] GIPHY sticker search for '{}': query='{}'",
                    speaker_name,
                    search_query
                );
                // Use limit=8 so we can filter for relevance and skip duds
                let giphy_url = format!(
                    "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit=8&rating=g&bundle=sticker_layering&lang=en",
                    giphy_key_val,
                    urlencoding::encode(&search_query)
                );

                if let Ok(resp) = client.get(&giphy_url).send().await {
                    if resp.status().is_success() {
                        if let Ok(body) = resp.json::<serde_json::Value>().await {
                            if let Some(data) = body.get("data").and_then(|v| v.as_array()) {
                                // Iterate through results (limit=8) and pick the
                                // first valid sticker. Skip non-sticker results,
                                // oversized files, and results already used by
                                // another speaker.
                                for sticker in data {
                                    // Defensive: verify this is actually a sticker
                                    let is_sticker = sticker
                                        .get("is_sticker")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(1);
                                    if is_sticker != 1 {
                                        continue;
                                    }

                                    let sticker_id = sticker
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if used_sticker_ids.contains(&sticker_id) {
                                        continue;
                                    }

                                    let images =
                                        sticker.get("images").cloned().unwrap_or(json!({}));
                                    let original =
                                        images.get("original").cloned().unwrap_or(json!({}));

                                    // Use GIF format (not WEBP) because FFmpeg's
                                    // native WEBP decoder cannot handle animated
                                    // WEBP stickers from GIPHY. GIF animation is
                                    // well-supported by FFmpeg's GIF decoder.
                                    // (Round-5 audit: animated WEBP caused
                                    // "Terminating thread with return code
                                    // -1145393733" in FFmpeg.)
                                    let sticker_url = original
                                        .get("url")
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or("");

                                    if sticker_url.is_empty() {
                                        continue;
                                    }

                                    // Skip static (non-animated) GIFs — check frame count.
                                    // (Round-11: "Some GIFs were static images" — user wants
                                    // animated stickers only.)
                                    let frames = original
                                        .get("frames")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| s.parse::<u32>().ok())
                                        .unwrap_or(0);
                                    if frames < 2 {
                                        tracing::info!(
                                            "[script.to_video] Skipping static GIPHY sticker (frames={}): {}",
                                            frames, sticker_id
                                        );
                                        continue;
                                    }

                                    // Skip oversized stickers (> 3MB)
                                    let size: i64 = original
                                        .get("webp_size")
                                        .and_then(|v| v.as_i64())
                                        .or_else(|| {
                                            original.get("size").and_then(|v| v.as_i64())
                                        })
                                        .unwrap_or(0);
                                    if size > 3_000_000 {
                                        continue;
                                    }

                                    // Always GIF (FFmpeg can't decode animated WEBP)
                                    let ext = "gif";
                                    let sticker_path = format!(
                                        "{}/giphy_{}.{}",
                                        stickers_dir, speaker_name, ext
                                    );
                                    if let Ok(dl_resp) = client.get(sticker_url).send().await {
                                        if dl_resp.status().is_success() {
                                            if let Ok(bytes) = dl_resp.bytes().await {
                                                std::fs::write(&sticker_path, &bytes).ok();
                                                speaker_stickers.insert(
                                                    speaker_name.clone(),
                                                    sticker_path.clone(),
                                                );
                                                used_sticker_ids.insert(sticker_id);
                                                tracing::info!(
                                                    "[script.to_video] Downloaded GIPHY sticker for {}: {} ({} bytes)",
                                                    speaker_name,
                                                    sticker_path,
                                                    bytes.len()
                                                );
                                                break; // Got a sticker for this speaker
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Phase 4: per-scene sticker variation for single-speaker videos.
        // When one speaker has 3+ scenes, download a DIFFERENT sticker per
        // scene using scene-specific queries (emote, salient noun from text)
        // so the overlay changes visually between scenes instead of repeating.
        if single_speaker_multi_scene && !giphy_key_val.is_empty() {
            if let Some((speaker_name, _speaker_spec)) = spec.speakers.iter().next() {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()
                    .unwrap_or_default();
                let stickers_dir = "mcp/assets/stickers";

                for (scene_idx, scene) in spec.scenes.iter().enumerate() {
                    // Candidate queries, tried IN ORDER until a fresh,
                    // non-duplicate sticker downloads: the scene's b-roll stock
                    // query FIRST (unified keyword source for broll + sticker),
                    // then sticker-friendly fallbacks (emote, salient noun,
                    // text snippet, "talking head"). If the topic query only
                    // surfaces already-used or static stickers, the next
                    // candidate is tried — variation never silently collapses
                    // to one repeated sticker.
                    let mut sticker_candidates: Vec<String> = Vec::new();
                    // Reactions from the unified keyword draft FIRST (G8 fix):
                    // stickers search reaction/emotion/intent terms — never the
                    // visual b-roll nouns that drive the footage search.
                    if let Some(reacts) = spec
                        .scenes
                        .get(scene_idx)
                        .and_then(|s| llm_scene_reactions.get(&s.id))
                    {
                        for r in reacts.iter() {
                            if !r.trim().is_empty() {
                                sticker_candidates.push(r.clone());
                            }
                        }
                    }
                    if let Some(q) = scene_stock_queries.get(scene_idx) {
                        if !q.trim().is_empty() {
                            sticker_candidates.push(q.clone());
                        }
                    }
                    if let Some(ref emote) = scene.emote {
                        if !emote.is_empty() {
                            sticker_candidates.push(emote.clone());
                        }
                    }
                    if let Some(noun) = extract_salient_noun(&scene.text) {
                        sticker_candidates.push(noun);
                    }
                    // Use first 3 words of scene text as fallback
                    let text_snippet: String = scene
                        .text
                        .split_whitespace()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !text_snippet.is_empty() {
                        sticker_candidates.push(text_snippet);
                    }
                    sticker_candidates.push("talking head".to_string());
                    let mut seen_q = std::collections::HashSet::new();
                    sticker_candidates.retain(|c| seen_q.insert(c.clone()));

                    let mut scene_placed = false;
                    for query in &sticker_candidates {
                        if scene_placed {
                            break;
                        }
                        if used_sticker_queries.contains(query) {
                            continue;
                        }
                        used_sticker_queries.insert(query.clone());
                        tracing::info!(
                            "[script.to_video] Per-scene sticker query for scene {}: '{}'",
                            scene_idx,
                            query
                        );

                        let giphy_url = format!(
                            "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit=8&rating=g&bundle=sticker_layering&lang=en",
                            giphy_key_val,
                            urlencoding::encode(query)
                        );

                        if let Ok(resp) = client.get(&giphy_url).send().await {
                            if resp.status().is_success() {
                                if let Ok(body) = resp.json::<serde_json::Value>().await {
                                    if let Some(data) = body.get("data").and_then(|v| v.as_array()) {
                                        for sticker in data {
                                            let is_sticker = sticker
                                                .get("is_sticker")
                                                .and_then(|v| v.as_i64())
                                                .unwrap_or(1);
                                            if is_sticker != 1 { continue; }

                                            let sticker_id = sticker
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            if used_sticker_ids.contains(&sticker_id) { continue; }

                                            let images = sticker.get("images").cloned().unwrap_or(json!({}));
                                            let original = images.get("original").cloned().unwrap_or(json!({}));

                                            let sticker_url = original
                                                .get("url")
                                                .and_then(|v| v.as_str())
                                                .filter(|s| !s.is_empty())
                                                .unwrap_or("");
                                            if sticker_url.is_empty() { continue; }

                                            let frames = original
                                                .get("frames")
                                                .and_then(|v| v.as_str())
                                                .and_then(|s| s.parse::<u32>().ok())
                                                .unwrap_or(0);
                                            if frames < 2 { continue; }

                                            let size: i64 = original
                                                .get("webp_size")
                                                .and_then(|v| v.as_i64())
                                                .or_else(|| original.get("size").and_then(|v| v.as_i64()))
                                                .unwrap_or(0);
                                            if size > 3_000_000 { continue; }

                                            let sticker_path = format!(
                                                "{}/giphy_s{}_{}.gif",
                                                stickers_dir, scene_idx, speaker_name
                                            );
                                            if let Ok(dl_resp) = client.get(sticker_url).send().await {
                                                if dl_resp.status().is_success() {
                                                    if let Ok(bytes) = dl_resp.bytes().await {
                                                        std::fs::write(&sticker_path, &bytes).ok();
                                                        scene_sticker_map.insert(scene_idx, sticker_path.clone());
                                                        used_sticker_ids.insert(sticker_id);
                                                        tracing::info!(
                                                            "[script.to_video] Per-scene sticker for scene {}: {}",
                                                            scene_idx, sticker_path
                                                        );
                                                        scene_placed = true;
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !scene_placed {
                        tracing::warn!(
                            "[script.to_video] No fresh sticker for scene {} after {} query candidate(s) — will use the speaker sticker",
                            scene_idx,
                            sticker_candidates.len()
                        );
                    }
                }
            }
        }

        // Local sticker fallback when GIPHY missing/failed (Phase CF).
        // Prefer animated GIFs (giphy_*.gif), then speaker PNGs, then any .gif/.png.
        let local_sticker_pool: Vec<String> = {
            let mut pool = Vec::new();
            if let Ok(entries) = std::fs::read_dir("mcp/assets/stickers") {
                for entry in entries.filter_map(|e| e.ok()) {
                    let p = entry.path();
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if matches!(ext, "gif" | "png" | "webp") && !name.starts_with('.') {
                        pool.push(p.to_string_lossy().to_string());
                    }
                }
            }
            // Prefer GIFs first for motion
            pool.sort_by(|a, b| {
                let ag = a.ends_with(".gif") as i32;
                let bg = b.ends_with(".gif") as i32;
                bg.cmp(&ag)
            });
            pool
        };
        let mut local_idx = 0usize;
        for (speaker_name, speaker_spec) in &spec.speakers {
            if speaker_stickers.contains_key(speaker_name) {
                continue;
            }
            // Named PNG first
            let position_parts: Vec<&str> = speaker_spec.position.split('-').collect();
            let facing = position_parts.last().unwrap_or(&"left");
            let png_path = format!(
                "mcp/assets/stickers/speaker_{}_{}.png",
                speaker_name, facing
            );
            if std::path::Path::new(&png_path).exists() {
                speaker_stickers.insert(speaker_name.clone(), png_path);
                continue;
            }
            // Generic named GIFs
            for candidate in [
                format!("mcp/assets/stickers/giphy_{}.gif", speaker_name),
                "mcp/assets/stickers/giphy_narrator.gif".to_string(),
                "mcp/assets/stickers/giphy_alice.gif".to_string(),
            ] {
                if Path::new(&candidate).exists() {
                    speaker_stickers.insert(speaker_name.clone(), candidate);
                    break;
                }
            }
            if speaker_stickers.contains_key(speaker_name) {
                continue;
            }
            // Cycle remaining local pool
            if !local_sticker_pool.is_empty() {
                let path = local_sticker_pool[local_idx % local_sticker_pool.len()].clone();
                local_idx += 1;
                speaker_stickers.insert(speaker_name.clone(), path);
            }
        }
        if speaker_stickers.is_empty() {
            render_warnings.push(
                "PRODUCTION_FAIL overlay_presence: no GIPHY key and no local stickers under mcp/assets/stickers/"
                    .into(),
            );
        } else if giphy_key_val.is_empty() {
            render_warnings.push(format!(
                "Using LOCAL sticker fallbacks ({} speaker(s)) — set GIPHY_API_KEY for topical animated stickers",
                speaker_stickers.len()
            ));
        }

        // Create sticker overlays per scene
        let mut current_ms = 0i64;
        let mut scene_idx = 0usize;
        if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
            for seg in segments {
                let speaker_name = seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
                let end_ms = seg
                    .get("end_ms")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(current_ms + 3000);

                if let Some(speaker_spec) = spec.speakers.get(speaker_name) {
                    // Phase 4: prefer per-scene sticker when available
                    let sticker_path = scene_sticker_map
                        .get(&scene_idx)
                        .or_else(|| speaker_stickers.get(speaker_name));
                    if let Some(sticker_path) = sticker_path {
                        let sticker_w = (spec.meta.width as f64 * speaker_spec.scale) as u32;
                        stickers.push(openscript_ffmpeg::multilayer_render::StickerOverlay {
                            path: sticker_path.clone(),
                            start_s: current_ms as f64 / 1000.0,
                            end_s: end_ms as f64 / 1000.0,
                            position: speaker_spec.position.clone(),
                            scale: speaker_spec.scale,
                            center_x: 0, // Will be computed in renderer
                            center_y: 0,
                            sticker_width: sticker_w,
                            sticker_height: sticker_w,
                        });
                    }
                }

                scene_idx += 1;
                current_ms = end_ms;
            }
        }
    }

    // Get music path
    // Music selection: use spec.music if provided, otherwise auto-select
    // from the 20-track stock catalog based on the theme. This ensures
    // every video has background music by default — the round-3 audit
    // found that agents who omitted the music field got silent videos,
    // which the user noted as a quality gap.
    // (Round-3 UX audit PROBLEM 3b fix.)
    let mut music_sel_tags: Vec<String> = Vec::new();
    let mut music_sel_query: Option<String> = None;
    let mut music_sel_source: Option<String> = None;

    let music_path = {
        let explicit = if let Some(ref m) = spec.music {
            if let Some(ref path) = m.path {
                if std::path::Path::new(path).exists() {
                    if is_synthetic_music_file(path) {
                        tracing::warn!(
                            "[script.to_video] Rejecting synthetic stock music: {}",
                            path
                        );
                        None
                    } else if openscript_core::production_quality::is_calm_focus_context(
                        Some(&spec.output.theme),
                        &spec.video_keywords,
                    ) && openscript_core::production_quality::music_hits_denylist(
                        path,
                        None,
                        &[],
                        Some(path),
                    ) {
                        tracing::warn!(
                            "[script.to_video] Rejecting denylist music for calm/focus: {}",
                            path
                        );
                        None
                    } else {
                        music_sel_source = Some("script".into());
                        music_sel_query = Some(path.clone());
                        Some(path.clone())
                    }
                } else {
                    tracing::warn!(
                        "[script.to_video] Music path not found: {} — auto-select",
                        path
                    );
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(p) = explicit {
            Some(p)
        } else if let Some(sel) =
            auto_select_music(&spec.output.theme, &spec.video_keywords).await
        {
            music_sel_tags = sel.tags;
            music_sel_query = Some(sel.selection_query);
            music_sel_source = Some(sel.source);
            Some(sel.path)
        } else {
            None
        }
    };

    // === MEME B-ROLLS: Full-screen reaction GIF clips per scene ===
    // GIPHY is a video-clip provider (like Pexels/YouTube). Meme b-rolls
    // are FULL-SCREEN video clips downloaded as MP4 from GIPHY that briefly
    // replace the background — like TikTok reaction cuts. They are NOT
    // stickers (small overlays). They are proper background video clips.
    // (Round-9: user said "Meme Brolls must be full-screen b-rolls, not
    // stickers. Stickers/GIF implementation is another thing.")
    let mut meme_clips: Vec<openscript_ffmpeg::multilayer_render::MemeClip> = Vec::new();
    // Track GIPHY GIF IDs that have already been used to prevent the same
    // meme appearing in multiple scenes.
    // (Round-16: "Ensure that the entire video timeline has unique videos,
    // not repeated video that might reduce the attention-hooking.")
    let mut used_meme_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if spec.meme_brolls.enabled {
        let giphy_key_val = giphy_key();
        if !giphy_key_val.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

            let meme_dir = "mcp/assets/meme_cache";
            std::fs::create_dir_all(meme_dir).ok();

            let mut scene_start_s: f64 = 0.0;

            for (scene_idx, scene) in spec.scenes.iter().enumerate() {
                let scene_dur_s = scene_durations.get(scene_idx).copied().unwrap_or(3.0);

                // Build multiple search queries ranked by specificity.
                // (Round-18: GIPHY SDK multi-query strategy with relevance
                // scoring. Tries specific → broad → trending fallback.)
                let search_strategies = build_meme_search_queries(
                    &scene.text,
                    &spec.video_keywords,
                    &spec.output.theme,
                );

                tracing::info!(
                    "[script.to_video] Meme b-roll scene {}: {} search strategies",
                    scene_idx + 1,
                    search_strategies.len()
                );
                for (i, (q, lim)) in search_strategies.iter().enumerate() {
                    tracing::info!(
                        "[script.to_video]   strategy {}: query='{}' limit={}",
                        i + 1, q, lim
                    );
                }

                let mut meme_found = false;

                // Try each search strategy in order until we find a suitable GIF
                for (query, limit) in &search_strategies {
                    if meme_found {
                        break;
                    }

                    // Build GIPHY URL — use search for non-empty queries,
                    // trending for empty query (ultimate fallback).
                    let giphy_url = if query.is_empty() {
                        format!(
                            "https://api.giphy.com/v1/gifs/trending?api_key={}&limit={}&rating=pg&bundle=sticker_layering",
                            giphy_key_val, limit
                        )
                    } else {
                        format!(
                            "https://api.giphy.com/v1/gifs/search?api_key={}&q={}&limit={}&rating=pg&lang=en&bundle=sticker_layering&remove_low_contrast=true",
                            giphy_key_val,
                            urlencoding::encode(query),
                            limit
                        )
                    };

                    let resp_result = client.get(&giphy_url).send().await;
                    if let Ok(resp) = resp_result {
                        if !resp.status().is_success() {
                            continue;
                        }
                        let body_result = resp.json::<serde_json::Value>().await;
                        if body_result.is_err() {
                            continue;
                        }
                        let body = body_result.unwrap();
                        let data_arr = body.get("data").and_then(|v| v.as_array());
                        if data_arr.is_none() {
                            continue;
                        }
                        let gifs = data_arr.unwrap();

                        // Score all results by relevance and pick the best
                        // non-duplicate, non-static GIF with MP4.
                        let mut best_gif: Option<(serde_json::Value, u32)> = None;
                        for g in gifs {
                            let gid = g.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            if !gid.is_empty() && used_meme_ids.contains(gid) {
                                continue;
                            }
                            let imgs = match g.get("images").and_then(|v| v.as_object()) {
                                Some(im) => im,
                                None => continue,
                            };
                            let orig = match imgs.get("original") {
                                Some(o) => o,
                                None => continue,
                            };
                            let frames = orig.get("frames")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<u32>().ok())
                                .unwrap_or(0);
                            if frames < 2 {
                                continue;
                            }
                            let mp4 = orig.get("mp4")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty());
                            if mp4.is_none() {
                                continue;
                            }
                            // Score relevance
                            let score = score_gif_relevance(g, query);
                            if best_gif.is_none() || score > best_gif.as_ref().unwrap().1 {
                                best_gif = Some((g.clone(), score));
                            }
                        }

                        if let Some((gif, score)) = best_gif {
                            let gif_id = gif.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let gif_title = gif.get("title").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                            tracing::info!(
                                "[script.to_video] Meme b-roll scene {}: FOUND query='{}' gif_id={} title='{}' relevance_score={}",
                                scene_idx + 1, query, gif_id, gif_title, score
                            );

                            let images = gif.get("images").cloned().unwrap_or(json!({}));
                            let original = images.get("original").cloned().unwrap_or(json!({}));

                            let mp4_url = original
                                .get("mp4")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .or_else(|| {
                                    images.get("original_mp4")
                                        .and_then(|v| v.get("mp4"))
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                })
                                .unwrap_or("");

                            if !mp4_url.is_empty() {
                                let meme_path = format!("{}/meme_scene_{}.mp4", meme_dir, scene_idx + 1);
                                if let Ok(dl_resp) = client.get(mp4_url).send().await {
                                    if dl_resp.status().is_success() {
                                        if let Ok(bytes) = dl_resp.bytes().await {
                                            std::fs::write(&meme_path, &bytes).ok();
                                            let meme_start_s = scene_start_s + (scene_dur_s * 0.4);
                                            let meme_end_s = meme_start_s + spec.meme_brolls.duration_s;
                                            let scene_end_s = scene_start_s + scene_dur_s;
                                            let meme_end_s = meme_end_s.min(scene_end_s);
                                            meme_clips.push(openscript_ffmpeg::multilayer_render::MemeClip {
                                                path: meme_path.clone(),
                                                start_s: meme_start_s,
                                                end_s: meme_end_s,
                                            });
                                            used_meme_ids.insert(gif_id.clone());
                                            tracing::info!(
                                                "[script.to_video] Downloaded meme b-roll MP4 for scene {}: {} ({} bytes, {:.1}s-{:.1}s, relevance={})",
                                                scene_idx + 1, meme_path, bytes.len(),
                                                meme_start_s, meme_end_s, score
                                            );
                                            meme_found = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if !meme_found {
                    tracing::warn!(
                        "[script.to_video] No suitable meme b-roll found for scene {} after all strategies",
                        scene_idx + 1
                    );
                }

                scene_start_s += scene_dur_s;
            }
        }
    }

    // Build timeline preview for agent inspection
    let bg_assignments: Vec<openscript_core::timeline_preview::BackgroundClipAssignment> =
        backgrounds
            .iter()
            .enumerate()
            .map(|(i, bg)| {
                let start_ms: i64 = scene_durations[..i].iter().sum::<f64>() as i64 * 1000;
                let end_ms = start_ms + (bg.duration_s * 1000.0) as i64;
                openscript_core::timeline_preview::BackgroundClipAssignment {
                    start_ms,
                    end_ms,
                    path: bg.path.clone(),
                    looped: bg.looped,
                }
            })
            .collect();

    let sticker_assignments: Vec<openscript_core::timeline_preview::StickerAssignment> = stickers
        .iter()
        .map(|s| {
            // Calculate sticker dimensions and center-based coordinates
            let sticker_w = (spec.meta.width as f64 * s.scale) as u32;
            let sticker_h = sticker_w; // Approximate square; actual aspect ratio varies
            let margin = 40i32;
            let (tl_x, tl_y): (i32, i32) = match s.position.as_str() {
                "top-left" => (margin, margin),
                "top-right" => (spec.meta.width as i32 - sticker_w as i32 - margin, margin),
                "bottom-left" => (margin, spec.meta.height as i32 - sticker_h as i32 - margin),
                "bottom-right" => (
                    spec.meta.width as i32 - sticker_w as i32 - margin,
                    spec.meta.height as i32 - sticker_h as i32 - margin,
                ),
                "center" => (
                    (spec.meta.width as i32 - sticker_w as i32) / 2,
                    (spec.meta.height as i32 - sticker_h as i32) / 2,
                ),
                _ => (margin, margin),
            };
            let center_x = tl_x + sticker_w as i32 / 2 - spec.meta.width as i32 / 2;
            let center_y = tl_y + sticker_h as i32 / 2 - spec.meta.height as i32 / 2;

            openscript_core::timeline_preview::StickerAssignment {
                start_ms: (s.start_s * 1000.0) as i64,
                end_ms: (s.end_s * 1000.0) as i64,
                path: s.path.clone(),
                position: s.position.clone(),
                scale: s.scale,
                speaker: String::new(),
                center_x,
                center_y,
                sticker_width: sticker_w,
                sticker_height: sticker_h,
            }
        })
        .collect();

    let layered_timeline = openscript_core::timeline_preview::build_layered_timeline(
        &manifest,
        &bg_assignments,
        music_path.as_deref(),
        // ponytail: ducking defaults to true whenever music is present — auto-selected
        // music should always duck under voiceover to avoid masking speech.
        spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some()),
        &sticker_assignments,
        Some(captions_path),
        &spec.captions.style,
        spec.meta.width,
        spec.meta.height,
        spec.meta.fps,
    );

    let timeline_preview = layered_timeline.preview();
    let timeline_issues = layered_timeline.validate();
    let timeline_summary = layered_timeline.summary();

    // Write timeline preview to file
    let preview_path = format!("{}/timeline_preview.txt", output_dir);
    std::fs::write(&preview_path, &timeline_preview)?;

    // ponytail: Update timeline JSON tracks with broll/music/caption/SFX events.
    // handle_script_to_timeline wrote sparse tracks (broll=1, music=0, captions=0, sfx=0).
    // The per-scene multi-broll, music selection, and SFX auto-generation happened AFTER
    // that, so the timeline JSON is stale. Reload, populate, and save so the KPI evaluation
    // (which reads Timeline::load()) sees the correct event counts.
    let sfx_hits = auto_select_sfx_hits(&scene_durations);
    {
        if let Ok(mut tl) = openscript_core::timeline::Timeline::load(&timeline_path) {
            use openscript_core::types::TrackType;
            use openscript_core::timeline::{EventKind, TimelineEvent};

            // Clear sparse tracks from handle_script_to_timeline before repopulating.
            // The sparse handler wrote broll=1 event covering the full video; we now have
            // per-scene broll assignments, music, captions, and SFX. Clear each track to
            // avoid duplicate/overlapping events.
            for track_type in [TrackType::Broll, TrackType::Music, TrackType::Captions, TrackType::Sfx, TrackType::Stickers] {
                if let Some(events) = tl.tracks.get_mut(&track_type) {
                    events.clear();
                }
            }

            // Broll track: one event per scene from bg_assignments.
            // Use accumulated scene_durations for timing (not bg.start/end_ms which use the
            // clip's raw duration and compound floating-point drift across scenes).
            let mut cumulative_ms: i64 = 0;
            for (i, bg) in bg_assignments.iter().enumerate() {
                let scene_ms = (scene_durations[i] * 1000.0) as i64;
                let start_ms = cumulative_ms;
                let end_ms = start_ms + scene_ms;
                cumulative_ms = end_ms;
                // Traceability: record the exact query + provider + title that
                // produced each scene's clip so the timeline is auditable
                // (previously concept/tags/provenance were all empty and the
                // query lived only in ephemeral logs).
                let stock_meta = scene_stock_meta.get(i).and_then(|m| m.as_ref());
                let (meta_id, s_query, s_title) = match stock_meta {
                    Some((mid, _h, q, _l, t, _v, _vr)) => (mid.clone(), q.clone(), t.clone()),
                    None => (String::new(), String::new(), String::new()),
                };
                let concept = if s_query.is_empty() {
                    format!("scene {}", i + 1)
                } else {
                    s_query.clone()
                };
                let source_provider = if meta_id.starts_with("pexels_") {
                    "pexels"
                } else if meta_id.is_empty() {
                    "procedural"
                } else {
                    "stock"
                };
                tl.add_track_event(
                    TrackType::Broll,
                    TimelineEvent {
                        id: format!("broll_{}", i + 1),
                        asset_id: bg.path.clone(),
                        start_ms,
                        end_ms,
                        offset_ms: 0,
                        gain_db: 0.0,
                        fade_in_ms: 0,
                        fade_out_ms: 0,
                        tags: vec![concept.clone(), source_provider.to_string(), s_title.clone()],
                        provenance: Some(openscript_core::timeline::Provenance {
                            tool: "script.to_video".into(),
                            editorial_role: None,
                            concept: Some(concept.clone()),
                        }),
                        kind: EventKind::Broll {
                            concept,
                            source_provider: source_provider.into(),
                            transition_style: "cut".into(),
                            crop_mode: "center".into(),
                            orientation: "portrait".into(),
                            motion_intensity: "low".into(),
                        },
                    },
                );
            }

            // Music track: single event spanning the full duration
            if let Some(ref mp) = music_path {
                tl.add_track_event(
                    TrackType::Music,
                    TimelineEvent {
                        id: "music_bg".into(),
                        asset_id: mp.clone(),
                        start_ms: 0,
                        end_ms: total_duration_ms,
                        offset_ms: 0,
                        gain_db: spec.music.as_ref().map(|m| m.gain_db).unwrap_or(-12.0),
                        fade_in_ms: 500,
                        fade_out_ms: 1000,
                        tags: music_sel_tags.clone(),
                        provenance: None,
                        kind: EventKind::Music {
                            mood: spec
                                .music
                                .as_ref()
                                .and_then(|m| m.mood.clone())
                                .unwrap_or_else(|| spec.output.theme.clone()),
                            energy: "low".into(),
                            bpm: None,
                            loopability: true,
                            intro_friendly: true,
                            cta_friendly: true,
                            loudness_target_lufs: -14.0,
                            loop_mode: "trim".into(),
                            ducking_policy: if spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some()) { "auto".into() } else { "none".into() },
                        },
                    },
                );
            }

            // Wire ducking directives so the filter graph sidechain compressor
            // ducks music during speech (was previously empty — music never ducked).
            let has_speech = !tl.tracks.get(&TrackType::Dialogue).map(|v| v.is_empty()).unwrap_or(true)
                || !tl.tracks.get(&TrackType::Voiceover).map(|v| v.is_empty()).unwrap_or(true);
            let music_has_ducking = spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some());
            if has_speech && music_has_ducking {
                tl.add_ducking_directive("dialogue_active", "music", 10.0, 50, 200);
            }

            // Captions track: summary event if captions file exists
            if !captions_path.is_empty() {
                tl.add_track_event(
                    TrackType::Captions,
                    TimelineEvent {
                        id: "captions_all".into(),
                        asset_id: captions_path.to_string(),
                        start_ms: 0,
                        end_ms: total_duration_ms,
                        offset_ms: 0,
                        gain_db: 0.0,
                        fade_in_ms: 0,
                        fade_out_ms: 0,
                        tags: vec![],
                        provenance: None,
                        kind: EventKind::Caption {
                            text: String::new(),
                            style: spec.captions.style.clone(),
                            word_timings: vec![],
                        },
                    },
                );
            }

            // SFX track: one event per auto-selected SFX hit
            for (i, sfx) in sfx_hits.iter().enumerate() {
                let start_ms = (sfx.start_s * 1000.0) as i64;
                // SFX are short (<1s typically), assume 500ms duration for timeline display
                let end_ms = start_ms + 500;
                let gain_db = if sfx.volume > 0.0 {
                    20.0 * sfx.volume.log10()
                } else {
                    -60.0
                };
                tl.add_track_event(
                    TrackType::Sfx,
                    TimelineEvent {
                        id: format!("sfx_{}", i + 1),
                        asset_id: sfx.path.clone(),
                        start_ms,
                        end_ms,
                        offset_ms: 0,
                        gain_db,
                        fade_in_ms: 0,
                        fade_out_ms: 50,
                        tags: vec![],
                        provenance: None,
                        kind: EventKind::Sfx {
                            editorial_role: "transition".into(),
                            category: "whoosh".into(),
                            subcategory: String::new(),
                            duration_ms: 500,
                            sample_rate: 44100,
                            peak_db: 0.0,
                            loudness_lufs: -14.0,
                            recommended_gain_db: gain_db,
                            recommended_use: "scene_transition".into(),
                            safe_overlay: true,
                        },
                    },
                );
            }

            // Register broll assets for unique-visual-asset count
            for (i, bg) in bg_assignments.iter().enumerate() {
                let asset_id = format!("broll_{}", i + 1);
                let (meta_id, s_query, s_title, lex, v_score) =
                    match scene_stock_meta.get(i).and_then(|m| m.as_ref()) {
                        Some((mid, _h, q, l, t, v, _vr)) => {
                            (mid.clone(), q.clone(), t.clone(), *l, *v)
                        }
                        None => (String::new(), String::new(), String::new(), 0.0, 0.0),
                    };
                tl.add_asset(
                    "broll",
                    asset_id,
                    serde_json::json!({
                        "path": bg.path,
                        "start_ms": bg.start_ms,
                        "end_ms": bg.end_ms,
                        "query": s_query,
                        "provider_id": meta_id,
                        "source_title": s_title,
                        "lexical_score": lex,
                        "vision_score": v_score,
                    }),
                );
            }

            // Register SFX assets so the validator can detect repetition.
            for (i, sfx) in sfx_hits.iter().enumerate() {
                let asset_id = format!("sfx_{}", i + 1);
                tl.add_asset(
                    "sfx",
                    asset_id,
                    serde_json::json!({"path": sfx.path, "volume": sfx.volume}),
                );
            }

            // Stickers track: persist the overlays the multilayer render
            // composites so verify.render / timeline inspection see them.
            // (Previously stickers existed only in the render spec — the
            // timeline's Stickers track stayed empty and verify reported 0.)
            for (i, st) in stickers.iter().enumerate() {
                let event_id = format!("sticker_{:03}", i + 1);
                let start_ms = (st.start_s * 1000.0) as i64;
                let end_ms = (st.end_s * 1000.0) as i64;
                tl.add_track_event(
                    TrackType::Stickers,
                    TimelineEvent {
                        id: event_id.clone(),
                        asset_id: event_id.clone(),
                        start_ms,
                        end_ms,
                        offset_ms: 0,
                        gain_db: 0.0,
                        fade_in_ms: 150,
                        fade_out_ms: 150,
                        tags: vec!["sticker".to_string(), st.position.clone()],
                        provenance: None,
                        kind: EventKind::Broll {
                            concept: format!("overlay:{}", st.position),
                            source_provider: st.path.clone(),
                            transition_style: "overlay".into(),
                            crop_mode: "none".into(),
                            orientation: "9:16".into(),
                            motion_intensity: "static".into(),
                        },
                    },
                );
                tl.add_asset(
                    "broll",
                    event_id.clone(),
                    serde_json::json!({
                        "path": st.path,
                        "position": st.position,
                        "scale": st.scale,
                        "overlay": true,
                    }),
                );
            }

            // Save updated timeline
            let _ = tl.save(&timeline_path);
            tracing::info!(
                "[script.to_video] Updated timeline tracks: broll={} music={} captions={} sfx={} stickers={}",
                bg_assignments.len(),
                if music_path.is_some() { 1 } else { 0 },
                if !captions_path.is_empty() { 1 } else { 0 },
                sfx_hits.len(),
                stickers.len(),
            );
        }
    }

    report_progress(60.0, 100.0, "Phase 3/3: Rendering multi-layer video...")
        .await
        .ok();

    // Build multi-layer render spec
    use openscript_ffmpeg::multilayer_render::{render_multilayer, MultiLayerRenderSpec};
    let music_sel_sfx_count = sfx_hits.len();

    // ponytail: compute ducking BEFORE moving music_path into the struct.
    // Ducking defaults to true whenever music is present — auto-selected
    // music should always duck under voiceover to avoid masking speech.
    let should_duck = spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some());

    let render_spec = MultiLayerRenderSpec {
        backgrounds,
        voiceover_paths,
        stickers,
        music_path,
        // P1 FIX: Clamp music gain_db to -8..-14 dB range (production quality sweet spot).
        // Agents writing gain_db=6.0 or gain_db=-30.0 produce inaudible or overpowering music.
        music_volume: {
            let raw_gain = spec.music.as_ref().map(|m| m.gain_db).unwrap_or(-20.0);
            let clamped = raw_gain.clamp(-14.0, -8.0);
            if (raw_gain - clamped).abs() > f64::EPSILON {
                tracing::info!(
                    "music gain_db={} clamped to {} dB (production range -14..-8)",
                    raw_gain, clamped
                );
            }
            10f64.powf(clamped / 20.0)
        },
        ducking: should_duck,
        ducking_depth_db: spec
            .music
            .as_ref()
            .map(|m| m.ducking_depth_db)
            .unwrap_or(12.0),
        captions_path: if std::path::Path::new(captions_path).exists() {
            Some(captions_path.to_string())
        } else {
            None
        },
        width: spec.meta.width,
        height: spec.meta.height,
        fps: spec.meta.fps,
        output_path: output_path.to_string(),
        crf: if preview_mode { 28 } else { spec.output.crf },
        preset: if preview_mode {
            "ultrafast".to_string()
        } else {
            "fast".to_string()
        },
        total_duration_s,
        meme_clips,
        sfx: sfx_hits,
        fonts_dir: resolve_fonts_dir(),
    };

    // Phase L: Branch on render_engine. When "hyperframes", compile the
    // timeline to HF HTML and render via hf.render instead of render_multilayer.
    // This connects HyperFrames to the golden trajectory — agents can now
    // choose the render engine via output.render_engine in the script JSON.
    let render_engine = spec.output.render_engine.as_str();
    let render_result = if render_engine == "hyperframes" {
        report_progress(70.0, 100.0, "Compiling timeline to HyperFrames HTML...")
            .await
            .ok();

        // Compile the timeline JSON to HF HTML via timeline.to_hyperframes
        let hf_compilation = handle_timeline_to_hyperframes(json!({
            "timeline_path": timeline_path,
            "output_dir": format!("{}/hf_composition", output_dir),
        }))
        .await?;

        let hf_project_dir = hf_compilation
            .get("project_dir")
            .and_then(|v| v.as_str())
            .unwrap_or("artifacts/hf_composition")
            .to_string();

        report_progress(80.0, 100.0, "Rendering via HyperFrames...")
            .await
            .ok();

        // Render via hf.render
        let hf_render_args = json!({
            "project_dir": hf_project_dir,
            "output_path": output_path,
            "quality": if preview_mode { "draft" } else { "standard" },
        });

        match crate::hf::handle_hf_render(hf_render_args).await {
            Ok(result) => {
                let out = result
                    .get("output_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&output_path)
                    .to_string();
                Ok(out)
            }
            Err(e) => Err(openscript_ffmpeg::FfmpegError::RenderFailed(format!(
                "HyperFrames render failed: {}",
                e
            ))),
        }
    } else {
        // Default: FFmpeg multilayer render
        render_multilayer(&render_spec).await
    };

    // Merge timeline-phase warnings (Value) with render-phase warnings (Vec<String>)
    // into a single JSON value for the response.
    let merged_warnings: serde_json::Value = {
        let mut all_warnings: Vec<String> = Vec::new();
        if let Some(arr) = warnings.as_array() {
            for w in arr {
                if let Some(s) = w.as_str() {
                    all_warnings.push(s.to_string());
                }
            }
        }
        all_warnings.extend(render_warnings);
        if all_warnings.is_empty() {
            serde_json::Value::Null
        } else {
            json!(all_warnings)
        }
    };

    match render_result {
        Ok(out_path) => {
            let file_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            report_progress(100.0, 100.0, "Video created").await.ok();

            // Production KPI v2 — architecture-level quality (source, cuts/s, music
            // variance, sticker design, section composition, timeline utilization).
            use openscript_core::production_quality::{
                evaluate_production_quality, BackgroundLayerInfo, MemeLayerInfo, MusicLayerInfo,
                RenderManifest, SectionInfo, SectionRole, StickerLayerInfo,
            };
            let (has_dialogue, rms_ok) = probe_dialogue_rms(&out_path).await;
            let mut t_cursor = 0i64;
            let mut bg_layers = Vec::new();
            for (i, b) in render_spec.backgrounds.iter().enumerate() {
                let dur_ms = (b.duration_s * 1000.0) as i64;
                let meta = scene_stock_meta.get(i).and_then(|m| m.as_ref());
                // Authoritative provenance FIRST: the 7-tuple's video_id is
                // `pexels_<id>` for Pexels, a raw YouTube id for yt-dlp.
                // Path heuristics run AFTER so a Pexels clip stored in
                // background_cache/ is not mislabeled "youtube".
                let hint = if meta
                    .map(|(id, _, _, _, _, _, _)| id.starts_with("pexels_"))
                    .unwrap_or(false)
                {
                    Some("pexels".into())
                } else if is_procedural_media_path(&b.path) {
                    Some("procedural".into())
                } else if b.path.contains("_yt") || b.path.contains("background_cache") {
                    Some("youtube".into())
                } else {
                    None
                };
                bg_layers.push(BackgroundLayerInfo {
                    path: b.path.clone(),
                    start_ms: t_cursor,
                    end_ms: t_cursor + dur_ms,
                    source_hint: hint,
                    content_hash: meta.map(|(_, h, _, _, _, _, _)| h.clone()),
                    video_id: meta.map(|(id, _, _, _, _, _, _)| id.clone()),
                    search_query: meta.map(|(_, _, q, _, _, _, _)| q.clone()),
                    lexical_score: meta.map(|(_, _, _, lex, _, _, _)| *lex),
                    source_title: meta.map(|(_, _, _, _, t, _, _)| t.clone()),
                    vision_score: meta.map(|(_, _, _, _, _, vs, _)| *vs),
                    vision_reason: meta.and_then(|(_, _, _, _, _, _, vr)| vr.clone()),
                });
                t_cursor += dur_ms;
            }
            let sticker_layers: Vec<StickerLayerInfo> = render_spec
                .stickers
                .iter()
                .map(|s| StickerLayerInfo {
                    path: s.path.clone(),
                    start_ms: (s.start_s * 1000.0) as i64,
                    end_ms: (s.end_s * 1000.0) as i64,
                    position: s.position.clone(),
                    scale: s.scale,
                })
                .collect();
            let meme_layers: Vec<MemeLayerInfo> = render_spec
                .meme_clips
                .iter()
                .map(|m| MemeLayerInfo {
                    path: m.path.clone(),
                    start_ms: (m.start_s * 1000.0) as i64,
                    end_ms: (m.end_s * 1000.0) as i64,
                })
                .collect();
            let music_layer = render_spec.music_path.as_ref().map(|p| {
                let gain_db = if render_spec.music_volume > 0.0 {
                    20.0 * render_spec.music_volume.log10()
                } else {
                    -60.0
                };
                MusicLayerInfo {
                    path: p.clone(),
                    gain_db,
                    ducking: render_spec.ducking,
                    mood: Some(
                        spec.music
                            .as_ref()
                            .and_then(|m| m.mood.clone())
                            .unwrap_or_else(|| spec.output.theme.clone()),
                    ),
                    energy: None,
                    tags: music_sel_tags.clone(),
                    selection_query: music_sel_query.clone(),
                    source: music_sel_source.clone(),
                }
            });
            // Section map from scenes (hook / body / cta)
            let n_scenes = spec.scenes.len().max(1);
            let mut sections = Vec::new();
            let mut s_cursor = 0i64;
            for (i, scene) in spec.scenes.iter().enumerate() {
                let dur_ms = scene_durations
                    .get(i)
                    .map(|d| (*d * 1000.0) as i64)
                    .unwrap_or(3000);
                let role = if i == 0 {
                    SectionRole::Hook
                } else if i + 1 == n_scenes {
                    SectionRole::Cta
                } else if i + 2 >= n_scenes {
                    SectionRole::Payoff
                } else {
                    SectionRole::Body
                };
                sections.push(SectionInfo {
                    role,
                    start_ms: s_cursor,
                    end_ms: s_cursor + dur_ms,
                    text: scene.text.clone(),
                    title_text: None,
                });
                s_cursor += dur_ms;
            }
            let render_manifest = RenderManifest {
                duration_ms: total_duration_ms,
                backgrounds: bg_layers.clone(),
                stickers: sticker_layers,
                memes: meme_layers,
                music: music_layer,
                captions_path: if !captions_path.is_empty() {
                    Some(captions_path.to_string())
                } else {
                    None
                },
                voiceover_count: render_spec.voiceover_paths.len(),
                sections,
                has_dialogue,
                rms_ok,
                video_keywords: spec.video_keywords.clone(),
                theme: Some(spec.output.theme.clone()),
                caption_style: Some(spec.captions.style.clone()),
                sfx_count: music_sel_sfx_count,
                ..Default::default()
            };
            let manifest_out = format!("{}/render_manifest.json", output_dir);
            if let Ok(s) = serde_json::to_string_pretty(&render_manifest) {
                let _ = std::fs::write(&manifest_out, s);
            }
            let timeline_for_kpi = Timeline::load(&timeline_path)
                .unwrap_or_else(|_| Timeline::new(std::path::PathBuf::from("out.mp4"), "9:16", 30, None));
            let pq = evaluate_production_quality(&timeline_for_kpi, &render_manifest);
            // Fail closed: hard_fails, draft outputs, or majority procedural never "success"
            let is_draft = out_path.contains(".draft.mp4")
                || merged_warnings
                    .as_array()
                    .map(|a| {
                        a.iter().any(|w| {
                            w.as_str()
                                .map(|s| s.contains("FAIL_CLOSED"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
    // P0 FIX: If render succeeded but MP4 is missing or empty, treat as failure.
    // This catches silent ffmpeg failures where render_multilayer returns Ok but
    // the output file was never written (e.g. CWD mismatch, permission errors).
    let delivery_status = if !out_path.is_empty() && !std::path::Path::new(&out_path).exists() {
        tracing::warn!("render returned Ok but MP4 not found at: {}", out_path);
        "rendered_production_fail"
    } else if file_size == 0 {
        tracing::warn!("render returned Ok but MP4 is 0 bytes at: {}", out_path);
        "rendered_production_fail"
    } else if is_draft {
        "draft"
    } else if !pq.hard_fails.is_empty() {
        // Hard failures (e.g. >=50% procedural, music mismatch) always win —
        // the more severe signal must not be masked by the procedural status.
        "rendered_production_fail"
    } else if fell_to_procedural_any
        && !std::env::var("OPENSCRIPT_ALLOW_PROCEDURAL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    {
        // Loud-warning procedural policy: render ships but the status says so.
        "rendered_with_procedural"
    } else if pq.production_score >= 70 {
        "rendered"
    } else if pq.production_score >= 40 {
        "rendered_below_production_grade"
    } else {
        "rendered_production_fail"
    };
            let bg_paths: Vec<String> = bg_layers.iter().map(|b| b.path.clone()).collect();

            Ok(json!({
                "status": delivery_status,
                "output_path": out_path,
                "file_size_bytes": file_size,
                "timeline_path": timeline_path,
                "timeline_preview_path": preview_path,
                "timeline_preview": timeline_preview,
                "timeline_summary": timeline_summary,
                "timeline_issues": if timeline_issues.is_empty() { serde_json::Value::Null } else { json!(timeline_issues) },
                "voiceover_manifest": manifest_path,
                "render_manifest_path": manifest_out,
                "captions_path": captions_path,
                "total_duration_ms": total_duration_ms,
                "scene_count": timeline_result.get("scene_count"),
                "speaker_count": timeline_result.get("speaker_count"),
                "background_count": render_spec.backgrounds.len(),
                "sticker_count": render_spec.stickers.len(),
                "meme_count": render_spec.meme_clips.len(),
                "background_sources": bg_paths,
                "music_path": render_spec.music_path,
                "production_quality": pq,
                "designed_voices": designed_voices,
                "warnings": merged_warnings,
            }))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!("Render failed: {}", e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_tts(json: &str) -> serde_json::Value {
        let out = apply_tts_config_defaults(json);
        serde_json::from_str(&out).unwrap()
    }

    /// Config defaults are injected when the script omits tts entirely.
    #[test]
    fn tts_defaults_injected_when_tts_absent() {
        let _guard = crate::config::TTS_ENV_TEST_MUTEX.lock().unwrap();
        std::env::set_var("OPENSCRIPT_TTS_BACKEND", "audio8");
        std::env::set_var("OPENSCRIPT_TTS_VOICE", "ishan");
        let v = parse_tts(r#"{"speakers": {"n": {"voice": "default"}}, "scenes": [{"speaker": "n", "text": "Hi"}]}"#);
        assert_eq!(v["tts"]["backend"], "audio8");
        assert_eq!(v["tts"]["voice"], "ishan");
        std::env::remove_var("OPENSCRIPT_TTS_BACKEND");
        std::env::remove_var("OPENSCRIPT_TTS_VOICE");
    }

    /// Config defaults are injected when tts exists but backend/voice omitted.
    #[test]
    fn tts_defaults_injected_when_fields_omitted() {
        let _guard = crate::config::TTS_ENV_TEST_MUTEX.lock().unwrap();
        std::env::set_var("OPENSCRIPT_TTS_BACKEND", "audio8");
        std::env::set_var("OPENSCRIPT_TTS_VOICE", "ishan");
        let v = parse_tts(r#"{"tts": {"default_speed": 1.2}, "speakers": {"n": {"voice": "default"}}, "scenes": [{"speaker": "n", "text": "Hi"}]}"#);
        assert_eq!(v["tts"]["backend"], "audio8");
        assert_eq!(v["tts"]["voice"], "ishan");
        // Explicit speed survives.
        assert_eq!(v["tts"]["default_speed"], 1.2);
        std::env::remove_var("OPENSCRIPT_TTS_BACKEND");
        std::env::remove_var("OPENSCRIPT_TTS_VOICE");
    }

    /// Explicit script fields always win over env config.
    #[test]
    fn tts_explicit_script_wins_over_env() {
        let _guard = crate::config::TTS_ENV_TEST_MUTEX.lock().unwrap();
        std::env::set_var("OPENSCRIPT_TTS_BACKEND", "audio8");
        std::env::set_var("OPENSCRIPT_TTS_VOICE", "ishan");
        let v = parse_tts(r#"{"tts": {"backend": "kokoro"}, "speakers": {"n": {"voice": "af_heart"}}, "scenes": [{"speaker": "n", "text": "Hi"}]}"#);
        // Explicit script backend always wins over env.
        assert_eq!(v["tts"]["backend"], "kokoro");
        // tts.voice is still injected as the script-level default (only
        // consulted when a speaker uses the literal "default" — inert here
        // because the speaker pins af_heart explicitly).
        assert_eq!(v["tts"]["voice"], "ishan");
        std::env::remove_var("OPENSCRIPT_TTS_BACKEND");
        std::env::remove_var("OPENSCRIPT_TTS_VOICE");
    }

    /// Malformed JSON passes through untouched — parse_script reports the error.
    #[test]
    fn tts_malformed_json_passthrough() {
        let out = apply_tts_config_defaults("not json at all");
        assert_eq!(out, "not json at all");
    }
}

