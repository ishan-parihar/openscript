// ---------------------------------------------------------------------------
// tools_audio — Audio handlers (voice profiles, tts, sfx, music, voiceover)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

pub(crate) async fn handle_voice_profile_add(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let profile_id = extract_str(&args, "profile_id")?;
    let ref_audio = extract_str(&args, "ref_audio")?;
    let ref_text = extract_str(&args, "ref_text")?;
    let provider = default_str(&args, "provider", "faster-qwen3-tts");
    let mode = default_str(&args, "mode", "clone");
    let model = default_str(&args, "model", "Qwen/Qwen3-TTS-12Hz-0.6B-Base");
    let language = default_str(&args, "language", "English");
    let description = default_opt_str(&args, "description");
    // Speaker gender metadata (male/female/nonbinary/auto). Drives the
    // content-format alternation strategy — scripts can resolve this field
    // for voicedesign/clone profiles that have no Kokoro-prefix hint.
    let gender = default_str(&args, "gender", "auto");

    // Emotion-template map: {emotion_id -> {ref_audio, ref_text, cfg_scale?}}.
    // Each entry is a SEPARATE reference recording of the same speaker
    // delivering that emotion; scene `emote` / tts.generate `emotion` then
    // selects the matching take at synthesis. Entries without ref_audio are
    // dropped with a warning (they'd silently fail at synth time).
    let mut emotions_map = serde_json::Map::new();
    let mut emotion_warnings: Vec<String> = Vec::new();
    if let Some(emotions_val) = args.get("emotions") {
        if let Some(emotions_obj) = emotions_val.as_object() {
            for (emotion_id, take_val) in emotions_obj {
                let take_obj = take_val.as_object();
                let take_ref = take_obj
                    .and_then(|o| o.get("ref_audio"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if take_ref.is_empty() {
                    emotion_warnings.push(format!(
                        "emotion '{}' skipped: missing ref_audio",
                        emotion_id
                    ));
                    continue;
                }
                let mut take = take_val.clone();
                // Normalize: ensure cfg_scale is present as null when omitted.
                if take.get("cfg_scale").is_none() {
                    take["cfg_scale"] = serde_json::Value::Null;
                }
                emotions_map.insert(emotion_id.clone(), take);
            }
        } else {
            emotion_warnings.push("emotions must be an object map {emotion_id: {ref_audio, ref_text}}".into());
        }
    }

    // Serialize registry mutations across processes (see RegistryLock) —
    // voice.profile.add races with character.design_emotion on this file.
    let _lock = RegistryLock::acquire(Path::new(&voice_profiles_path()))?;
    let mut profiles = load_voice_profiles()?;
    let obj = json!({
        "profile_id": profile_id,
        "ref_audio": ref_audio,
        "ref_text": ref_text,
        "provider": provider,
        "mode": mode,
        "model": model,
        "language": language,
        "description": description,
        "gender": gender,
        "emotions": serde_json::Value::Object(emotions_map.clone()),
    });
    profiles[profile_id] = obj;
    save_voice_profiles(&profiles)?;

    // Audio8 (zero-shot cloning): register the reference voice with the
    // sidecar so synthesis can use it. Registration failure is NOT fatal —
    // the profile is saved and can be re-registered later (e.g. via
    // voice.profile.add with the same id + overwrite).
    let mut registered_audio8 = false;
    let mut audio8_warning: Option<String> = None;
    if provider == "audio8" {
        if ref_audio.is_empty() || ref_text.is_empty() {
            audio8_warning = Some(
                "audio8 profile needs ref_audio + ref_text for voice cloning; \
                 registration skipped until both are provided."
                    .into(),
            );
        } else {
            match openscript_tts::audio8::audio8_register(&profile_id, &ref_audio, &ref_text) {
                Ok(()) => registered_audio8 = true,
                Err(e) => {
                    audio8_warning = Some(format!(
                        "audio8 voice registration failed (profile saved; retry later): {}",
                        e
                    ));
                }
            }
        }
    }

    // Gepard (high-quality native-English zero-shot cloning): register the
    // reference WAV with the gepard sidecar. ref_text is metadata only
    // (Gepard's Q-Former cloning needs audio, not a transcript). Emotion
    // takes need NO registration — the router passes each take's ref_audio
    // as a per-request override.
    let mut registered_gepard = false;
    let mut gepard_warning: Option<String> = None;
    if provider == "gepard" {
        if ref_audio.is_empty() {
            gepard_warning = Some(
                "gepard profile needs ref_audio for voice cloning; \
                 registration skipped until it is provided."
                    .into(),
            );
        } else {
            match openscript_tts::gepard::gepard_register(&profile_id, &ref_audio, &ref_text) {
                Ok(()) => registered_gepard = true,
                Err(e) => {
                    gepard_warning = Some(format!(
                        "gepard voice registration failed (profile saved; retry later): {}",
                        e
                    ));
                }
            }
        }
    }

    // Higgs Audio v3 (expressive zero-shot cloning): register the reference
    // WAV + transcript with the higgs sidecar. Like gepard, registration
    // failure is NOT fatal — the profile is saved and can be re-registered.
    let mut registered_higgs = false;
    let mut higgs_warning: Option<String> = None;
    if provider == "higgs" {
        if ref_audio.is_empty() || ref_text.is_empty() {
            higgs_warning = Some(
                "higgs profile needs ref_audio + ref_text for voice cloning; \
                 registration skipped until both are provided."
                    .into(),
            );
        } else {
            match openscript_tts::higgs::higgs_register(&profile_id, &ref_audio, &ref_text) {
                Ok(()) => registered_higgs = true,
                Err(e) => {
                    higgs_warning = Some(format!(
                        "higgs voice registration failed (profile saved; retry later): {}",
                        e
                    ));
                }
            }
        }
    }

    // Audio8 emotion takes: register each as a compound voice `{id}@{emotion}`
    // so the router can select it at synth time (audio8 conditions on the
    // reference at registration — a raw ref override is not supported).
    let mut audio8_emotions_registered: Vec<String> = Vec::new();
    if provider == "audio8" && !emotions_map.is_empty() {
        for (emotion_id, take_val) in &emotions_map {
            let take_ref = take_val
                .get("ref_audio")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let take_text = take_val
                .get("ref_text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if take_ref.is_empty() || take_text.is_empty() {
                continue;
            }
            let compound = format!("{}@{}", profile_id, emotion_id);
            match openscript_tts::audio8::audio8_register(&compound, take_ref, take_text) {
                Ok(()) => audio8_emotions_registered.push(compound),
                Err(e) => {
                    emotion_warnings.push(format!(
                        "audio8 emotion '{}' registration failed ({}); retry later",
                        emotion_id, e
                    ));
                }
            }
        }
    }

    Ok(json!({
        "status": "profile_added",
        "profile_id": profile_id,
        "audio8_registered": registered_audio8,
        "audio8_warning": audio8_warning,
        "gepard_registered": registered_gepard,
        "gepard_warning": gepard_warning,
        "higgs_registered": registered_higgs,
        "higgs_warning": higgs_warning,
        "emotions_count": emotions_map.len(),
        "emotions_registered_audio8": audio8_emotions_registered,
        "emotion_warnings": emotion_warnings,
    }))
}

pub(crate) async fn handle_voice_profile_list(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let _ = args;
    let profiles = load_voice_profiles()?;
    let profile_list = profiles
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(key, v)| {
                    let profile_id = v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .or_else(|| v.get("profile_id").and_then(|x| x.as_str()))
                        .unwrap_or(key);
                    json!({
                        "profile_id": profile_id,
                        "provider": v.get("provider").and_then(|x| x.as_str()).unwrap_or(""),
                        "language": v.get("language").and_then(|x| x.as_str()).unwrap_or(""),
                        "gender": v.get("gender").and_then(|x| x.as_str()).unwrap_or("auto"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(json!({
        "status": "success",
        "profiles": profile_list,
        "count": profile_list.len(),
    }))
}

pub(crate) async fn handle_voice_profile_remove(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let profile_id = extract_str(&args, "profile_id")?;
    // Serialize registry mutations across processes (see RegistryLock).
    let _lock = RegistryLock::acquire(Path::new(&voice_profiles_path()))?;
    let mut profiles = load_voice_profiles()?;
    let existed = profiles
        .as_object_mut()
        .map(|obj| obj.remove(profile_id).is_some())
        .unwrap_or(false);

    if existed {
        save_voice_profiles(&profiles)?;
        Ok(json!({
            "status": "profile_removed",
            "profile_id": profile_id,
        }))
    } else {
        Err(ToolError::NotFound(format!(
            "Voice profile not found: {}",
            profile_id
        )))
    }
}

/// Handle voice.design: design a NOVEL character voice from a natural-language
/// description (Qwen3-TTS-1.7B-VoiceDesign, ONNX int4 — no reference audio).
/// Optionally auto-registers the designed voice as a reusable `voicedesign`
/// profile when `profile_id` is given, so the character voice can be used via
/// tts.generate or script speakers — scene lines then synthesize DIRECTLY on
/// the Qwen3 VoiceDesign model (personality + per-line emotion/tone instruct),
/// never through a cloning engine.
pub(crate) async fn handle_voice_design(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let instruct = extract_str(&args, "instruct")?;
    let text = extract_str(&args, "text")?;
    let language = default_str(&args, "language", "english");
    let profile_id = default_opt_str(&args, "profile_id");
    // Explicit gender metadata for the designed voice (default "auto" =
    // infer from the instruct free-text at parse time).
    let gender = default_str(&args, "gender", "auto");
    let seed = args.get("seed").and_then(|v| v.as_i64());
    let max_tokens = default_u32(&args, "max_tokens", 2048);
    let temperature = args
        .get("temperature")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.9);
    let top_k = default_u32(&args, "top_k", 50);

    if instruct.trim().is_empty() {
        return Err(ToolError::InvalidArg(
            "voice.design requires a non-empty 'instruct' voice description".into(),
        ));
    }
    if text.trim().is_empty() {
        return Err(ToolError::InvalidArg(
            "voice.design requires a non-empty 'text' sample line".into(),
        ));
    }

    // Output path: explicit, or a timestamped default under artifacts/voices.
    let output_path = match default_opt_str(&args, "output_path") {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
            format!("artifacts/voices/designed_{}.wav", ts)
        }
    };
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ToolError::Io(std::io::Error::new(e.kind(), format!("mkdir {}: {}", parent.display(), e))))?;
    }

    report_progress(0.0, 100.0, "Designing voice...").await.ok();
    let (duration_ms, sample_rate, _written) = openscript_tts::voicedesign::voicedesign_design(
        &instruct,
        &text,
        &output_path,
        &language,
        seed,
        Some(max_tokens),
        Some(temperature),
        Some(top_k),
    )
    .map_err(|e| ToolError::Tts(e))?;
    report_progress(100.0, 100.0, "Voice designed").await.ok();

    // Optional: auto-register the designed voice as a reusable voicedesign
    // profile. The profile entry routes tts.generate / script speakers to
    // DIRECT Qwen3 VoiceDesign synthesis (personality stored in description is
    // the base instruct); the WAV is a design artifact, not a clone reference.
    let mut registered_profile: Option<String> = None;
    if let Some(pid) = profile_id {
        // Serialize registry mutations across processes (see RegistryLock).
        let _lock = RegistryLock::acquire(Path::new(&voice_profiles_path()))?;
        let mut profiles = load_voice_profiles()?;
        let obj = json!({
            "profile_id": pid,
            "ref_audio": output_path,
            "ref_text": text,
            "provider": "voicedesign",
            "mode": "design",
            "model": "Qwen3-TTS-12Hz-1.7B-VoiceDesign",
            "language": language,
            "description": format!("voice.design persona: {}", instruct),
            "gender": gender,
        });
        profiles[pid.clone()] = obj;
        save_voice_profiles(&profiles)?;
        registered_profile = Some(pid.clone());
    }

    Ok(json!({
        "status": "designed",
        "output_path": output_path,
        "duration_ms": duration_ms,
        "sample_rate": sample_rate,
        "language": language,
        "profile_id": registered_profile,
        "engine": "qwen3-tts-1.7b-voicedesign-onnx-int4",
        "note": "Reuse the designed voice by setting a script speaker's voice to this profile and tts.backend='voicedesign' — scene lines synthesize DIRECTLY with Qwen3 VoiceDesign (no cloning).",
    }))
}

pub(crate) async fn handle_tts_generate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_tts::profiles::VoiceProfileRegistry;
    use std::path::Path;

    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let text = extract_str(&args, "text")?;
    let output_path = extract_str(&args, "output_path")?;
    let speed = default_f64(&args, "speed", 1.0);
    let pitch = default_f64(&args, "pitch", 1.0);
    let volume = default_f64(&args, "volume", 1.0);
    let format = default_str(&args, "format", "wav");
    // Emotion: selects the profile's emotion-take (tonality template) when
    // one is registered, e.g. "angry", "whisper", "excited".
    let emotion = default_opt_str(&args, "emotion");

    report_progress(0.0, 100.0, "Generating speech...")
        .await
        .ok();

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:")
        && !voice_profile_id.starts_with("faster-qwen")
        && !voice_profile_id.starts_with("audio8:")
        && !voice_profile_id.starts_with("gepard:")
        && !voice_profile_id.starts_with("voicedesign:")
        && !voice_profile_id.starts_with("higgs:")
    {
        format!("kokoro:{}", voice_profile_id)
    } else {
        voice_profile_id.to_string()
    };
    let profile = registry
        .get(voice_profile_id)
        .or_else(|| registry.get(&normalized_id))
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile '{}' not found. Try '{}' or add via voice.profile.add.", voice_profile_id, normalized_id))
        })?
        .clone();

    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Delegate to the shared provider router (audio8 / kokoro / faster-qwen3-tts).
    // tone: natural-language delivery direction (diagnostic + future engine
    // instruction channel; the emotion take carries tonality today).
    // Expression knobs: optional temperature / top_k / top_p / cfg_scale
    // override the engine defaults (production-grade: 0.7 temp for clones;
    // explicit values here win).
    let tone = default_opt_str(&args, "tone");
    let temperature = args.get("temperature").and_then(|v| v.as_f64());
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).map(|v| v as u32);
    let top_p = args.get("top_p").and_then(|v| v.as_f64());
    let cfg_scale = args.get("cfg_scale").and_then(|v| v.as_f64());

    let result = tts_generate_routed(
        &voice_profile_id,
        &text,
        &output_path,
        speed,
        pitch,
        volume,
        &format,
        emotion.as_deref(),
        tone.as_deref(),
        None, // control_tags: tts.generate has no scene control tags
        temperature,
        top_k,
        top_p,
        cfg_scale,
        &profile,
    )
    .await?;

    report_progress(100.0, 100.0, "Speech generated").await.ok();

    Ok(json!({
        "status": "generated",
        "backend": result.backend,
        "output_path": result.output_path,
        "duration_ms": result.duration_ms,
        "cached": result.cached,
    }))
}

pub(crate) async fn handle_tts_estimate_duration(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let text = extract_str(&args, "text")?;
    let speed = default_f64(&args, "speed", 1.0);
    let word_count = text.split_whitespace().count();
    let estimated_ms = ((word_count as f64 / 2.5) * 1000.0 / speed) as i64;

    Ok(json!({
        "status": "estimated",
        "text": text,
        "word_count": word_count,
        "estimated_duration_ms": estimated_ms,
        "speed": speed,
    }))
}

pub(crate) async fn handle_sfx_index(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::sfx::SfxIndex;

    // Prefer portable in-repo pack for cold-start, then env, then large local library.
    let sfx_path = default_opt_str(&args, "sfx_path")
        .or_else(|| std::env::var("OPENSCRIPT_SFX_PATH").ok())
        .unwrap_or_else(|| {
            let pack = resolve_repo_path("mcp/assets/sfx_pack");
            if pack.is_dir() {
                return pack.to_string_lossy().into_owned();
            }
            if let Ok(h) = std::env::var("HOME") {
                let local = format!("{}/Videos/Assets/SFX", h);
                if std::path::Path::new(&local).is_dir() {
                    return local;
                }
            }
            "mcp/assets/sfx_pack".to_string()
        });
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| "mcp/assets/sfx_index.json".to_string());

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    report_progress(0.0, 100.0, "Scanning SFX directory...")
        .await
        .ok();

    let index = SfxIndex::scan_directory(&sfx_path).map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(80.0, 100.0, "Saving index...").await.ok();

    index
        .save(&output_path)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(100.0, 100.0, "SFX index complete")
        .await
        .ok();

    Ok(json!({
        "status": "indexed",
        "output_path": output_path,
        "count": index.len(),
        "sfx_path": sfx_path,
    }))
}

pub(crate) async fn handle_sfx_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::sfx::SfxIndex;

    let query = default_str(&args, "query", "");
    let editorial_role = default_opt_str(&args, "editorial_role");
    let category = default_opt_str(&args, "category");
    let limit = default_u32(&args, "limit", 10) as usize;

    let index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
        .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());

    let index = SfxIndex::load(Some(&index_path)).map_err(|e| ToolError::Asset(e.to_string()))?;

    let results = index.search(
        &query,
        editorial_role.as_deref(),
        category.as_deref(),
        limit,
    );

    let result_json: Vec<serde_json::Value> = results
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "filename": s.filename,
                "path": s.path,
                "category": s.category,
                "editorial_role": s.editorial_role,
                "duration_ms": s.duration_ms,
                "recommended_gain_db": s.recommended_gain_db,
                "recommended_use": s.recommended_use,
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "results": result_json,
        "count": result_json.len(),
    }))
}

pub(crate) async fn handle_sfx_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::sfx::SfxIndex;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let editorial_role = extract_str(&args, "editorial_role")?;
    let query = default_str(&args, "query", "");
    let position_ms = default_i64(&args, "position_ms", 0);
    let gain_db = default_f64(&args, "gain_db", -10.0);
    // Expose fade_in_ms / fade_out_ms as parameters (were hardcoded to 50).
    let fade_in_ms = default_u32(&args, "fade_in_ms", 50);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 50);

    // P1-1 fix: map "hook" -> "intro". The SFX index uses "intro" for opening
    // effects, but the tool documentation and `` refer to the
    // opening slot as "hook". Without this mapping, `sfx.assign(editorial_role="hook")`
    // returns 0 results even though perfectly suitable "intro" SFX exist.
    let mapped_role = if editorial_role == "hook" {
        "intro"
    } else {
        editorial_role
    };

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("sfx_{:03}", track_count(&timeline, &TrackType::Sfx) + 1);

    let index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
        .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());
    // Capture the full matched asset (not just the path) so we can read its
    // actual duration_ms instead of hardcoding 1000.
    let sfx_index = SfxIndex::load(Some(&index_path)).ok();
    let matched_asset: Option<openscript_assets::sfx::SfxAsset> = sfx_index
        .as_ref()
        .and_then(|idx| {
            idx.search(&query, Some(mapped_role), None, 1)
                .into_iter()
                .next()
                .cloned()
        });
    let sfx_path = matched_asset.as_ref().map(|a| a.path.clone());
    // Fix: read the actual duration from the matched asset. Prior versions
    // hardcoded 1000ms, so a 3.3s SFX was reported as 1s on the timeline
    // and the render could cut it short.
    let actual_duration_ms = matched_asset
        .as_ref()
        .map(|a| a.duration_ms)
        .unwrap_or(1000);

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: sfx_path.clone().unwrap_or_else(|| query.clone()),
        start_ms: position_ms,
        end_ms: position_ms + actual_duration_ms,
        offset_ms: 0,
        gain_db,
        fade_in_ms,
        fade_out_ms,
        tags: vec![editorial_role.to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "sfx.assign".into(),
            editorial_role: Some(editorial_role.to_string()),
            concept: None,
        }),
        kind: openscript_core::timeline::EventKind::Sfx {
            editorial_role: editorial_role.to_string(),
            category: query.to_string(),
            subcategory: String::new(),
            duration_ms: actual_duration_ms,
            sample_rate: 44100,
            peak_db: 0.0,
            loudness_lufs: -14.0,
            recommended_gain_db: gain_db,
            recommended_use: "single_hit".into(),
            safe_overlay: true,
        },
    };

    timeline.add_track_event(TrackType::Sfx, event);
    if let Some(ref path) = sfx_path {
        timeline.add_asset("sfx", event_id.clone(), json!({"path": path}));
    } else {
        timeline.add_asset("sfx", event_id.clone(), json!({"query": query}));
    }
    timeline.save(timeline_path)?;

    // P1-4 fix: return status "warning" (not "assigned") when no asset matched,
    // plus an explicit `matched` flag and a human-readable message. Prior
    // versions returned "assigned" with asset_path:null, which led agents to
    // believe the operation succeeded.
    let (status, matched, message) = if sfx_path.is_some() {
        (
            "assigned",
            true,
            format!(
                "SFX assigned for role '{}' at {} ms",
                editorial_role, position_ms
            ),
        )
    } else {
        (
            "warning",
            false,
            format!(
                "No SFX found for role '{}' (mapped to '{}'). Placeholder event created at {} ms — render will skip this event. Try sfx.search to inspect available assets.",
                editorial_role, mapped_role, position_ms
            ),
        )
    };

    Ok(json!({
        "status": status,
        "matched": matched,
        "message": message,
        "event_id": event_id,            "position_ms": position_ms,
            "timeline_path": timeline_path,
            "asset_path": sfx_path,
        }))
    }

pub(crate) async fn handle_music_index(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::music::MusicIndex;

    let music_paths = default_opt_arr(&args, "music_paths");
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| "mcp/assets/music_index.json".to_string());

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Prefer committed stock dir so `music.index` does not silently overwrite
    // mcp/assets/music_index.json with an empty/unrelated ~/Videos/Assets/Music scan.
    let stock_music = "mcp/assets/music".to_string();
    let home_music = std::env::var("HOME")
        .ok()
        .map(|h| format!("{}/Videos/Assets/Music", h));
    let env_music = std::env::var("OPENSCRIPT_MUSIC_PATH").ok();
    let default_path = env_music
        .or_else(|| {
            if std::path::Path::new(&stock_music).is_dir() {
                Some(stock_music.clone())
            } else {
                home_music
            }
        })
        .unwrap_or(stock_music);
    let default_paths = vec![default_path];
    let paths = music_paths.as_deref().unwrap_or(&default_paths);

    report_progress(0.0, 100.0, "Scanning music directories...")
        .await
        .ok();

    let index = MusicIndex::scan_directories(paths).map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(80.0, 100.0, "Saving index...").await.ok();

    index
        .save(&output_path)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(100.0, 100.0, "Music index complete")
        .await
        .ok();

    Ok(json!({
        "status": "indexed",
        "output_path": output_path,
        "count": index.len(),
    }))
}

pub(crate) async fn handle_music_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let index_path = default_opt_str(&args, "index_path")
        .unwrap_or_else(|| "mcp/assets/music_index.json".to_string());
    let query = default_opt_str(&args, "query");
    let mood_filter = default_opt_str(&args, "mood");
    let energy_filter = default_opt_str(&args, "energy");
    let limit = default_u32(&args, "limit", 10) as usize;

    if !Path::new(&index_path).exists() {
        return Ok(json!({
            "status": "warning",
            "message": format!("Music index not found at {}. Run music.index first.", index_path),
            "tracks": [],
        }));
    }

    let raw = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&raw)?;

    let assets = index.get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let query_lower = query.as_deref().unwrap_or("").to_lowercase();

    let mut matched: Vec<serde_json::Value> = assets.into_iter().filter(|a| {
        // Filter by mood
        if let Some(ref m) = mood_filter {
            let asset_mood = a.get("mood").and_then(|v| v.as_str()).unwrap_or("");
            if !asset_mood.eq_ignore_ascii_case(m) {
                return false;
            }
        }
        // Filter by energy
        if let Some(ref e) = energy_filter {
            let asset_energy = a.get("energy").and_then(|v| v.as_str()).unwrap_or("");
            if !asset_energy.eq_ignore_ascii_case(e) {
                return false;
            }
        }
        // Filter by query (match against title, tags, genre)
        if !query_lower.is_empty() {
            let title = a.get("title").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let genre = a.get("genre").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let tags = a.get("tags").and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(" ").to_lowercase())
                .unwrap_or_default();
            if !title.contains(&query_lower) && !genre.contains(&query_lower) && !tags.contains(&query_lower) {
                return false;
            }
        }
        // Verify the file actually exists on disk
        if let Some(p) = a.get("path").and_then(|v| v.as_str()) {
            Path::new(p).exists()
        } else {
            false
        }
    }).collect();

    matched.truncate(limit);

    Ok(json!({
        "status": "success",
        "count": matched.len(),
        "tracks": matched,
    }))
}

pub(crate) async fn handle_music_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let music_path = extract_str(&args, "path")?;
    let mood = default_str(&args, "mood", "neutral");
    let energy = default_str(&args, "energy", "medium");
    let start_ms = default_i64(&args, "start_ms", 0);
    let end_ms = default_opt_i64(&args, "end_ms");
    let gain_db = default_f64(&args, "gain_db", -12.0);
    let ducking = default_bool(&args, "ducking", true);
    // Expose fade_in_ms / fade_out_ms as parameters (were hardcoded to 500).
    let fade_in_ms = default_u32(&args, "fade_in_ms", 500);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 500);

    // Validate the music file exists
    if !Path::new(music_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Music file not found: {}. Use library.search to find tracks. Accepts both local paths and URLs..",
            music_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;

    let total_ms = timeline.total_duration_ms();
    let end = end_ms.unwrap_or(total_ms);
    let event_id = format!("music_{:03}", track_count(&timeline, &TrackType::Music) + 1);

    // P2-2 fix: only add a ducking directive when speech tracks actually
    // exist on the timeline. Prior versions added a "dialogue_active"
    // directive unconditionally, which would attenuate the music even when
    // there was no dialogue to duck against — silently producing a quieter
    // mix than the user intended for music-only videos.
    if ducking {
        let has_speech = track_count(&timeline, &TrackType::Dialogue) > 0
            || track_count(&timeline, &TrackType::Voiceover) > 0;
        if has_speech {
            timeline.add_ducking_directive("dialogue_active", "music", 10.0, 50, 200);
        }
    }

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: event_id.clone(),
        start_ms,
        end_ms: end,
        offset_ms: 0,
        gain_db,
        fade_in_ms,
        fade_out_ms,
        tags: vec![mood.clone(), energy.clone()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "music.assign".into(),
            editorial_role: None,
            concept: None,
        }),
        kind: openscript_core::timeline::EventKind::Music {
            mood,
            energy,
            bpm: None,
            loopability: true,
            intro_friendly: true,
            cta_friendly: true,
            loudness_target_lufs: -14.0,
            loop_mode: "loop".into(),
            ducking_policy: if ducking { "auto" } else { "none" }.into(),
        },
    };

    // Register the music asset path so render_from_timeline can find it
    timeline.add_asset("music", event_id.clone(), json!({"path": music_path}));

    timeline.add_track_event(TrackType::Music, event);
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "assigned",
        "event_id": event_id,
        "asset_path": music_path,
        "start_ms": start_ms,
        "end_ms": end,
        "timeline_path": timeline_path,
    }))
}

pub(crate) async fn handle_voiceover_generate(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use openscript_tts::profiles::VoiceProfileRegistry;
    use std::path::Path;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let text = extract_str(&args, "text")?;
    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let position_ms = default_i64(&args, "position_ms", 0);
    let speed = default_f64(&args, "speed", 1.0);
    let gain_db = default_f64(&args, "gain_db", -6.0);
    let pitch = default_f64(&args, "pitch", 1.0);
    let volume = default_f64(&args, "volume", 1.0);
    // Emotion: selects the profile's emotion-take when registered.
    let emotion = default_opt_str(&args, "emotion");

    let mut timeline = Timeline::load(timeline_path)?;

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:")
        && !voice_profile_id.starts_with("faster-qwen")
        && !voice_profile_id.starts_with("audio8:")
        && !voice_profile_id.starts_with("gepard:")
        && !voice_profile_id.starts_with("voicedesign:")
        && !voice_profile_id.starts_with("higgs:")
    {
        format!("kokoro:{}", voice_profile_id)
    } else {
        voice_profile_id.to_string()
    };
    let profile = registry
        .get(voice_profile_id)
        .or_else(|| registry.get(&normalized_id))
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile '{}' not found. Try '{}' or add via voice.profile.add.", voice_profile_id, normalized_id))
        })?
        .clone();

    let timeline_dir = Path::new(&timeline_path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let event_id = format!(
        "voiceover_{:03}",
        track_count(&timeline, &TrackType::Voiceover) + 1
    );
    let output_path = timeline_dir
        .join(format!("voiceover_{}.wav", event_id))
        .to_string_lossy()
        .to_string();

    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    report_progress(0.0, 100.0, "Generating voiceover...")
        .await
        .ok();

    let result = tts_generate_routed(
        voice_profile_id,
        text,
        &output_path,
        speed,
        pitch,
        volume,
        "wav",
        emotion.as_deref(),
        None, // tone: voiceover.generate has no scene tone
        None, // control_tags: voiceover.generate has no scene control tags
        None, // temperature: engine default (expressive 0.7)
        None, // top_k
        None, // top_p
        None, // cfg_scale
        &profile,
    )
    .await?;

    let duration_ms = result.duration_ms;

    timeline.add_asset(
        "voices",
        event_id.clone(),
        json!({
            "path": output_path.clone(),
            "voice_profile_id": voice_profile_id,
            "text": text,
        }),
    );

    let end_ms = position_ms + duration_ms;
    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: output_path.clone(),
        start_ms: position_ms,
        end_ms,
        offset_ms: 0,
        gain_db,
        fade_in_ms: 50,
        fade_out_ms: 50,
        tags: vec!["voiceover".to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "voiceover.generate".into(),
            editorial_role: None,
            concept: None,
        }),
        kind: openscript_core::timeline::EventKind::Voiceover {
            voice_profile_id: voice_profile_id.to_string(),
            text: text.to_string(),
            estimated_duration_ms: duration_ms,
        },
    };

    timeline.add_track_event(TrackType::Voiceover, event);
    timeline.save(timeline_path)?;

    report_progress(100.0, 100.0, "Voiceover generated")
        .await
        .ok();

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "duration_ms": duration_ms,
        "event_id": event_id,
    }))
}

pub(crate) async fn handle_tts_commentary(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_tts::profiles::VoiceProfileRegistry;
    use std::path::Path;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let commentary_type = extract_str(&args, "commentary_type")?;
    let intro_text = default_opt_str(&args, "intro_text");
    let outro_text = default_opt_str(&args, "outro_text");
    let speed = default_f64(&args, "speed", 1.0);

    let mut timeline = Timeline::load(timeline_path)?;
    let total_ms = timeline.total_duration_ms();

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:")
        && !voice_profile_id.starts_with("faster-qwen")
        && !voice_profile_id.starts_with("audio8:")
        && !voice_profile_id.starts_with("gepard:")
        && !voice_profile_id.starts_with("voicedesign:")
        && !voice_profile_id.starts_with("higgs:")
    {
        format!("kokoro:{}", voice_profile_id)
    } else {
        voice_profile_id.to_string()
    };
    let profile = registry
        .get(voice_profile_id)
        .or_else(|| registry.get(&normalized_id))
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile '{}' not found. Try '{}' or add via voice.profile.add.", voice_profile_id, normalized_id))
        })?
        .clone();

    let timeline_dir = Path::new(&timeline_path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let do_intro = commentary_type == "intro" || commentary_type == "all";
    let do_outro = commentary_type == "outro" || commentary_type == "all";
    let do_transitions = commentary_type == "transitions" || commentary_type == "all";

    let mut generated = Vec::new();
    let mut positions = Vec::new();

    if do_intro {
        let text = intro_text.unwrap_or_else(|| "Welcome to this video.".to_string());
        let (event_id, _dur) = generate_commentary_segment(
            &mut timeline,
            &timeline_dir,
            voice_profile_id,
            &text,
            0,
            "intro",
            speed,
            &profile,
        )
        .await?;
        generated.push(event_id);
        positions.push(0);
    }

    if do_transitions {
        let segments = timeline.segments.clone();
        let total_segs = segments.len();
        for (i, seg) in segments.iter().enumerate() {
            // Report progress per voiceover to prevent client timeouts
            report_progress(
                (i as f64 / total_segs.max(1) as f64) * 100.0,
                100.0,
                &format!("Voiceover {}/{}", i + 1, total_segs),
            )
            .await
            .ok();

            let seg_start_ms = (seg.start * 1000.0) as i64;
            if seg_start_ms <= 0 {
                continue;
            }
            let concept = seg
                .caption
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            let text = format!("Now, let's look at {}.", concept);
            let (event_id, _dur) = generate_commentary_segment(
                &mut timeline,
                &timeline_dir,
                voice_profile_id,
                &text,
                seg_start_ms,
                "transition",
                speed,
                &profile,
            )
            .await?;
            generated.push(event_id);
            positions.push(seg_start_ms);
        }
    }

    if do_outro {
        let text = outro_text.unwrap_or_else(|| "Thanks for watching!".to_string());
        let (event_id, _dur) = generate_commentary_segment(
            &mut timeline,
            &timeline_dir,
            voice_profile_id,
            &text,
            total_ms,
            "outro",
            speed,
            &profile,
        )
        .await?;
        generated.push(event_id);
        positions.push(total_ms);
    }

    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "generated",
        "voiceovers_generated": generated,
        "positions": positions,
        "count": generated.len(),
    }))
}

pub(crate) async fn handle_tts_preview(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let text = extract_str(&args, "text")?;
    let speed = default_f64(&args, "speed", 1.0);

    let profiles = load_voice_profiles()?;
    let profile = profiles.get(voice_profile_id).cloned();

    let word_count = text.split_whitespace().count();
    let estimated_ms = ((word_count as f64 / 2.5) * 1000.0 / speed) as i64;

    Ok(json!({
        "status": "preview",
        "voice_profile_id": voice_profile_id,
        "voice_profile": profile,
        "text": text,
        "word_count": word_count,
        "estimated_duration_ms": estimated_ms,
        "speed": speed,
    }))
}

pub(crate) async fn handle_music_ducking_plan(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let reduction_db = default_f64(&args, "reduction_db", 10.0);

    let timeline = Timeline::load(timeline_path)?;

    let mut ducking_events = Vec::new();
    let dialogue = timeline
        .tracks
        .get(&TrackType::Dialogue)
        .cloned()
        .unwrap_or_default();
    let voiceover = timeline
        .tracks
        .get(&TrackType::Voiceover)
        .cloned()
        .unwrap_or_default();

    for event in dialogue.iter().chain(voiceover.iter()) {
        ducking_events.push(json!({
            "start_ms": event.start_ms,
            "end_ms": event.end_ms,
            "reduction_db": reduction_db,
            "attack_ms": 50,
            "release_ms": 200,
        }));
    }

    Ok(json!({
        "status": "success",
        "timeline_path": timeline_path,
        "reduction_db": reduction_db,
        "ducking_events": ducking_events,
        "count": ducking_events.len(),
    }))
}

/// List all available TTS voices: registered profiles from voices.json plus
/// the full list of Kokoro preset voice IDs. Agents use this to discover
/// available voices before generating TTS.
pub(crate) async fn handle_voices_list(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let language_filter = default_opt_str(&args, "language");

    // Load registered profiles from voices.json
    let voices_path = std::env::var("OPENSCRIPT_VOICES_PATH")
        .unwrap_or_else(|_| "mcp/assets/voices.json".to_string());

    let mut registered: Vec<serde_json::Value> = Vec::new();
    if let Ok(content) = std::fs::read_to_string(&voices_path) {
        if let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content) {
            for (id, val) in &map {
                let lang = val.get("language").and_then(|v| v.as_str()).unwrap_or("en");
                if let Some(ref filter) = language_filter {
                    if lang != filter {
                        continue;
                    }
                }
                registered.push(json!({
                    "id": id,
                    "provider": val.get("provider").and_then(|v| v.as_str()).unwrap_or("kokoro"),
                    "language": lang,
                    "description": val.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    "mode": val.get("mode").and_then(|v| v.as_str()).unwrap_or("preset"),
                }));
            }
        }
    }

    // Kokoro preset voice IDs (from the Kokoro v1.0 model).
    // These can be used directly with script.generate_voices / tts.generate
    // without registration in voices.json.
    let kokoro_presets = [
        // American English
        ("af_heart", "en", "warm American female"),
        ("af_bella", "en", "soft American female"),
        ("af_nicole", "en", "American female"),
        ("af_sky", "en", "American female, bright"),
        ("am_michael", "en", "American male"),
        ("am_adam", "en", "American male"),
        ("am_eric", "en", "American male, deep"),
        // British English
        ("bf_emma", "en", "British female"),
        ("bf_isabella", "en", "British female, warm"),
        ("bm_george", "en", "British male"),
        ("bm_lewis", "en", "British male, young"),
        // Spanish
        ("ef_dora", "es", "Spanish female"),
        ("em_alex", "es", "Spanish male"),
        // French
        ("ff_evelyne", "fr", "French female"),
        ("fm_pierre", "fr", "French male"),
        // Hindi
        ("hf_alpha", "hi", "Hindi female"),
        ("hf_beta", "hi", "Hindi female, warm"),
        ("hm_omega", "hi", "Hindi male"),
        ("hm_psi", "hi", "Hindi male, deep"),
        // Italian
        ("if_sara", "it", "Italian female"),
        ("im_nicola", "it", "Italian male"),
        // Japanese
        ("jf_alpha", "ja", "Japanese female"),
        ("jf_gongitsune", "ja", "Japanese female, character"),
        ("jf_nezumi", "ja", "Japanese female, mouse"),
        ("jf_tebukuro", "ja", "Japanese female, warm"),
        ("jf_tomoko", "ja", "Japanese female, neutral"),
        ("jm_kumo", "ja", "Japanese male"),
        // Portuguese (Brazilian)
        ("pf_dora", "pt", "Portuguese female"),
        ("pm_alex", "pt", "Portuguese male"),
        // Chinese (Mandarin)
        ("zf_xiaobei", "zh", "Chinese female, Beijing"),
        ("zf_xiaoni", "zh", "Chinese female, neutral"),
        ("zf_xiaoxiao", "zh", "Chinese female, bright"),
        ("zf_xiaoyi", "zh", "Chinese female, Yi"),
        ("zm_yunjian", "zh", "Chinese male, Jian"),
        ("zm_yunxi", "zh", "Chinese male, Xi"),
        ("zm_yunxia", "zh", "Chinese male, Xia"),
        ("zm_yunyang", "zh", "Chinese male, Yang"),
    ];

    let mut presets: Vec<serde_json::Value> = Vec::new();
    for (id, lang, desc) in &kokoro_presets {
        if let Some(ref filter) = language_filter {
            if *lang != filter {
                continue;
            }
        }
        presets.push(json!({
            "id": id,
            "provider": "kokoro",
            "language": lang,
            "description": desc,
            "usage": format!("Use '{}' directly as the voice parameter in script.generate_voices or tts.generate", id),
        }));
    }

    Ok(json!({
        "status": "success",
        "registered_profiles": registered,
        "registered_count": registered.len(),
        "kokoro_presets": presets,
        "kokoro_preset_count": presets.len(),
        "total_voices": registered.len() + presets.len(),
        "note": "Kokoro preset IDs (e.g. 'af_heart') can be used directly without registration. Registered profiles in voices.json are named aliases that map to Kokoro presets.",
    }))
}

