// ---------------------------------------------------------------------------
// tools_system — System / meta handlers (llm.complete, system.*, vision.*, help.tool)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

/// Text LLM via OpenCode zen → OpenRouter free models.
pub(crate) async fn handle_llm_complete(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let prompt = extract_str(&args, "prompt")?;
    let system = default_str(
        &args,
        "system",
        "You are a helpful short-form video director assistant.",
    );
    let backend = default_str(&args, "backend", "auto");
    let result = crate::llm::chat_complete_with_backend(&system, prompt, None, &backend)
        .await
        .map_err(|e| ToolError::Asset(format!("LLM cascade failed: {}", e)))?;
    Ok(json!({
        "status": "success",
        "text": result.text,
        "backend": result.backend,
        "model": result.model,
        "backend_requested": backend,
    }))
}

/// Redacted view of ~/.openscript/config.json + env overrides.
pub(crate) async fn handle_system_config_get(
    _args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    // Ensure directory exists so agents always know where to write keys
    let _ = crate::config::ensure_user_config(None);
    crate::config::reload_config();
    Ok(json!({
        "status": "success",
        "config": crate::config::config_public_view(),
    }))
}

/// Deep-merge a patch into ~/.openscript/config.json.
pub(crate) async fn handle_system_config_set(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let patch = args
        .get("patch")
        .cloned()
        .ok_or_else(|| ToolError::MissingArg("patch".into()))?;
    if !patch.is_object() {
        return Err(ToolError::InvalidArg(
            "patch must be a JSON object".into(),
        ));
    }
    let _ = crate::config::ensure_user_config(None);
    crate::config::reload_config();
    let mut cfg = crate::config::config();

    // api_keys
    if let Some(keys) = patch.get("api_keys").and_then(|v| v.as_object()) {
        if let Some(s) = keys.get("pexels").and_then(|v| v.as_str()) {
            cfg.api_keys.pexels = s.to_string();
        }
        if let Some(s) = keys.get("giphy").and_then(|v| v.as_str()) {
            cfg.api_keys.giphy = s.to_string();
        }
        if let Some(s) = keys.get("pixabay").and_then(|v| v.as_str()) {
            cfg.api_keys.pixabay = s.to_string();
        }
        if let Some(s) = keys.get("openrouter").and_then(|v| v.as_str()) {
            cfg.api_keys.openrouter = s.to_string();
        }
        if let Some(s) = keys.get("opencode").and_then(|v| v.as_str()) {
            cfg.api_keys.opencode = s.to_string();
        }
        // legacy aliases inside patch.api_keys
        if let Some(s) = keys.get("openrouter_api_key").and_then(|v| v.as_str()) {
            cfg.api_keys.openrouter = s.to_string();
        }
    }
    // top-level legacy
    if let Some(s) = patch.get("openrouter_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.openrouter = s.to_string();
    }
    if let Some(s) = patch.get("pexels_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.pexels = s.to_string();
    }
    if let Some(s) = patch.get("giphy_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.giphy = s.to_string();
    }
    if let Some(s) = patch.get("pixabay_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.pixabay = s.to_string();
    }

    // llm
    if let Some(llm) = patch.get("llm").and_then(|v| v.as_object()) {
        if let Some(s) = llm.get("openrouter_base_url").and_then(|v| v.as_str()) {
            cfg.llm.openrouter_base_url = s.to_string();
        }
        if let Some(arr) = llm.get("openrouter_models").and_then(|v| v.as_array()) {
            cfg.llm.openrouter_models = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
        }
        if let Some(s) = llm.get("opencode_base_url").and_then(|v| v.as_str()) {
            cfg.llm.opencode_base_url = s.to_string();
        }
        if let Some(s) = llm.get("opencode_model").and_then(|v| v.as_str()) {
            cfg.llm.opencode_model = s.to_string();
        }
    }

    // paths
    if let Some(paths) = patch.get("paths").and_then(|v| v.as_object()) {
        if let Some(s) = paths.get("sfx_path").and_then(|v| v.as_str()) {
            cfg.paths.sfx_path = Some(s.to_string());
        }
        if let Some(s) = paths.get("music_path").and_then(|v| v.as_str()) {
            cfg.paths.music_path = Some(s.to_string());
        }
        if let Some(s) = paths.get("tts_url").and_then(|v| v.as_str()) {
            cfg.paths.tts_url = Some(s.to_string());
        }
        if let Some(s) = paths.get("workspace_root").and_then(|v| v.as_str()) {
            cfg.paths.workspace_root = Some(s.to_string());
        }
    }

    // render
    if let Some(render) = patch.get("render").and_then(|v| v.as_object()) {
        if let Some(s) = render.get("default_aspect").and_then(|v| v.as_str()) {
            cfg.render.default_aspect = s.to_string();
        }
        if let Some(n) = render.get("normalize_lufs").and_then(|v| v.as_f64()) {
            cfg.render.normalize_lufs = n;
        }
    }

    let path = crate::config::write_user_config(&cfg)
        .map_err(|e| ToolError::Io(std::io::Error::other(e)))?;

    Ok(json!({
        "status": "success",
        "written": path.display().to_string(),
        "config": crate::config::config_public_view(),
    }))
}

/// Extract a frame and describe it (OpenRouter multimodal free → local text).
pub(crate) async fn handle_vision_analyze_clip(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    let at_s = args.get("at_s").and_then(|v| v.as_f64());
    let prompt = default_opt_str(&args, "prompt");
    crate::llm::analyze_clip(&video_path, at_s, prompt.as_deref())
        .await
        .map_err(|e| ToolError::Asset(format!("vision.analyze_clip failed: {}", e)))
}

/// Score stock clip relevance vs scene dialogue + keywords.
pub(crate) async fn handle_vision_score_clip(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    let scene_text = extract_str(&args, "scene_text")?;
    let keywords: Vec<String> = args
        .get("video_keywords")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let search_query = default_opt_str(&args, "search_query");
    crate::llm::score_clip_relevance(
        &video_path,
        scene_text,
        &keywords,
        search_query.as_deref(),
    )
    .await
    .map_err(|e| ToolError::Asset(format!("vision.score_clip failed: {}", e)))
}

/// Probe every backend subsystem and report availability. Agents should call
/// this once at the start of a session to know which tools will work.
pub(crate) async fn handle_system_capabilities(
    _args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::music::MusicIndex;
    use openscript_assets::sfx::SfxIndex;

    // Resolve the repo root for CWD-independent path checks.
    // Priority: OPENSCRIPT_ROOT env var > CARGO_MANIFEST_DIR (compile-time) > CWD
    // The fresh-agent UX audit found that system.capabilities returned false
    // negatives when run from the wrong directory because all paths were
    // relative. This helper resolves them to absolute paths.
    let repo_root = std::env::var("OPENSCRIPT_ROOT")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            option_env!("CARGO_MANIFEST_DIR")
                .map(std::path::PathBuf::from)
                .and_then(|d| d.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()))
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let resolve = |rel: &str| -> std::path::PathBuf {
        let p = std::path::Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo_root.join(rel)
        }
    };

    let path_exists = |rel: &str| -> bool {
        let p = resolve(rel);
        p.exists()
    };

    // Pexels API key
    let pexels_available = !pexels_key().is_empty();
    let pexels = json!({
        "available": pexels_available,
        "reason": if pexels_available {
            serde_json::Value::Null
        } else {
            "PEXELS_API_KEY not set. Set it in mcp/assets/.openscript_config.json or as an env var. Get a free key at https://www.pexels.com/api/".into()
        },
    });

    // GIPHY API key
    let giphy_available = !giphy_key().is_empty();
    let giphy = json!({
        "available": giphy_available,
        "reason": if giphy_available {
            serde_json::Value::Null
        } else {
            "GIPHY_API_KEY not set. Set it in mcp/assets/.openscript_config.json or as an env var. Get a key at https://developers.giphy.com/".into()
        },
    });

    // Pixabay API key
    let pixabay_available = !pixabay_key().is_empty();
    let pixabay = json!({
        "available": pixabay_available,
        "reason": if pixabay_available {
            serde_json::Value::Null
        } else {
            "PIXABAY_API_KEY not set. Set it in mcp/assets/.openscript_config.json or as an env var. Optional — only needed for stock.search/stock.fetch.".into()
        },
    });

    // SFX library — resolve path CWD-independently (same fix as music).
    let sfx_index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
        .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());
    let sfx_index_resolved = resolve(&sfx_index_path);
    let sfx_count = SfxIndex::load(Some(&sfx_index_resolved.to_string_lossy()))
        .map(|idx| idx.len())
        .unwrap_or(0);
    let sfx = json!({
        "available": sfx_count > 0,
        "indexed_count": sfx_count,
        "index_path": sfx_index_path,
    });

    // Music library — the committed 20-track stock index at
    // music_library_index.json is the single source of truth (500+ YouTube-scraped
    // copyright-free tracks). music_index.json was deleted (synthetic sine stubs).
    // music_production/ was deleted (synthetic sine stubs).
    let music_library_index = resolve("mcp/assets/music_library_index.json");
    let real_library = music_library_index.exists();
    let music_library_count = if real_library {
        MusicIndex::load(Some(&music_library_index.to_string_lossy()))
            .map(|idx| idx.len())
            .unwrap_or(0)
    } else {
        0
    };
    let music = json!({
        "available": real_library || !pixabay_key().is_empty(),
        "library_count": music_library_count,
        "library_path": "mcp/assets/music_library_index.json",
        "usable_for_production": real_library || !pixabay_key().is_empty(),
        "reason": if real_library {
            serde_json::Value::Null
        } else {
            "Run library.build to populate the music index, or set PIXABAY_API_KEY.".into()
        },
    });

    // Voicebox TTS (qwen3 / faster-tts sidecar at OPENSCRIPT_TTS_URL)
    let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());
    let voicebox_available = probe_http(&format!("{}/models", tts_url)).await;
    let voicebox = json!({
        "available": voicebox_available,
        "url": tts_url,
        "reason": if voicebox_available {
            serde_json::Value::Null
        } else {
            "No voicebox server responding at OPENSCRIPT_TTS_URL. Kokoro (script.generate_voices) is the default TTS and does NOT need voicebox.".into()
        },
    });

    // Kokoro TTS (Python sidecar). Runtime expects:
    //   model:  mcp/assets/kokoro/onnx/kokoro-v1.0.onnx  (or KOKORO_MODEL)
    //   voices: mcp/assets/kokoro/voices/voices-v1.0.bin  (or KOKORO_VOICES)
    //   sidecar script + optional voices.json profile registry
    // Prior bug: only checked sidecar + voices.json and reported a wrong
    // model path (mcp/assets/kokoro-v1.0.onnx), so available=true while
    // script.to_video hard-failed with "Kokoro model not found".
    let kokoro_model = std::env::var("KOKORO_MODEL").unwrap_or_else(|_| {
        "mcp/assets/kokoro/onnx/kokoro-v1.0.onnx".to_string()
    });
    let kokoro_voices_bin = std::env::var("KOKORO_VOICES").unwrap_or_else(|_| {
        "mcp/assets/kokoro/voices/voices-v1.0.bin".to_string()
    });
    let kokoro_profiles =
        std::env::var("KOKORO_PROFILES").unwrap_or_else(|_| "mcp/assets/voices.json".to_string());
    let kokoro_sidecar = std::env::var("KOKORO_SIDECAR")
        .unwrap_or_else(|_| "mcp/scripts/kokoro_tts_sidecar.py".to_string());
    let kokoro_model_ok = path_exists(&kokoro_model);
    let kokoro_voices_ok = path_exists(&kokoro_voices_bin);
    let kokoro_sidecar_ok = path_exists(&kokoro_sidecar);
    // Probe the resolved Python interpreter for kokoro_onnx importability.
    // This catches the common case: assets are on disk but the Python env
    // doesn't have kokoro_onnx installed (conda env mismatch, PEP-668, etc).
    let kokoro_python = std::env::var("KOKORO_PYTHON").unwrap_or_else(|_| {
        // Mirror the priority from kokoro_sidecar::resolve_kokoro_python()
        // inline to avoid importing the whole module in doctor context.
        if let Some(home) = std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(std::path::PathBuf::from) {
            for env_name in &["kokoro-tts", "kokoro"] {
                let candidate = home.join("miniconda3/envs").join(env_name).join("bin/python");
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
        "python3".to_string()
    });
    let kokoro_python_ok = std::process::Command::new(&kokoro_python)
        .arg("-c")
        .arg("import kokoro_onnx")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let kokoro_available = kokoro_model_ok && kokoro_voices_ok && kokoro_sidecar_ok && kokoro_python_ok;
    let kokoro_reason = if kokoro_available {
        serde_json::Value::Null
    } else {
        let mut missing = Vec::new();
        if !kokoro_model_ok {
            missing.push(format!("model ({})", kokoro_model));
        }
        if !kokoro_voices_ok {
            missing.push(format!("voices bin ({})", kokoro_voices_bin));
        }
        if !kokoro_sidecar_ok {
            missing.push(format!("sidecar ({})", kokoro_sidecar));
        }
        if !kokoro_python_ok {
            missing.push(format!(
                "Python module 'kokoro_onnx' not importable via {} — set KOKORO_PYTHON to a Python with kokoro_onnx installed",
                kokoro_python
            ));
        }
        format!(
            "Kokoro incomplete — missing: {}. Run: bash setup.sh (downloads model+voices, installs kokoro-onnx). Or set KOKORO_PYTHON.",
            missing.join(", ")
        )
        .into()
    };
    let kokoro = json!({
        "available": kokoro_available,
        "sidecar_path": kokoro_sidecar,
        "model_path": kokoro_model,
        "voices_path": kokoro_voices_bin,
        "profiles_path": kokoro_profiles,
        "profiles_available": path_exists(&kokoro_profiles),
        "python_path": kokoro_python,
        "python_module_ok": kokoro_python_ok,
        "reason": kokoro_reason,
    });

    // Transcription engine (HinglishGgml — the sole engine)
    let transcription = {
        let result = openscript_transcribe::transcriber::check_hinglish_ggml_health().await;
        match result {
            Ok(_) => json!({
                "available": true,
                "engine": "hinglish-ggml",
                "reason": serde_json::Value::Null,
            }),
            Err(reason) => json!({
                "available": false,
                "engine": "hinglish-ggml",
                "reason": reason,
            }),
        }
    };

    // HyperFrames (default render engine)
    let hf_dir = resolve("hyperframes");
    let hyperframes = json!({
        "available": hf_dir.exists(),
        "path": hf_dir.to_string_lossy(),
    });

    // FFmpeg
    let ffmpeg_available = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ffmpeg = json!({
        "available": ffmpeg_available,
        "reason": if ffmpeg_available {
            serde_json::Value::Null
        } else {
            "ffmpeg binary not found on PATH. Required for all video rendering tools.".into()
        },
    });

    // yt-dlp (required for youtube.search, youtube.download, library.download,
    // and background.fetch YouTube fallback)
    let ytdlp_available = std::process::Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let yt_dlp = json!({
        "available": ytdlp_available,
        "reason": if ytdlp_available {
            serde_json::Value::Null
        } else {
            "yt-dlp binary not found on PATH. Required for youtube.search, youtube.download, library.download, and background.fetch YouTube fallback.".into()
        },
    });

    // Parakeet TDT force-alignment (required for script.build_captions
    // word-level timing). Replaces the old whisper_align.py which depended
    // on the `openai-whisper` Python package. Parakeet TDT runs via
    // `onnxruntime` and the model is at mcp/assets/parakeet/.
    // We check: (1) the script exists, (2) onnxruntime is importable,
    // (3) the encoder/decoder ONNX model files exist.
    let parakeet_script_path = "mcp/scripts/parakeet_align.py";
    let parakeet_script_exists = path_exists(parakeet_script_path);
    let onnxruntime_importable = std::process::Command::new("python3")
        .args(["-c", "import onnxruntime; print('ok')"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let parakeet_encoder = resolve("mcp/assets/parakeet/encoder-model.int8.onnx");
    let parakeet_decoder = resolve("mcp/assets/parakeet/decoder_joint-model.int8.onnx");
    let parakeet_models_exist = parakeet_encoder.exists() && parakeet_decoder.exists();
    let parakeet_align_available = parakeet_script_exists && onnxruntime_importable && parakeet_models_exist;
    let parakeet_align = json!({
        "available": parakeet_align_available,
        "path": parakeet_script_path,
        "script_exists": parakeet_script_exists,
        "onnxruntime_importable": onnxruntime_importable,
        "models_exist": parakeet_models_exist,
        "reason": if parakeet_align_available {
            serde_json::Value::Null
        } else if !parakeet_script_exists {
            "parakeet_align.py not found. script.build_captions will fall back to even-spacing estimation (less accurate word timings).".into()
        } else if !onnxruntime_importable {
            "parakeet_align.py exists but the Python `onnxruntime` module is not installed. Install with: pip3 install --user onnxruntime. script.build_captions will fall back to even-spacing estimation.".into()
        } else {
            "Parakeet ONNX model files not found at mcp/assets/parakeet/. Download from https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx. script.build_captions will fall back to even-spacing estimation.".into()
        },
    });

    // tsx (required for timeline.to_hyperframes — compiles EDL v2 to HF HTML)
    let tsx_available = std::process::Command::new("npx")
        .arg("tsx")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let tsx = json!({
        "available": tsx_available,
        "reason": if tsx_available {
            serde_json::Value::Null
        } else {
            "tsx not found (npx tsx --version failed). Required for timeline.to_hyperframes.".into()
        },
    });

    // ASS caption font (BebasNeue — required for burned-in captions)
    let font_path = "mcp/fonts/BebasNeue-Regular.ttf";
    let font_available = path_exists(font_path);
    let ass_font = json!({
        "available": font_available,
        "path": font_path,
        "reason": if font_available {
            serde_json::Value::Null
        } else {
            "BebasNeue-Regular.ttf not found. Caption burning will use ffmpeg's default font.".into()
        },
    });

    // SVG sticker presets
    let presets_dir = "mcp/assets/svg_presets";
    let preset_count = if path_exists(presets_dir) {
        std::fs::read_dir(presets_dir)
            .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
            .unwrap_or(0)
    } else {
        0
    };
    let svg_presets = json!({
        "available": preset_count > 0,
        "preset_count": preset_count,
        "path": presets_dir,
    });

    // Audio8 TTS (zero-shot voice cloning, ONNX INT4)
    let audio8_model_present = std::path::Path::new("mcp/assets/audio8/model/runtime_manifest.json").exists();
    let audio8_voices_dir = std::path::Path::new("mcp/assets/audio8/voices");
    let audio8_voice_count = if audio8_voices_dir.exists() {
        std::fs::read_dir(audio8_voices_dir)
            .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
            .unwrap_or(0)
    } else {
        0
    };
    let audio8 = json!({
        "available": audio8_model_present && openscript_tts::audio8::audio8_available(),
        "model_present": audio8_model_present,
        "voice_count": audio8_voice_count,
        "model_dir": "mcp/assets/audio8/model",
        "voices_dir": "mcp/assets/audio8/voices",
        "sample_rate": 44100,
        "languages": ["en", "es", "fr", "de", "it", "nl", "pl", "ja", "ko", "zh", "yue"],
        "note": "Zero-shot voice cloning via Audio8 TTS Preview 0.6B (ONNX INT4). English default for the script-to-video workflow.",
    });

    // Gepard TTS (high-quality native-English voice cloning — Qwen3.5 AR + NeMo
    // NanoCodec via the .venv-gepard inference venv; Apache-2.0 weights).
    let gepard_voices_dir = std::path::Path::new("mcp/assets/gepard/voices");
    let gepard_voice_count = if gepard_voices_dir.exists() {
        std::fs::read_dir(gepard_voices_dir)
            .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().extension().map(|x| x == "wav").unwrap_or(false)).count())
            .unwrap_or(0)
    } else {
        0
    };
    let gepard = json!({
        "available": openscript_tts::gepard::gepard_available(),
        "model": "nineninesix/gepard-1.0",
        "voice_count": gepard_voice_count,
        "voices_dir": "mcp/assets/gepard/voices",
        "sample_rate": 22050,
        "languages": ["en", "es-MX", "pt-BR", "nl"],
        "setup": "bash scripts/setup_gepard.sh (builds .venv-gepard: Python 3.12 + CUDA torch + NeMo codec + transformers 5.3.0)",
        "note": "High-quality native-English zero-shot voice cloning (Gepard 1.0, Apache-2.0; NeMo NanoCodec under NVIDIA OML). Voice.profile.add with provider=gepard.",
    });

    // VoiceDesign TTS (Qwen3-TTS-1.7B-VoiceDesign, ONNX int4 — designs
    // NOVEL character voices from a text description, zero reference audio).
    let voicedesign_available = openscript_tts::voicedesign::voicedesign_available();
    let voicedesign_model_present = openscript_tts::voicedesign::voicedesign_model_present();
    // Probe the resolved interpreter for the sidecar deps (onnxruntime, numpy,
    // soundfile, transformers) — same pattern as the kokoro `python_module_ok`
    // probe, so a python without the deps is never advertised as ready.
    let voicedesign_python = openscript_tts::voicedesign::resolve_voicedesign_python();
    let voicedesign_python_module_ok = std::process::Command::new(&voicedesign_python)
        .arg("-c")
        .arg("import onnxruntime, numpy, soundfile, transformers")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let voicedesign = json!({
        "available": voicedesign_available && voicedesign_model_present && voicedesign_python_module_ok,
        "sidecar_available": voicedesign_available,
        "model_present": voicedesign_model_present,
        "python_module_ok": voicedesign_python_module_ok,
        "python_path": voicedesign_python,
        "model": "wavekat/Qwen3-TTS-1.7B-VoiceDesign-ONNX (int4)",
        "model_dir": "mcp/assets/voicedesign",
        "sample_rate": 24000,
        "languages": ["english", "chinese", "japanese", "korean", "german", "french", "russian", "portuguese", "spanish", "italian"],
        "setup": "bash scripts/setup_voicedesign.sh (builds .venv-voicedesign + downloads model ~4.3GB)",
        "note": "Designs brand-new character voices from a natural-language description (voice.design). No reference audio needed — pair with gepard clone registration for reusable personas.",
    });

    // Whisper word alignment (multilingual — primary alignment engine for
    // Hinglish/Hindi scripts; Parakeet TDT is English-only and drifts on
    // Hinglish). Used by script.generate_voices when script.language is
    // hi/hinglish. Requires the openai-whisper Python package.
    let whisper_script = "mcp/scripts/whisper_align.py";
    let whisper_script_exists = path_exists(whisper_script);
    let whisper_importable = std::process::Command::new("python3")
        .args(["-c", "import whisper; print('ok')"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let whisper_align = json!({
        "available": whisper_script_exists && whisper_importable,
        "path": whisper_script,
        "script_exists": whisper_script_exists,
        "whisper_importable": whisper_importable,
        "model": "base",
        "languages": ["hi", "hinglish", "en", "es", "fr", "de"],
        "reason": if whisper_script_exists && whisper_importable {
            serde_json::Value::Null
        } else if !whisper_script_exists {
            "whisper_align.py not found. Hinglish scripts fall back to Parakeet alignment (English-only — caption sync on Hinglish will be approximate).".into()
        } else {
            "openai-whisper not installed (pip install openai-whisper). Hinglish scripts fall back to Parakeet alignment.".into()
        },
    });

    // LLM / vision cascade: OpenCode zen + OpenRouter free multimodal
    let llm = crate::llm::probe_llm_capabilities().await;
    let openscript_config = crate::config::config_public_view();

    Ok(json!({
        "status": "success",
        "voicebox": voicebox,
        "kokoro": kokoro,
        "audio8": audio8,
        "gepard": gepard,
        "voicedesign": voicedesign,
        "transcription": transcription,
        "parakeet_align": parakeet_align,
        "whisper_align": whisper_align,
        "pexels": pexels,
        "giphy": giphy,
        "pixabay": pixabay,
        "sfx_library": sfx,
        "music_library": music,
        "ffmpeg": ffmpeg,
        "yt_dlp": yt_dlp,
        "tsx": tsx,
        "ass_font": ass_font,
        "svg_presets": svg_presets,
        "hyperframes": hyperframes,
        "llm": llm,
        "openscript_config": openscript_config,
    }))
}

pub(crate) async fn handle_system_doctor(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let ffmpeg_ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ffprobe_ok = std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ytdlp_ok = std::process::Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let pexels_ok = !pexels_key().is_empty();
    let giphy_ok = !giphy_key().is_empty();
    let music_prod = resolve_repo_path("mcp/assets/music_production/index.json").exists();
    let music_lib = resolve_repo_path("mcp/assets/music_library_index.json").exists();
    let music_ok = music_prod || music_lib || !pixabay_key().is_empty();
    let sfx_index = resolve_repo_path("mcp/assets/sfx_index.json");
    let sfx_pack = resolve_repo_path("mcp/assets/sfx_pack");
    let mut sfx_resolvable = 0usize;
    if let Ok(raw) = std::fs::read_to_string(&sfx_index) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
                for a in assets.iter().take(30) {
                    let p = a.get("path").and_then(|x| x.as_str()).unwrap_or("");
                    if Path::new(p).exists() || resolve_repo_path(p).exists() {
                        sfx_resolvable += 1;
                    }
                }
            }
        }
    }
    let sfx_ok = sfx_resolvable >= 5 || sfx_pack.is_dir();
    let kokoro_ok = resolve_repo_path("mcp/assets/kokoro/onnx/kokoro-v1.0.onnx").exists();
    let config_ok = crate::config::config_file_path().exists() || pexels_ok;

    let mut checklist = Vec::new();
    let mut next_actions = Vec::new();
    let push = |items: &mut Vec<serde_json::Value>,
                next: &mut Vec<String>,
                id: &str,
                ok: bool,
                detail: &str,
                action: Option<&str>| {
        items.push(json!({
            "id": id,
            "ok": ok,
            "detail": detail,
        }));
        if !ok {
            if let Some(a) = action {
                next.push(a.to_string());
            }
        }
    };

    push(
        &mut checklist,
        &mut next_actions,
        "ffmpeg",
        ffmpeg_ok && ffprobe_ok,
        if ffmpeg_ok {
            "ffmpeg + ffprobe on PATH"
        } else {
            "ffmpeg/ffprobe missing"
        },
        Some("Install ffmpeg (apt install ffmpeg / brew install ffmpeg)"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "yt_dlp",
        ytdlp_ok,
        if ytdlp_ok {
            "yt-dlp on PATH"
        } else {
            "yt-dlp missing (YouTube stock/music fallback)"
        },
        Some("pip install --user yt-dlp"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "pexels",
        pexels_ok,
        if pexels_ok {
            "PEXELS_API_KEY present (env or ~/.openscript/config.json)"
        } else {
            "Pexels key missing — multi-broll will fail-closed to draft without stock"
        },
        Some("bash scripts/setup_openscript_config.sh --pexels-key YOUR_KEY"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "giphy",
        giphy_ok,
        if giphy_ok {
            "GIPHY_API_KEY present"
        } else {
            "GIPHY key missing (local sticker fallbacks only)"
        },
        Some("bash scripts/setup_openscript_config.sh --giphy-key YOUR_KEY"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "music",
        music_ok,
        if music_prod {
            "music_production pack present (cold-start beds)"
        } else if music_lib {
            "music_library_index.json present"
        } else {
            "No production music path"
        },
        Some("Ensure mcp/assets/music_production/ exists or run library.build"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "sfx",
        sfx_ok,
        &format!(
            "SFX: {} resolvable of first 30 index rows; pack_dir={}",
            sfx_resolvable,
            sfx_pack.is_dir()
        ),
        Some("Use mcp/assets/sfx_pack or sfx.index against OPENSCRIPT_SFX_PATH"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "kokoro",
        kokoro_ok,
        if kokoro_ok {
            "Kokoro ONNX model present"
        } else {
            "Kokoro model missing"
        },
        Some("bash setup.sh  # downloads Kokoro models"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "config",
        config_ok,
        if crate::config::config_file_path().exists() {
            "openscript config file present"
        } else if pexels_ok {
            "keys via env (config file optional)"
        } else {
            "no ~/.openscript/config.json"
        },
        Some("bash scripts/setup_openscript_config.sh"),
    );

    // HinglishGgml transcription engine check
    let hinglish_available = openscript_transcribe::transcriber::check_hinglish_ggml_health().await;
    let hinglish_ok = hinglish_available.is_ok();
    let hinglish_msg = if hinglish_ok { hinglish_available.unwrap() } else { hinglish_available.unwrap_err() };
    push(&mut checklist, &mut next_actions, "hinglish-ggml", hinglish_ok, &hinglish_msg, Some("Build whisper.cpp + download GGML model — run bash setup.sh"));

    // Production-ready: binaries + pexels + music + kokoro. GIPHY optional.
    // HinglishGgml transcription engine check
    let hinglish_available = openscript_transcribe::transcriber::check_hinglish_ggml_health().await;
    let hinglish_ok = hinglish_available.is_ok();
    let hinglish_msg = if hinglish_ok { hinglish_available.unwrap() } else { hinglish_available.unwrap_err() };
    push(&mut checklist, &mut next_actions, "hinglish-ggml", hinglish_ok, &hinglish_msg, Some("Build whisper.cpp + download GGML model - run bash setup.sh"));
    let ready_for_production = ffmpeg_ok && ffprobe_ok && pexels_ok && music_ok && kokoro_ok;
    if ready_for_production && next_actions.is_empty() {
        next_actions.push(
            "Run director.run on a 5-scene script; expect ≥4/5 non-procedural stock + music bed"
                .into(),
        );
    } else if !ready_for_production {
        next_actions.push("bash scripts/bootstrap_media.sh".into());
        next_actions.push("See docs/INSTALL.md".into());
    }

    Ok(json!({
        "status": if ready_for_production { "ready" } else { "not_ready" },
        "ready_for_production": ready_for_production,
        "checklist": checklist,
        "next_actions": next_actions,
        "hints": {
            "allow_procedural": "OPENSCRIPT_ALLOW_PROCEDURAL=1 forces gradient B-roll (draft-grade only)",
            "config": crate::config::config_file_path().display().to_string(),
            "install_plan": "docs/INSTALL_MEDIA_DEPS_PLAN.md",
        },
    }))
}

/// Natural-language tool discovery. Tokenises the query, scores each tool by
/// keyword overlap with its name + description, and returns the top N matches.
pub(crate) async fn handle_help_tool(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 8).clamp(1, 20) as usize;

    // Normalise the query into a set of lowercase tokens, dropping stopwords.
    let stop = [
        "a", "an", "the", "to", "for", "of", "in", "on", "at", "by", "with", "and", "or", "is",
        "are", "be", "do", "does", "how", "i", "my", "me", "want", "need", "please", "can",
        "could", "would", "should",
    ];

    // Synonym map: expand query tokens with common synonyms so "burn" matches
    // "burned-in", "footage" matches "broll", "VO" matches "voiceover", etc.
    // This fixes the 3/4 broken example queries from the UX audit.
    let synonyms: &[(&str, &[&str])] = &[
        ("burn", &["burned", "burning", "burn-in", "burned-in"]),
        ("footage", &["broll", "b-roll", "background", "clip", "video"]),
        ("vo", &["voiceover", "voice", "narration"]),
        ("subtitles", &["captions", "subtitle", "caption", "srt"]),
        ("sidechain", &["ducking", "duck", "compress"]),
        ("render", &["rendered", "rendering", "render"]),
        ("music", &["audio", "track", "song", "background"]),
        ("sfx", &["sound", "effect", "effects", " Foley"]),
        ("sticker", &["overlay", "gif", "png", "image", "sticker"]),
        ("transcribe", &["transcription", "transcribe", "whisper", "speech"]),
        ("voice", &["tts", "voiceover", "kokoro", "speech", "voice"]),
        ("animate", &["animation", "animated", "motion", "gsap", "hyperframes"]),
    ];

    let expand_token = |t: &str| -> Vec<String> {
        let mut expanded = vec![t.to_string()];
        for (key, vals) in synonyms {
            if t == *key {
                expanded.extend(vals.iter().map(|v| v.to_string()));
            }
            // Also check reverse: if t is a synonym, add the key
            if vals.contains(&t) {
                expanded.push(key.to_string());
            }
        }
        expanded
    };

    let query_tokens: std::collections::HashSet<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !stop.contains(t))
        .flat_map(expand_token)
        .collect();

    if query_tokens.is_empty() {
        return Ok(json!({
            "status": "success",
            "query": query,
            "results": [],
            "count": 0,
            "message": "Query contained no searchable keywords. Try describing the task, e.g. 'add voiceover to a timeline'."
        }));
    }

    // Detect NLE / existing-footage intent so we do not boost from-scratch
    // orchestrators (script.to_video) for "edit existing footage" queries.
    // Strong markers alone are enough; "edit"/"clip" only count with video context.
    let nle_intent = {
        let q = query.to_lowercase();
        let strong = [
            "existing",
            "footage",
            "transcribe",
            "raw video",
            "reelize",
            "nle",
            "recording",
            "source video",
            "hinglish",
        ]
        .iter()
        .any(|m| q.contains(m));
        let soft_edit = (q.contains("edit") || q.contains("cut"))
            && (q.contains("video")
                || q.contains("footage")
                || q.contains("clip")
                || q.contains("reel")
                || q.contains("timeline"));
        (strong || soft_edit)
            && !q.contains("from scratch")
            && !q.contains("script json")
            && !q.contains("from a script")
    };
    let from_scratch_intent = {
        let q = query.to_lowercase();
        q.contains("script")
            || q.contains("from scratch")
            || q.contains("tts")
            || q.contains("create a video")
            || q.contains("generate a video")
    };

    // Tool weight table: golden-path tools get a base boost, orchestrators get
    // a medium boost, primitives get no boost. Trajectory-aware: NLE queries
    // boost transcribe/reelize/timeline.
    let tool_weight = |name: &str| -> f64 {
        if nle_intent {
            if matches!(
                name,
                "transcribe"
                    | "reelize.direct"
                    | "reelize.brief"
                    | "timeline.render"
                    | "timeline.build"
                    | "srt.prepare"
                    | "edl.build"
            ) {
                return 0.20;
            }
            // Demote from-scratch golden path on NLE queries
            if matches!(name, "script.to_video" | "script.parse" | "script.to_timeline") {
                return 0.0;
            }
        }
        if from_scratch_intent && !nle_intent
            && matches!(name, "script.to_video" | "script.parse") {
                return 0.20;
            }
        // Golden-path defaults
        if matches!(
            name,
            "script.to_video"
                | "script.parse"
                | "transcribe"
                | "timeline.render"
                | "system.capabilities"
                | "help.tool"
        ) {
            0.15
        // Orchestrators
        } else if matches!(
            name,
                | "reelize.direct"
                | "composition.render"
                | "tts.commentary"
                | "script.to_timeline"
                | "script.generate_voices"
                | "script.build_captions"
        ) {
            0.10
        // Common operations
        } else if matches!(
            name,
            "music.assign"
                | "sfx.assign"
                | "broll.assign"
                | "overlay.assign"
                | "voiceover.generate"
                | "tts.generate"
                | "background.fetch"
                | "music.search"
                | "sfx.search"
                | "broll.fetch"
                | "gif.search"
                | "media.search"
                | "library.search"
                | "stock.search"
        ) {
            0.05
        } else {
            0.0
        }
    };

    // Iterate all tool definitions, score each by token overlap with name + description.
    let all_tools = tool_definitions();
    let mut scored: Vec<serde_json::Value> = Vec::new();

    if let Some(arr) = all_tools.as_array() {
        for tool in arr {
            let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let desc = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Combine name + description, lowercase, tokenise
            let combined = format!("{} {}", name, desc).to_lowercase();
            let tool_tokens: std::collections::HashSet<&str> = combined
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
                .collect();

            // Score = (matching tokens) / (query tokens) + name_boost + tool_weight.
            let mut matches = 0usize;
            let mut name_boost = 0.0;
            for qt in &query_tokens {
                if tool_tokens.contains(qt.as_str()) {
                    matches += 1;
                    if name.to_lowercase().contains(qt.as_str()) {
                        name_boost += 0.15;
                    }
                }
            }
            let coverage = matches as f64 / query_tokens.len() as f64;
            let weight = tool_weight(name);
            let mut score = (coverage + name_boost + weight).min(1.0);
            // Hard demote from-scratch tools on NLE queries even if token
            // overlap is high (e.g. "captions" matches script.to_video desc).
            if nle_intent
                && matches!(
                    name,
                    "script.to_video" | "script.parse" | "script.to_timeline"
                        | "script.generate_voices" | "script.build_captions"
                )
            {
                score *= 0.35;
            }

            if score > 0.0 {
                // Short description = first sentence of the description, capped at 180 chars.
                let short_desc = desc
                    .split('.')
                    .next()
                    .unwrap_or(desc)
                    .chars()
                    .take(180)
                    .collect::<String>();
                scored.push(json!({
                    "name": name,
                    "relevance": (score * 100.0).round() / 100.0,
                    "description": short_desc,
                }));
            }
        }
    }

    // Sort by relevance desc (tool-weight table breaks ties instead of alphabet)
    scored.sort_by(|a, b| {
        let ra = a.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rb = b.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);

    let count = scored.len();
    if count == 0 {
        return Ok(json!({
            "status": "success",
            "query": query,
            "results": [],
            "count": 0,
            "message": "No tools matched. Try tools/list to browse all 76 tools, or system.capabilities to probe available subsystems."
        }));
    }

    Ok(json!({
        "status": "success",
        "query": query,
        "results": scored,
        "count": count,
    }))
}

