# Migration Plan: faster-qwen3-tts → Voicebox (Full Integration)

**Status: ✅ COMPLETE** — All 6 tasks done, all tests passing, all MCP tools verified against live voicebox instance.

## Overview

Replace the `faster-qwen3-tts` sidecar with the already-running **voicebox** Docker container at `http://127.0.0.1:17493`. 

**Key architectural shift**: Voicebox is the **single source of truth** for voice profiles. Our JSON registry (`.openscript/voice_profiles.json`, `mcp/assets/voices.json`) is **eliminated**. AI agents query voicebox's API directly to discover the ever-expanding voice library, and use voicebox's generation endpoints to produce audio.

## Architecture Change

```
BEFORE:                               AFTER:
                                     
Rust openscript-tts ──HTTP──► faster-  Rust openscript-mcp ──HTTP──► voicebox Docker
                         qwen3-tts                            (127.0.0.1:17493)
                         sidecar (:8000)                         │
                         (third_party/)                          ├─ /profiles (list/create/delete)
                                                                 ├─ /profiles/{id}/samples (add samples)
                                                                 ├─ /profiles/presets/{engine} (preset voices)
                                                                 ├─ /generate (full generation via profile_id)
                                                                 └─ /tts/generate (lightweight, ref_audio inline)

Our JSON registry ──► DELETED. Voicebox SQLite DB is the registry.
```

## Voicebox API Surface (What We Expose to AI Agents)

| MCP Tool | Voicebox Endpoint | Purpose |
|---|---|---|
| `voice.profile.list` | `GET /profiles` | List all voices (cloned + presets) |
| `voice.profile.list_presets` | `GET /profiles/presets/{engine}` | List preset voices for an engine |
| `voice.profile.get` | `GET /profiles/{id}` | Get detailed profile info |
| `voice.profile.add_sample` | `POST /profiles/{id}/samples` | Add reference audio to existing profile |
| `tts.generate` | `POST /generate` | Generate speech using voicebox profile (async, full-featured) |
| `tts.estimate_duration` | *(local math)* | Estimate audio duration (unchanged) |
| `tts.preview` | *(local math)* | Preview without generating (unchanged) |
| `tts.commentary` | `POST /generate` (multiple calls) | Multi-position commentary generation |
| `voiceover.generate` | `POST /generate` | Generate + place on timeline |

**Removed MCP tools** (no longer needed):
- `voice.profile.add` — voices are created in voicebox UI/API directly
- `voice.profile.remove` — voices are deleted in voicebox UI/API directly

## Files to Modify

### Phase 1: TTS Client Layer — Rewrite for Voicebox API

#### 1.1 `crates/openscript-tts/src/client.rs` — **FULL REWRITE**

**Current `TtsClient`**: Single-purpose client for `/tts/generate` with ref_audio inline.

**New `VoiceboxClient`**: Multi-endpoint client matching voicebox's full API:

```rust
pub struct VoiceboxClient {
    http: Client,
    base_url: String,  // default: http://127.0.0.1:17493
    cache_dir: PathBuf,
}
```

**Methods to implement:**

| Method | Voicebox Endpoint | Purpose |
|---|---|---|
| `health_check()` | `GET /health` | Check voicebox is running + model loaded |
| `list_profiles()` | `GET /profiles` | Return all voice profiles |
| `list_preset_voices(engine)` | `GET /profiles/presets/{engine}` | Return preset voices for engine |
| `get_profile(profile_id)` | `GET /profiles/{id}` | Get single profile details |
| `add_profile_sample(profile_id, audio_path, ref_text)` | `POST /profiles/{id}/samples` | Add audio sample to profile |
| `generate_by_profile(profile_id, text, language, engine)` | `POST /generate` | Full generation (uses voicebox DB profile, async) |
| `generate_with_ref(text, ref_audio_path, ref_text, language)` | `POST /tts/generate` | Lightweight generation (no profile needed) |
| `download_audio(generation_id)` | `GET /audio/{generation_id}` | Download generated audio file |

**Key struct changes:**

```rust
// Voicebox profile response (maps to Voicebox's VoiceProfileResponse)
pub struct VoiceboxProfile {
    pub id: String,           // UUID from voicebox
    pub name: String,
    pub description: Option<String>,
    pub language: String,     // 2-letter code: en, zh, ja, etc.
    pub voice_type: String,   // "cloned", "preset", "designed"
    pub preset_engine: Option<String>,
    pub preset_voice_id: Option<String>,
    pub default_engine: Option<String>,
    pub sample_count: u32,
    pub generation_count: u32,
    pub created_at: String,
}

// Preset voice entry
pub struct PresetVoice {
    pub voice_id: String,
    pub name: String,
    pub gender: String,
    pub language: String,
}

// Generation response (maps to voicebox GenerationResponse)
pub struct VoiceboxGeneration {
    pub id: String,           // generation UUID
    pub profile_id: String,
    pub text: String,
    pub status: String,       // "completed", "generating", "failed"
    pub audio_path: Option<String>,
    pub duration: Option<f64>,
    pub engine: Option<String>,
    pub error: Option<String>,
}

// Lightweight TTS response (from /tts/generate)
pub struct VoiceboxTtsResult {
    pub audio_b64: String,
    pub duration_ms: i64,
    pub sample_rate: u32,
}
```

**What stays the same:**
- `estimate_duration()` — pure math, no API dependency
- Caching logic — but now caches by `(profile_id, text, language, engine)` instead of ref_audio hash
- `extract_audio_duration()` — ffprobe-based, unchanged
- `TtsError` enum — extend with `Voicebox(String)` variant

#### 1.2 `crates/openscript-tts/src/profiles.rs` — **DELETE or MINIMIZE**

**Option**: Delete this file entirely. The `VoiceProfileRegistry` and JSON-backed storage are no longer needed. Voicebox is the registry.

**Alternative**: Keep a thin stub that re-exports `VoiceboxProfile` types from `client.rs` for use by the MCP layer.

**Decision**: Delete. MCP tools will work directly with `VoiceboxProfile` from `client.rs`.

### Phase 2: MCP Tool Layer — Full Rewrite of Voice Tools

#### 2.1 `crates/openscript-mcp/src/tools.rs` — **MAJOR CHANGES**

**Tool: `voice.profile.list`** (REWRITTEN)
- **Before**: Reads `.openscript/voice_profiles.json`
- **After**: Calls `GET /profiles` on voicebox
- **Returns**: Array of `{id, name, language, voice_type, engine, sample_count}`
- **New param**: `engine` (optional filter — "qwen", "kokoro", etc.)
- **New param**: `voice_type` (optional filter — "cloned", "preset")

**Tool: `voice.profile.list_presets`** (NEW)
- Calls `GET /profiles/presets/{engine}`
- **Params**: `engine` (required — "qwen_custom_voice", "kokoro")
- **Returns**: Array of `{voice_id, name, gender, language}`
- **Use case**: AI agent discovers built-in voices without cloning

**Tool: `voice.profile.get`** (NEW)
- Calls `GET /profiles/{id}`
- **Params**: `profile_id` (required)
- **Returns**: Full profile details including sample count, generation count, effects_chain

**Tool: `voice.profile.add_sample`** (NEW)
- Calls `POST /profiles/{id}/samples`
- **Params**: `profile_id`, `audio_path`, `reference_text`
- **Use case**: Improve an existing cloned voice with more reference samples

**Tool: `voice.profile.add`** (REMOVED)
- Voices are created in voicebox directly (UI or API)
- **Migration**: Existing `voice.profile.add` callers should use voicebox's `/profiles` POST endpoint

**Tool: `voice.profile.remove`** (REMOVED)
- Voices are deleted in voicebox directly

**Tool: `tts.generate`** (REWRITTEN)
- **Before**: Loads JSON profile → sends ref_audio inline → returns WAV
- **After**: Calls `POST /generate` with voicebox profile_id
- **Params**: `profile_id`, `text`, `output_path`, `language` (optional, defaults to profile's), `engine` (optional), `seed` (optional), `instruct` (optional)
- **Flow**: 
  1. POST `/generate` with JSON body → voicebox creates async generation
  2. Poll generation status until "completed"
  3. Download audio from voicebox → save to `output_path`
  4. Cache locally
- **Fallback**: If voicebox is down or profile not found, try `/tts/generate` with inline ref_audio (if profile has samples)

**Tool: `voiceover.generate`** (REWRITTEN)
- Same flow as `tts.generate` but adds event to timeline voiceover track
- **Params**: `timeline_path`, `profile_id`, `text`, `position_ms`, `gain_db`, `language`, `engine`

**Tool: `tts.commentary`** (REWRITTEN)
- Calls `tts.generate` multiple times for intro/transitions/outro
- Same params but `profile_id` instead of `voice_profile_id`

**Tool: `tts.preview`** (MINOR UPDATE)
- **Before**: Loads JSON profile to get info
- **After**: Calls `GET /profiles/{id}` to get profile info
- **Returns**: Same format — profile info + estimated duration

**Tool: `tts.estimate_duration`** (NO CHANGE)
- Pure math — `word_count / 2.5 * 1000`

### Phase 3: Generation Flow — Sync vs Async

Voicebox's `/generate` endpoint is **asynchronous** — it creates a generation record and processes in background. We need to handle this:

```
1. POST /generate { profile_id, text, language, engine }
   → returns { id: "gen-uuid", status: "generating" }

2. Poll GET /history?limit=1 (or track by generation id)
   → returns { status: "completed", audio_path: "/data/..." }

3. GET /audio/{generation_id} or read audio_path
   → raw WAV bytes

4. Save to output_path + cache
```

**Polling strategy**:
- Initial poll after 500ms
- Subsequent polls every 200ms
- Timeout after 60 seconds
- Max 300 polls

**Alternative**: Use voicebox's `/tts/generate` (lightweight, synchronous) for short texts where ref_audio can be sent inline. Reserve `/generate` (async) for longer texts or multi-sample profiles.

**Decision**: Use `/generate` (async) as primary path. It uses voicebox's full profile system (multi-sample voices, effects chains, engine selection). Fall back to `/tts/generate` only for the `tts.preview` path.

### Phase 4: Configuration & Cleanup

#### 4.1 Environment Variables
| Variable | Old Default | New Default | Notes |
|---|---|---|---|
| `OPENSCRIPT_TTS_URL` | `http://localhost:8000` | `http://127.0.0.1:17493` | voicebox URL |
| `OPENSCRIPT_TTS_CACHE` | `artifacts/tts` | `artifacts/tts` | Unchanged |

#### 4.2 Delete Files
- `crates/openscript-tts/src/profiles.rs` — no longer needed
- `mcp/assets/voices.json` — voicebox is the registry
- `.openscript/voice_profiles.json` — voicebox is the registry
- `third_party/faster-qwen3-tts/` — optional, can remove

#### 4.3 Update `crates/openscript-tts/src/lib.rs`
```rust
pub mod client;
// profiles module removed
```

#### 4.4 Update `crates/openscript-mcp/Cargo.toml`
- No dependency changes needed (openscript-tts still used)

#### 4.5 Update README.md
- TTS section: "voicebox Docker container" instead of "faster-qwen3-tts"
- Architecture diagram update
- Voice management: "Create and manage voices in voicebox UI at http://127.0.0.1:17493"
- MCP tools table: update voice/TTS tools

## Implementation Order

### Task 1: Rewrite `client.rs` (openscript-tts crate)
- New `VoiceboxClient` struct
- All API methods: `health_check`, `list_profiles`, `list_preset_voices`, `get_profile`, `add_profile_sample`, `generate`, `generate_with_ref`
- New response structs: `VoiceboxProfile`, `PresetVoice`, `VoiceboxGeneration`, `VoiceboxTtsResult`
- Async generation polling logic
- Caching by `(profile_id, text, language, engine)`
- Error handling with `Voicebox(String)` variant

### Task 2: Delete `profiles.rs`, update `lib.rs`
- Remove `profiles.rs` file
- Update `lib.rs` to only export `client` module

### Task 3: Rewrite MCP voice tools (tools.rs)
- **Rewrite**: `voice.profile.list`, `tts.generate`, `tts.commentary`, `tts.preview`, `voiceover.generate`
- **Add**: `voice.profile.list_presets`, `voice.profile.get`, `voice.profile.add_sample`
- **Remove**: `voice.profile.add`, `voice.profile.remove`
- Update tool schemas and descriptions for AI agents

### Task 4: Update tool definitions (tools.rs schema section)
- Update all 8 voice/TTS tool definitions with new params and descriptions
- Remove `voice.profile.add` and `voice.profile.remove` from tool list
- Add `voice.profile.list_presets`, `voice.profile.get`, `voice.profile.add_sample`

### Task 5: Cleanup and config
- Update env var defaults (`OPENSCRIPT_TTS_URL`)
- Delete `mcp/assets/voices.json`
- Delete `.openscript/voice_profiles.json`
- Update `Cargo.toml` if needed (remove unused deps like serde for profiles)
- Update README.md

### Task 6: Build and verify
- `cargo build --workspace`
- `cargo test --workspace`
- Verify MCP server starts clean
- Test `voice.profile.list` against running voicebox

## MCP Tool Summary (After Migration)

| Tool | Params | Returns |
|---|---|---|
| `voice.profile.list` | `engine?`, `voice_type?` | `{profiles: [{id, name, language, voice_type, engine, sample_count}], count}` |
| `voice.profile.list_presets` | `engine` | `{voices: [{voice_id, name, gender, language}]}` |
| `voice.profile.get` | `profile_id` | `{id, name, description, language, voice_type, engine, sample_count, generation_count}` |
| `voice.profile.add_sample` | `profile_id`, `audio_path`, `reference_text` | `{sample_id, profile_id, reference_text}` |
| `tts.generate` | `profile_id`, `text`, `output_path`, `language?`, `engine?`, `seed?`, `instruct?` | `{status: "generated", output_path, duration_ms, generation_id}` |
| `tts.estimate_duration` | `text`, `speed?` | `{word_count, estimated_duration_ms}` |
| `tts.preview` | `profile_id`, `text`, `speed?` | `{profile_info, word_count, estimated_duration_ms}` |
| `tts.commentary` | `timeline_path`, `profile_id`, `commentary_type`, `intro_text?`, `outro_text?`, `speed?`, `language?`, `engine?` | `{voiceovers_generated, positions, count}` |
| `voiceover.generate` | `timeline_path`, `profile_id`, `text`, `position_ms?`, `speed?`, `gain_db?`, `language?`, `engine?` | `{status, output_path, duration_ms, event_id}` |

## AI Agent Workflow (After Migration)

```
1. AI agent calls: voice.profile.list()
   → Gets all available voices from voicebox
   
2. AI agent calls: voice.profile.list_presets(engine="kokoro")
   → Gets built-in Kokoro voices (af_heart, etc.)
   
3. AI agent picks a voice: profile_id = "3e4e241c-..." (Warm Storyteller)

4. AI agent calls: tts.generate(profile_id="...", text="Hello world", output_path="...")
   → Voicebox generates audio using that profile's samples + settings
   → Audio saved to output_path
   
5. AI agent calls: voiceover.generate(timeline_path="...", profile_id="...", text="...")
   → Generates audio AND places it on the timeline voiceover track
```

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Voicebox model not loaded | Medium | High | Check `model_loaded` in health check; auto-trigger `POST /models/load` |
| Async generation timeout | Medium | Medium | 60s timeout with polling; clear error message to agent |
| Voicebox container down | Low | High | Health check fails fast with actionable error |
| Profile ID format change (string → UUID) | Low | Low | Our code treats IDs as opaque strings |
| Breaking change: `voice.profile.add` removed | Medium | Medium | Document in README; agents should use voicebox UI |
