// ---------------------------------------------------------------------------
// tools_character — Character-development workflow (Thread 2: voice-design).
//
// The voice-design workflow is a TWO-PART pipeline:
//   1. character-development: define characters (schema + properties) and
//      design each one's voice + per-emotion deliveries.
//   2. transcript-development: the script references characters by id; each
//      scene's `emote` selects the character's matching emotional take.
//
// Persistence: `.openscript/characters.json` (development-side schema). Each
// character's base voice is a `voicedesign` profile (`{character_id}`) whose
// provider routes synthesis DIRECTLY to the Qwen3 VoiceDesign model: the
// character's personality + the scene's emotion instruct are passed as the
// `instruct` at synth time, so the voice-design model generates every line
// (no cloning — gepard/audio8 are separate clone engines and never touch a
// voicedesign profile). The `emotions` map stores each take's `instruct`,
// which the router uses to attune per-line tonality.
// ---------------------------------------------------------------------------
use super::*;

/// Path to the character registry (development-side character schema).
fn characters_path() -> String {
    std::env::var("OPENSCRIPT_CHARACTERS_PATH")
        .unwrap_or_else(|_| ".openscript/characters.json".to_string())
}

fn load_characters() -> Result<serde_json::Value, ToolError> {
    let path = characters_path();
    if Path::new(&path).exists() {
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    } else {
        Ok(json!({}))
    }
}

fn save_characters(chars: &serde_json::Value) -> Result<(), ToolError> {
    atomic_write_json(&characters_path(), chars)
}

/// Sanitize an id for use in filenames — MUST match gepard's `_voice_path`
/// sanitizer (alnum + `-_.`) so the design WAV and the registered voice copy
/// land on the same path (no orphaned files for ids containing dashes).
fn sanitize_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect::<String>()
}

/// Character base voice output path (gepard voices dir = the standard
/// registered-voice location; emotion takes live here too).
fn character_voice_path(character_id: &str) -> String {
    format!("mcp/assets/gepard/voices/{}.wav", sanitize_id(character_id))
}

fn character_emotion_path(character_id: &str, emotion: &str) -> String {
    format!(
        "mcp/assets/gepard/voices/{}__{}.wav",
        sanitize_id(character_id),
        sanitize_id(emotion)
    )
}

/// Design a voice (VoiceDesign ONNX int4) and write the WAV.
/// Shared by character.create (base voice) and character.design_emotion.
fn design_voice_wav(
    instruct: &str,
    text: &str,
    output_path: &str,
    language: &str,
    seed: Option<i64>,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    top_k: Option<u32>,
) -> Result<(i64, u32), ToolError> {
    openscript_tts::voicedesign::voicedesign_design(
        instruct,
        text,
        output_path,
        language,
        seed,
        max_tokens,
        temperature,
        top_k,
    )
    .map(|(dur, sr, _written)| (dur, sr))
    .map_err(|e| {
        ToolError::Tts(format!(
            "voice design failed ({}) — run scripts/setup_voicedesign.sh to provision the VoiceDesign ONNX engine",
            e
        ))
    })
}

/// PART 1 of the two-part workflow: define a character and design its base
/// voice. `voice` may reference an existing profile (skip design); otherwise
/// the base voice is designed from `personality` + `sample_text` and
/// registered as a `voicedesign` profile `{character_id}` — scene synthesis
/// then runs DIRECTLY on the Qwen3 VoiceDesign model (per-line instruct), not
/// through a cloning engine.
pub(crate) async fn handle_character_create(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let character_id = extract_str(&args, "character_id")?;
    let name = default_str(&args, "name", &character_id);
    let role = default_str(&args, "role", "character");
    let personality = extract_str(&args, "personality")?;
    let language = default_str(&args, "language", "english");
    let sample_text = default_opt_str(&args, "sample_text");
    let existing_voice = default_opt_str(&args, "voice");
    let seed = args.get("seed").and_then(|v| v.as_i64());

    if personality.trim().is_empty() {
        return Err(ToolError::InvalidArg(
            "character.create requires a non-empty 'personality' voice description".into(),
        ));
    }

    // --- Read-only validation (no lock; the registry only grows here) ---
    let chars = load_characters()?;
    if chars.get(&character_id).is_some() {
        return Err(ToolError::InvalidArg(format!(
            "Character '{}' already exists — use character.design_emotion to add emotional takes",
            character_id
        )));
    }

    // Base voice: use an existing profile, or design one (VoiceDesign + gepard
    // sidecar registration are registry-free; the JSON profile entry is written
    // inside the lock below).
    let (base_voice, wav_path, ref_text): (String, Option<String>, Option<String>) =
        match existing_voice {
            Some(v) if !v.is_empty() => {
                // Validate the referenced profile exists.
                let profiles = load_voice_profiles()?;
                if profiles.get(&v).is_none() {
                    return Err(ToolError::NotFound(format!(
                        "voice profile '{}' not found — create it first via voice.profile.add or voice.design",
                        v
                    )));
                }
                (v, None, None)
            }
            _ => {
                let sample = sample_text.as_deref().unwrap_or("");
                if sample.trim().is_empty() {
                    return Err(ToolError::InvalidArg(
                        "character.create needs 'sample_text' (a line the base voice should speak) when no existing 'voice' is given".into(),
                    ));
                }
                let wav = character_voice_path(&character_id);
                if let Some(parent) = Path::new(&wav).parent() {
                    std::fs::create_dir_all(parent)?;
                }
                report_progress(0.0, 100.0, &format!("Designing base voice for '{}'...", character_id))
                    .await
                    .ok();
                let (_dur, _sr) = design_voice_wav(
                    &personality, sample, &wav, &language, seed, None, None, None,
                )?;
                report_progress(100.0, 100.0, "Base voice designed").await.ok();

                // No cloning-engine registration: the designed WAV is a design
                // artifact. Synthesis happens DIRECTLY on Qwen3 VoiceDesign via
                // the voicedesign provider (personality + per-line instruct).
                (character_id.to_string(), Some(wav), Some(sample.to_string()))
            }
        };

    // --- Mutating phase: lock BOTH registries, re-read fresh state, write. ---
    // Order: characters.json first, then voice_profiles.json (tools_audio
    // handlers only lock profiles, so this ordering cannot deadlock).
    let _lock_chars = RegistryLock::acquire(Path::new(&characters_path()))?;
    let _lock_profiles = RegistryLock::acquire(Path::new(&voice_profiles_path()))?;

    let mut chars = load_characters()?;
    if chars.get(&character_id).is_some() {
        return Err(ToolError::InvalidArg(format!(
            "Character '{}' already exists — use character.design_emotion to add emotional takes",
            character_id
        )));
    }
    if let (Some(wav), Some(sample)) = (&wav_path, &ref_text) {
        let mut profiles = load_voice_profiles()?;
        profiles[&character_id] = json!({
            "profile_id": character_id,
            "ref_audio": wav,
            "ref_text": sample,
            // voicedesign: scene lines synthesize DIRECTLY with the Qwen3
            // VoiceDesign model (personality + emotion instruct) — the
            // ref WAV is a design artifact, not a clone reference.
            "provider": "voicedesign",
            "mode": "design",
            "model": "Qwen3-TTS-12Hz-1.7B-VoiceDesign",
            "language": language,
            "description": format!("character base voice: {}", personality),
            "emotions": {},
        });
        save_voice_profiles(&profiles)?;
    }
    chars[&character_id] = json!({
        "id": character_id,
        "name": name,
        "role": role,
        "personality": personality,
        "language": language,
        "voice": base_voice,
        "emotions": {},
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    save_characters(&chars)?;

    Ok(json!({
        "status": "character_created",
        "character": chars.get(&character_id).cloned().unwrap_or(json!({})),
        "voice_profile_id": base_voice,
        "note": "Design emotional takes with character.design_emotion, then reference the character in a script: speakers.<id>.voice = <character_id> and set each scene's emote to select the take.",
    }))
}

/// PART 1 continued: design one emotional delivery take for a character.
/// Runs VoiceDesign with the character's personality + the emotion description,
/// writes the take WAV into the gepard voices dir, and attaches it BOTH to the
/// character schema AND to the character's base voice profile `emotions` map —
/// so scene `emote` selects it at synthesis with no special casing.
pub(crate) async fn handle_character_design_emotion(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let character_id = extract_str(&args, "character_id")?;
    let emotion = extract_str(&args, "emotion")?;
    let instruct = default_opt_str(&args, "instruct");
    let sample_text = extract_str(&args, "sample_text")?;
    let explicit_language = default_opt_str(&args, "language");
    let seed = args.get("seed").and_then(|v| v.as_i64());
    let max_tokens = default_u32(&args, "max_tokens", 2048);
    let temperature = args
        .get("temperature")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.9);
    let top_k = default_u32(&args, "top_k", 50);

    if emotion.trim().is_empty() || sample_text.trim().is_empty() {
        return Err(ToolError::InvalidArg(
            "character.design_emotion requires 'emotion' and 'sample_text'".into(),
        ));
    }

    // --- Read-only validation (no lock): fetch character + check provider. ---
    let chars = load_characters()?;
    let character = chars.get(&character_id).cloned().ok_or_else(|| {
        ToolError::NotFound(format!(
            "Character '{}' not found — create it first via character.create",
            character_id
        ))
    })?;
    let personality = character
        .get("personality")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Language: explicit arg, else inherit the character's stored language.
    let character_lang = character
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("english")
        .to_string();
    let language = match explicit_language {
        Some(l) if !l.is_empty() => l,
        _ => character_lang,
    };

    // Base profile must exist AND be a voicedesign (or legacy gepard) base —
    // the character workflow is VoiceDesign-direct by design (emotion takes
    // are per-line `instruct` overrides at synthesis time; the take WAV is a
    // design artifact). Attaching takes to a kokoro base would be silently
    // dead (kokoro's synth path never reads emotions) and to an audio8 base
    // would break at synth time (the compound voice is never registered
    // here); character.remove would then also delete a shared profile
    // wholesale.
    let profiles = load_voice_profiles()?;
    let base_provider = profiles
        .get(&character_id)
        .and_then(|p| p.get("provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if base_provider != "voicedesign" && base_provider != "gepard" {
        return Err(ToolError::InvalidArg(format!(
            "character '{}' base voice provider is '{}' — character emotion takes require a voicedesign (Qwen3 VoiceDesign) base. Re-run character.create WITHOUT an explicit 'voice' (designs a voicedesign base) or with a voicedesign voice profile.",
            character_id, if base_provider.is_empty() { "<missing>" } else { base_provider }
        )));
    }

    // Default instruct: personality + the emotion, e.g. "grumpy detective, low
    // gravelly voice — angry delivery, raised voice, clipped words".
    let instruct_text = match instruct {
        Some(i) if !i.trim().is_empty() => i,
        _ => format!("{} — {} delivery", personality, emotion),
    };

    let wav = character_emotion_path(&character_id, &emotion);
    if let Some(parent) = Path::new(&wav).parent() {
        std::fs::create_dir_all(parent)?;
    }
    report_progress(
        0.0,
        100.0,
        &format!("Designing '{}' take for '{}'...", emotion, character_id),
    )
    .await
    .ok();
    let (_dur, _sr) = design_voice_wav(
        &instruct_text,
        &sample_text,
        &wav,
        &language,
        seed,
        Some(max_tokens),
        Some(temperature),
        Some(top_k),
    )?;
    report_progress(100.0, 100.0, "Emotion take designed").await.ok();

    // --- Mutating phase: lock BOTH registries, re-read fresh state, write. ---
    // The WAV is already on disk, so the lock only guards the JSON pointers.
    // Lock order: characters.json first, then voice_profiles.json (no handler
    // ever takes profiles-then-characters, so this cannot deadlock).
    let _lock_chars = RegistryLock::acquire(Path::new(&characters_path()))?;
    let _lock_profiles = RegistryLock::acquire(Path::new(&voice_profiles_path()))?;

    let mut chars = load_characters()?;
    if chars.get(&character_id).is_none() {
        return Err(ToolError::NotFound(format!(
            "Character '{}' not found — create it first via character.create",
            character_id
        )));
    }
    let mut profiles = load_voice_profiles()?;
    // Re-check the provider under the lock (a concurrent character.remove +
    // recreate with a different provider between the read-only validation and
    // here would otherwise attach takes to a non-gepard base).
    let base_provider_locked = profiles
        .get(&character_id)
        .and_then(|p| p.get("provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if base_provider_locked != "voicedesign" && base_provider_locked != "gepard" {
        return Err(ToolError::InvalidArg(format!(
            "character '{}' base voice provider changed to '{}' while designing — character emotion takes require a voicedesign base",
            character_id,
            if base_provider_locked.is_empty() { "<missing>" } else { base_provider_locked }
        )));
    }

    // Attach to the character schema.
    if let Some(obj) = chars.get_mut(&character_id).and_then(|v| v.as_object_mut()) {
        if let Some(emotions) = obj
            .get_mut("emotions")
            .and_then(|v| v.as_object_mut())
        {
            emotions.insert(
                emotion.to_string(),
                json!({
                    "instruct": instruct_text,
                    "sample_text": sample_text,
                    "seed": seed,
                    "ref_audio": wav,
                }),
            );
        }
    }
    save_characters(&chars)?;

    // Attach to the base voice profile's emotions template (the runtime side
    // that script.generate_voices / tts.generate read). For voicedesign bases
    // the router reads the CHARACTER schema's per-emotion `instruct`; the
    // profile entry mirrors it for tooling/registry completeness.
    if let Some(obj) = profiles.get_mut(&character_id).and_then(|v| v.as_object_mut()) {
        if let Some(emotions) = obj
            .get_mut("emotions")
            .and_then(|v| v.as_object_mut())
        {
            emotions.insert(
                emotion.to_string(),
                json!({
                    "ref_audio": wav,
                    "ref_text": sample_text,
                    "cfg_scale": null,
                }),
            );
        }
    }
    save_voice_profiles(&profiles)?;

    Ok(json!({
        "status": "emotion_designed",
        "character_id": character_id,
        "emotion": emotion,
        "ref_audio": wav,
        "sample_rate": 24000,
        "note": format!(
            "Scene emote '{}' on this character now synthesizes with the emotional take. Use tts.generate with emotion=\"{}\" or a script scene with emote=\"{}\".",
            emotion, emotion, emotion
        ),
    }))
}

/// List all defined characters with their designed emotional takes.
pub(crate) async fn handle_character_list(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let _ = args;
    let chars = load_characters()?;
    let char_list = chars
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(key, v)| {
                    let emotions = v
                        .get("emotions")
                        .and_then(|e| e.as_object())
                        .map(|e| {
                            e.iter()
                                .map(|(em, take)| {
                                    json!({
                                        "emotion": em,
                                        "ref_audio": take.get("ref_audio").and_then(|r| r.as_str()).unwrap_or(""),
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    json!({
                        "character_id": key,
                        "name": v.get("name").and_then(|n| n.as_str()).unwrap_or(key),
                        "role": v.get("role").and_then(|r| r.as_str()).unwrap_or(""),
                        "voice": v.get("voice").and_then(|x| x.as_str()).unwrap_or(""),
                        "language": v.get("language").and_then(|x| x.as_str()).unwrap_or(""),
                        "emotions": emotions,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(json!({
        "status": "success",
        "characters": char_list,
        "count": char_list.len(),
    }))
}

/// Remove a character: its schema entry AND its base voice profile (including
/// emotion takes in the profile's emotions map). WAV files are left on disk
/// (regenerable artifacts).
pub(crate) async fn handle_character_remove(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let character_id = extract_str(&args, "character_id")?;
    // Serialize registry mutations across processes (see RegistryLock). Both
    // registries are written below; take characters.json first, then profiles.
    let _lock_chars = RegistryLock::acquire(Path::new(&characters_path()))?;
    let _lock_profiles = RegistryLock::acquire(Path::new(&voice_profiles_path()))?;
    let mut chars = load_characters()?;
    let existed = chars
        .as_object_mut()
        .map(|obj| obj.remove(character_id).is_some())
        .unwrap_or(false);
    if !existed {
        return Err(ToolError::NotFound(format!(
            "Character '{}' not found",
            character_id
        )));
    }
    save_characters(&chars)?;

    // Best-effort: drop the base voice profile + its emotion takes.
    let mut removed_profile = false;
    let mut profiles = load_voice_profiles()?;
    if let Some(obj) = profiles.as_object_mut() {
        if obj.remove(character_id).is_some() {
            removed_profile = true;
        }
    }
    save_voice_profiles(&profiles)?;

    Ok(json!({
        "status": "character_removed",
        "character_id": character_id,
        "voice_profile_removed": removed_profile,
        "note": "WAV artifacts left on disk (regenerable via character.create / character.design_emotion).",
    }))
}
