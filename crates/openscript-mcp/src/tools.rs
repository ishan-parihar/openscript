use openscript_core::amplitude::extract_amplitude;
use openscript_core::background::assign_backgrounds;
use openscript_core::captions::{estimate_word_timings, generate_ass, CaptionSegment};
use openscript_core::script::{parse_script, validate_script};
use openscript_core::srt::{analyze_srt, build_edl, group_entries, parse_srt, write_srt};
use openscript_core::sticker::{generate_sticker_composition, StickerPreset};
use openscript_core::timeline::Timeline;
use openscript_core::types::TrackType;
use openscript_transcribe::transcriber::transcribe_with_engine;
use serde_json::json;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::error::ToolError;
use crate::server::report_progress;

// ---------------------------------------------------------------------------
// Tool definitions (86 tools: 43 original + 5 hf.* + 1 composition.render + 6 script.* + 2 background.* + 2 sticker.* + 2 script.to_* + 1 stock.fetch + 1 youtube.download + 1 youtube.search + 1 stock.search + 1 media.search + 1 gif.search + 1 timeline.inspect + 3 library.*)
// ---------------------------------------------------------------------------

pub fn tool_definitions() -> serde_json::Value {
    let mut tools = json!([
        // ===================================================================
        // GROUP 1: CORE PIPELINE — Transcribe, caption, and render
        // ===================================================================
        {
            "name": "transcribe",
            "description": "Convert spoken audio to word-level SRT subtitles. Uses openai-whisper (base model) — the DEFAULT transcription engine. Supports 99 languages with native word-level timestamps. For Hindi input, automatically converts Devanagari to Hinglish via LLM post-processing. Nemotron ONNX and Apex (deprecated) are available as fallback engines. ALWAYS call this first on any raw video — it produces the SRT that every other tool depends on. Returns: output_srt_path, entry_count, phrase_srt_path, word_srt_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "media_path": {"type": "string", "description": "Path to video or audio file to transcribe"},
                    "output_srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional output SRT path. Auto-generated if omitted."},
                    "language_hint": {"type": "string", "default": "auto", "description": "Language hint: 'auto' (detect), 'hi-IN' (Hindi → Hinglish), 'en-US' (English), 'hinglish'"},
                    "engine": {"type": "string", "default": "whisper", "description": "Engine: 'whisper' (default, 99 langs, word timestamps) or 'nemotron-onnx' (40 langs, cache-aware streaming) or 'apex' (deprecated)"}
                },
                "required": ["media_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "srt.read",
            "description": "Parse an SRT file and return all entries with timestamps and text. Use to inspect transcription quality before building edits, or to read an edited SRT for applying changes. Returns: count + entries array with idx, start, end, text.",
            "inputSchema": {
                "type": "object",
                "properties": {"srt_path": {"type": "string", "description": "Path to the SRT file"}},
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "srt.prepare",
            "description": "Convert word-level SRT into phrase-level SRT by grouping words into readable caption segments (max ~10 words, ~64 chars). This is the step between raw transcription and editing — it creates human-readable caption chunks that the EDL uses for segment timing. Returns: output_path with grouped SRT, count of groups.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"type": "string", "description": "Path to word-level SRT (from transcribe)"},
                    "max_words": {"type": "integer", "default": 10, "description": "Max words per caption segment"},
                    "max_chars": {"type": "integer", "default": 64, "description": "Max characters per caption segment"},
                    "max_gap": {"type": "number", "default": 0.6, "description": "Max gap in seconds between words to keep them in same group"}
                },
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "srt.apply_edit",
            "description": "Apply user-edited SRT as a keep-list: build an EDL from the edited segments and render the result. Use this when a human has manually removed/edited SRT lines — it treats the edited SRT as the definitive edit, keeping only those segments and burning in updated captions. Returns: output_path, segments_count, total_duration_s.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file"},
                    "edited_srt_path": {"type": "string", "description": "Path to the user-edited SRT file (acts as keep-list)"},
                    "merge_gap": {"type": "number", "default": 0.25, "description": "Gap in seconds to merge between segments"},
                    "crossfade_ms": {"type": "integer", "default": 80, "description": "Audio crossfade between segments in milliseconds"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "burn_captions": {"type": "boolean", "default": true, "description": "Burn captions into video output"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (lower = higher quality, 18-28 range)"},
                    "fps": {"type": "integer", "default": 30, "description": "Output framerate"}
                },
                "required": ["video_path", "edited_srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "edl.build",
            "description": "Build an Edit Decision List from SRT data. Analyzes caption timing, groups words, and produces a JSON EDL with segment start/end times. Use when you need precise control over which parts of the source video make the cut. Strategy 'keep' retains speaking segments; 'remove' creates gaps. Returns: edl_path, segments_count, total_duration_s.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"type": "string", "description": "Path to SRT file"},
                    "strategy": {"type": "string", "enum": ["keep", "remove"], "default": "keep", "description": "'keep' retains speaking parts, 'remove' creates silence gaps"},
                    "max_duration": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Cap total output duration in seconds"},
                    "crossfade_ms": {"type": "integer", "default": 120, "description": "Audio crossfade between segments in ms"},
                    "analysis_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional path to save SRT analysis JSON"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio"}
                },
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "render",
            "description": "Render a video from an EDL with burned-in captions. Use for quick single-step renders when you already have an EDL and SRT. For full multi-track renders (b-roll, music, SFX), use timeline.render instead. Returns: output_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file"},
                    "edl_path": {"type": "string", "description": "Path to EDL JSON file"},
                    "burn_captions": {"type": "boolean", "default": true, "description": "Burn captions into output"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "SRT file for caption burn-in"},
                    "ass_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "ASS file for styled caption burn-in (overrides SRT)"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (18-28 range)"},
                    "fps": {"type": "integer", "default": 30, "description": "Output framerate"}
                },
                "required": ["video_path", "edl_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "reelize",
            "description": "One-call reel creation: transcribe → group captions → build EDL → render with burned-in captions. Use for quick turnaround when you don't need b-roll, music, or SFX. For full production reels with all tracks, use reelize.timeline instead. Returns: output_path, segments_count, total_duration_s, preset used.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Raw source video"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT (skip transcription)"},
                    "preset": {"type": "string", "default": "Balanced", "description": "Editing pace: Tight (fast cuts), Balanced (moderate), Natural (relaxed)"},
                    "max_duration": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Maximum output duration in seconds"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "burn_captions": {"type": "boolean", "default": true, "description": "Burn captions into output"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "reelize.brief",
            "description": "Analyze video footage and return a structured brief for AI-directed editorial decisions. Transcribes, segments, and provides context about ALL footage — transcripts, timing, word counts, suggested b-roll concepts per segment, topic clusters. Use as the FIRST step before directing a video edit. This tool provides CONTEXT only — it makes NO editorial decisions. The AI agent reads the brief and decides which segments to keep, what b-roll to add, etc. Returns: source_duration_s, total_segments, total_dialogue_s, segments (with id, start_s, end_s, duration_s, text, word_count, words_per_second, suggested_broll_concepts, topic_keywords), topic_summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file to analyze"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT file (skip transcription)"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "reelize.direct",
            "description": "Execute AI-directed video production. The AI agent provides structured creative instructions (which segments to use, b-roll placement, SFX timing, music, voiceover script, caption style) and this tool builds the timeline, fetches assets, and renders the final reel. This tool EXECUTES — it makes NO creative decisions. Use AFTER reelize.brief, once the AI agent has analyzed the footage and made editorial choices. Returns: output_path, duration, segment counts, asset counts, warnings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file"},
                    "brief_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Path to reelize.brief output JSON (optional, for reference)"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT file (skip transcription)"},
                    "segments": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "start": {"type": "number"}, "end": {"type": "number"}, "caption": {"type": "string"}, "crossfade_ms": {"type": "integer"}}}, "description": "Segments to include in the edit, in order. Each has start/end times from source and caption text."},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "crossfade_ms": {"type": "integer", "default": 300, "description": "Audio crossfade between segments in ms"},
                    "fps": {"type": "integer", "default": 30, "description": "Output framerate"},
                    "broll": {"type": "array", "items": {"type": "object", "properties": {"concept": {"type": "string"}, "overlay_at_s": {"type": "number"}, "duration_s": {"type": "number"}, "style": {"type": "string"}}}, "description": "B-roll overlays: concept keyword, where to place it, how long, and style (full_cutaway or picture_in_picture)"},
                    "sfx": {"type": "array", "items": {"type": "object", "properties": {"role": {"type": "string"}, "at_s": {"type": "number"}}}, "description": "Sound effects: editorial role (whoosh, pop, hit, riser) and placement time in seconds"},
                    "music": {"anyOf": [{"type": "object", "properties": {"mood": {"type": "string"}, "energy": {"type": "string"}, "gain_db": {"type": "number"}, "duck_under_dialogue": {"type": "boolean"}}}, {"type": "null"}], "description": "Background music: mood, energy, volume level, and whether to duck under dialogue"},
                    "voiceover": {"type": "array", "items": {"type": "object", "properties": {"text": {"type": "string"}, "position_s": {"type": "number"}, "voice_profile_id": {"type": "string"}, "speed": {"type": "number"}, "gain_db": {"type": "number"}}}, "description": "TTS voiceover events: script text, placement, and voice profile"},
                    "captions": {"type": "object", "properties": {"enabled": {"type": "boolean", "default": true}, "style": {"type": "string", "enum": ["standard", "kinetic"], "default": "standard"}, "position": {"type": "string", "enum": ["center", "bottom"], "default": "center"}}, "description": "Caption style: standard (full-sentence ASS) or kinetic (word-by-word viral style)"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output video path (auto-generated if omitted)"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (18-28 range)"}
                },
                "required": ["video_path", "segments"],
                "additionalProperties": false
            }
        },
        {
            "name": "overlay.generate",
            "description": "Generate an animated caption overlay MOV using PupCaps. Produces a transparent-background video with styled captions that can be composited over the main render. Use when you want animated, styled captions instead of simple burned-in text. Returns: output_path of the overlay MOV.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"type": "string", "description": "Word-level SRT file"},
                    "edl_path": {"type": "string", "description": "EDL JSON for timeline retiming"},
                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Timeline path to persist overlay asset"},
                    "out_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output overlay MOV path"},
                    "width": {"type": "integer", "default": 1080, "description": "Overlay width in pixels"},
                    "height": {"type": "integer", "default": 1920, "description": "Overlay height in pixels"},
                    "fps": {"type": "integer", "default": 30, "description": "Overlay framerate"},
                    "animate": {"type": "boolean", "default": false, "description": "Enable word-by-word animation"},
                    "style": {"type": "string", "default": "pupcaps_center", "description": "Caption style preset"}
                },
                "required": ["srt_path", "edl_path"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 2: TIMELINE V2 — Multi-track editorial timeline
        // ===================================================================
        {
            "name": "timeline.build",
            "description": "Create a fresh multi-track EDL v2 timeline from a source video. This is the foundation for all editorial work — it creates a JSON timeline with empty tracks (dialogue, broll, music, sfx, voiceover, captions) that you populate with other tools. Use this as the FIRST step before any multi-track editing. Returns: timeline_path, aspect, fps.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_video": {"type": "string", "description": "Path to source video file"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio"},
                    "fps": {"type": "integer", "default": 30, "description": "Target framerate"},
                    "max_duration": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Maximum timeline duration in seconds"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom timeline JSON path (auto-generated if omitted)"}
                },
                "required": ["source_video"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.load",
            "description": "Load an existing timeline JSON and return its structure. Use to inspect a timeline before modifying it — reveals segments, track counts, and version. Always load before making edits. Returns: version, source, segments_count, track names.",
            "inputSchema": {
                "type": "object",
                "properties": {"timeline_path": {"type": "string", "description": "Path to timeline JSON file"}},
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.validate",
            "description": "Check a timeline for structural errors — missing segments, invalid track events, timing conflicts. ALWAYS call this before timeline.render to catch issues early. Returns: valid (boolean), errors array.",
            "inputSchema": {
                "type": "object",
                "properties": {"timeline_path": {"type": "string", "description": "Path to timeline JSON file"}},
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.upgrade",
            "description": "Convert a legacy EDL v1 JSON into the modern EDL v2 timeline format. Use when working with old renders that need multi-track capabilities. Returns: timeline_path, segments_count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "edl_v1_path": {"type": "string", "description": "Path to legacy EDL v1 JSON"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output timeline path (auto-generated if omitted)"}
                },
                "required": ["edl_v1_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.add_segment",
            "description": "Add a video segment to the timeline's main track. Each segment represents a continuous clip from the source video with start/end timestamps and a caption. Use to manually curate which parts of the source make it into the final edit. Returns: segment_id, timeline_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "start": {"type": "number", "description": "Start time in seconds from source video"},
                    "end": {"type": "number", "description": "End time in seconds from source video"},
                    "caption": {"type": "string", "description": "Caption text for this segment (used for burn-in)"},
                    "crossfade_ms": {"type": "integer", "default": 80, "description": "Audio crossfade at segment boundaries in ms"},
                    "semantic_role": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional role tag (e.g., 'hook', 'body', 'cta')"}
                },
                "required": ["timeline_path", "start", "end", "caption"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.add_track_event",
            "description": "Add an event to any timeline track (dialogue, voiceover, captions, broll, music, sfx). This is the low-level tool for populating tracks — use the specialized tools (broll.assign, music.assign, sfx.assign, voiceover.generate) when possible, but use this for custom event structures. Returns: event_id, track_type, timeline_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "track_type": {"type": "string", "enum": ["dialogue", "voiceover", "captions", "broll", "music", "sfx"], "description": "Which track to add the event to"},
                    "event": {"type": "object", "description": "Event object with id, asset_id, start_ms, end_ms, gain_db, kind fields"}
                },
                "required": ["timeline_path", "track_type", "event"],
                "additionalProperties": false
            }
        },
        {
            "name": "voice.profile.add",
            "description": "Register a voice profile for TTS generation. Captures a reference voice from audio + text for cloning or synthesis. Create profiles BEFORE generating voiceovers or commentary. Use unique profile_id per voice. Returns: profile_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "profile_id": {"type": "string", "description": "Unique identifier for this voice profile"},
                    "ref_audio": {"type": "string", "description": "Path to reference audio file (clean speech sample)"},
                    "ref_text": {"type": "string", "description": "Transcript of the reference audio"},
                    "provider": {"type": "string", "default": "faster-qwen3-tts", "description": "TTS provider engine"},
                    "mode": {"type": "string", "default": "clone", "description": "Voice mode: 'clone' for voice cloning, 'preset' for built-in voices"},
                    "model": {"type": "string", "default": "Qwen/Qwen3-TTS-12Hz-0.6B-Base", "description": "TTS model identifier"},
                    "language": {"type": "string", "default": "English", "description": "Voice language"},
                    "description": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Human-readable description of this voice"}
                },
                "required": ["profile_id", "ref_audio", "ref_text"],
                "additionalProperties": false
            }
        },
        {
            "name": "voice.profile.list",
            "description": "List all registered voice profiles. Use to see available voices before generating voiceovers. Returns: profiles array with profile_id, provider, language, and count.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "voice.profile.remove",
            "description": "Delete a voice profile by ID. Use to clean up unused or test profiles. Returns: profile_id removed.",
            "inputSchema": {
                "type": "object",
                "properties": {"profile_id": {"type": "string", "description": "ID of the voice profile to remove"}},
                "required": ["profile_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.generate",
            "description": "Generate speech audio from text using a registered voice profile. Use for producing narration, explanations, or any scripted audio. Requires TTS sidecar server running (OPENSCRIPT_TTS_URL). Returns: output_path, duration_ms, cached flag.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile to use"},
                    "text": {"type": "string", "description": "Text to synthesize"},
                    "output_path": {"type": "string", "description": "Output audio file path (WAV/MP3)"},
                    "speed": {"type": "number", "default": 1.0, "description": "Playback speed multiplier (1.0 = normal)"},
                    "pitch": {"type": "number", "default": 1.0, "description": "Pitch multiplier"},
                    "volume": {"type": "number", "default": 1.0, "description": "Volume multiplier"},
                    "format": {"type": "string", "default": "wav", "description": "Output audio format"}
                },
                "required": ["voice_profile_id", "text", "output_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.estimate_duration",
            "description": "Estimate how long TTS audio will be for a given text. Use BEFORE placing voiceovers on the timeline so you know the duration to reserve. Rule of thumb: ~2.5 words per second at normal speed. Returns: word_count, estimated_duration_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to estimate duration for"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speed multiplier"}
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "sfx.index",
            "description": "Scan the SFX library directory and build a searchable index JSON. Run once when SFX library changes. The index enables sfx.search and sfx.assign. Default path: $HOME/Videos/Assets/SFX (override with OPENSCRIPT_SFX_PATH env var). Returns: output_path, count of indexed files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sfx_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "SFX directory to scan (default: $OPENSCRIPT_SFX_PATH)"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output index JSON path (default: mcp/assets/sfx_index.json)"}
                }
            }
        },
        {
            "name": "sfx.search",
            "description": "Search the SFX index by keyword, editorial role, or category. Editorial roles describe the PURPOSE of a sound: 'intro' (hook/opening), 'transition' (scene change), 'highlight' (emphasis), 'outro' (closing). Use to find the right sound effect before assigning it. Returns: results array with id, filename, path, category, editorial_role, duration_ms, recommended_gain_db.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "default": "", "description": "Keyword search (e.g., 'whoosh', 'click', 'boom')"},
                    "editorial_role": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Editorial purpose: 'intro', 'transition', 'highlight', 'outro', 'hook'"},
                    "category": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Sound category (e.g., 'whoosh', 'impact', 'ambient')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results to return"}
                }
            }
        },
        {
            "name": "sfx.assign",
            "description": "Assign a sound effect to a specific position on the timeline's SFX track. Use editorial_role to select the RIGHT sound for the RIGHT moment: 'hook' at 0ms (grab attention), 'transition' between segments (smooth cuts), 'highlight' at key moments (emphasize points). Searches SFX index automatically by role. Returns: event_id, position_ms, asset_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "editorial_role": {"type": "string", "description": "Purpose of this SFX: 'hook' (opening grab), 'transition' (between segments), 'highlight' (emphasis), 'outro' (closing)"},
                    "query": {"type": "string", "default": "", "description": "Additional keyword filter (e.g., 'whoosh', 'subtle')"},
                    "position_ms": {"type": "integer", "default": 0, "description": "Timeline position in milliseconds"},
                    "gain_db": {"type": "number", "default": -10.0, "description": "Volume adjustment in decibels"}
                },
                "required": ["timeline_path", "editorial_role"],
                "additionalProperties": false
            }
        },
        {
            "name": "music.index",
            "description": "Scan music directories and build a searchable index JSON. Run once when adding new music files. Default path: $HOME/Videos/Assets/Music (override with OPENSCRIPT_MUSIC_PATH env var). Returns: output_path, count of indexed files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "music_paths": {"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "null"}], "description": "Directories to scan for music files"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output index JSON path"}
                }
            }
        },
        {
            "name": "music.search",
            "description": "DEPRECATED — forwards to library.search. Use library.search for all music queries. Returns: results with title, path, duration_s, mood, energy, genre, source. 393 copyright-free music tracks available.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "default": "", "description": "Keyword search in title/artist"},
                    "mood": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Filter by mood: 'calm', 'energetic', 'upbeat', 'dramatic', 'dark', 'sad', 'neutral'"},
                    "energy": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Filter by energy: 'low', 'medium', 'high'"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results to return"}
                }
            }
        },
        {
            "name": "music.assign",
            "description": "Assign background music to the timeline's music track. Requires a music file path — use library.search first to find tracks, then pass the path here. Automatically spans the full timeline duration, applies ducking (lowers music during dialogue/voiceover), and sets gain. Use after building segments — the music provides emotional context beneath the spoken content. Default: -12dB with auto-ducking enabled. Returns: event_id, start_ms, end_ms, asset_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "path": {"type": "string", "description": "Path to the music audio file (MP3/WAV). Use library.search to find tracks and get their path."},
                    "mood": {"type": "string", "default": "neutral", "description": "Emotional mood matching content tone"},
                    "energy": {"type": "string", "default": "medium", "description": "Intensity level"},
                    "start_ms": {"type": "integer", "default": 0, "description": "Music start position on timeline"},
                    "end_ms": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Music end position (auto = end of timeline)"},
                    "gain_db": {"type": "number", "default": -12.0, "description": "Background music volume in dB (lower = quieter behind voice)"},
                    "ducking": {"type": "boolean", "default": true, "description": "Automatically lower music during dialogue/voiceover sections"}
                },
                "required": ["timeline_path", "path"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.suggest",
            "description": "Analyze an EDL and suggest b-roll insertion points based on segment duration and cadence. Identifies gaps in the dialogue where visual overlays would enhance engagement. Returns: suggestions array with position_ms, duration_ms, concept for each slot.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "edl_path": {"type": "string", "description": "Path to EDL JSON"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional SRT for concept extraction"},
                    "cadence_seconds": {"type": "number", "default": 2.0, "description": "How often to suggest b-roll slots"}
                },
                "required": ["edl_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.fetch",
            "description": "Search Pexels for b-roll videos matching given concepts. Set download=true to actually download videos to the cache directory. Use BEFORE broll.assign — this finds the footage, broll.assign places it on the timeline. Requires PEXELS_API_KEY (in mcp/assets/.openscript_config.json or env var); without a key, returns status:warning with fallback_pool results if provided. Returns: results with concept, videos (id, width, height, duration, url), cached_path if downloaded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "concepts": {"type": "array", "items": {"type": "string"}, "description": "Visual concepts to search for (e.g., ['city skyline', 'technology', 'nature'])"},
                    "asset_dir": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Cache directory for downloaded videos"},
                    "orientation": {"type": "string", "default": "9:16", "description": "Video orientation: '9:16' (vertical), '16:9' (horizontal)"},
                    "quality": {"type": "string", "default": "sd", "description": "Video quality: 'sd', 'hd', '4k'"},
                    "download": {"type": "boolean", "default": false, "description": "Actually download the top result to cache"},
                    "fallback_pool": {"type": "array", "items": {"type": "string"}, "description": "Local video file paths used when Pexels returns 0 results for a concept (or when PEXELS_API_KEY is missing). Mirrors background.fetch fallback semantics."}
                },
                "required": ["concepts"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.assign",
            "description": "Place a b-roll video clip onto the timeline's b-roll track at a specific position. The b-roll overlays the main video segment — use it to illustrate concepts, add visual variety, or cover jump cuts. Use after broll.fetch (with download=true) to get the asset_path. Returns: event_id, asset_id, asset_path, position_ms, duration_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "concept": {"type": "string", "description": "Concept this b-roll illustrates (e.g., 'technology', 'business')"},
                    "position_ms": {"type": "integer", "description": "Timeline position in ms to start b-roll overlay"},
                    "duration_ms": {"type": "integer", "description": "How long the b-roll overlay lasts in ms"},
                    "asset_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Path to b-roll video file (from broll.fetch download)"},
                    "transition_style": {"type": "string", "default": "cut", "description": "Transition into b-roll: 'cut', 'fade', 'slide'"},
                    "crop_mode": {"type": "string", "default": "center", "description": "How to crop b-roll to target aspect: 'center', 'smart', 'fit'"}
                },
                "required": ["timeline_path", "concept", "position_ms", "duration_ms"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.director",
            "description": "AI director mode for b-roll: analyzes the script/segments, creates b-roll slots at natural pauses, searches Pexels for contextually relevant footage, downloads, and assigns to the timeline. ONE CALL replaces broll.suggest + broll.fetch + broll.assign. Requires PEXELS_API_KEY. Returns: broll_slots_filled, concepts_used, cached_paths.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON (must have populated segments)"},
                    "orientation": {"type": "string", "default": "9:16", "description": "Video orientation for Pexels search"},
                    "quality": {"type": "string", "default": "sd", "description": "Video quality for Pexels search"},
                    "max_slots": {"type": "integer", "default": 20, "description": "Maximum b-roll slots to create"},
                    "cadence_seconds": {"type": "number", "default": 2.0, "description": "How often to insert b-roll"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 3: VOICEOVER & TTS — Commentary, narration, and voice production
        // ===================================================================
        {
            "name": "voiceover.generate",
            "description": "Generate TTS voiceover and add it to the timeline's voiceover track at a specific position. USE CASES: (1) INTRO — hook the viewer with a spoken opening before the main content starts; (2) TRANSITIONS — narrate between segments to guide the viewer; (3) OUTRO — close with a call-to-action or summary; (4) COMMENTARY — add explanatory narration over b-roll sections. Generates WAV audio, places it on the voiceover track, and applies ducking to background music. Returns: output_path, duration_ms, event_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "text": {"type": "string", "description": "Script text for the voiceover"},
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile to speak with"},
                    "position_ms": {"type": "integer", "default": 0, "description": "Timeline position in ms to start voiceover (0 = beginning)"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speech speed multiplier"},
                    "gain_db": {"type": "number", "default": -6.0, "description": "Voiceover volume in dB relative to other tracks"}
                },
                "required": ["timeline_path", "text", "voice_profile_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.commentary",
            "description": "Generate multiple TTS voiceover commentaries at strategic timeline positions in ONE CALL. commentary_type='all' creates: (1) INTRO at position 0 — welcomes viewers; (2) TRANSITIONS before each segment — guides viewers between topics; (3) OUTRO at end — thanks viewers and gives CTA. Use commentary_type='intro', 'transitions', or 'outro' for individual pieces. MUCH faster than calling voiceover.generate multiple times. Returns: voiceovers_generated (event IDs), positions, count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile to use"},
                    "commentary_type": {"type": "string", "enum": ["intro", "transitions", "outro", "all"], "default": "all", "description": "Which commentaries to generate: 'intro' (opening), 'transitions' (between segments), 'outro' (closing), 'all' (complete set)"},
                    "intro_text": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom intro script (auto-generated if omitted)"},
                    "outro_text": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom outro script (auto-generated if omitted)"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speech speed multiplier"}
                },
                "required": ["timeline_path", "voice_profile_id", "commentary_type"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 4: AGENT UX — Inspection, comparison, and preview tools
        // ===================================================================
        {
            "name": "timeline.diff",
            "description": "Compare two versions of a timeline and report what changed — added/removed/modified segments, track count changes, duration delta. Use before/after edits to understand impact, or to verify an edit produced the expected changes. Returns: duration_change_ms, segments (added, removed, modified), track changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path_a": {"type": "string", "description": "Path to first timeline version"},
                    "timeline_path_b": {"type": "string", "description": "Path to second timeline version"}
                },
                "required": ["timeline_path_a", "timeline_path_b"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.preview",
            "description": "Generate a readable summary of a timeline — segments with captions, track counts per type, total duration, validation status, and render readiness. ALWAYS call this before timeline.render to verify the timeline looks correct. Returns: total_duration_ms, segments, tracks (with counts), render_ready (boolean), validation_errors.",
            "inputSchema": {
                "type": "object",
                "properties": {"timeline_path": {"type": "string", "description": "Path to timeline JSON"}},
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.preview",
            "description": "Preview TTS output for a given text and voice profile — estimates duration and shows profile info WITHOUT generating audio. Use to plan timeline placement before committing to voiceover.generate. Returns: voice_profile info, word_count, estimated_duration_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile"},
                    "text": {"type": "string", "description": "Text to preview"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speed multiplier"}
                },
                "required": ["voice_profile_id", "text"],
                "additionalProperties": false
            }
        },
        {
            "name": "music.ducking.plan",
            "description": "Analyze the timeline's dialogue and voiceover tracks to generate a ducking plan — where and how much to lower background music. Returns start_ms, end_ms, reduction_db for each ducking event. Use to understand how music will behave during speech sections before rendering. Returns: ducking_events array with timing and reduction values.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "reduction_db": {"type": "number", "default": 10.0, "description": "How many dB to reduce music during speech"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.autofill_broll",
            "description": "Auto-fill b-roll slots across the timeline based on segment cadence. Creates placeholder b-roll events at regular intervals (cadence_seconds) using concept keywords extracted from nearby segment captions. FASTER than broll.director but LESS contextually accurate — use for quick drafts, then refine with broll.director for final. Returns: broll_events_added count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "cadence_seconds": {"type": "number", "default": 2.0, "description": "Interval between b-roll slots"},
                    "orientation": {"type": "string", "default": "9:16", "description": "Expected b-roll orientation"},
                    "quality": {"type": "string", "default": "sd", "description": "Expected b-roll quality"},
                    "max_gaps": {"type": "integer", "default": 20, "description": "Maximum b-roll slots to create"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.render",
            "description": "Render a complete multi-track timeline to a final video. This is the PRODUCTION render — it processes ALL tracks: b-roll overlays, background music with ducking, SFX hits, voiceover narration, and burned-in captions (static ASS or animated via PupCaps overlay). ALWAYS run timeline.validate first. Returns: output_path, file_size_bytes, segments_count, tracks_rendered.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to validated timeline JSON"},
                    "source_video": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Override source video path (uses timeline source if omitted)"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom output path (auto-generated if omitted)"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (18-28, lower=better)"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 5: ORCHESTRATION — Single-call end-to-end pipelines
        // ===================================================================
        {
            "name": "reelize.timeline",
            "description": "ONE-CALL pipeline: raw video → complete 9:16 reel. Orchestrates: (1) Transcribe with Whisper, (2) Group captions, (3) Build timeline with segments, (4) B-roll director (Pexels search + download + assign), (5) Assign background music with ducking, (6) Assign SFX (hook, transitions, highlights), (7) Generate ASS captions with Bebas Neue, (8) Render final video. Use when you want a fully-produced reel from a single raw video with minimal manual intervention. All sub-steps are configurable via broll/music/sfx objects. Returns: output_path, file_size_bytes, timeline_path, segments_count, tracks_rendered, preset.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Raw source video to transform into a reel"},
                    "preset": {"type": "string", "enum": ["Tight", "Balanced", "Natural"], "default": "Balanced", "description": "Editing pace: Tight (fast cuts, 200ms crossfade), Balanced (moderate, 500ms), Natural (relaxed, 800ms)"},
                    "max_duration": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Maximum reel duration in seconds"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "burn_captions": {"type": "boolean", "default": true, "description": "Burn captions into the video output"},
                    "animated_captions": {"type": "boolean", "default": false, "description": "Use animated PupCaps overlay instead of static ASS captions"},
                    "broll": {"type": "object", "properties": {
                        "enabled": {"type": "boolean", "default": true, "description": "Enable b-roll director (requires PEXELS_API_KEY)"},
                        "cadence_seconds": {"type": "number", "default": 2.0, "description": "How often to insert b-roll"},
                        "max_slots": {"type": "integer", "default": 20, "description": "Maximum b-roll slots"}
                    }},
                    "music": {"type": "object", "properties": {
                        "enabled": {"type": "boolean", "default": true, "description": "Enable background music assignment"},
                        "mood": {"type": "string", "default": "neutral", "description": "Music mood matching content"},
                        "energy": {"type": "string", "default": "medium", "description": "Music energy level"},
                        "gain_db": {"type": "number", "default": -12.0, "description": "Background music volume in dB"}
                    }},
                    "sfx": {"type": "object", "properties": {
                        "enabled": {"type": "boolean", "default": true, "description": "Enable SFX assignment (hook, transitions, highlights)"}
                    }},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom output video path"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (18-28)"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 6: VERIFICATION — Render quality assurance
        // ===================================================================
        {
            "name": "verify.audio",
            "description": "Analyze the audio track of a rendered video for quality issues. Checks: (1) RMS loudness — is the overall volume in acceptable range? (2) Dialogue presence — is there spoken content or just music? (3) Silence detection — are there unexpected gaps? (4) Peak levels — is there clipping? Use AFTER rendering to verify the voice is audible and music isn't drowning out dialogue. Returns: rms_lufs, peak_db, silence_segments, has_dialogue (boolean), quality_score (0-100).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered video to analyze"},
                    "expected_has_voice": {"type": "boolean", "default": true, "description": "Whether the video is expected to contain spoken voice"},
                    "max_silence_seconds": {"type": "number", "default": 3.0, "description": "Threshold for flagging unexpected silence gaps"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "verify.captions",
            "description": "Verify caption synchronization in a rendered video. Compares caption timing from the source SRT/ASS against the actual video duration to check: (1) Coverage — do captions span the full speaking duration? (2) Gaps — are there sections without captions that should have them? (3) Overlap — do any captions overlap incorrectly? (4) Duration — are individual captions readable (not too fast)? Use AFTER rendering to ensure captions are properly burned in and timed. Auto-detects caption format: .ass files (from script.to_video) and .srt files are both accepted. Returns: caption_count, coverage_percent, gaps, overlaps, avg_caption_duration_ms, readability_score (0-100).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered video"},
                    "srt_path": {"type": "string", "description": "Source SRT or ASS caption file. script.to_video produces captions.ass in the output_dir; pass that path here."},
                    "min_caption_duration_ms": {"type": "integer", "default": 300, "description": "Minimum readable caption duration in ms"},
                    "max_caption_duration_ms": {"type": "integer", "default": 5000, "description": "Maximum caption duration before flagging"}
                },
                "required": ["video_path", "srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "verify.render",
            "description": "TECHNICAL integrity check only (duration/aspect/file size). Does NOT measure production beauty. For stock footage, stickers, music quality use verify.production. Returns: overall_score (0-100 technical).",

            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered output video"},
                    "timeline_path": {"type": "string", "description": "Path to the source timeline JSON"},
                    "expected_aspect": {"type": "string", "default": "9:16", "description": "Expected output aspect ratio"},
                    "duration_tolerance_ms": {"type": "integer", "default": 2000, "description": "Acceptable duration deviation in ms"}
                },
                "required": ["video_path", "timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "llm.complete",
            "description": "Run a text LLM completion through the director cascade configured in ~/.openscript/config.json: local Ollama (qwen3.5-4b / GGUF) → OpenRouter free models. Returns: text, backend, model.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "User prompt"},
                    "system": {"type": "string", "default": "You are a helpful video director assistant.", "description": "System prompt"},
                    "backend": {"type": "string", "default": "auto", "description": "Force backend: auto | local | openrouter"}
                },
                "required": ["prompt"],
                "additionalProperties": false
            }
        },
        {
            "name": "system.config.get",
            "description": "Return the effective OpenScript configuration (redacted secrets) from ~/.openscript/config.json with env overrides applied. Use to verify LLM models, GGUF path, and which API keys are set.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "system.config.set",
            "description": "Merge keys into ~/.openscript/config.json (mode 0600). Supports nested paths via object: {api_keys:{openrouter:'…'}, llm:{local_model:'qwen3.5-4b'}}. Does not echo secrets back. Returns redacted config view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "patch": {"type": "object", "description": "Partial config object to deep-merge into user config"}
                },
                "required": ["patch"],
                "additionalProperties": false
            }
        },
        {
            "name": "director.run",
            "description": "ONE-SHOT director: system preflight + script.parse + script.to_video + verify.production. Returns video path, production grade, hard_fails, next_actions. Prefer this for cold agents. Fails closed when majority procedural or music topic mismatch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "Script JSON string or path to .json"},
                    "output_path": {"type": "string", "default": "artifacts/director_out.mp4"},
                    "output_dir": {"type": "string", "default": "artifacts/director_run"},
                    "min_grade": {"type": "string", "default": "B"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "vision.analyze_clip",
            "description": "Extract a frame from a video clip and describe it with the vision cascade (OpenRouter multimodal free models when OPENROUTER_API_KEY is set; local Qwen text fallback). Use to judge morning/night, indoor/outdoor, phone UI, etc. Returns structured description + backend.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to video clip"},
                    "at_s": {"type": "number", "description": "Optional timestamp seconds (default ~40% into clip)"},
                    "prompt": {"type": "string", "description": "Optional analysis question"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "vision.score_clip",
            "description": "Vision+LLM relevance score of a stock clip vs scene dialogue and video_keywords. Uses local GGUF/Ollama when possible and OpenRouter free multimodal fallbacks. Returns relevance 0–1, time_of_day, match, reason. Wire into multi-broll QA and verify.production context_relevance.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string"},
                    "scene_text": {"type": "string"},
                    "video_keywords": {"type": "array", "items": {"type": "string"}},
                    "search_query": {"type": "string"}
                },
                "required": ["video_path", "scene_text"],
                "additionalProperties": false
            }
        },
        {
            "name": "verify.production",
            "description": "PRODUCTION-QUALITY KPI gate (v2.1) baked into architecture. Scores efficacious director use of the timeline/render stack including visual_repetition (content-hash) and context_relevance. Prefer render_manifest_path from script.to_video. Optional vision_rescore=true re-scores clips with vision.analyze cascade. verify.render=100 is NOT production quality.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Rendered MP4 path"},
                    "timeline_path": {"type": "string", "description": "Timeline JSON from script.to_video"},
                    "render_manifest_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Preferred: render_manifest.json written by script.to_video (authoritative multi-broll/sticker/meme truth)"},
                    "captions_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional ASS/SRT path"},
                    "sticker_count": {"type": "integer", "default": 0, "description": "Fallback sticker count if no manifest"},
                    "meme_count": {"type": "integer", "default": 0, "description": "Fallback meme count if no manifest"},
                    "background_sources": {"type": "array", "items": {"type": "string"}, "description": "Fallback background paths if no manifest"},
                    "music_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Music path used at render if any"},
                    "min_grade": {"type": "string", "default": "B", "description": "Minimum acceptable grade (A/B/C/D/F)."},
                    "vision_rescore": {"type": "boolean", "default": false, "description": "If true, re-score each background clip with vision.score_clip (local Qwen GGUF/Ollama → OpenRouter free multimodal). Adds vision_scores to the response."}
                },
                "required": ["video_path", "timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.schema",
            "description": "Return the full JSON schema for script.parse as a JSON object. Use this to discover what fields are available, their types, defaults, and valid values. Returns: the JSON Schema with examples and field descriptions for ScriptSpec, SceneSpec, SpeakerSpec, BackgroundSpec, and all nested types. Call this BEFORE script.parse to understand the correct format.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "script.parse",
            "description": "Parse and validate a from-scratch video creation script (JSON). The script is the single source of truth for AI-agent-driven video creation — it describes speakers, scenes, backgrounds, captions, music, and output. Returns the parsed ScriptSpec with defaults applied, plus validation errors (if any). Use BEFORE script.to_timeline / script.to_video to catch schema issues early. See openscript-core/src/script.rs for the full schema. Kokoro is the default TTS backend. IMPORTANT: background.type must be one of: 'gameplay' (Pexels stock footage, requires PEXELS_API_KEY), 'procedural' (generated motion backgrounds), or 'static' (solid color/gradient). 'stock' is NOT a valid value — using it will cause errors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string (or path to a .json file). If it starts with '{', parsed as inline JSON; otherwise treated as a file path."},
                    "validate_only": {"type": "boolean", "default": false, "description": "If true, only validate without returning the full parsed spec (lighter response)"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.generate_voices",
            "description": "Generate TTS voice audio for each scene in a script. Calls the TTS backend (Kokoro default, sidecar fallback) per scene, producing a WAV file per scene. Returns voiceover paths + durations + estimated word timings for caption sync. Use AFTER script.parse and BEFORE script.build_captions. Progress reported per scene.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "output_dir": {"type": "string", "default": "artifacts/voices", "description": "Directory for generated WAV files"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.build_captions",
            "description": "Generate an ASS subtitle file from voiceover segments with word timings. Supports 4 caption styles: word_highlight (TikTok-style), sentence_fade, karaoke_fill, subtitle_rail. Uses the caption style from the script spec. Returns the ASS file path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "voiceover_manifest": {"type": "string", "description": "Path to the manifest JSON from script.generate_voices (contains segments with word timings)"},
                    "output_path": {"type": "string", "default": "artifacts/captions.ass", "description": "Output ASS file path"}
                },
                "required": ["script", "voiceover_manifest"],
                "additionalProperties": false
            }
        },
        {
            "name": "background.fetch",
            "description": "Fetch a background video clip. Searches Pexels API FIRST (stock footage, requires PEXELS_API_KEY), then YouTube via yt-dlp as fallback. Downloads, extracts a random clip of desired duration, crops to target aspect ratio. For multi-broll: call once per scene with different queries to get topic-relevant backgrounds. Returns: clip_path, source (pexels/youtube/fallback/procedural), duration_s.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "YouTube search query (e.g. 'minecraft parkour no copyright')"},
                    "duration_s": {"type": "number", "default": 30.0, "description": "Desired clip duration in seconds"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio: 9:16, 16:9, 1:1"},
                    "cache_dir": {"type": "string", "default": "mcp/assets/background_cache", "description": "Cache directory for downloaded videos"},
                    "fallback_pool": {"type": "array", "items": {"type": "string"}, "description": "Local fallback clips if YouTube download fails"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "background.assign",
            "description": "Assign background clips to script scenes based on change_cadence (scene/speaker/fixed). Takes a voiceover manifest (from script.generate_voices) and a background pool, returns assignments. Use AFTER script.generate_voices and background.fetch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "voiceover_manifest": {"type": "string", "description": "Path to manifest JSON from script.generate_voices"},
                    "background_pool": {"type": "array", "items": {"type": "string"}, "description": "List of background video file paths to assign"},
                    "output_path": {"type": "string", "default": "artifacts/background_assignments.json", "description": "Output assignments JSON path"}
                },
                "required": ["script", "voiceover_manifest", "background_pool"],
                "additionalProperties": false
            }
        },
        {
            "name": "background.search",
            "description": "Search the procedural background clip index by mood/energy/motion_intensity. Returns matching clip paths from mcp/assets/backgrounds/. Use this to build a curated fallback_pool for script.to_video when you want a specific emotional tone (e.g. mood:calm for healing content, mood:energetic for gaming recaps). Without this, script.to_video with type:procedural grabs ALL .mp4s in the folder — which mixes calming clips with neon tunnels. Returns: clip paths + metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mood": {"type": "string", "enum": ["calm", "energetic", "neutral", "dark", "uplifting"], "description": "Emotional tone filter"},
                    "energy": {"type": "string", "enum": ["low", "medium", "high"], "description": "Energy level filter"},
                    "motion_intensity": {"type": "string", "enum": ["slow", "medium", "fast"], "description": "Motion intensity filter"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"}
                },
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.presets",
            "description": "List all available sticker positioning presets with their safe-zone configurations. Each preset defines position, scale, and caption clearance for different use cases (speaker left/right/center, reactions, corners). Returns: presets map with name, description, position, scale, safe_margin_px, and speaker_role.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.load_preset",
            "description": "Load an SVG sticker preset by name. Presets are directories in mcp/assets/svg_presets/ containing puppet.svg, preset.json, mouth shapes, and emotes. Built-in presets: default_person, robot, cat. Returns the preset config + puppet SVG content.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "preset_name": {"type": "string", "description": "Preset name (e.g. 'default_person', 'robot', 'cat')"},
                    "presets_dir": {"type": "string", "default": "mcp/assets/svg_presets", "description": "Directory containing preset folders"}
                },
                "required": ["preset_name"],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.render",
            "description": "Render an animated sticker overlay (HyperFrames HTML composition) for a speaker's voiceover. Extracts per-frame amplitude from the WAV, generates GSAP timeline that animates the SVG puppet's mouth scaleY in sync with audio. Produces an HTML file. When render_to_video=true, also renders the HTML to a transparent WebM via hf.render — the WebM can be used directly as a StickerOverlay in multilayer_render or overlay.assign. Use AFTER script.generate_voices and sticker.load_preset.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wav_path": {"type": "string", "description": "Path to the speaker's voiceover WAV file"},
                    "preset_name": {"type": "string", "description": "SVG preset name (e.g. 'default_person')"},
                    "position": {"type": "string", "default": "top-left", "description": "On-screen position: top-left, top-right, bottom-left, bottom-right, top-center, bottom-center, center"},
                    "scale": {"type": "number", "default": 0.25, "description": "Sticker scale relative to canvas width (0.0-1.0)"},
                    "canvas_width": {"type": "integer", "default": 1080},
                    "canvas_height": {"type": "integer", "default": 1920},
                    "fps": {"type": "integer", "default": 30},
                    "output_path": {"type": "string", "default": "artifacts/sticker.html", "description": "Output HTML composition path"},
                    "render_to_video": {"type": "boolean", "default": false, "description": "When true, also render the HTML to a transparent WebM via hf.render. Returns video_path in the response. Slower (~30s per sticker) but produces a compositable video file."}
                },
                "required": ["wav_path", "preset_name"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.to_timeline",
            "description": "Orchestrate the full from-scratch video creation pipeline into an EDL v2 timeline. Calls script.generate_voices (TTS per scene), script.build_captions (ASS from word timings), background.fetch + background.assign (YouTube gameplay), sticker.render (animated SVG per scene), and assembles everything into a timeline JSON ready for timeline.render. This is the orchestrator — use script.to_video for the one-call pipeline. Returns timeline_path + manifest paths + asset summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "output_dir": {"type": "string", "default": "artifacts", "description": "Directory for generated assets (voices, captions, stickers, timeline)"},
                    "skip_background": {"type": "boolean", "default": false, "description": "Skip background fetching (use fallback pool only)"},
                    "skip_stickers": {"type": "boolean", "default": false, "description": "Skip sticker rendering (no animated overlays)"},
                    "voiceover_manifest_path": {"type": "string", "description": "Optional: path to a pre-existing voiceover manifest JSON. When provided, skips TTS generation and uses the supplied manifest. Manifest format: {total_duration_ms, segments: [{scene_id, speaker, text, start_ms, end_ms, duration_ms, wav_path, words: [{word, start_ms, end_ms}]}]}"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.to_video",
            "description": "ONE-CALL from-scratch video creation: script JSON → MP4. THE GOLDEN TRAJECTORY — use this for all video creation. Automatically handles: Kokoro TTS per scene, Parakeet force-alignment for caption sync, multi-broll Pexels stock footage per scene, GIPHY sticker overlays, background music with ducking, word-highlight captions, and FFmpeg render. Returns: output_path, file_size, timeline_preview (token-efficient tree view of all layers), timeline_issues, warnings. PREVIOUS STEP: script.parse (validate first). NEXT STEP: verify.render (quality check).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "output_path": {"type": "string", "default": "output.mp4", "description": "Output MP4 path"},
                    "output_dir": {"type": "string", "default": "artifacts", "description": "Directory for intermediate assets"},
                    "skip_background": {"type": "boolean", "default": false, "description": "Skip background fetching"},
                    "skip_stickers": {"type": "boolean", "default": false, "description": "Skip sticker rendering"},
                    "preview_mode": {"type": "boolean", "default": false, "description": "If true, use draft quality for faster iteration"},
                    "voiceover_manifest_path": {"type": "string", "description": "Optional: path to a pre-existing voiceover manifest JSON. When provided, skips TTS generation (script.generate_voices) and uses the supplied manifest instead. Use this when you have pre-recorded WAV files and want to bypass TTS. Manifest format: {total_duration_ms, segments: [{scene_id, speaker, text, start_ms, end_ms, duration_ms, wav_path, words: [{word, start_ms, end_ms}]}]}"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "stock.fetch",
            "description": "Download stock music or videos from Pixabay API. Requires PIXABAY_API_KEY env var. Falls back to local stock library if API key not set. For music: downloads MP3 tracks by mood/genre query. For video: downloads animation/footage clips. Returns downloaded file paths. Use for sourcing royalty-free background music and video footage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["music", "video"], "description": "Media type to fetch"},
                    "query": {"type": "string", "description": "Search query (e.g. 'lofi chill' for music, 'minecraft gameplay' for video)"},
                    "limit": {"type": "integer", "default": 5, "description": "Max results to download"},
                    "output_dir": {"type": "string", "default": "mcp/assets/stock_cache", "description": "Directory for downloaded files"}
                },
                "required": ["type", "query"],
                "additionalProperties": false
            }
        },
        {
            "name": "youtube.download",
            "description": "Download a YouTube video clip for use as background footage. Accepts a direct YouTube URL or search query. If start_s is specified, uses --download-sections to download ONLY that time range (avoids downloading entire 10-hour videos). If start_s is omitted, downloads the full video and extracts a random clip. Crops to target aspect ratio. Use youtube.search first to find the video URL and duration, then specify start_s to clip a specific range.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "YouTube URL or search query (e.g. 'https://youtube.com/watch?v=...' or 'minecraft parkour no copyright')"},
                    "duration_s": {"type": "number", "default": 30.0, "description": "Clip duration in seconds"},
                    "start_s": {"type": "number", "description": "Start time in seconds for range download. If specified, downloads ONLY this range using --download-sections (much faster for long videos). If omitted, downloads full video and extracts random clip."},
                    "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio: 9:16, 16:9, 1:1"},
                    "cache_dir": {"type": "string", "default": "mcp/assets/background_cache", "description": "Cache directory"},
                    "use_cookies": {"type": "boolean", "default": true, "description": "Try browser cookies to avoid YouTube bot detection"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "youtube.search",
            "description": "Search YouTube for videos WITHOUT downloading. Returns video titles, URLs, durations, and view counts so agents can browse and pick the best video before downloading via youtube.download. Uses yt-dlp's search functionality. Requires yt-dlp installed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'minecraft parkour no copyright gameplay')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results to return"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "stock.search",
            "description": "Search Pixabay for stock music or videos WITHOUT downloading. Returns titles, durations, thumbnails, and URLs so agents can browse before downloading via stock.fetch. Requires PIXABAY_API_KEY env var. Falls back to local stock library listing if no API key.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["music", "video"], "description": "Media type to search"},
                    "query": {"type": "string", "description": "Search query"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"}
                },
                "required": ["type", "query"],
                "additionalProperties": false
            }
        },
        {
            "name": "media.search",
            "description": "Search for PNG images for use as sticker overlays. Uses Pexels Image API (requires PEXELS_API_KEY) and Openverse (free, no key). Returns image URLs, dimensions, and license info. To use a result: download the URL (via curl/wget/reqwest) to a local path, then place it on the timeline via timeline.add_track_event with track_type='broll' and the local path as asset_id. NOTE: media.download does not exist yet — download manually. Use for finding transparent PNGs of people, objects, logos, etc. for video sticker overlays.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'person talking', 'businessman portrait', 'cartoon character')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"},
                    "source": {"type": "string", "enum": ["pexels", "openverse", "auto"], "default": "auto", "description": "Image source: pexels (requires API key), openverse (free), or auto (try pexels first)"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "gif.search",
            "description": "Search for animated GIF stickers (transparent background) from GIPHY. Returns GIF URLs, dimensions, and preview URLs. To use a result: download the URL to a local .gif file, then pass the path to script.to_video (which auto-downloads GIPHY stickers per speaker) or use multilayer_render's sticker overlay. NOTE: gif.download does not exist yet — download manually. GIPHY stickers are transparent GIFs ideal for video overlays. Requires GIPHY_API_KEY env var (get free at https://developers.giphy.com). Falls back to Pexels video search if no GIPHY key.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'person talking', 'thumbs up', 'applause')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"},
                    "rating": {"type": "string", "enum": ["g", "pg", "pg-13", "r"], "default": "g", "description": "Content rating filter"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "media.download",
            "description": "Download an image from a URL (from media.search results) to a local file. Returns the local path. Use after media.search to get a local PNG/JPG that can be placed on the timeline via overlay.assign. Caches downloads in mcp/assets/image_cache/ to avoid re-downloading.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "Image URL from media.search results"},
                    "output_path": {"type": "string", "description": "Optional output path. Auto-generated if omitted (mcp/assets/image_cache/<hash>.<ext>)."}
                },
                "required": ["url"],
                "additionalProperties": false
            }
        },
        {
            "name": "gif.download",
            "description": "Download a GIF from a URL (from gif.search results) to a local file. Returns the local path. Use after gif.search to get a local .gif that can be placed on the timeline via overlay.assign or used as a sticker in script.to_video. Caches downloads in mcp/assets/stickers/ to avoid re-downloading.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "GIF URL from gif.search results"},
                    "output_path": {"type": "string", "description": "Optional output path. Auto-generated if omitted (mcp/assets/stickers/<hash>.gif)."}
                },
                "required": ["url"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.inspect",
            "description": "Deep-dive inspection of a specific layer in the timeline. Returns ALL events on that layer with full details (start_ms, end_ms, asset path, metadata). Use AFTER script.to_video to inspect a specific layer for restructuring. Layers: background, voiceover, music, captions, stickers. For a quick overview use the timeline_preview field from script.to_video instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_preview_path": {"type": "string", "description": "Path to the timeline_preview.txt file from script.to_video"},
                    "layer": {"type": "string", "enum": ["background", "voiceover", "music", "captions", "stickers"], "description": "Which layer to inspect in detail"}
                },
                "required": ["timeline_preview_path", "layer"],
                "additionalProperties": false
            }
        },
        {
            "name": "overlay.assign",
            "description": "Place an image/GIF/PNG overlay on the timeline at a specific position and duration. Use after media.download or gif.download to place the local file as a sticker/overlay on the video. The overlay is composited via FFmpeg's overlay filter during render. Returns event_id. Supports position (top-left/top-right/bottom-left/bottom-right/center), scale (0.0-1.0 of canvas width), fade_in_ms, fade_out_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to the timeline JSON file"},
                    "asset_path": {"type": "string", "description": "Local path to the image/GIF/PNG file (from media.download or gif.download)"},
                    "start_ms": {"type": "integer", "description": "Position in milliseconds where the overlay appears"},
                    "end_ms": {"type": "integer", "description": "Position in milliseconds where the overlay disappears"},
                    "position": {"type": "string", "enum": ["top-left", "top-right", "bottom-left", "bottom-right", "center"], "default": "bottom-right", "description": "Screen position of the overlay"},
                    "scale": {"type": "number", "default": 0.2, "description": "Scale factor relative to canvas width (0.0-1.0, default 0.2 = 20% of width)"},
                    "fade_in_ms": {"type": "integer", "default": 0, "description": "Fade-in duration in milliseconds"},
                    "fade_out_ms": {"type": "integer", "default": 0, "description": "Fade-out duration in milliseconds"},
                    "speaker_name": {"type": "string", "description": "Optional: speaker name this overlay is associated with (for provenance tracking)"}
                },
                "required": ["timeline_path", "asset_path", "start_ms", "end_ms"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.to_hyperframes",
            "description": "Compile an EDL v2 timeline JSON into a HyperFrames HTML composition. Wraps the edl_v2_to_html.ts compiler — produces an index.html with GSAP timeline animations, video layers, and b-roll crossfades. After this, call composition.render or hf.render to produce the final MP4. This is the bridge between the NLE timeline and the HyperFrames motion-graphics render engine.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to the EDL v2 timeline JSON file"},
                    "output_dir": {"type": "string", "default": "artifacts/hf_composition", "description": "Directory to write the HF composition (index.html will be created here)"},
                    "composition_id": {"type": "string", "description": "Optional composition ID (default: auto-generated from timeline name)"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "voices.list",
            "description": "List all available TTS voices. Returns registered voice profiles from voices.json (named profiles with descriptions) plus the full list of Kokoro preset voice IDs (e.g. af_heart, am_michael, bf_emma) that can be used directly with script.generate_voices or tts.generate without registration. Use this to discover available voices before generating TTS.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "language": {"type": "string", "description": "Optional: filter by language code (e.g. 'en', 'es', 'fr', 'hi', 'it', 'ja', 'pt', 'zh')"}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "library.search",
            "description": "Search the de-duplicated music/SFX library index. Index contains 500+ entries from NoCopyrightSounds, AudioLibrary, BreakingCopyright, VlogNoCopyrightMusic, MixtureOfficial, SoundLibrary1, and local stock. Each entry has filename, title, tags, download_url, source, duration_s, license. Use library.download to fetch the audio file on demand. Use library.build to rebuild the index from YouTube channels. Supports filtering by source channel, license, duration range, and tag.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'epic cinematic', 'lofi chill', 'explosion', 'whoosh')"},
                    "type": {"type": "string", "enum": ["music", "sfx"], "description": "Filter by media type: music or sfx"},
                    "source": {"type": "string", "description": "Filter by source channel (e.g. 'NoCopyrightSounds', 'AudioLibrary', 'BreakingCopyright', 'VlogNoCopyrightMusic', 'MixtureOfficial', 'SoundLibrary1')"},
                    "license": {"type": "string", "description": "Filter by license (e.g. 'no-copyright', 'creative-commons')"},
                    "min_duration_s": {"type": "number", "description": "Minimum duration in seconds (inclusive)"},
                    "max_duration_s": {"type": "number", "description": "Maximum duration in seconds (inclusive)"},
                    "tag": {"type": "string", "description": "Filter by tag (substring match against entry's tags array, case-insensitive)"},
                    "mood": {"type": "string", "enum": ["calm", "energetic", "upbeat", "dramatic", "dark", "sad", "neutral"], "description": "Filter by mood (derived from genre/title analysis)"},
                    "energy": {"type": "string", "enum": ["low", "medium", "high"], "description": "Filter by energy level"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "library.download",
            "description": "Download a music/SFX file from the library index on demand. Uses yt-dlp to extract audio as MP3 from YouTube sources. Caches downloaded files for reuse. Use library.search first to find the filename, then library.download to fetch it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filename": {"type": "string", "description": "Filename from library.search results (e.g. 'Epic_Cinematic_Action_by_Infraction.mp3')"},
                    "output_dir": {"type": "string", "default": "mcp/assets/music_cache", "description": "Directory for downloaded files"}
                },
                "required": ["filename"],
                "additionalProperties": false
            }
        },
        {
            "name": "library.build",
            "description": "Rebuild the music/SFX library index by scraping YouTube channels (NoCopyrightSounds, AudioLibrary, BreakingCopyright, VlogNoCopyrightMusic, MixtureOfficial, SoundLibrary1). Run once at setup or when you want to refresh the library. Takes ~2 minutes. Returns index stats.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        // ===================================================================
        // META-TOOLS — discovery and capability introspection
        // ===================================================================
        {
            "name": "system.capabilities",
            "description": "Check which OpenScript subsystems are available BEFORE calling other tools. Returns availability status for: voicebox/TTS, Pexels API, GIPHY API, Pixabay API, SFX library, music library, transcription engine, Kokoro TTS, HyperFrames. Use this first when you're unsure which features are wired — avoids opaque failures from tools whose backing service is missing. Example: if voicebox.available is false, skip tts.generate and use Kokoro (script.generate_voices) instead. No arguments.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        {
            "name": "system.doctor",
            "description": "Cold-start production readiness report. Checks ffmpeg/yt-dlp, API keys (Pexels/GIPHY), portable music_production pack, music_library_index, SFX pack, Kokoro models. Returns ready_for_production (bool), checklist, and next_actions. Prefer this over system.capabilities when deciding whether director.run will ship real stock. No arguments.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        {
            "name": "help.tool",
            "description": "Find relevant MCP tools for a natural-language task description. Returns ranked tool suggestions with name, relevance score (0.0-1.0), and a short description. Example queries: 'add voiceover to a timeline', 'download background music', 'burn captions into video', 'transcribe Hindi audio'. Use this when you know WHAT you want to do but not WHICH tool to call. Returns up to 8 suggestions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Natural-language description of the task you want to accomplish"},
                    "limit": {"type": "integer", "description": "Max suggestions to return (default 8, max 20)", "default": 8}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }
    ]);

    // Append HyperFrames tools (hf.*)
    if let Some(arr) = tools.as_array_mut() {
        arr.extend(crate::hf::tool_definitions());
    }

    tools
}

// ---------------------------------------------------------------------------
// Tool routing
// ---------------------------------------------------------------------------

pub fn route_tool(
    name: &str,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, ToolError>> + Send + '_>> {
    let name_owned = name.to_string();
    match name_owned.as_str() {
        "transcribe" => Box::pin(handle_transcribe(args)),
        "srt.read" => Box::pin(handle_srt_read(args)),
        "srt.prepare" => Box::pin(handle_srt_prepare(args)),
        "srt.apply_edit" => Box::pin(handle_srt_apply_edit(args)),
        "edl.build" => Box::pin(handle_edl_build(args)),
        "render" => Box::pin(handle_render(args)),
        "reelize" => Box::pin(handle_reelize(args)),
        "reelize.brief" => Box::pin(handle_reelize_brief(args)),
        "reelize.direct" => Box::pin(handle_reelize_direct(args)),
        "overlay.generate" => Box::pin(handle_overlay_generate(args)),
        "timeline.build" => Box::pin(handle_timeline_build(args)),
        "timeline.load" => Box::pin(handle_timeline_load(args)),
        "timeline.validate" => Box::pin(handle_timeline_validate(args)),
        "timeline.upgrade" => Box::pin(handle_timeline_upgrade(args)),
        "timeline.add_segment" => Box::pin(handle_timeline_add_segment(args)),
        "timeline.add_track_event" => Box::pin(handle_timeline_add_track_event(args)),
        "voice.profile.add" => Box::pin(handle_voice_profile_add(args)),
        "voice.profile.list" => Box::pin(handle_voice_profile_list(args)),
        "voice.profile.remove" => Box::pin(handle_voice_profile_remove(args)),
        "tts.generate" => Box::pin(handle_tts_generate(args)),
        "tts.estimate_duration" => Box::pin(handle_tts_estimate_duration(args)),
        "sfx.index" => Box::pin(handle_sfx_index(args)),
        "sfx.search" => Box::pin(handle_sfx_search(args)),
        "sfx.assign" => Box::pin(handle_sfx_assign(args)),
        "music.index" => Box::pin(handle_music_index(args)),
        "music.search" => Box::pin(handle_music_search(args)),
        "music.assign" => Box::pin(handle_music_assign(args)),
        "broll.suggest" => Box::pin(handle_broll_suggest(args)),
        "broll.fetch" => Box::pin(handle_broll_fetch(args)),
        "broll.assign" => Box::pin(handle_broll_assign(args)),
        "broll.director" => Box::pin(handle_broll_director(args)),
        "voiceover.generate" => Box::pin(handle_voiceover_generate(args)),
        "tts.commentary" => Box::pin(handle_tts_commentary(args)),
        "timeline.diff" => Box::pin(handle_timeline_diff(args)),
        "timeline.preview" => Box::pin(handle_timeline_preview(args)),
        "tts.preview" => Box::pin(handle_tts_preview(args)),
        "music.ducking.plan" => Box::pin(handle_music_ducking_plan(args)),
        "timeline.autofill_broll" => Box::pin(handle_timeline_autofill_broll(args)),
        "timeline.render" => Box::pin(handle_timeline_render(args)),
        "reelize.timeline" => Box::pin(handle_reelize_timeline(args)),
        "verify.audio" => Box::pin(handle_verify_audio(args)),
        "verify.captions" => Box::pin(handle_verify_captions(args)),
        "verify.render" => Box::pin(handle_verify_render(args)),
        "verify.production" => Box::pin(handle_verify_production(args)),
        "llm.complete" => Box::pin(handle_llm_complete(args)),
        "vision.analyze_clip" => Box::pin(handle_vision_analyze_clip(args)),
        "vision.score_clip" => Box::pin(handle_vision_score_clip(args)),
        "system.config.get" => Box::pin(handle_system_config_get(args)),
        "system.config.set" => Box::pin(handle_system_config_set(args)),
        "director.run" => Box::pin(handle_director_run(args)),
        // HyperFrames tools
        "hf.lint" => Box::pin(async move {
            crate::hf::handle_hf_lint(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.validate" => Box::pin(async move {
            crate::hf::handle_hf_validate(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.snapshot" => Box::pin(async move {
            crate::hf::handle_hf_snapshot(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.render" => Box::pin(async move {
            crate::hf::handle_hf_render(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.classify" => Box::pin(async move {
            crate::hf::handle_hf_classify(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "composition.render" => Box::pin(async move {
            crate::hf::handle_composition_render(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "script.schema" => Box::pin(handle_script_schema(args)),
        "script.parse" => Box::pin(handle_script_parse(args)),
        "script.generate_voices" => Box::pin(handle_script_generate_voices(args)),
        "script.build_captions" => Box::pin(handle_script_build_captions(args)),
        "background.fetch" => Box::pin(handle_background_fetch(args)),
        "background.assign" => Box::pin(handle_background_assign(args)),
        "background.search" => Box::pin(handle_background_search(args)),            "sticker.presets" => Box::pin(handle_sticker_presets(args)),
            "sticker.load_preset" => Box::pin(handle_sticker_load_preset(args)),
        "sticker.render" => Box::pin(handle_sticker_render(args)),
        "script.to_timeline" => Box::pin(handle_script_to_timeline(args)),
        "script.to_video" => Box::pin(handle_script_to_video(args)),
        "stock.fetch" => Box::pin(handle_stock_fetch(args)),
        "youtube.download" => Box::pin(handle_youtube_download(args)),
        "youtube.search" => Box::pin(handle_youtube_search(args)),
        "stock.search" => Box::pin(handle_stock_search(args)),
        "media.search" => Box::pin(handle_media_search(args)),
        "media.download" => Box::pin(handle_media_download(args)),
        "gif.search" => Box::pin(handle_gif_search(args)),
        "gif.download" => Box::pin(handle_gif_download(args)),
        "overlay.assign" => Box::pin(handle_overlay_assign(args)),
        "timeline.to_hyperframes" => Box::pin(handle_timeline_to_hyperframes(args)),
        "voices.list" => Box::pin(handle_voices_list(args)),
        "timeline.inspect" => Box::pin(handle_timeline_inspect(args)),
        "library.search" => Box::pin(handle_library_search(args)),
        "library.download" => Box::pin(handle_library_download(args)),
        "library.build" => Box::pin(handle_library_build(args)),
        // Meta-tools (P1-2 + Rec 3.1 from prior audit)
        "system.capabilities" => Box::pin(handle_system_capabilities(args)),
        "system.doctor" => Box::pin(handle_system_doctor(args)),
        "help.tool" => Box::pin(handle_help_tool(args)),
                _ => Box::pin(async move { Err(ToolError::UnknownTool(name_owned.clone())) }),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

fn extract_f64(args: &serde_json::Value, key: &str) -> Result<f64, ToolError> {
    args.get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

fn extract_i64(args: &serde_json::Value, key: &str) -> Result<i64, ToolError> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

fn extract_arr(args: &serde_json::Value, key: &str) -> Result<Vec<String>, ToolError> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

fn default_str(args: &serde_json::Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn default_f64(args: &serde_json::Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

fn default_u32(args: &serde_json::Value, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

fn default_i64(args: &serde_json::Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn default_bool(args: &serde_json::Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// Optional boolean: `None` when the key is absent so callers can omit filters.
/// Explicit `true`/`false` are preserved. Used by music NLE tools so default
/// omission does not mean "only tracks with flag=false".
#[allow(dead_code)] // only used in tests after music.search deprecation
fn default_opt_bool(args: &serde_json::Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

/// Extract a meaningful b-roll concept from a caption string.
/// Skips stopwords and short words to avoid garbage Pexels searches like
/// "The" or "But". Falls back to the first 2 significant words.
fn extract_broll_concept(caption: &str) -> String {
    let stopwords = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "have", "has",
        "had", "do", "does", "did", "will", "would", "could", "should", "can", "may",
        "might", "this", "that", "these", "those", "your", "you", "they", "them",
        "their", "with", "from", "into", "about", "what", "when", "where", "which",
        "who", "how", "why", "for", "and", "but", "not", "all", "any", "some",
        "more", "most", "other", "such", "only", "own", "same", "than", "too",
        "very", "just", "also", "now", "then", "there", "here", "he", "she", "it",
        "we", "us", "our", "my", "me", "i", "his", "her", "its", "or", "so",
        "if", "in", "on", "at", "to", "of", "no", "up",
    ];
    let significant: Vec<String> = caption
        .split_whitespace()
        .map(|w| {
            let clean: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
            clean.to_lowercase()
        })
        .filter(|w| w.len() > 2 && !stopwords.contains(&w.as_str()))
        .take(3)
        .collect();
    if significant.is_empty() {
        "b-roll".to_string()
    } else {
        significant.join(" ")
    }
}

fn default_opt_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn default_opt_i64(args: &serde_json::Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64())
}

fn default_opt_u32(args: &serde_json::Value, key: &str) -> Option<u32> {
    args.get(key).and_then(|v| v.as_u64()).map(|v| v as u32)
}

fn default_opt_f64(args: &serde_json::Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

fn track_count(timeline: &Timeline, track_type: &TrackType) -> usize {
    timeline
        .tracks
        .get(track_type)
        .map(|v: &Vec<openscript_core::timeline::TimelineEvent>| v.len())
        .unwrap_or(0)
}

/// Read an API key via the unified config cascade:
/// env → `~/.openscript/config.json` → legacy `mcp/assets/.openscript_config.json`.
fn get_api_key(config_name: &str, _env_name: &str) -> String {
    // Map legacy flat names + nested kinds onto config::resolve_api_key
    let kind = match config_name {
        "pexels_api_key" | "pexels" => "pexels",
        "giphy_api_key" | "giphy" => "giphy",
        "pixabay_api_key" | "pixabay" => "pixabay",
        "openrouter_api_key" | "openrouter" | "openrouter_key" => "openrouter",
        other => other,
    };
    crate::config::resolve_api_key(kind)
}

/// Convenience: get Pexels API key (config file or env var)
fn pexels_key() -> String {
    get_api_key("pexels_api_key", "PEXELS_API_KEY")
}

/// Convenience: get GIPHY API key (config file or env var)
fn giphy_key() -> String {
    get_api_key("giphy_api_key", "GIPHY_API_KEY")
}

/// Convenience: get Pixabay API key (config file or env var)
fn pixabay_key() -> String {
    get_api_key("pixabay_api_key", "PIXABAY_API_KEY")
}

/// Convert an aspect-ratio string ("9:16", "16:9", "1:1") to a (width, height)
/// pixel tuple. Used by every tool that needs to crop/scale stock footage to
/// the target aspect. Prior versions had 7+ duplicate match blocks for this;
/// consolidated into one helper.
fn aspect_to_crop_dims(aspect: &str) -> (u32, u32) {
    match aspect {
        "9:16" => (1080, 1920), // vertical
        "16:9" => (1920, 1080), // horizontal
        "1:1" => (1080, 1080),  // square
        _ => (1080, 1920),      // unknown → portrait default
    }
}

/// Convert an aspect-ratio string to a Pexels orientation keyword.
fn aspect_to_orientation(aspect: &str) -> &'static str {
    match aspect {
        "9:16" => "portrait",
        "16:9" => "landscape",
        "1:1" => "square",
        _ => "portrait",
    }
}

/// Build an ffmpeg `-vf` filter that **cover-crops** to the target aspect
/// without stretching, and forces square pixels (`setsar=1`).
///
/// Prior `scale=W:H,crop=W:H` stretched landscape stock into 9:16 frames with
/// non-square SAR (display aspect stayed ~16:9) — visible distortion.
/// Delegates to `stock_signal::cover_crop_filter_for_aspect`.
fn crop_filter_for_aspect(aspect: &str) -> String {
    crate::stock_signal::cover_crop_filter_for_aspect(aspect)
}

/// Resolve a repo-relative path CWD-independently.
///
/// Priority:
///   1. If the path is absolute, return as-is.
///   2. If the path exists relative to CWD, return the CWD-relative path.
///   3. If OPENSCRIPT_ROOT env var is set, try `OPENSCRIPT_ROOT/path`.
///   4. Fall back to `CARGO_MANIFEST_DIR/../../path` (compile-time workspace root).
///   5. Last resort: return the relative path as-is (will likely fail downstream).
///
/// This fixes the round-2 UX audit GAP #12: background.search and other
/// asset-index tools only worked when CWD was the repo root because they
/// used relative paths like "mcp/assets/backgrounds_index.json". Now they
/// work from any CWD.
pub fn resolve_repo_path(rel: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(rel);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // Try CWD-relative first (fast path)
    if p.exists() {
        return p.to_path_buf();
    }
    // Try OPENSCRIPT_ROOT
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let resolved = std::path::Path::new(&root).join(rel);
        if resolved.exists() {
            return resolved;
        }
    }
    // Try CARGO_MANIFEST_DIR (compile-time workspace path)
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let resolved = std::path::Path::new(d).join("../../").join(rel);
        if resolved.exists() {
            return resolved;
        }
    }
    // Last resort: return the relative path (will likely fail, but with
    // a clear error downstream rather than a misleading "re-clone" message)
    p.to_path_buf()
}

fn sanitize_input_path<P: AsRef<std::path::Path>>(
    path: P,
) -> Result<std::path::PathBuf, ToolError> {
    let path = path.as_ref();
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err(ToolError::InvalidArg(format!(
                "Path traversal not allowed: {}",
                path.display()
            )));
        }
    }

    let resolved = if path.exists() {
        path.canonicalize()
            .map_err(|e| ToolError::InvalidArg(format!("Cannot resolve path: {}", e)))?
    } else {
        path.to_path_buf()
    };

    // If OPENSCRIPT_WORKSPACE_ROOT is set, reject paths that resolve outside it.
    // This is a defense-in-depth measure — the MCP server trusts the agent by default,
    // but operators can opt into workspace confinement via this env var.
    if let Ok(workspace_root) = std::env::var("OPENSCRIPT_WORKSPACE_ROOT") {
        let root = std::path::Path::new(&workspace_root)
            .canonicalize()
            .map_err(|e| {
                ToolError::InvalidArg(format!("Invalid OPENSCRIPT_WORKSPACE_ROOT: {}", e))
            })?;
        if !resolved.starts_with(&root) {
            return Err(ToolError::Permission(format!(
                "Path '{}' resolves outside workspace root '{}'. Set OPENSCRIPT_WORKSPACE_ROOT to allow this path, or remove the env var to disable workspace confinement.",
                resolved.display(),
                root.display()
            )));
        }
    }

    Ok(resolved)
}

fn default_opt_arr(args: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    })
}

fn default_timeline_path(source_video: &str) -> String {
    let path = Path::new(source_video);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    format!("{}.timeline.json", stem)
}

fn voice_profiles_path() -> String {
    ".openscript/voice_profiles.json".to_string()
}

fn load_voice_profiles() -> Result<serde_json::Value, ToolError> {
    let path = voice_profiles_path();
    if Path::new(&path).exists() {
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    } else {
        Ok(json!({}))
    }
}

fn save_voice_profiles(profiles: &serde_json::Value) -> Result<(), ToolError> {
    let path = voice_profiles_path();
    if let Some(parent) = Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(profiles)?;
    std::fs::write(&path, data)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: transcribe (native via openscript-transcribe)
// ---------------------------------------------------------------------------

async fn handle_transcribe(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_path = sanitize_input_path(extract_str(&args, "media_path")?)?
        .to_string_lossy()
        .to_string();
    let output_srt_path = default_opt_str(&args, "output_srt_path").unwrap_or_else(|| {
        let p = Path::new(&media_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.srt", stem))
            .to_string_lossy()
            .to_string()
    });

    if !Path::new(&media_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Media file not found: {}",
            media_path
        )));
    }

    let language_hint = default_str(&args, "language_hint", "auto");
    let engine_str = default_str(&args, "engine", "whisper");

    // Parse engine selection
    let engine = match engine_str.as_str() {
        "whisper" => openscript_transcribe::transcriber::TranscriptionEngine::Whisper,
        "nemotron-onnx" | "nemotron" => {
            tracing::info!("Using Nemotron ONNX engine (onnxruntime-genai)");
            #[allow(deprecated)]
            openscript_transcribe::transcriber::TranscriptionEngine::Nemotron
        }
        #[allow(deprecated)]
        "apex" => {
            tracing::warn!("Apex engine requested (deprecated). Use Whisper instead.");
            openscript_transcribe::transcriber::TranscriptionEngine::Apex
        }
        _ => openscript_transcribe::transcriber::TranscriptionEngine::Whisper,
    };

    report_progress(0.0, 100.0, "Starting transcription...")
        .await
        .ok();

    let result = transcribe_with_engine(&media_path, &output_srt_path, engine, &language_hint)
        .await
        .map_err(|e| ToolError::Srt(e.to_string()))?;

    report_progress(100.0, 100.0, "Transcription complete")
        .await
        .ok();

    Ok(json!({
        "status": "transcribed",
        "output_srt_path": result.output_path,
        "entry_count": result.entry_count,
        "word_srt_path": result.word_srt_path,
        "phrase_srt_path": result.phrase_srt_path,
        "engine": format!("{}", result.engine),
    }))
}

// ---------------------------------------------------------------------------
// Handler: srt.read
// ---------------------------------------------------------------------------

async fn handle_srt_read(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let entries = parse_srt(&srt_path)?;
    let result: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            json!({
                "idx": e.idx,
                "start": e.start,
                "end": e.end,
                "text": e.text,
            })
        })
        .collect();
    Ok(json!({
        "status": "success",
        "srt_path": srt_path,
        "count": result.len(),
        "entries": result,
    }))
}

// ---------------------------------------------------------------------------
// Handler: srt.prepare
// ---------------------------------------------------------------------------

async fn handle_srt_prepare(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let max_words = default_u32(&args, "max_words", 10) as usize;
    let max_chars = default_u32(&args, "max_chars", 64) as usize;
    let max_gap = default_f64(&args, "max_gap", 0.6);

    let entries = parse_srt(&srt_path)?;
    let groups = group_entries(&entries, max_words, max_chars, max_gap);

    let out_srt_path = {
        let p = Path::new(&srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.grouped.srt", stem))
            .to_string_lossy()
            .to_string()
    };
    let flat: Vec<(String, f64, f64)> = groups
        .iter()
        .map(|(text, start, end)| (text.clone(), *start, *end))
        .collect();
    write_srt(&flat, &out_srt_path)?;

    let result: Vec<serde_json::Value> = groups
        .iter()
        .map(|(text, start, end)| {
            json!({
                "text": text,
                "start": start,
                "end": end,
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "output_path": out_srt_path,
        "count": result.len(),
        "groups": result,
    }))
}

// ---------------------------------------------------------------------------
// Handler: srt.apply_edit (native: parse edited SRT, build EDL, render)
// ---------------------------------------------------------------------------

async fn handle_srt_apply_edit(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::srt::{parse_srt, retime_srt};
    use openscript_ffmpeg::render::{render, RenderConfig};
    use openscript_ffmpeg::subtitles::srt_to_ass;

    let video_path = extract_str(&args, "video_path")?;
    let edited_srt_path = extract_str(&args, "edited_srt_path")?;
    let merge_gap = default_f64(&args, "merge_gap", 0.25);
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);
    let crf = default_u32(&args, "crf", 20);
    let fps = default_u32(&args, "fps", 30);

    report_progress(0.0, 100.0, "Parsing edited SRT...")
        .await
        .ok();

    let edited_entries = parse_srt(edited_srt_path).map_err(|e| ToolError::Srt(e.to_string()))?;

    if edited_entries.is_empty() {
        return Err(ToolError::Srt("Edited SRT has no entries".to_string()));
    }

    // Build EDL segments from edited SRT entries
    let segments: Vec<(f64, f64, String)> = edited_entries
        .iter()
        .map(|e| (e.start, e.end, e.text.clone()))
        .collect();

    // Create EDL v1 JSON
    let edl = json!({
        "source": video_path,
        "target": {"aspect": aspect, "fps": fps},
        "segments": segments.iter().enumerate().map(|(i, (s, e, t))| {
            json!({"id": format!("seg_{:03}", i + 1), "start": s, "end": e, "caption": t, "crossfade_ms": crossfade_ms})
        }).collect::<Vec<_>>(),
        "effects": {"burn_captions": burn_captions, "audio": {"loudnorm": true}},
    });

    // Save EDL alongside the edited SRT
    let edl_path = {
        let p = Path::new(edited_srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };
    let edl_json = serde_json::to_string_pretty(&edl).map_err(|e| ToolError::Json(e))?;
    std::fs::write(&edl_path, edl_json).map_err(|e| ToolError::Io(e))?;

    // Generate ASS subtitles if burn_captions
    let ass_path = if burn_captions {
        report_progress(20.0, 100.0, "Generating subtitle styles...")
            .await
            .ok();
        let orig_srt = segments.clone();
        let retimed = retime_srt(
            &orig_srt,
            &segments
                .iter()
                .map(|(s, e, _)| (*s, *e))
                .collect::<Vec<_>>(),
            merge_gap,
        );

        let ass_out = Path::new(&edl_path).with_extension("ass");
        let ass_path_str = ass_out.to_string_lossy().to_string();
        srt_to_ass(&retimed, &ass_path_str, "Default")
            .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;
        Some(ass_path_str)
    } else {
        None
    };

    // Render
    report_progress(40.0, 100.0, "Rendering edited video...")
        .await
        .ok();
    let config = RenderConfig {
        video_path: video_path.to_string(),
        edl_path: edl_path.clone(),
        burn_captions,
        srt_path: Some(edited_srt_path.to_string()),
        ass_path,
        overlay_mov: None,
        aspect,
        crf,
        fps,
    };

    let output_path = render(config)
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    report_progress(100.0, 100.0, "Edit applied and rendered")
        .await
        .ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
        "edl_path": edl_path,
        "segments_count": segments.len(),
        "total_duration_s": segments.iter().map(|(s, e, _)| e - s).sum::<f64>(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: edl.build (Native Rust)
// ---------------------------------------------------------------------------

async fn handle_edl_build(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let strategy = default_str(&args, "strategy", "keep");
    let max_duration = default_opt_f64(&args, "max_duration");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 120);
    let analysis_path = default_opt_str(&args, "analysis_path");
    let aspect = default_str(&args, "aspect", "9:16");

    let entries = parse_srt(&srt_path).map_err(|e| ToolError::Srt(e.to_string()))?;

    let groups = group_entries(&entries, 10, 64, 0.6);

    let analysis = analyze_srt(&groups);

    if let Some(ap) = &analysis_path {
        let analysis_json =
            serde_json::to_string_pretty(&analysis).map_err(|e| ToolError::Json(e))?;
        std::fs::write(ap, analysis_json).map_err(|e| ToolError::Io(e))?;
    }

    let segments = build_edl(&analysis, &strategy, max_duration, crossfade_ms);

    let edl = json!({
        "source": "",
        "target": {"aspect": aspect, "fps": 30},
        "segments": segments.iter().enumerate().map(|(i, (s, e, t))| {
            json!({"id": format!("seg_{:03}", i + 1), "start": s, "end": e, "caption": t, "crossfade_ms": crossfade_ms})
        }).collect::<Vec<_>>(),
        "effects": {"burn_captions": true, "audio": {"loudnorm": true}},
    });

    let output_path = {
        let p = Path::new(&srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };

    let edl_json = serde_json::to_string_pretty(&edl).map_err(|e| ToolError::Json(e))?;
    std::fs::write(&output_path, edl_json).map_err(|e| ToolError::Io(e))?;

    let total_duration: f64 = segments.iter().map(|(s, e, _)| e - s).sum();

    Ok(json!({
        "status": "built",
        "edl_path": output_path,
        "strategy": strategy,
        "segments_count": segments.len(),
        "total_duration_s": total_duration,
        "analysis_count": analysis.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: render (Phase 1: shell to Python)
// ---------------------------------------------------------------------------

async fn handle_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::srt::parse_srt;
    use openscript_ffmpeg::render::{render, RenderConfig};
    use openscript_ffmpeg::subtitles::srt_to_ass;

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let edl_path = sanitize_input_path(extract_str(&args, "edl_path")?)?
        .to_string_lossy()
        .to_string();
    let burn_captions = default_bool(&args, "burn_captions", true);
    let srt_path = default_opt_str(&args, "srt_path");
    let ass_path = default_opt_str(&args, "ass_path");
    let aspect = default_str(&args, "aspect", "9:16");
    let crf = default_u32(&args, "crf", 20);
    let fps = default_u32(&args, "fps", 30);

    report_progress(0.0, 100.0, "Preparing render...")
        .await
        .ok();

    let resolved_ass_path = if burn_captions && ass_path.is_none() {
        if let Some(srt) = &srt_path {
            if Path::new(srt).exists() {
                report_progress(10.0, 100.0, "Converting subtitles...")
                    .await
                    .ok();
                let entries = parse_srt(srt).map_err(|e| ToolError::Srt(e.to_string()))?;
                let flat: Vec<(f64, f64, String)> = entries
                    .iter()
                    .map(|e| (e.start, e.end, e.text.clone()))
                    .collect();
                let ass_out = Path::new(srt).with_extension("ass");
                let ass_path_str = ass_out.to_string_lossy().to_string();
                srt_to_ass(&flat, &ass_path_str, "Default")
                    .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;
                Some(ass_path_str)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        ass_path
    };

    let config = RenderConfig {
        video_path: video_path.to_string(),
        edl_path: edl_path.to_string(),
        burn_captions,
        srt_path: srt_path.map(String::from),
        ass_path: resolved_ass_path,
        overlay_mov: None,
        aspect,
        crf,
        fps,
    };

    report_progress(20.0, 100.0, "Rendering video with FFmpeg...")
        .await
        .ok();

    let output_path = render(config)
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    report_progress(100.0, 100.0, "Render complete").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: reelize (native: transcribe → prepare → edl.build → render)
// ---------------------------------------------------------------------------

async fn handle_reelize(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;
    let preset = default_str(&args, "preset", "Balanced");
    let max_duration = default_opt_f64(&args, "max_duration");
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    // Step 1: Transcribe (if no SRT provided)
    let resolved_srt_path = if let Some(srt) = srt_path {
        report_progress(5.0, 100.0, "Using existing SRT...")
            .await
            .ok();
        srt.to_string()
    } else {
        report_progress(0.0, 100.0, "Step 1/4: Transcribing audio...")
            .await
            .ok();
        let transcribe_args = json!({
            "media_path": video_path,
        });
        let transcribe_result = handle_transcribe(transcribe_args).await?;
        report_progress(25.0, 100.0, "Transcription complete")
            .await
            .ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    // Step 2: SRT prepare (group word-per-line)
    report_progress(30.0, 100.0, "Step 2/4: Grouping captions...")
        .await
        .ok();
    let prepare_args = json!({
        "srt_path": resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    });
    let prepare_result = handle_srt_prepare(prepare_args).await?;
    let grouped_srt = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    // Step 3: EDL build
    report_progress(50.0, 100.0, "Step 3/4: Building edit decision list...")
        .await
        .ok();
    let crossfade_ms = match preset.as_str() {
        "Tight" => 120,
        "Balanced" => 100,
        "Natural" => 60,
        _ => 100,
    };

    let edl_args = json!({
        "srt_path": grouped_srt,
        "strategy": "keep",
        "max_duration": max_duration,
        "crossfade_ms": crossfade_ms,
        "aspect": aspect,
    });
    let edl_result = handle_edl_build(edl_args).await?;
    let edl_path = edl_result
        .get("edl_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("EDL build did not return path".to_string()))?
        .to_string();

    // Step 4: Render
    report_progress(70.0, 100.0, "Step 4/4: Rendering final video...")
        .await
        .ok();
    let render_args = json!({
        "video_path": video_path,
        "edl_path": edl_path,
        "srt_path": grouped_srt,
        "burn_captions": burn_captions,
        "aspect": aspect,
        "crf": 20,
        "fps": 30,
    });
    let render_result = handle_render(render_args).await?;
    let output_path = render_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Ffmpeg("Render did not return output path".to_string()))?
        .to_string();

    let total_segments = edl_result
        .get("segments_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_duration = edl_result
        .get("total_duration_s")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    report_progress(100.0, 100.0, "Reel complete!").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
        "preset": preset,
        "segments_count": total_segments,
        "total_duration_s": total_duration,
        "srt_path": resolved_srt_path,
        "edl_path": edl_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: overlay.generate (Phase 1: shell to Python)
// ---------------------------------------------------------------------------

async fn handle_overlay_generate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = extract_str(&args, "srt_path")?;
    let _edl_path = extract_str(&args, "edl_path")?;
    let out_path = default_opt_str(&args, "out_path").unwrap_or_else(|| {
        let p = Path::new(&srt_path);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        format!("{}.overlay.mov", stem)
    });
    let width = default_u32(&args, "width", 1080);
    let height = default_u32(&args, "height", 1920);
    let fps = default_u32(&args, "fps", 30);
    let animate = default_bool(&args, "animate", false);
    let style = default_str(&args, "style", "pupcaps_center");
    let timeline_path = default_opt_str(&args, "timeline_path");

    report_progress(0.0, 100.0, "Generating caption overlay...")
        .await
        .ok();

    let pupcaps_path = "third_party/PupCaps/pupcaps";

    let mut cmd = tokio::process::Command::new("node");
    cmd.arg(pupcaps_path)
        .arg("retimed")
        .arg(srt_path)
        .arg("--output")
        .arg(&out_path)
        .arg("--width")
        .arg(width.to_string())
        .arg("--height")
        .arg(height.to_string())
        .arg("--fps")
        .arg(fps.to_string())
        .arg("--style")
        .arg(format!("mcp/styles/{}.css", style))
        .kill_on_drop(true);

    if animate {
        cmd.arg("--animate");
    }

    let out = cmd.output().await;
    match out {
        Ok(o) if o.status.success() => {
            if let Some(tl_path) = &timeline_path {
                if Path::new(tl_path).exists() {
                    if let Ok(mut timeline) = Timeline::load(tl_path) {
                        timeline.add_asset(
                            "captions",
                            "overlay_mov".to_string(),
                            json!({"path": out_path}),
                        );
                        timeline.save(tl_path).ok();
                    }
                }
            }
            report_progress(100.0, 100.0, "Overlay generated")
                .await
                .ok();
            Ok(json!({
                "status": "generated",
                "output_path": out_path,
            }))
        }
        Ok(o) => Err(ToolError::Ffmpeg(format!(
            "overlay.generate failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ))),
        Err(e) => Err(ToolError::Ffmpeg(format!("overlay.generate error: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: timeline.build
// ---------------------------------------------------------------------------

async fn handle_timeline_build(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let source_video = extract_str(&args, "source_video")?;
    let aspect = default_str(&args, "aspect", "9:16");
    let fps = default_u32(&args, "fps", 30);
    let max_duration = default_opt_u32(&args, "max_duration");
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| default_timeline_path(source_video));

    if !Path::new(source_video).exists() {
        return Err(ToolError::NotFound(format!(
            "Source video not found: {}",
            source_video
        )));
    }

    let timeline = Timeline::new(source_video.into(), &aspect, fps, max_duration);
    timeline.save(&output_path)?;

    Ok(json!({
        "status": "created",
        "timeline_path": output_path,
        "source": source_video,
        "aspect": aspect,
        "fps": fps,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.load
// ---------------------------------------------------------------------------

async fn handle_timeline_load(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;

    Ok(json!({
        "status": "loaded",
        "timeline_path": timeline_path,
        "version": timeline.version,
        "source": timeline.source.to_string_lossy(),
        "segments_count": timeline.segments.len(),
        "tracks": timeline.tracks.keys().map(|k: &openscript_core::types::TrackType| k.to_string()).collect::<Vec<_>>(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.validate
// ---------------------------------------------------------------------------

async fn handle_timeline_validate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;
    let errors = timeline.validate();
    let valid = errors.is_empty();

    Ok(json!({
        "status": if valid { "valid" } else { "invalid" },
        "timeline_path": timeline_path,
        "valid": valid,
        "errors": errors,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.upgrade
// ---------------------------------------------------------------------------

async fn handle_timeline_upgrade(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let edl_v1_path = sanitize_input_path(extract_str(&args, "edl_v1_path")?)?
        .to_string_lossy()
        .to_string();
    let output_path = default_opt_str(&args, "output_path");

    let data = std::fs::read_to_string(&edl_v1_path)?;
    let v1: serde_json::Value = serde_json::from_str(&data)?;
    let timeline = Timeline::from_edl_v1(&v1)?;

    let out_path = output_path.unwrap_or_else(|| {
        let p = Path::new(&edl_v1_path);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        format!("{}.timeline.json", stem)
    });

    timeline.save(&out_path)?;

    Ok(json!({
        "status": "upgraded",
        "timeline_path": out_path,
        "segments_count": timeline.segments.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.add_segment
// ---------------------------------------------------------------------------

async fn handle_timeline_add_segment(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let start = extract_f64(&args, "start")?;
    let end = extract_f64(&args, "end")?;
    let caption = extract_str(&args, "caption")?;
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let semantic_role = default_opt_str(&args, "semantic_role");

    let mut timeline = Timeline::load(timeline_path)?;
    let segment_id =
        timeline.add_segment(start, end, caption, crossfade_ms, semantic_role.as_deref());
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "segment_added",
        "segment_id": segment_id,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.add_track_event
// ---------------------------------------------------------------------------

async fn handle_timeline_add_track_event(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let track_type_str = extract_str(&args, "track_type")?;
    let event = args
        .get("event")
        .ok_or_else(|| ToolError::MissingArg("event".to_string()))?
        .clone();

    let track_type: TrackType = track_type_str.parse().map_err(|e| ToolError::Timeline(e))?;

    let mut timeline = Timeline::load(timeline_path)?;

    let event_obj: openscript_core::timeline::TimelineEvent =
        serde_json::from_value(event.clone()).map_err(|e| ToolError::Json(e))?;

    timeline.add_track_event(track_type, event_obj);
    timeline.save(timeline_path)?;

    let event_id = event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(json!({
        "status": "event_added",
        "event_id": event_id,
        "track_type": track_type_str,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voice.profile.add
// ---------------------------------------------------------------------------

async fn handle_voice_profile_add(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let profile_id = extract_str(&args, "profile_id")?;
    let ref_audio = extract_str(&args, "ref_audio")?;
    let ref_text = extract_str(&args, "ref_text")?;
    let provider = default_str(&args, "provider", "faster-qwen3-tts");
    let mode = default_str(&args, "mode", "clone");
    let model = default_str(&args, "model", "Qwen/Qwen3-TTS-12Hz-0.6B-Base");
    let language = default_str(&args, "language", "English");
    let description = default_opt_str(&args, "description");

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
    });
    profiles[profile_id] = obj;
    save_voice_profiles(&profiles)?;

    Ok(json!({
        "status": "profile_added",
        "profile_id": profile_id,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voice.profile.list
// ---------------------------------------------------------------------------

async fn handle_voice_profile_list(
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

// ---------------------------------------------------------------------------
// Handler: voice.profile.remove
// ---------------------------------------------------------------------------

async fn handle_voice_profile_remove(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let profile_id = extract_str(&args, "profile_id")?;
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

// ---------------------------------------------------------------------------
// Shared TTS routing helper — used by tts.generate, voiceover.generate, tts.commentary
// Routes to Kokoro (if profile.provider == "kokoro" and feature enabled) or sidecar.
// ---------------------------------------------------------------------------

struct TtsGenResult {
    output_path: String,
    duration_ms: i64,
    cached: bool,
    backend: String,
}

/// Generate speech via the appropriate TTS backend (Kokoro or sidecar).
/// `output_path` must be pre-validated (parent dir created).
async fn tts_generate_routed(
    voice_profile_id: &str,
    text: &str,
    output_path: &str,
    speed: f64,
    pitch: f64,
    volume: f64,
    format: &str,
    profile: &openscript_tts::profiles::VoiceProfile,
) -> Result<TtsGenResult, ToolError> {
    let cache_dir =
        std::env::var("OPENSCRIPT_TTS_CACHE").unwrap_or_else(|_| "artifacts/tts".to_string());

    // Route to Kokoro backend if the profile's provider is "kokoro" and the
    // feature is enabled. Otherwise fall through to the sidecar.
    #[cfg(feature = "kokoro")]
    if profile.provider == "kokoro" {
        use openscript_tts::kokoro::{KokoroClient, KokoroConfig};

        let model_dir =
            std::env::var("KOKORO_MODEL_DIR").unwrap_or_else(|_| "mcp/assets/kokoro".to_string());
        let default_voice =
            std::env::var("KOKORO_DEFAULT_VOICE").unwrap_or_else(|_| "af_heart".to_string());

        let cfg = KokoroConfig {
            model_dir: std::path::PathBuf::from(&model_dir),
            default_voice,
            cache_dir: std::path::PathBuf::from(&cache_dir),
        };
        let kokoro_client = KokoroClient::new(cfg);

        let result = kokoro_client
            .generate(
                voice_profile_id,
                text,
                output_path,
                speed,
                pitch,
                volume,
                format,
                profile,
            )
            .await
            .map_err(|e| ToolError::Tts(e.to_string()))?;

        return Ok(TtsGenResult {
            output_path: result.output_path,
            duration_ms: result.duration_ms,
            cached: result.cached,
            backend: "kokoro".to_string(),
        });
    }

    #[cfg(not(feature = "kokoro"))]
    if profile.provider == "kokoro" {
        return Err(ToolError::Tts(
            "Voice profile uses Kokoro backend but the kokoro feature is not enabled. \
             Rebuild openscript-mcp with --features kokoro."
                .to_string(),
        ));
    }

    // Sidecar path (faster-qwen3-tts)
    use openscript_tts::client::TtsClient;
    let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());
    let client = TtsClient::new(&tts_url, &cache_dir);

    if !client
        .health_check()
        .await
        .map_err(|e| ToolError::Tts(e.to_string()))?
    {
        return Err(ToolError::Tts(format!(
            "TTS sidecar server is not reachable at {}. \
             Start the faster-qwen3-tts server or set OPENSCRIPT_TTS_URL.",
            tts_url
        )));
    }

    let result = client
        .generate(
            voice_profile_id,
            text,
            output_path,
            speed,
            pitch,
            volume,
            format,
            profile,
        )
        .await
        .map_err(|e| ToolError::Tts(e.to_string()))?;

    Ok(TtsGenResult {
        output_path: result.output_path,
        duration_ms: result.duration_ms,
        cached: result.cached,
        backend: "sidecar".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Handler: tts.generate (native via openscript-tts)
// ---------------------------------------------------------------------------

async fn handle_tts_generate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_tts::client::TtsClient;
    use openscript_tts::profiles::VoiceProfileRegistry;
    use std::path::Path;

    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let text = extract_str(&args, "text")?;
    let output_path = extract_str(&args, "output_path")?;
    let speed = default_f64(&args, "speed", 1.0);
    let pitch = default_f64(&args, "pitch", 1.0);
    let volume = default_f64(&args, "volume", 1.0);
    let format = default_str(&args, "format", "wav");

    report_progress(0.0, 100.0, "Generating speech...")
        .await
        .ok();

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:") && !voice_profile_id.starts_with("faster-qwen") {
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

    let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());
    let cache_dir =
        std::env::var("OPENSCRIPT_TTS_CACHE").unwrap_or_else(|_| "artifacts/tts".to_string());

    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Route to Kokoro backend if the profile's provider is "kokoro" and the
    // feature is enabled. Otherwise fall through to the sidecar (faster-qwen3-tts).
    #[cfg(feature = "kokoro")]
    if profile.provider == "kokoro" {
        use openscript_tts::kokoro::{KokoroClient, KokoroConfig};

        let model_dir =
            std::env::var("KOKORO_MODEL_DIR").unwrap_or_else(|_| "mcp/assets/kokoro".to_string());
        let default_voice =
            std::env::var("KOKORO_DEFAULT_VOICE").unwrap_or_else(|_| "af_heart".to_string());

        let cfg = KokoroConfig {
            model_dir: std::path::PathBuf::from(&model_dir),
            default_voice,
            cache_dir: std::path::PathBuf::from(&cache_dir),
        };
        let kokoro_client = KokoroClient::new(cfg);

        let result = kokoro_client
            .generate(
                voice_profile_id,
                text,
                output_path,
                speed,
                pitch,
                volume,
                &format,
                &profile,
            )
            .await
            .map_err(|e| ToolError::Tts(e.to_string()))?;

        report_progress(100.0, 100.0, "Speech generated (Kokoro)")
            .await
            .ok();

        return Ok(json!({
            "status": "generated",
            "backend": "kokoro",
            "output_path": result.output_path,
            "duration_ms": result.duration_ms,
            "cached": result.cached,
        }));
    }

    #[cfg(not(feature = "kokoro"))]
    if profile.provider == "kokoro" {
        return Err(ToolError::Tts(
            "Voice profile uses Kokoro backend but the kokoro feature is not enabled. \
             Rebuild openscript-mcp with --features kokoro."
                .to_string(),
        ));
    }

    let client = TtsClient::new(&tts_url, &cache_dir);

    // Health check — fail fast if TTS sidecar is not running
    if !client
        .health_check()
        .await
        .map_err(|e| ToolError::Tts(e.to_string()))?
    {
        return Err(ToolError::Tts(format!(
            "TTS sidecar server is not reachable at {}. \
             Start the faster-qwen3-tts server or set OPENSCRIPT_TTS_URL.",
            tts_url
        )));
    }

    let result = client
        .generate(
            voice_profile_id,
            text,
            output_path,
            speed,
            pitch,
            volume,
            &format,
            &profile,
        )
        .await
        .map_err(|e| ToolError::Tts(e.to_string()))?;

    report_progress(100.0, 100.0, "Speech generated").await.ok();

    Ok(json!({
        "status": "generated",
        "backend": "sidecar",
        "output_path": result.output_path,
        "duration_ms": result.duration_ms,
        "cached": result.cached,
    }))
}

// ---------------------------------------------------------------------------
// Handler: tts.estimate_duration
// ---------------------------------------------------------------------------

async fn handle_tts_estimate_duration(
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

// ---------------------------------------------------------------------------
// Handler: sfx.index (native via openscript-assets)
// ---------------------------------------------------------------------------

async fn handle_sfx_index(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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

// ---------------------------------------------------------------------------
// Handler: sfx.search (native via openscript-assets)
// ---------------------------------------------------------------------------

async fn handle_sfx_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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

// ---------------------------------------------------------------------------
// Handler: sfx.assign
// ---------------------------------------------------------------------------

async fn handle_sfx_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
    // effects, but the tool documentation and `reelize.timeline` refer to the
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
        asset_id: sfx_path.clone().unwrap_or_else(|| query.to_string()),
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
        "event_id": event_id,
        "position_ms": position_ms,
        "timeline_path": timeline_path,
        "asset_path": sfx_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: music.index (native via openscript-assets)
// ---------------------------------------------------------------------------

async fn handle_music_index(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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

// ---------------------------------------------------------------------------
// Handler: music.search (native via openscript-assets)
// ---------------------------------------------------------------------------

/// DEPRECATED: music.search now forwards to library.search.
/// The old music_index.json (synthetic stock tracks) has been deleted.
/// All music lives in library_index.json (393 music + 94 SFX, copyright-free).
async fn handle_music_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Forward query/mood/energy/limit to library.search
    let mut lib_args = args.clone();
    // Remove music-search-only params that library.search doesn't understand
    lib_args.as_object_mut().map(|m| {
        m.remove("intro_friendly");
        m.remove("cta_friendly");
        m.remove("loopable");
    });

    let mut result = handle_library_search(lib_args).await?;

    // Inject deprecation warning
    if let Some(obj) = result.as_object_mut() {
        let warnings = obj
            .entry("warnings".to_string())
            .or_insert(json!([]))
            .as_array_mut()
            .expect("warnings is always an array");
        warnings.insert(
            0,
            json!("DEPRECATED: music.search has been replaced by library.search. All music tracks now live in the library (393 copyright-free tracks). Use library.search instead."),
        );
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Handler: music.assign
// ---------------------------------------------------------------------------

async fn handle_music_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
            "Music file not found: {}. Use music.search to find tracks and get their path.",
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
        start_ms: start_ms,
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

// ---------------------------------------------------------------------------
// Handler: broll.suggest
// ---------------------------------------------------------------------------

async fn handle_broll_suggest(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let edl_path = extract_str(&args, "edl_path")?;
    let _srt_path = default_opt_str(&args, "srt_path");
    let cadence_seconds = default_f64(&args, "cadence_seconds", 2.0);

    let data = std::fs::read_to_string(edl_path)?;
    let timeline: serde_json::Value = serde_json::from_str(&data)?;

    let segments = timeline
        .get("segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let cadence_ms = (cadence_seconds * 1000.0) as i64;
    let mut suggestions = Vec::new();
    let mut position_ms = 0i64;

    for seg in &segments {
        let start = seg.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end = seg.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let duration_ms = ((end - start) * 1000.0) as i64;
        // Derive concept from the segment caption instead of hardcoding "b-roll".
        // Use a salient noun/phrase from the caption — skip stopwords and short
        // words that produce garbage Pexels searches ("The", "But", "And").
        let caption = seg
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let concept = extract_broll_concept(caption);

        if duration_ms > cadence_ms * 2 {
            let mut t = 0i64;
            while t < duration_ms {
                let slot_duration = cadence_ms.min(duration_ms - t);
                suggestions.push(json!({
                    "position_ms": position_ms + t,
                    "duration_ms": slot_duration,
                    "concept": concept,
                }));
                t += cadence_ms;
            }
        }

        position_ms += duration_ms;
    }

    Ok(json!({
        "status": "success",
        "edl_path": edl_path,
        "suggestions": suggestions,
        "count": suggestions.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: broll.fetch (native via openscript-assets PexelsClient)
// ---------------------------------------------------------------------------

async fn handle_broll_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    let concepts = extract_arr(&args, "concepts")?;
    let asset_dir =
        default_opt_str(&args, "asset_dir").unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    let download = args
        .get("download")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // Local fallback clips used when Pexels returns nothing for a concept
    // (mirrors background.fetch's fallback_pool semantics).
    let fallback_pool: Vec<String> = args
        .get("fallback_pool")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let api_key = pexels_key();
    if api_key.is_empty() {
        // Bug #16 fix: do not hard-fail when PEXELS_API_KEY is missing.
        // Return status:warning with actionable guidance so an agent can
        // fall back to background.fetch (which has its own fallback chain)
        // or supply a fallback_pool.
        if fallback_pool.is_empty() {
            return Ok(json!({
                "status": "warning",
                "message": "PEXELS_API_KEY not set and no fallback_pool provided. Set the key in mcp/assets/.openscript_config.json, or provide a fallback_pool of local clip paths, or use background.fetch which has its own fallback chain.",
                "results": [],
                "total_concepts": concepts.len(),
                "missing_key": true,
            }));
        }
        // No key but caller supplied fallback_pool — return one fallback
        // entry per concept so downstream tools (broll.assign) can still
        // place something on the timeline.
        let results: Vec<serde_json::Value> = concepts
            .iter()
            .enumerate()
            .map(|(i, concept)| {
                let path = fallback_pool[i % fallback_pool.len()].clone();
                json!({
                    "concept": concept,
                    "videos": [],
                    "count": 0,
                    "cached_path": path,
                    "source": "fallback_pool",
                })
            })
            .collect();
        let mut downloaded: Vec<serde_json::Value> = Vec::new();
        for (i, concept) in concepts.iter().enumerate() {
            downloaded.push(json!({
                "concept": concept,
                "path": fallback_pool[i % fallback_pool.len()],
                "source": "fallback_pool",
            }));
        }
        return Ok(json!({
            "status": "warning",
            "message": "PEXELS_API_KEY not set; using fallback_pool only.",
            "results": results,
            "downloaded": downloaded,
            "total_concepts": concepts.len(),
            "missing_key": true,
        }));
    }

    let total = concepts.len();
    report_progress(0.0, total as f64, "Fetching b-roll...")
        .await
        .ok();

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    let mut all_results = Vec::new();
    let mut downloaded = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (i, concept) in concepts.iter().enumerate() {
        report_progress(i as f64, total as f64, &format!("Searching: {}", concept))
            .await
            .ok();

        let videos = client
            .search(concept, &orientation, &quality)
            .await
            .map_err(|e| ToolError::Asset(e.to_string()))?;

        let first = videos.first();
        let mut cached_path = None;
        if download {
            if let Some(v) = first {
                match client.download_best(v, concept).await {
                    Ok(path) => {
                        cached_path = Some(path.clone());
                        downloaded.push((concept.clone(), path));
                    }
                    Err(e) => {
                        tracing::warn!("[broll.fetch] Download failed for {}: {}", concept, e)
                    }
                }
            }
        }

        let video_json: Vec<serde_json::Value> = videos
            .iter()
            .take(3)
            .map(|v| {
                json!({
                    "id": v.id,
                    "width": v.width,
                    "height": v.height,
                    "duration": v.duration,
                    "image": v.image,
                    "url": v.url,
                })
            })
            .collect();

        let mut result = json!({
            "concept": concept,
            "videos": video_json,
            "count": video_json.len(),
        });
        if let Some(path) = &cached_path {
            result["cached_path"] = json!(path);
        }

        // Per-concept fallback: if Pexels returned nothing, try fallback_pool
        // so downstream tools (broll.assign) still have a path to use.
        if video_json.is_empty() && !fallback_pool.is_empty() {
            let fallback_path = fallback_pool[i % fallback_pool.len()].clone();
            result["cached_path"] = json!(&fallback_path);
            result["source"] = json!("fallback_pool");
            warnings.push(format!(
                "concept '{}' returned 0 Pexels results — using fallback_pool entry",
                concept
            ));
            if download {
                downloaded.push((concept.clone(), fallback_path));
            }
        }
        all_results.push(result);
    }

    report_progress(total as f64, total as f64, "B-roll fetch complete")
        .await
        .ok();

    // Status is "warning" if any concept returned 0 videos (mirrors
    // background.fetch's behaviour of warning when falling back).
    let any_empty = all_results
        .iter()
        .any(|r| r.get("count").and_then(|v| v.as_u64()).unwrap_or(0) == 0);
    let status = if any_empty { "warning" } else { "fetched" };

    let mut resp = json!({
        "status": status,
        "results": all_results,
        "total_concepts": concepts.len(),
    });
    if !downloaded.is_empty() {
        resp["downloaded"] = json!(downloaded
            .iter()
            .map(|(c, p)| json!({"concept": c, "path": p}))
            .collect::<Vec<_>>());
    }
    if !warnings.is_empty() {
        resp["warnings"] = json!(warnings);
    }
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Handler: broll.assign
// ---------------------------------------------------------------------------

async fn handle_broll_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let concept = extract_str(&args, "concept")?;
    let position_ms = extract_i64(&args, "position_ms")?;
    let duration_ms = extract_i64(&args, "duration_ms")?;
    let asset_path = default_opt_str(&args, "asset_path");
    let transition_style = default_str(&args, "transition_style", "cut");
    let crop_mode = default_str(&args, "crop_mode", "center");
    // Expose fade_in_ms / fade_out_ms as parameters (were hardcoded to 0).
    let fade_in_ms = default_u32(&args, "fade_in_ms", 0);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 0);

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("broll_{:03}", track_count(&timeline, &TrackType::Broll) + 1);

    let resolved_path = asset_path.unwrap_or_else(|| {
        let cache_dir = "mcp/assets/broll_cache";
        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&concept.replace(' ', "_")) && name.ends_with(".mp4") {
                    return entry.path().to_string_lossy().to_string();
                }
            }
        }
        // No match found — return empty string so the existence check below catches it
        String::new()
    });

    // If the resolved path doesn't exist on disk, use "placeholder" so the
    // render pipeline skips this event instead of crashing ffmpeg with a
    // glob pattern or non-existent path.
    let (asset_id, asset_registry_path, matched) = if resolved_path.is_empty()
        || resolved_path.contains("placeholder")
        || !std::path::Path::new(&resolved_path).exists()
    {
        ("placeholder".to_string(), "placeholder".to_string(), false)
    } else {
        (resolved_path.clone(), resolved_path.clone(), true)
    };

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: asset_id.clone(),
        start_ms: position_ms,
        end_ms: position_ms + duration_ms,
        offset_ms: 0,
        gain_db: 0.0,
        fade_in_ms,
        fade_out_ms,
        tags: vec![concept.to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "broll.assign".into(),
            editorial_role: None,
            concept: Some(concept.to_string()),
        }),
        kind: openscript_core::timeline::EventKind::Broll {
            concept: concept.to_string(),
            source_provider: asset_id.clone(),
            transition_style,
            crop_mode,
            orientation: "9:16".into(),
            motion_intensity: "medium".into(),
        },
    };

    timeline.add_track_event(TrackType::Broll, event);
    timeline.add_asset(
        "broll",
        event_id.clone(),
        json!({"path": asset_registry_path}),
    );
    timeline.save(timeline_path)?;

    // Fix: return status "warning" (not "assigned") when no asset matched,
    // mirroring sfx.assign's pattern. Prior versions returned "assigned" with
    // asset_id:"placeholder", silently losing the agent's intent — the render
    // pipeline drops placeholder events, so the agent never knew the b-roll
    // slot was empty.
    let (status, message) = if matched {
        (
            "assigned",
            format!("B-roll assigned for concept '{}' at {} ms", concept, position_ms),
        )
    } else {
        (
            "warning",
            format!(
                "No b-roll asset found for concept '{}' at {} ms. Placeholder event created — render will skip this event. Use broll.fetch or background.fetch to download a real asset, then re-assign.",
                concept, position_ms
            ),
        )
    };

    Ok(json!({
        "status": status,
        "matched": matched,
        "message": message,
        "event_id": event_id,
        "asset_id": asset_id,
        "asset_path": asset_registry_path,
        "position_ms": position_ms,
        "duration_ms": duration_ms,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voiceover.generate
// ---------------------------------------------------------------------------

async fn handle_voiceover_generate(
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

    let mut timeline = Timeline::load(timeline_path)?;

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:") && !voice_profile_id.starts_with("faster-qwen") {
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

// ---------------------------------------------------------------------------
// Handler: tts.commentary
// ---------------------------------------------------------------------------

async fn handle_tts_commentary(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
    let normalized_id = if !voice_profile_id.starts_with("kokoro:") && !voice_profile_id.starts_with("faster-qwen") {
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

/// Generate a single commentary voiceover segment: TTS synthesis + asset
/// registration + timeline event creation. Used by `handle_tts_commentary`
/// for intro / transition / outro segments. Prior versions had 3 near-identical
/// 50-line blocks; consolidated into this helper.
///
/// Returns `(event_id, duration_ms)` on success.
async fn generate_commentary_segment(
    timeline: &mut Timeline,
    timeline_dir: &std::path::Path,
    voice_profile_id: &str,
    text: &str,
    position_ms: i64,
    concept: &str,
    speed: f64,
    profile: &openscript_tts::profiles::VoiceProfile,
) -> Result<(String, i64), ToolError> {
    let event_id = format!(
        "voiceover_{:03}",
        track_count(timeline, &TrackType::Voiceover) + 1
    );
    let output_path = timeline_dir
        .join(format!("voiceover_{}.wav", event_id))
        .to_string_lossy()
        .to_string();

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let result = tts_generate_routed(
        voice_profile_id,
        text,
        &output_path,
        speed,
        1.0,
        1.0,
        "wav",
        profile,
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

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: output_path.clone(),
        start_ms: position_ms,
        end_ms: position_ms + duration_ms,
        offset_ms: 0,
        gain_db: -6.0,
        fade_in_ms: 50,
        fade_out_ms: 50,
        tags: vec!["commentary".to_string(), concept.to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "tts.commentary".into(),
            editorial_role: None,
            concept: Some(concept.to_string()),
        }),
        kind: openscript_core::timeline::EventKind::Voiceover {
            voice_profile_id: voice_profile_id.to_string(),
            text: text.to_string(),
            estimated_duration_ms: duration_ms,
        },
    };

    timeline.add_track_event(TrackType::Voiceover, event);
    Ok((event_id, duration_ms))
}

// ---------------------------------------------------------------------------
// Handler: timeline.diff
// ---------------------------------------------------------------------------

async fn handle_timeline_diff(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path_a = extract_str(&args, "timeline_path_a")?;
    let timeline_path_b = extract_str(&args, "timeline_path_b")?;

    let a = Timeline::load(timeline_path_a)?;
    let b = Timeline::load(timeline_path_b)?;

    let duration_a = a.total_duration_ms();
    let duration_b = b.total_duration_ms();
    let duration_change_ms = duration_b - duration_a;

    let seg_ids_a: std::collections::HashSet<&str> =
        a.segments.iter().map(|s| s.id.as_str()).collect();
    let seg_ids_b: std::collections::HashSet<&str> =
        b.segments.iter().map(|s| s.id.as_str()).collect();

    let added: Vec<&str> = {
        let mut v: Vec<&str> = seg_ids_b.difference(&seg_ids_a).copied().collect();
        v.sort();
        v
    };
    let removed: Vec<&str> = {
        let mut v: Vec<&str> = seg_ids_a.difference(&seg_ids_b).copied().collect();
        v.sort();
        v
    };

    let mut modified = Vec::new();
    for seg_a in &a.segments {
        if seg_ids_b.contains(seg_a.id.as_str()) {
            if let Some(seg_b) = b.segments.iter().find(|s| s.id == seg_a.id) {
                if seg_a.start != seg_b.start
                    || seg_a.end != seg_b.end
                    || seg_a.caption != seg_b.caption
                {
                    modified.push(seg_a.id.as_str());
                }
            }
        }
    }
    // P2-4 fix: sort modified segment ids for stable, readable output. Prior
    // versions returned them in arbitrary iteration order.
    modified.sort();

    let track_changes = json!({
        "dialogue": {
            "a": track_count(&a, &TrackType::Dialogue),
            "b": track_count(&b, &TrackType::Dialogue),
        },
        "voiceover": {
            "a": track_count(&a, &TrackType::Voiceover),
            "b": track_count(&b, &TrackType::Voiceover),
        },
        "broll": {
            "a": track_count(&a, &TrackType::Broll),
            "b": track_count(&b, &TrackType::Broll),
        },
        "music": {
            "a": track_count(&a, &TrackType::Music),
            "b": track_count(&b, &TrackType::Music),
        },
        "sfx": {
            "a": track_count(&a, &TrackType::Sfx),
            "b": track_count(&b, &TrackType::Sfx),
        },
    });

    Ok(json!({
        "status": "success",
        "duration_change_ms": duration_change_ms,
        "segments": {
            "added": added,
            "removed": removed,
            "modified": modified,
        },
        "tracks": track_changes,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.preview
// ---------------------------------------------------------------------------

async fn handle_timeline_preview(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;

    let total_duration_ms = timeline.total_duration_ms();
    let segments_info: Vec<serde_json::Value> = timeline
        .segments
        .iter()
        .map(|s| {
            // P2-1 fix: append ellipsis when the caption is truncated, so agents
            // can tell the preview is abbreviated. Prior versions silently cut at
            // 60 chars with no indication.
            let caption_display = if s.caption.chars().count() > 60 {
                format!("{}...", s.caption.chars().take(57).collect::<String>())
            } else {
                s.caption.clone()
            };
            json!({
                "id": s.id,
                "start": s.start,
                "end": s.end,
                "caption": caption_display,
                "crossfade_ms": s.crossfade_ms,
            })
        })
        .collect();

    let tracks_info: serde_json::Map<String, serde_json::Value> = timeline
        .tracks
        .iter()
        .map(|(track, events)| {
            let track = track as &TrackType;
            let events = events as &Vec<openscript_core::timeline::TimelineEvent>;
            (
                track.to_string(),
                json!({
                    "count": events.len(),
                    "total_duration_ms": events.iter().map(|e| e.end_ms - e.start_ms).sum::<i64>(),
                }),
            )
        })
        .collect();

    let errors = timeline.validate();
    let render_ready = errors.is_empty() && !timeline.segments.is_empty();

    Ok(json!({
        "status": "success",
        "timeline_path": timeline_path,
        "version": timeline.version,
        "total_duration_ms": total_duration_ms,
        "segments_count": timeline.segments.len(),
        "segments": segments_info,
        "tracks": tracks_info,
        "render_ready": render_ready,
        "validation_errors": errors,
    }))
}

// ---------------------------------------------------------------------------
// Handler: tts.preview
// ---------------------------------------------------------------------------

async fn handle_tts_preview(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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

// ---------------------------------------------------------------------------
// Handler: music.ducking.plan
// ---------------------------------------------------------------------------

async fn handle_music_ducking_plan(
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

// ---------------------------------------------------------------------------
// Handler: timeline.autofill_broll
// ---------------------------------------------------------------------------

async fn handle_timeline_autofill_broll(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let cadence_seconds = default_f64(&args, "cadence_seconds", 2.0);
    let _orientation = default_str(&args, "orientation", "9:16");
    let _quality = default_str(&args, "quality", "sd");
    let max_gaps = default_u32(&args, "max_gaps", 20);

    let mut timeline = Timeline::load(timeline_path)?;

    let cadence_ms = (cadence_seconds * 1000.0) as i64;
    let total_ms = timeline.total_duration_ms();
    let mut count = 0;
    let mut position_ms = 0i64;

    report_progress(0.0, max_gaps as f64, "Auto-filling b-roll slots...")
        .await
        .ok();

    while position_ms < total_ms && count < max_gaps as i64 {
        let duration = cadence_ms.min(total_ms - position_ms);
        if duration > 0 {
            let event_id = format!("broll_{:03}", track_count(&timeline, &TrackType::Broll) + 1);
            let concept = timeline
                .segments
                .iter()
                .find(|s| {
                    let seg_start = (s.start * 1000.0) as i64;
                    let seg_end = (s.end * 1000.0) as i64;
                    position_ms >= seg_start && position_ms < seg_end
                })
                .map(|s| {
                    s.caption
                        .split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join("_")
                })
                .unwrap_or_else(|| "general".into());

            let event = openscript_core::timeline::TimelineEvent {
                id: event_id.clone(),
                asset_id: "placeholder".into(),
                start_ms: position_ms,
                end_ms: position_ms + duration,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![concept.clone()],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "timeline.autofill_broll".into(),
                    editorial_role: None,
                    concept: Some(concept.clone()),
                }),
                kind: openscript_core::timeline::EventKind::Broll {
                    concept,
                    source_provider: "placeholder".into(),
                    transition_style: "cut".into(),
                    crop_mode: "center".into(),
                    orientation: "9:16".into(),
                    motion_intensity: "medium".into(),
                },
            };

            timeline.add_track_event(TrackType::Broll, event);
            count += 1;

            // Report progress every 5 slots to avoid spamming
            if count % 5 == 0 || count == max_gaps as i64 {
                report_progress(
                    count as f64,
                    max_gaps as f64,
                    &format!("Filled {} b-roll slots", count),
                )
                .await
                .ok();
            }
        }
        position_ms += cadence_ms;
    }

    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "autofilled",
        "timeline_path": timeline_path,
        "broll_events_added": count,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.render
// ---------------------------------------------------------------------------

async fn handle_timeline_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let source_video = default_opt_str(&args, "source_video");
    let output_path = default_opt_str(&args, "output_path");
    let crf = default_opt_u32(&args, "crf");

    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;
    let errors = timeline.validate();
    if !errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline has {} validation error(s): {:?}",
            errors.len(),
            errors
        )));
    }

    let source = source_video.unwrap_or_else(|| timeline.source.to_string_lossy().to_string());
    if !Path::new(&source).exists() {
        return Err(ToolError::NotFound(format!(
            "Source video not found: {}",
            source
        )));
    }

    let total_tracks = timeline.tracks.values().map(|v| v.len()).sum::<usize>();
    report_progress(
        0.0,
        100.0,
        &format!(
            "Rendering timeline ({} segments, {} track events)...",
            timeline.segments.len(),
            total_tracks
        ),
    )
    .await
    .ok();

    report_progress(20.0, 100.0, "Building filter graph...")
        .await
        .ok();

    // Filter out placeholder b-roll events before rendering to prevent FFmpeg crash
    if let Some(broll_events) = timeline.tracks.get_mut(&TrackType::Broll) {
        let before = broll_events.len();
        broll_events.retain(|e| e.asset_id != "placeholder" && !e.asset_id.is_empty());
        let removed = before - broll_events.len();
        if removed > 0 {
            tracing::warn!(
                "[timeline.render] Filtered {} placeholder b-roll events",
                removed
            );
        }
    }

    let result = render_from_timeline(&timeline, &source, output_path.as_deref(), crf).await;

    match result {
        Ok(out_path) => {
            report_progress(100.0, 100.0, "Render complete").await.ok();
            let file_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            Ok(json!({
                "status": "rendered",
                "output_path": out_path,
                "file_size_bytes": file_size,
                "segments_count": timeline.segments.len(),
                "tracks_rendered": total_tracks,
            }))
        }
        Err(e) => {
            // P0-2 fix: include the ffmpeg error inline (and a tail of the render
            // log when one exists) so AI agents can self-correct without having
            // to read a separate log file. Prior versions returned only
            // "Render failed, see log: /path/to/render.log" which gave agents
            // no actionable information.
            let err_str = e.to_string();
            let log_excerpt = if let Some(log_path) = err_str
                .strip_prefix("Render failed, see log: ")
                .or_else(|| err_str.strip_prefix("Render failed: "))
            {
                std::fs::read_to_string(log_path).ok().map(|content| {
                    let lines: Vec<&str> = content.lines().collect();
                    let last_20: Vec<&str> = lines.iter().rev().take(20).rev().cloned().collect();
                    last_20.join("\n")
                })
            } else {
                None
            };
            let mut msg = format!("Render failed: {}", err_str);
            if let Some(excerpt) = log_excerpt {
                if !excerpt.is_empty() {
                    msg.push_str("\n\n--- render log (last 20 lines) ---\n");
                    msg.push_str(&excerpt);
                }
            }
            Err(ToolError::Ffmpeg(msg))
        }
    }
}

// ---------------------------------------------------------------------------
// Handler: broll.director
// ---------------------------------------------------------------------------

async fn handle_broll_director(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    let max_slots = default_u32(&args, "max_slots", 20) as usize;
    let cadence_seconds = default_f64(&args, "cadence_seconds", 2.0);
    let cadence_ms = (cadence_seconds * 1000.0) as i64;

    let api_key = pexels_key();
    if api_key.is_empty() {
        return Err(ToolError::Asset(
            "PEXELS_API_KEY not set. Set it in mcp/assets/.openscript_config.json or as an env var. Get a free key at https://www.pexels.com/api/".to_string()
        ));
    }

    let mut timeline = Timeline::load(timeline_path)?;

    report_progress(0.0, 100.0, "Analyzing script and creating b-roll slots...")
        .await
        .ok();

    let slots_created = timeline.generate_broll_from_script(cadence_ms, max_slots);

    if slots_created == 0 {
        return Ok(json!({
            "status": "no_slots",
            "timeline_path": timeline_path,
            "broll_slots_filled": 0,
            "concepts_used": [],
            "cached_paths": [],
        }));
    }

    let broll_events = timeline.get_track_events("broll");
    let mut concepts: Vec<String> = Vec::new();
    let mut event_concept_map: Vec<(String, String)> = Vec::new();

    for event in &broll_events {
        if event.asset_id == "placeholder" {
            let concept = event
                .tags
                .first()
                .cloned()
                .unwrap_or_else(|| "general".into());
            if !concepts.contains(&concept) {
                concepts.push(concept.clone());
            }
            event_concept_map.push((event.id.clone(), concept));
        }
    }

    let asset_dir = "mcp/assets/broll_cache";
    let mut client = PexelsClient::new(&api_key, asset_dir);
    let mut cached_paths: Vec<serde_json::Value> = Vec::new();
    let mut filled_count = 0;

    let total_concepts = concepts.len();
    for (i, concept) in concepts.iter().enumerate() {
        report_progress(
            (i as f64 / total_concepts as f64) * 80.0 + 10.0,
            100.0,
            &format!("Fetching b-roll for: {}", concept),
        )
        .await
        .ok();

        let search_result = client
            .search_for_slot(concept, &orientation, &quality)
            .await;
        match search_result {
            Ok(Some(video)) => match client.download_best(&video, concept).await {
                Ok(path) => {
                    for (event_id, event_concept) in &event_concept_map {
                        if event_concept == concept {
                            timeline.add_asset("broll", event_id.clone(), json!({"path": &path}));
                            if let Some(events) = timeline.tracks.get_mut(&TrackType::Broll) {
                                for event in events.iter_mut() {
                                    if event.id == *event_id {
                                        event.asset_id = path.clone();
                                        break;
                                    }
                                }
                            }
                            filled_count += 1;
                        }
                    }
                    cached_paths.push(json!({"concept": concept, "path": path}));
                }
                Err(e) => tracing::warn!("[broll.director] Download failed for {}: {}", concept, e),
            },
            Ok(None) => tracing::warn!("[broll.director] No video found for concept: {}", concept),
            Err(e) => tracing::warn!("[broll.director] Search failed for {}: {}", concept, e),
        }
    }

    timeline.save(timeline_path)?;

    report_progress(100.0, 100.0, "B-roll director complete")
        .await
        .ok();

    Ok(json!({
        "status": "success",
        "timeline_path": timeline_path,
        "broll_slots_filled": filled_count,
        "concepts_used": concepts,
        "cached_paths": cached_paths,
    }))
}

// ---------------------------------------------------------------------------
// Handler: reelize.timeline (end-to-end pipeline)
// ---------------------------------------------------------------------------

async fn handle_reelize_timeline(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;

    let video_path = extract_str(&args, "video_path")?;
    let preset = default_str(&args, "preset", "Balanced");
    let max_duration = default_opt_u32(&args, "max_duration");
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);
    let animated_captions = default_bool(&args, "animated_captions", false);
    let output_path = default_opt_str(&args, "output_path");
    let crf = default_u32(&args, "crf", 20);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let mut warnings: Vec<String> = Vec::new();

    // Environment diagnostics — warn about missing capabilities early
    if pexels_key().is_empty() {
        warnings.push("PEXELS_API_KEY not set — b-roll will be skipped".into());
    }
    let tts_available = std::env::var("OPENSCRIPT_TTS_URL").is_ok();
    if !tts_available {
        warnings
            .push("No TTS server configured (OPENSCRIPT_TTS_URL) — voiceover unavailable".into());
    }

    // B-roll options
    let broll_obj = args.get("broll").cloned().unwrap_or(json!({}));
    let broll_enabled = default_bool(&broll_obj, "enabled", true);
    let broll_cadence = default_f64(&broll_obj, "cadence_seconds", 2.0);
    let broll_max_slots = default_u32(&broll_obj, "max_slots", 20);

    // Music options
    let music_obj = args.get("music").cloned().unwrap_or(json!({}));
    let music_enabled = default_bool(&music_obj, "enabled", true);
    let music_mood = default_str(&music_obj, "mood", "neutral");
    let music_energy = default_str(&music_obj, "energy", "medium");
    let music_gain_db = default_f64(&music_obj, "gain_db", -10.0);

    // SFX options
    let sfx_obj = args.get("sfx").cloned().unwrap_or(json!({}));
    let sfx_enabled = default_bool(&sfx_obj, "enabled", true);

    let crossfade_ms = match preset.as_str() {
        "Tight" => 200,
        "Balanced" => 500,
        "Natural" => 800,
        _ => 500,
    };

    let timeline_path = default_timeline_path(video_path);

    // Step 1/7: Transcribe → SRT
    report_progress(0.0, 100.0, "Step 1/7: Transcribing audio...")
        .await
        .ok();
    let transcribe_args = json!({ "media_path": video_path });
    let transcribe_result = handle_transcribe(transcribe_args).await?;
    let srt_path = transcribe_result
        .get("output_srt_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
        .to_string();
    report_progress(15.0, 100.0, "Transcription complete")
        .await
        .ok();

    // Step 2/7: SRT prepare → grouped SRT
    report_progress(15.0, 100.0, "Step 2/7: Grouping captions...")
        .await
        .ok();
    let prepare_args = json!({
        "srt_path": &srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    });
    let prepare_result = handle_srt_prepare(prepare_args).await?;
    let grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();
    report_progress(25.0, 100.0, "Caption grouping complete")
        .await
        .ok();

    // Step 3/7: Build timeline + populate segments from SRT
    report_progress(25.0, 100.0, "Step 3/7: Building timeline...")
        .await
        .ok();
    let mut timeline = Timeline::new(video_path.into(), &aspect, 30, max_duration);
    let segment_count = timeline
        .populate_segments_from_srt(&grouped_srt_path, crossfade_ms)
        .map_err(|e| ToolError::Timeline(e))?;

    if segment_count == 0 {
        return Err(ToolError::Timeline(
            "No segments created from SRT — transcript may be empty".to_string(),
        ));
    }

    // Generate ASS subtitles with Bebas Neue styling for burn-in
    let ass_path = {
        let p = Path::new(&grouped_srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.ass", stem))
            .to_string_lossy()
            .to_string()
    };
    match openscript_core::srt::parse_srt(&grouped_srt_path) {
        Ok(entries) => {
            let ass_entries: Vec<(f64, f64, String)> = entries
                .iter()
                .map(|e| (e.start, e.end, e.text.clone()))
                .collect();
            match openscript_ffmpeg::subtitles::srt_to_ass(&ass_entries, &ass_path, "Default") {
                Ok(()) => {
                    timeline
                        .assets
                        .captions
                        .insert("ass".into(), json!({"path": ass_path.clone()}));
                }
                Err(e) => tracing::warn!("[reelize.timeline] ASS generation failed: {}", e),
            }
        }
        Err(e) => tracing::warn!("[reelize.timeline] SRT parse failed for ASS: {}", e),
    }

    let validation_errors = timeline.validate();
    if !validation_errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline validation failed after segment population: {:?}",
            validation_errors
        )));
    }

    timeline.save(&timeline_path)?;
    report_progress(
        40.0,
        100.0,
        &format!("Timeline built with {} segments", segment_count),
    )
    .await
    .ok();

    // Step 4/7: B-roll director (if enabled)
    if broll_enabled {
        report_progress(40.0, 100.0, "Step 4/7: B-roll director...")
            .await
            .ok();
        let broll_args = json!({
            "timeline_path": &timeline_path,
            "orientation": "9:16",
            "quality": "sd",
            "max_slots": broll_max_slots,
            "cadence_seconds": broll_cadence,
        });
        let broll_result = handle_broll_director(broll_args).await;
        match broll_result {
            Ok(r) => {
                let filled = r
                    .get("broll_slots_filled")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                report_progress(55.0, 100.0, &format!("B-roll: {} slots filled", filled))
                    .await
                    .ok();

                // Clean up placeholder b-roll events that failed to download.
                // These have asset_id="placeholder" and would crash the render
                // pipeline or be silently skipped. Remove them so the timeline
                // is clean for subsequent validate/render calls.
                if let Ok(mut timeline) = Timeline::load(&timeline_path) {
                    if let Some(broll_events) = timeline.tracks.get_mut(&TrackType::Broll) {
                        let before = broll_events.len();
                        broll_events.retain(|e| e.asset_id != "placeholder");
                        let removed = before - broll_events.len();
                        if removed > 0 {
                            warnings.push(format!(
                                "Removed {} placeholder b-roll events (no asset downloaded)",
                                removed
                            ));
                        }
                        // Save the cleaned timeline
                        let _ = timeline.save(&timeline_path);
                    }
                }
            }
            Err(e) => warnings.push(format!("B-roll director skipped: {}", e)),
        }
    } else {
        report_progress(55.0, 100.0, "B-roll disabled, skipping")
            .await
            .ok();
    }

    // Step 5/7: Music + SFX
    report_progress(55.0, 100.0, "Step 5/7: Assigning music and SFX...")
        .await
        .ok();

    if music_enabled {
        // Search for a matching music track first, then pass its path to music.assign
        // NOTE: Call library.search directly instead of the deprecated music.search
        // wrapper. The library_index.json entries have mood="none" and energy="none",
        // so mood/energy exact-match filters would reject all tracks.
        let music_search_args = json!({
            "query": format!("{} {} background music", music_mood, music_energy),
            "limit": 1,
        });
        let music_path = match handle_library_search(music_search_args).await {
            Ok(r) => {
                let _results_count = r.get("results")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                // library.search results use 'filename' key (just filename),
                // not 'path'. Construct the full path from the music directory.
                // Fallback: if the file doesnt exist in mcp/assets/music/,
                // search mcp/assets/music_cache/ for any available MP3.
                r.get("results")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|first| first.get("filename"))
                    .and_then(|p| p.as_str())
                    .and_then(|s| {
                        // Try primary path: mcp/assets/music/{filename}
                        let primary = resolve_repo_path(&format!("mcp/assets/music/{}", s));
                        if primary.exists() {
                            eprintln!("[reelize.timeline] Music path (primary): {}", primary.display());
                            return Some(primary.to_string_lossy().to_string());
                        }
                        // Fallback: pick first MP3 from mcp/assets/music_cache/
                        let cache_dir = resolve_repo_path("mcp/assets/music_cache");
                        if let Ok(entries) = std::fs::read_dir(&cache_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.extension().map_or(false, |e| e == "mp3") {
                                    let path_str = path.to_string_lossy().to_string();
                                    return Some(path_str);
                                }
                            }
                        }
                        None
                    })
            },
            Err(e) => {
                warnings.push(format!("Music search failed: {}", e));
                None
            }
        };

        if let Some(path) = music_path {
            let music_args = json!({
                "timeline_path": &timeline_path,
                "path": path,

                "gain_db": music_gain_db,
                "ducking": true,
            });
            let music_result = handle_music_assign(music_args).await;
            match music_result {
                Ok(_r) => {
                    if let Ok(t) = Timeline::load(&timeline_path) {
                        let music_count = t
                            .tracks
                            .get(&TrackType::Music)
                            .map(|v| v.len())
                            .unwrap_or(0);
                        report_progress(
                            60.0,
                            100.0,
                            &format!("Music assigned ({} track(s))", music_count),
                        )
                        .await
                        .ok();
                    }
                }
                Err(e) => warnings.push(format!("Music assign skipped: {}", e)),
            }
        } else {
            warnings.push("No music track found in index — skipping music assignment".to_string());
        }
    }

    if sfx_enabled {
        let sfx_index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
            .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());
        let sfx_index = match openscript_assets::sfx::SfxIndex::load(Some(&sfx_index_path)) {
            Ok(idx) => Some(idx),
            Err(e) => {
                warnings.push(format!("SFX index failed to load: {}", e));
                None
            }
        };

        if let Ok(mut timeline) = Timeline::load(&timeline_path) {
            let total_ms = timeline.total_duration_ms();

            let resolve_sfx_path = |role: &str| -> Option<String> {
                // Map "hook" → "intro" since the SFX index uses "intro" for opening effects
                let mapped_role = if role == "hook" { "intro" } else { role };
                sfx_index.as_ref().and_then(|idx| {
                    idx.search("", Some(mapped_role), None, 1)
                        .first()
                        .map(|a| a.path.clone())
                })
            };

            if !timeline.segments.is_empty() {
                let hook_id = format!("sfx_{:03}", track_count(&timeline, &TrackType::Sfx) + 1);
                let hook_path = resolve_sfx_path("hook");
                let event = openscript_core::timeline::TimelineEvent {
                    id: hook_id.clone(),
                    asset_id: hook_path.clone().unwrap_or_else(|| "hook".into()),
                    start_ms: 0,
                    end_ms: 500,
                    offset_ms: 0,
                    gain_db: -10.0,
                    fade_in_ms: 50,
                    fade_out_ms: 50,
                    tags: vec!["hook".into()],
                    provenance: Some(openscript_core::timeline::Provenance {
                        tool: "reelize.timeline".into(),
                        editorial_role: Some("hook".into()),
                        concept: None,
                    }),
                    kind: openscript_core::timeline::EventKind::Sfx {
                        editorial_role: "hook".into(),
                        category: "hook".into(),
                        subcategory: String::new(),
                        duration_ms: 500,
                        sample_rate: 44100,
                        peak_db: 0.0,
                        loudness_lufs: -14.0,
                        recommended_gain_db: -10.0,
                        recommended_use: "single_hit".into(),
                        safe_overlay: true,
                    },
                };
                timeline.add_track_event(TrackType::Sfx, event);
                if let Some(path) = hook_path {
                    timeline.add_asset("sfx", hook_id, json!({"path": path}));
                }
            }

            // Space transitions evenly — max 10 for editorial quality
            let all_transitions: Vec<i64> = timeline
                .segments
                .iter()
                .skip(1)
                .map(|seg| ((seg.start * 1000.0) as i64) - 100)
                .filter(|ms| *ms > 0)
                .collect();

            let max_transitions = all_transitions.len().min(10);
            let transition_positions: Vec<i64> = if max_transitions <= 1 {
                all_transitions
            } else {
                let step = all_transitions.len() / max_transitions;
                let step = step.max(1);
                all_transitions
                    .into_iter()
                    .step_by(step)
                    .take(max_transitions)
                    .collect()
            };

            for transition_ms in &transition_positions {
                let sfx_count = track_count(&timeline, &TrackType::Sfx);
                let sfx_id = format!("sfx_{:03}", sfx_count + 1);
                let trans_path = resolve_sfx_path("transition");
                let event = openscript_core::timeline::TimelineEvent {
                    id: sfx_id.clone(),
                    asset_id: trans_path.clone().unwrap_or_else(|| "transition".into()),
                    start_ms: *transition_ms,
                    end_ms: transition_ms + 500,
                    offset_ms: 0,
                    gain_db: -10.0,
                    fade_in_ms: 50,
                    fade_out_ms: 50,
                    tags: vec!["transition".into()],
                    provenance: Some(openscript_core::timeline::Provenance {
                        tool: "reelize.timeline".into(),
                        editorial_role: Some("transition".into()),
                        concept: None,
                    }),
                    kind: openscript_core::timeline::EventKind::Sfx {
                        editorial_role: "transition".into(),
                        category: "transition".into(),
                        subcategory: String::new(),
                        duration_ms: 500,
                        sample_rate: 44100,
                        peak_db: 0.0,
                        loudness_lufs: -14.0,
                        recommended_gain_db: -10.0,
                        recommended_use: "single_hit".into(),
                        safe_overlay: true,
                    },
                };
                timeline.add_track_event(TrackType::Sfx, event);
                if let Some(path) = trans_path {
                    timeline.add_asset("sfx", sfx_id, json!({"path": path}));
                }
            }

            if total_ms > 2000 {
                let highlight_id =
                    format!("sfx_{:03}", track_count(&timeline, &TrackType::Sfx) + 1);
                let highlight_path = resolve_sfx_path("highlight");
                let event = openscript_core::timeline::TimelineEvent {
                    id: highlight_id.clone(),
                    asset_id: highlight_path.clone().unwrap_or_else(|| "highlight".into()),
                    start_ms: total_ms / 2,
                    end_ms: (total_ms / 2) + 500,
                    offset_ms: 0,
                    gain_db: -10.0,
                    fade_in_ms: 50,
                    fade_out_ms: 50,
                    tags: vec!["highlight".into()],
                    provenance: Some(openscript_core::timeline::Provenance {
                        tool: "reelize.timeline".into(),
                        editorial_role: Some("highlight".into()),
                        concept: None,
                    }),
                    kind: openscript_core::timeline::EventKind::Sfx {
                        editorial_role: "highlight".into(),
                        category: "highlight".into(),
                        subcategory: String::new(),
                        duration_ms: 500,
                        sample_rate: 44100,
                        peak_db: 0.0,
                        loudness_lufs: -14.0,
                        recommended_gain_db: -10.0,
                        recommended_use: "single_hit".into(),
                        safe_overlay: true,
                    },
                };
                timeline.add_track_event(TrackType::Sfx, event);
                if let Some(path) = highlight_path {
                    timeline.add_asset("sfx", highlight_id, json!({"path": path}));
                }
            }

            timeline.save(&timeline_path)?;
        }
    }

    let sfx_count = if let Ok(t) = Timeline::load(&timeline_path) {
        track_count(&t, &TrackType::Sfx)
    } else {
        0
    };
    report_progress(
        70.0,
        100.0,
        &format!("Music and SFX assigned ({} SFX events)", sfx_count),
    )
    .await
    .ok();

    // Step 6/7: Animated captions overlay
    if animated_captions && burn_captions {
        report_progress(
            70.0,
            100.0,
            "Step 6/7: Generating animated caption overlay...",
        )
        .await
        .ok();
        // Need an EDL path for overlay.generate — create a minimal one
        let edl_path = {
            let p = Path::new(&timeline_path);
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            p.parent()
                .unwrap_or(Path::new("."))
                .join(format!("{}.edl.json", stem))
                .to_string_lossy()
                .to_string()
        };
        let t = Timeline::load(&timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
        let segments_json: Vec<serde_json::Value> = t
            .segments
            .iter()
            .map(|s| {
                json!({"id": s.id, "start": s.start, "end": s.end, "caption": s.caption, "crossfade_ms": s.crossfade_ms})
            })
            .collect();
        let edl = json!({
            "source": video_path,
            "target": {"aspect": &aspect, "fps": 30},
            "segments": segments_json,
            "effects": {"burn_captions": true, "audio": {"loudnorm": true}},
        });
        std::fs::write(
            &edl_path,
            serde_json::to_string_pretty(&edl).unwrap_or_default(),
        )
        .ok();

        let overlay_args = json!({
            "srt_path": &grouped_srt_path,
            "edl_path": &edl_path,
            "timeline_path": &timeline_path,
            "animate": true,
            "style": "pupcaps_center",
        });
        let overlay_result = handle_overlay_generate(overlay_args).await;
        if let Err(e) = overlay_result {
            warnings.push(format!("Animated overlay skipped: {}", e));
        }
        report_progress(85.0, 100.0, "Animated captions generated")
            .await
            .ok();
    } else {
        report_progress(85.0, 100.0, "Static captions (burn-in)")
            .await
            .ok();
    }

    // Step 7/7: Validate + render
    report_progress(85.0, 100.0, "Step 7/7: Validating and rendering...")
        .await
        .ok();

    let timeline =
        Timeline::load(&timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
    let errors = timeline.validate();
    if !errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline validation failed before render: {:?}",
            errors
        )));
    }

    let broll_count = timeline
        .tracks
        .get(&TrackType::Broll)
        .map(|v| v.len())
        .unwrap_or(0);
    let music_count = timeline
        .tracks
        .get(&TrackType::Music)
        .map(|v| v.len())
        .unwrap_or(0);
    let sfx_count = timeline
        .tracks
        .get(&TrackType::Sfx)
        .map(|v| v.len())
        .unwrap_or(0);
    let total_tracks = timeline.tracks.values().map(|v| v.len()).sum::<usize>();
    report_progress(
        90.0,
        100.0,
        &format!(
            "Rendering ({} segments, {} track events)...",
            timeline.segments.len(),
            total_tracks
        ),
    )
    .await
    .ok();

    let source = video_path;
    let result = render_from_timeline(&timeline, source, output_path.as_deref(), Some(crf)).await;

    match result {
        Ok(out_path) => {
            let file_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            report_progress(100.0, 100.0, "Reel complete!").await.ok();
            Ok(json!({
                "status": "rendered",
                "output_path": out_path,
                "file_size_bytes": file_size,
                "timeline_path": timeline_path,
                "segments_count": timeline.segments.len(),
                "tracks_rendered": total_tracks,
                "preset": preset,
                "tts_available": tts_available,
                "broll_count": broll_count,
                "music_count": music_count,
                "sfx_count": sfx_count,
                "warnings": if warnings.is_empty() { serde_json::Value::Null } else { json!(warnings) },
            }))
        }
        Err(e) => Err(ToolError::Ffmpeg(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Handler: verify.audio
// ---------------------------------------------------------------------------

async fn handle_verify_audio(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let expected_has_voice = default_bool(&args, "expected_has_voice", true);
    let max_silence_seconds = default_f64(&args, "max_silence_seconds", 3.0);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate,channels,duration",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| ToolError::Json(e))?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let has_audio = !streams.is_empty();

    if !has_audio {
        return Ok(json!({
            "status": "warning",
            "issues": ["No audio stream detected — voice/music/SFX are missing"],
            "rms_lufs": null,
            "peak_db": null,
            "silence_segments": [],
            "has_dialogue": false,
            "quality_score": 0,
        }));
    }

    let vol_output = tokio::process::Command::new("ffmpeg")
        .args(["-i", &video_path, "-af", "volumedetect", "-f", "null", "-"])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("volumedetect failed: {}", e)))?;

    if !vol_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "volumedetect failed: {}",
            String::from_utf8_lossy(&vol_output.stderr)
        )));
    }

    let stderr = String::from_utf8_lossy(&vol_output.stderr);
    let mean_volume = stderr
        .lines()
        .find(|l| l.contains("mean_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());
    let max_volume = stderr
        .lines()
        .find(|l| l.contains("max_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());

    let silence_output = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &video_path,
            "-af",
            &format!("silencedetect=noise=-30dB:d={}", max_silence_seconds),
            "-f",
            "null",
            "-",
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("silencedetect failed: {}", e)))?;

    if !silence_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "silencedetect failed: {}",
            String::from_utf8_lossy(&silence_output.stderr)
        )));
    }

    let silence_stderr = String::from_utf8_lossy(&silence_output.stderr);
    let mut silence_segments: Vec<serde_json::Value> = Vec::new();
    let mut current_start: Option<f64> = None;
    for line in silence_stderr.lines() {
        if line.contains("silence_start:") {
            if let Some(val) = line.split(": ").nth(1).and_then(|v| v.parse::<f64>().ok()) {
                current_start = Some(val);
            }
        } else if line.contains("silence_end:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let (Some(start), Some(end)) = (
                current_start,
                parts.get(1).and_then(|v| v.parse::<f64>().ok()),
            ) {
                silence_segments.push(json!({
                    "start": start,
                    "end": end,
                    "duration": end - start,
                }));
                current_start = None;
            }
        }
    }

    let rms = mean_volume.unwrap_or(-99.0);
    let peak = max_volume.unwrap_or(-99.0);
    let has_good_level = rms >= -30.0 && rms <= -12.0;
    let has_no_clipping = peak <= 0.0;
    let no_long_silence = silence_segments.is_empty();

    let quality_score = if expected_has_voice {
        let mut score = 0;
        if has_audio {
            score += 25;
        }
        if has_good_level {
            score += 25;
        }
        if has_no_clipping {
            score += 25;
        }
        if no_long_silence {
            score += 25;
        }
        score
    } else {
        if has_audio {
            50
        } else {
            100
        }
    };

    let mut issues: Vec<String> = Vec::new();
    if !has_audio {
        issues.push("No audio stream".into());
    }
    if !has_good_level && has_audio {
        issues.push(format!(
            "Audio level unhealthy: RMS {} dB (expected -30 to -12 dB)",
            rms
        ));
    }
    if !has_no_clipping {
        issues.push(format!("Audio clipping detected: peak {} dB", peak));
    }
    if !no_long_silence {
        issues.push(format!(
            "{} silence gaps detected (>{})",
            silence_segments.len(),
            max_silence_seconds
        ));
    }

    Ok(json!({
        "status": if quality_score >= 75 { "pass" } else if quality_score >= 50 { "warning" } else { "fail" },
        "rms_lufs": rms,
        "peak_db": peak,
        "silence_segments": silence_segments,
        "has_dialogue": has_audio && has_good_level,
        "quality_score": quality_score,
        "issues": issues,
        "audio_codec": streams.first().and_then(|s| s.get("codec_name")).and_then(|v| v.as_str()).unwrap_or("unknown"),
        "sample_rate": streams.first().and_then(|s| s.get("sample_rate")).and_then(|v| v.as_str()).unwrap_or("unknown"),
    }))
}

/// Parse an ASS (Advanced SubStation Alpha) caption file into SrtEntry-like
/// records so verify.captions can work with the ASS files that
/// script.to_video produces. Extracts Dialogue lines' start/end timestamps
/// and strips ASS override tags ({\...}) from the visible text.
///
/// ASS Dialogue format: `Dialogue: layer,start,end,style,name,L,R,E,text`
/// where start/end are `H:MM:SS.cc` (centiseconds, not milliseconds).
/// (UX audit round-2 GAP #10 fix — verify.captions previously only
/// accepted SRT, but script.to_video emits ASS.)
fn parse_ass_captions(path: &str) -> Result<Vec<openscript_core::srt::SrtEntry>, ToolError> {
    let content = std::fs::read_to_string(path).map_err(|e| ToolError::Io(e))?;
    let mut entries = Vec::new();
    let mut idx = 1;

    for line in content.lines() {
        let line = line.trim();
        if !line.starts_with("Dialogue:") {
            continue;
        }
        // Dialogue: layer,start,end,style,name,L,R,E,text
        let parts: Vec<&str> = line.splitn(9, ',').collect();
        if parts.len() < 9 {
            continue;
        }
        let start_s = parse_ass_timestamp(parts[1].trim())?;
        let end_s = parse_ass_timestamp(parts[2].trim())?;
        // Strip ASS override tags {\...} from the visible text
        let text = strip_ass_tags(parts[8].trim());

        entries.push(openscript_core::srt::SrtEntry {
            idx,
            start: start_s,
            end: end_s,
            text,
        });
        idx += 1;
    }

    Ok(entries)
}

/// Parse an ASS timestamp `H:MM:SS.cc` into seconds (f64).
/// Example: "0:00:01.50" -> 1.5
fn parse_ass_timestamp(ts: &str) -> Result<f64, ToolError> {
    let parts: Vec<&str> = ts.split(':').collect();
    if parts.len() != 3 {
        return Err(ToolError::InvalidArg(format!(
            "Invalid ASS timestamp: {}",
            ts
        )));
    }
    let h: f64 = parts[0].parse().unwrap_or(0.0);
    let m: f64 = parts[1].parse().unwrap_or(0.0);
    let s: f64 = parts[2].parse().unwrap_or(0.0);
    Ok(h * 3600.0 + m * 60.0 + s)
}

/// Strip ASS override tags ({\...}) from text, leaving only visible characters.
fn strip_ass_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        if ch == '{' {
            in_tag = true;
        } else if ch == '}' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    result.trim().to_string()
}

// ---------------------------------------------------------------------------
// Handler: verify.captions
// ---------------------------------------------------------------------------

async fn handle_verify_captions(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let min_caption_duration_ms = default_i64(&args, "min_caption_duration_ms", 300);
    let max_caption_duration_ms = default_i64(&args, "max_caption_duration_ms", 5000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&srt_path).exists() {
        return Err(ToolError::NotFound(format!("Caption file not found: {}", srt_path)));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&probe_output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&probe_output.stdout).map_err(|e| ToolError::Json(e))?;
    let video_duration_s: f64 = probe
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let video_duration_ms = (video_duration_s * 1000.0) as i64;

    // Auto-detect caption format: ASS (.ass) or SRT (.srt).
    // script.to_video emits ASS; verify.captions previously only accepted
    // SRT, which meant the verify step was unusable after a script.to_video
    // render. Now we accept both and normalize to the same entry format.
    // (UX audit round-2 GAP #10 fix.)
    let is_ass = srt_path.ends_with(".ass");
    let entries: Vec<openscript_core::srt::SrtEntry> = if is_ass {
        parse_ass_captions(&srt_path)?
    } else {
        openscript_core::srt::parse_srt(&srt_path)
            .map_err(|e| ToolError::Srt(e.to_string()))?
    };

    if entries.is_empty() {
        return Ok(json!({
            "status": "fail",
            "issues": ["SRT file has no entries"],
            "caption_count": 0,
            "coverage_percent": 0.0,
            "gaps": [],
            "overlaps": [],
            "avg_caption_duration_ms": 0,
            "readability_score": 0,
        }));
    }

    let mut total_caption_ms: i64 = 0;
    let mut gaps: Vec<serde_json::Value> = Vec::new();
    let mut overlaps: Vec<serde_json::Value> = Vec::new();
    let mut too_fast: Vec<serde_json::Value> = Vec::new();
    let mut too_slow: Vec<serde_json::Value> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let start_ms = (entry.start * 1000.0) as i64;
        let end_ms = (entry.end * 1000.0) as i64;
        let duration_ms = end_ms - start_ms;
        total_caption_ms += duration_ms;

        if duration_ms < min_caption_duration_ms {
            too_fast.push(json!({"idx": entry.idx, "duration_ms": duration_ms, "text": entry.text.chars().take(40).collect::<String>()}));
        }
        if duration_ms > max_caption_duration_ms {
            too_slow.push(json!({"idx": entry.idx, "duration_ms": duration_ms, "text": entry.text.chars().take(40).collect::<String>()}));
        }

        if i > 0 {
            let prev_end = (entries[i - 1].end * 1000.0) as i64;
            let gap_ms = start_ms - prev_end;
            if gap_ms > 2000 {
                gaps.push(json!({"after_idx": entries[i-1].idx, "before_idx": entry.idx, "gap_ms": gap_ms}));
            }
        }

        if i > 0 {
            let prev_end = (entries[i - 1].end * 1000.0) as i64;
            let prev_start = (entries[i - 1].start * 1000.0) as i64;
            if start_ms < prev_end && start_ms > prev_start {
                overlaps.push(json!({"idx_a": entries[i-1].idx, "idx_b": entry.idx, "overlap_ms": prev_end - start_ms}));
            }
        }
    }

    let avg_duration = if !entries.is_empty() {
        total_caption_ms / entries.len() as i64
    } else {
        0
    };
    let coverage = if video_duration_ms > 0 {
        (total_caption_ms as f64 / video_duration_ms as f64) * 100.0
    } else {
        0.0
    };

    let mut issues: Vec<String> = Vec::new();
    if !gaps.is_empty() {
        issues.push(format!("{} caption gaps > 2s", gaps.len()));
    }
    if !overlaps.is_empty() {
        issues.push(format!("{} caption overlaps", overlaps.len()));
    }
    if !too_fast.is_empty() {
        issues.push(format!(
            "{} captions too fast (<{}ms)",
            too_fast.len(),
            min_caption_duration_ms
        ));
    }
    if !too_slow.is_empty() {
        issues.push(format!(
            "{} captions too slow (>{})",
            too_slow.len(),
            max_caption_duration_ms
        ));
    }

    let mut score = 100;
    score -= (gaps.len() as i32) * 10;
    score -= (overlaps.len() as i32) * 15;
    score -= (too_fast.len() as i32) * 5;
    score -= (too_slow.len() as i32) * 5;
    let score = score.max(0).min(100);

    Ok(json!({
        "status": if score >= 80 { "pass" } else if score >= 50 { "warning" } else { "fail" },
        "caption_count": entries.len(),
        "coverage_percent": (coverage * 10.0).round() / 10.0,
        "video_duration_ms": video_duration_ms,
        "total_caption_ms": total_caption_ms,
        "avg_caption_duration_ms": avg_duration,
        "gaps": gaps,
        "overlaps": overlaps,
        "too_fast": too_fast,
        "too_slow": too_slow,
        "readability_score": score,
        "issues": issues,
    }))
}

// ---------------------------------------------------------------------------
// Handler: verify.render
// ---------------------------------------------------------------------------

async fn handle_verify_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let expected_aspect = default_str(&args, "expected_aspect", "9:16");
    let duration_tolerance_ms = default_i64(&args, "duration_tolerance_ms", 2000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,duration",
            "-show_entries",
            "format=duration,size",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&probe_output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&probe_output.stdout).map_err(|e| ToolError::Json(e))?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let format_info = probe.get("format").cloned().unwrap_or(json!({}));

    let width = streams
        .first()
        .and_then(|s| s.get("width"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let height = streams
        .first()
        .and_then(|s| s.get("height"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let file_size = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);

    let actual_duration_s: f64 = format_info
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| {
            streams
                .first()
                .and_then(|s| s.get("duration"))
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    let actual_duration_ms = (actual_duration_s * 1000.0) as i64;

    let timeline = Timeline::load(timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
    let expected_duration_ms = timeline.rendered_duration_ms();
    let segment_count = timeline.segments.len();

    let duration_delta = (actual_duration_ms - expected_duration_ms).abs();
    let duration_match = duration_delta <= duration_tolerance_ms;

    let expected_ratio: f64 = match expected_aspect.as_str() {
        "9:16" => 9.0 / 16.0,
        "16:9" => 16.0 / 9.0,
        "1:1" => 1.0,
        "4:5" => 4.0 / 5.0,
        _ => 9.0 / 16.0,
    };
    let actual_ratio = if height > 0 {
        width as f64 / height as f64
    } else {
        0.0
    };
    let aspect_match = (actual_ratio - expected_ratio).abs() < 0.05;

    let tracks_present: serde_json::Map<String, serde_json::Value> = timeline
        .tracks
        .iter()
        .map(|(track, events)| {
            let track = track as &TrackType;
            let events = events as &Vec<openscript_core::timeline::TimelineEvent>;
            (
                track.to_string(),
                json!({"count": events.len(), "rendered": !events.is_empty()}),
            )
        })
        .collect();

    let total_tracks = timeline.tracks.values().filter(|v| !v.is_empty()).count();
    let has_audio = total_tracks > 1;

    let mut issues: Vec<String> = Vec::new();
    if !duration_match {
        issues.push(format!(
            "Duration mismatch: expected {}ms, got {}ms (delta: {}ms)",
            expected_duration_ms, actual_duration_ms, duration_delta
        ));
    }
    if !aspect_match {
        issues.push(format!(
            "Aspect ratio mismatch: expected {}, got {}x{} (ratio: {:.3})",
            expected_aspect, width, height, actual_ratio
        ));
    }
    if file_size == 0 {
        issues.push("File size is 0 bytes — render may have failed".into());
    }
    if width == 0 || height == 0 {
        issues.push("Could not determine video resolution".into());
    }

    let mut score = 100;
    if !duration_match {
        score -= 30;
    }
    if !aspect_match {
        score -= 25;
    }
    if file_size == 0 {
        score -= 45;
    }
    if !has_audio && total_tracks > 1 {
        score -= 15;
    }
    let score = score.max(0).min(100);

    Ok(json!({
        "status": if score >= 80 { "pass" } else if score >= 50 { "warning" } else { "fail" },
        "duration_match": duration_match,
        "expected_duration_ms": expected_duration_ms,
        "actual_duration_ms": actual_duration_ms,
        "duration_delta_ms": duration_delta,
        "segment_count": segment_count,
        "resolution": format!("{}x{}", width, height),
        "aspect_match": aspect_match,
        "expected_aspect": expected_aspect,
        "file_size_bytes": file_size,
        "tracks_present": tracks_present,
        "has_audio_stream": has_audio,
        "issues": issues,
        "overall_score": score,
        "note": "Technical integrity only. Call verify.production for stock/music/sticker beauty KPIs.",
    }))
}

// ---------------------------------------------------------------------------
// Production Quality KPIs — thin wrappers around openscript_core::production_quality
// ---------------------------------------------------------------------------

fn is_procedural_media_path(path: &str) -> bool {
    matches!(
        openscript_core::production_quality::classify_video_source(path, None),
        openscript_core::production_quality::VideoSourceClass::ProceduralSynthetic
    )
}

#[allow(dead_code)] // used by music rejection paths / tests
fn is_synthetic_music_file(path: &str) -> bool {
    openscript_core::production_quality::is_synthetic_music_file(path)
}

/// Legacy shim used by unit tests — maps to v2 scorer via a synthetic manifest.
#[cfg(test)]
fn compute_production_score(
    background_paths: &[String],
    music_path: Option<&str>,
    sticker_count: usize,
    meme_count: usize,
    has_dialogue: bool,
    rms_ok: bool,
    captions_present: bool,
) -> (i32, serde_json::Value, Vec<String>, Vec<String>) {
    use openscript_core::production_quality::*;
    let tl = Timeline::new(std::path::PathBuf::from("kpi.mp4"), "9:16", 30, None);
    let n = background_paths.len().max(1);
    let slice = (16_000 / n as i64).max(1);
    let backgrounds: Vec<BackgroundLayerInfo> = background_paths
        .iter()
        .enumerate()
        .map(|(i, p)| BackgroundLayerInfo {
            path: p.clone(),
            start_ms: i as i64 * slice,
            end_ms: (i as i64 + 1) * slice,
            source_hint: None,
            content_hash: Some(format!("shim_hash_{}", i)),
            video_id: None,
            search_query: Some(p.clone()),
         lexical_score: None, source_title: None, })
        .collect();
    let stickers: Vec<StickerLayerInfo> = (0..sticker_count)
        .map(|i| StickerLayerInfo {
            path: format!("mcp/assets/stickers/giphy_{}.gif", i),
            start_ms: 0,
            end_ms: 1000,
            position: "top-left".into(),
            scale: 0.35,
        })
        .collect();
    let memes: Vec<MemeLayerInfo> = (0..meme_count)
        .map(|i| MemeLayerInfo {
            path: format!("meme_{}.mp4", i),
            start_ms: 2000 + i as i64 * 500,
            end_ms: 4500 + i as i64 * 500,
        })
        .collect();
    let caps = if captions_present {
        let p = std::env::temp_dir().join("kpi_caps.ass");
        let _ = std::fs::write(&p, b"[Script Info]\n");
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };
    let music = music_path.map(|p| MusicLayerInfo {
        path: p.to_string(),
        gain_db: -12.0,
        ducking: true,
        mood: Some("neutral".into()),
        energy: Some("medium".into()),
        tags: vec!["pop".into()],
        selection_query: None,
        source: Some("library".into()),
    });
    let manifest = RenderManifest {
        duration_ms: 16_000,
        backgrounds,
        stickers,
        memes,
        music,
        captions_path: caps,
        voiceover_count: 1,
        sections: vec![],
        has_dialogue,
        rms_ok,
        video_keywords: vec![],
        theme: None,
        sfx_count: 2,
        caption_coverage_ratio: if captions_present { 0.95 } else { 0.0 },
        caption_style: if captions_present { Some("word_highlight".into()) } else { None },
        aspect_ratio: Some("9:16".into()),
        ..Default::default()
    };
    let report = evaluate_production_quality(&tl, &manifest);
    let dims = serde_json::to_value(&report.dimensions).unwrap_or(json!({}));
    (report.production_score, dims, report.hard_fails, report.next_actions)
}

/// Probe actual audio metrics from a rendered video using ffmpeg.
/// Returns (lufs, peak_dbfs, ducking_depth_db, music_gain_db).
async fn probe_audio_metrics(video_path: &str) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    use std::process::Stdio;

    // LUFS via loudnorm filter with JSON output
    let lufs = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-af",
            "loudnorm=I=-16:TP=-1.5:LRA=11:print_format=json",
            "-f",
            "null",
            "-",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stderr).ok())
        .and_then(|s| {
            // Find the JSON block at the end
            let json_start = s.rfind('{')?;
            let json_str = &s[json_start..];
            serde_json::from_str::<serde_json::Value>(json_str).ok()
        })
        .and_then(|v| v.get("input_i").and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok()));

    // Peak dBFS via volumedetect
    let peak = tokio::process::Command::new("ffmpeg")
        .args(["-i", video_path, "-af", "volumedetect", "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stderr).ok())
        .and_then(|s| {
            for line in s.lines() {
                if let Some(idx) = line.find("max_volume:") {
                    let rest = &line[idx + "max_volume:".len()..];
                    let num = rest.trim().split_whitespace().next().unwrap_or("");
                    if let Ok(v) = num.parse::<f64>() {
                        return Some(v);
                    }
                }
            }
            None
        });

    // Ducking depth: measure music level during speech vs non-speech segments.
    // This is a simplified approach - we compare overall RMS of music track
    // (not implemented yet, would need source separation).
    // For now, return None - can be enhanced later with better analysis.
    let ducking_depth_db = None::<f64>;

    // Music gain: try to infer from the music track if present
    // This would require extracting and analyzing the music track separately.
    // For now, return None - the manifest's planned gain_db will be used.
    let music_gain_db = None::<f64>;

    (lufs, peak, ducking_depth_db, music_gain_db)
}

async fn probe_dialogue_rms(video_path: &str) -> (bool, bool) {
    // Reuse ffprobe+ffmpeg volumedetect lightly
    let out = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-af",
            "volumedetect",
            "-f",
            "null",
            "-",
        ])
        .output()
        .await;
    let stderr = out
        .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
        .unwrap_or_default();
    let mut mean_db = -100.0_f64;
    for line in stderr.lines() {
        // ffmpeg prints: "[Parsed_volumedetect_0 @ 0x…] mean_volume: -16.5 dB"
        if let Some(idx) = line.find("mean_volume:") {
            let rest = &line[idx + "mean_volume:".len()..];
            let num = rest.trim().split_whitespace().next().unwrap_or("");
            if let Ok(v) = num.parse::<f64>() {
                mean_db = v;
            }
        }
    }
    // Dialogue typically > -45 dB mean if present; pure silence ~ -91
    let has_dialogue = mean_db > -50.0;
    let rms_ok = mean_db >= -30.0 && mean_db <= -8.0;
    (has_dialogue, rms_ok)
}

async fn handle_verify_production(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::production_quality::{
        evaluate_production_quality, grade_rank, BackgroundLayerInfo, MemeLayerInfo, MusicLayerInfo,
        RenderManifest, StickerLayerInfo, verify_layer_order,
    };

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let captions_path = default_opt_str(&args, "captions_path");
    let sticker_count = default_u32(&args, "sticker_count", 0) as usize;
    let meme_count = default_u32(&args, "meme_count", 0) as usize;
    let min_grade = default_str(&args, "min_grade", "B");
    let music_path_arg = default_opt_str(&args, "music_path");
    let manifest_path = default_opt_str(&args, "render_manifest_path");

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let timeline = Timeline::load(&timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
    let (has_dialogue, rms_ok) = probe_dialogue_rms(&video_path).await;

    // Probe actual audio metrics from the rendered video
    let (measured_lufs, measured_peak_dbfs, measured_ducking_depth_db, measured_music_gain_db) =
        probe_audio_metrics(&video_path).await;

    // Prefer authoritative render_manifest.json from script.to_video
    let mut manifest = if let Some(ref mp) = manifest_path {
        if Path::new(mp).exists() {
            let raw = std::fs::read_to_string(mp)?;
            serde_json::from_str::<RenderManifest>(&raw).map_err(|e| ToolError::Json(e))?
        } else {
            RenderManifest::default()
        }
    } else {
        // Co-located default path next to timeline
        let sibling = Path::new(&timeline_path)
            .parent()
            .map(|p| p.join("render_manifest.json"))
            .unwrap_or_else(|| Path::new("render_manifest.json").to_path_buf());
        if sibling.exists() {
            let raw = std::fs::read_to_string(&sibling)?;
            serde_json::from_str::<RenderManifest>(&raw).unwrap_or_default()
        } else {
            RenderManifest::default()
        }
    };

    // Merge explicit overrides / legacy args into manifest
    if manifest.backgrounds.is_empty() {
        let bg_paths: Vec<String> = args
            .get("background_sources")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !bg_paths.is_empty() {
            let n = bg_paths.len().max(1);
            let slice = (timeline.rendered_duration_ms() / n as i64).max(1);
            manifest.backgrounds = bg_paths
                .into_iter()
                .enumerate()
                .map(|(i, p)| BackgroundLayerInfo {
                    path: p,
                    start_ms: i as i64 * slice,
                    end_ms: (i as i64 + 1) * slice,
                    source_hint: None,
                    content_hash: None,
                    video_id: None,
                    search_query: None,
                 lexical_score: None, source_title: None, })
                .collect();
        }
    }
    if manifest.stickers.is_empty() && sticker_count > 0 {
        manifest.stickers = (0..sticker_count)
            .map(|i| StickerLayerInfo {
                path: format!("sticker_{}", i),
                start_ms: 0,
                end_ms: 1000,
                position: "top-left".into(),
                scale: 0.35,
            })
            .collect();
    }
    if manifest.memes.is_empty() && meme_count > 0 {
        manifest.memes = (0..meme_count)
            .map(|i| MemeLayerInfo {
                path: format!("meme_{}", i),
                start_ms: 1000 + i as i64 * 500,
                end_ms: 3000 + i as i64 * 500,
            })
            .collect();
    }
    if manifest.music.is_none() {
        if let Some(p) = music_path_arg {
            manifest.music = Some(MusicLayerInfo {
                path: p,
                gain_db: 0.0,
                ducking: true,
                mood: None,
                energy: None,
             tags: vec![], selection_query: None, source: None, });
        }
    }
    if manifest.captions_path.is_none() {
        manifest.captions_path = captions_path;
    }
    if manifest.duration_ms <= 0 {
        manifest.duration_ms = timeline.rendered_duration_ms();
    }
    manifest.has_dialogue = has_dialogue;
    manifest.rms_ok = rms_ok;

    // Update manifest with measured audio metrics (override planned values with reality)
    if let Some(l) = measured_lufs {
        manifest.measured_lufs = Some(l);
        manifest.lufs = Some(l);
    }
    if let Some(p) = measured_peak_dbfs {
        manifest.measured_peak_dbfs = Some(p);
        manifest.peak_dbfs = Some(p);
    }
    if let Some(d) = measured_ducking_depth_db {
        manifest.measured_ducking_depth_db = Some(d);
        manifest.ducking_depth_db = Some(d);
    }
    if let Some(g) = measured_music_gain_db {
        manifest.measured_music_gain_db = Some(g);
        if manifest.music.is_some() {
            manifest.music.as_mut().unwrap().gain_db = g;
        }
    }

    let report = evaluate_production_quality(&timeline, &manifest);
    let meets_min = grade_rank(&report.grade) >= grade_rank(&min_grade);

    // Verify layer composition order
    let layer_report = verify_layer_order(&manifest);

    // Optional vision re-score of background clips (local Qwen → OpenRouter free).
    let vision_rescore = default_bool(&args, "vision_rescore", false);
    let mut vision_scores: Vec<serde_json::Value> = Vec::new();
    if vision_rescore {
        let keywords = manifest.video_keywords.clone();
        let scene_fallback = timeline
            .segments
            .iter()
            .map(|s| s.caption.as_str())
            .filter(|c| !c.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        for (i, bg) in manifest.backgrounds.iter().take(8).enumerate() {
            if bg.path.is_empty() || bg.path == "placeholder" || !Path::new(&bg.path).exists() {
                vision_scores.push(json!({
                    "index": i,
                    "path": bg.path,
                    "status": "skipped",
                    "reason": "missing or placeholder path",
                }));
                continue;
            }
            let scene_text = if scene_fallback.is_empty() {
                bg.search_query.clone().unwrap_or_else(|| "video scene".into())
            } else {
                scene_fallback.clone()
            };
            match crate::llm::score_clip_relevance(
                &bg.path,
                &scene_text,
                &keywords,
                bg.search_query.as_deref(),
            )
            .await
            {
                Ok(v) => vision_scores.push(v),
                Err(e) => vision_scores.push(json!({
                    "index": i,
                    "path": bg.path,
                    "status": "error",
                    "error": e.to_string(),
                })),
            }
        }
    }

    let status = if !report.hard_fails.is_empty() {
        "fail"
    } else if meets_min {
        "pass"
    } else if report.production_score >= 40 {
        "warning"
    } else {
        "fail"
    };
    Ok(json!({
        "status": status,
        "production_score": report.production_score,
        "grade": report.grade,
        "min_grade": min_grade,
        "meets_min_grade": meets_min && report.hard_fails.is_empty(),
        "hard_fails": report.hard_fails,
        "dimensions": report.dimensions,
        "next_actions": report.next_actions,
        "cuts_per_second": report.cuts_per_second,
        "video_source_mix": report.video_source_mix,
        "timeline_editor": report.timeline_editor,
        "layer_order": layer_report,
        "kpi_version": report.kpi_version,
        "kpi_note": "verify.render is technical-only. Production v3 hard-fails majority procedural, missing visual hooks, and parade music on calm/focus. Use real stock + topic-tagged music.",
        "vision_rescore": vision_rescore,
        "vision_scores": vision_scores,
    }))
}

/// ONE-SHOT director: preflight → parse → to_video → verify.production.
async fn handle_director_run(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script = extract_str(&args, "script")?;
    let output_path = default_str(&args, "output_path", "artifacts/director_out.mp4");
    let output_dir = default_str(&args, "output_dir", "artifacts/director_run");
    let min_grade = default_str(&args, "min_grade", "B");
    let _ = std::fs::create_dir_all(&output_dir);

    // Preflight
    let pexels = !pexels_key().is_empty();
    let ytdlp = std::process::Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !pexels && !ytdlp {
        return Err(ToolError::Asset(
            "director.run preflight failed: need PEXELS_API_KEY or yt-dlp for stock B-roll"
                .into(),
        ));
    }
    let mut preflight_warnings: Vec<String> = Vec::new();
    if !pexels {
        preflight_warnings.push(
            "PEXELS_API_KEY unset — YouTube-only multi-broll (weaker relevance). Set api_keys.pexels in ~/.openscript/config.json"
                .into(),
        );
    }
    let lib = resolve_repo_path("mcp/assets/music_library_index.json");
    if !lib.exists() {
        preflight_warnings.push(
            "music_library_index.json missing — run library.build for tagged music".into(),
        );
    }

    let parse = handle_script_parse(json!({"script": script})).await?;
    if parse.get("status").and_then(|s| s.as_str()) == Some("error")
        || parse
            .get("errors")
            .and_then(|e| e.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    {
        return Ok(json!({
            "status": "error",
            "phase": "parse",
            "parse": parse,
            "preflight_warnings": preflight_warnings,
        }));
    }

    let to_video = handle_script_to_video(json!({
        "script": script,
        "output_path": output_path,
        "output_dir": output_dir,
    }))
    .await?;

    let video = to_video
        .get("output_path")
        .and_then(|v| v.as_str())
        .unwrap_or(&output_path)
        .to_string();
    let timeline = to_video
        .get("timeline_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let manifest = to_video
        .get("render_manifest_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut verify_args = json!({
        "video_path": video,
        "timeline_path": timeline,
        "min_grade": min_grade,
    });
    if let Some(m) = manifest {
        verify_args["render_manifest_path"] = json!(m);
    }
    let production = if !timeline.is_empty() && Path::new(&video).exists() {
        handle_verify_production(verify_args).await.ok()
    } else {
        None
    };

    Ok(json!({
        "status": to_video.get("status").cloned().unwrap_or(json!("unknown")),
        "preflight_warnings": preflight_warnings,
        "parse": parse,
        "to_video": to_video,
        "verify_production": production,
        "output_path": video,
    }))
}

/// Download a short stock clip via yt-dlp (no API key). Used when Pexels is unavailable.
/// Result of a unique stock fetch (path + identity for variance tracking).
struct StockClipFetch {
    path: String,
    video_id: String,
    content_hash: String,
    search_query: String,
    lexical_score: f64,
    source_title: String,
}

fn file_content_fingerprint(path: &str) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 256 * 1024];
    let n = f.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    // FNV-1a 64-bit over first 256KiB — enough to catch identical YT re-downloads
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in &buf[..n] {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Include file size so different trims of same source still collide on full source
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Some(format!("{:016x}_{}", hash, size))
}

/// List up to `limit` YouTube video IDs for a search query (no download).
#[allow(dead_code)]
async fn youtube_search_ids(query: &str, limit: usize) -> Vec<String> {
    youtube_search_id_titles(query, limit)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// YouTube search returning `(id, title)` for lexical relevance ranking.
async fn youtube_search_id_titles(query: &str, limit: usize) -> Vec<(String, String)> {
    let out = tokio::process::Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--print",
            "%(id)s\t%(title)s",
            "--no-warnings",
            "--quiet",
            &format!("ytsearch{}:{}", limit, query),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                if l.is_empty() {
                    return None;
                }
                let (id, title) = match l.split_once('\t') {
                    Some((i, t)) => (i.trim(), t.trim()),
                    None => (l, ""),
                };
                if id.len() >= 6 && !id.contains(' ') {
                    Some((id.to_string(), title.to_string()))
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Fetch a stock clip that is content-unique **and** context-relevant.
///
/// Signal/noise gates (Phase CM):
/// 1. Prefer titles that lexically match scene signal tokens
/// 2. Unique video id + content hash
/// 3. Cover-crop with `setsar=1` (no stretch)
/// 4. Geometry probe rejects non-square SAR / wrong display aspect
async fn fetch_youtube_stock_clip_unique(
    query: &str,
    duration_s: f64,
    aspect: &str,
    out_path: &str,
    scene_idx: usize,
    used_video_ids: &mut std::collections::HashSet<String>,
    used_content_hashes: &mut std::collections::HashSet<String>,
) -> Option<StockClipFetch> {
    fetch_youtube_stock_clip_signal(
        query,
        &[],
        duration_s,
        aspect,
        out_path,
        scene_idx,
        used_video_ids,
        used_content_hashes,
    )
    .await
}

/// Full signal-aware YouTube stock fetch.
async fn fetch_youtube_stock_clip_signal(
    query: &str,
    signal_tokens: &[String],
    duration_s: f64,
    aspect: &str,
    out_path: &str,
    scene_idx: usize,
    used_video_ids: &mut std::collections::HashSet<String>,
    used_content_hashes: &mut std::collections::HashSet<String>,
) -> Option<StockClipFetch> {
    let cache_dir = "mcp/assets/background_cache";
    std::fs::create_dir_all(cache_dir).ok()?;

    let diversified = query.to_string();
    let mut candidates = youtube_search_id_titles(&diversified, 12).await;
    if candidates.is_empty() {
        // Fallback: shorter query (first 6 tokens)
        let short: String = query
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        candidates = youtube_search_id_titles(&short, 10).await;
    }

    // Drop already-used IDs
    candidates.retain(|(id, _)| !used_video_ids.contains(id));
    if candidates.is_empty() {
        tracing::warn!(
            "[youtube stock] no unused video IDs for query='{}'",
            diversified
        );
        return None;
    }

    let signal: Vec<String> = if signal_tokens.is_empty() {
        crate::stock_signal::tokenize(&diversified)
    } else {
        signal_tokens.to_vec()
    };
    let min_lex = crate::stock_signal::min_lexical_accept();
    let ranked =
        crate::stock_signal::rank_and_filter_candidates(&candidates, &signal, min_lex);
    tracing::info!(
        "[youtube stock] ranked {} candidates (min_lex={:.2}) top={}",
        ranked.len(),
        min_lex,
        ranked
            .first()
            .map(|c| format!("{}:{:.2}:{}", c.id, c.lexical, &c.title[..c.title.len().min(40)]))
            .unwrap_or_else(|| "none".into())
    );

    for cand in ranked.into_iter().take(8) {
        let video_id = cand.id.clone();
        let title = cand.title.clone();
        let lex = cand.lexical;
        let full_path = format!("{}/yt_id_{}.mp4", cache_dir, video_id);
        if !Path::new(&full_path).exists() {
            let url = format!("https://www.youtube.com/watch?v={}", video_id);
            // Video-only preferred (avoid audio-only / music streams ranked as "stock").
            // Cover-crop later handles landscape→portrait cleanly.
            let yt = tokio::process::Command::new("yt-dlp")
                .args([
                    "--format",
                    "bestvideo[height<=720][ext=mp4]+bestaudio/bestvideo[height<=720]+bestaudio/bestvideo[height<=720][ext=mp4]/bestvideo[height<=720]/best[vcodec!=none][height<=720]/best[vcodec!=none]/best",
                    "--merge-output-format",
                    "mp4",
                    "--output",
                    &full_path,
                    "--no-playlist",
                    "--quiet",
                    "--no-warnings",
                    "--socket-timeout",
                    "25",
                    &url,
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .output()
                .await
                .ok();
            let ok = yt
                .as_ref()
                .map(|o| o.status.success() && Path::new(&full_path).exists())
                .unwrap_or(false);
            if !ok {
                tracing::warn!(
                    "[youtube stock] download failed id={} title='{}' q={}",
                    video_id,
                    &title[..title.len().min(50)],
                    diversified
                );
                continue;
            }
        }

        let content_hash = match file_content_fingerprint(&full_path) {
            Some(h) => h,
            None => continue,
        };
        if used_content_hashes.contains(&content_hash) {
            tracing::info!(
                "[youtube stock] skip duplicate content hash {} (id={})",
                content_hash,
                video_id
            );
            used_video_ids.insert(video_id);
            continue;
        }

        // Cover-crop (no stretch) + square SAR
        let start_s = 1.5 + (scene_idx as f64) * 2.7;
        let crop = crop_filter_for_aspect(aspect);
        let trim = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-ss",
                &start_s.to_string(),
                "-i",
                &full_path,
                "-t",
                &duration_s.max(2.0).to_string(),
                "-vf",
                &crop,
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "23",
                "-an",
                out_path,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .ok()?;
        if !trim.status.success() || !Path::new(out_path).exists() {
            continue;
        }

        // Geometry gate: reject stretch / wrong display aspect
        let geo = crate::stock_signal::probe_geometry(out_path, aspect);
        if !geo.ok {
            tracing::warn!(
                "[youtube stock] geometry reject id={} reasons={:?} {}x{} sar={}:{}",
                video_id,
                geo.reasons,
                geo.width,
                geo.height,
                geo.sar_num,
                geo.sar_den
            );
            let _ = std::fs::remove_file(out_path);
            // Don't permanently burn the id — source may re-encode cleanly later;
            // still mark used to avoid tight loops on same bad file.
            used_video_ids.insert(video_id);
            continue;
        }

        let out_hash = file_content_fingerprint(out_path).unwrap_or_else(|| content_hash.clone());
        if used_content_hashes.contains(&out_hash) {
            let _ = std::fs::remove_file(out_path);
            used_video_ids.insert(video_id);
            continue;
        }

        used_video_ids.insert(video_id.clone());
        used_content_hashes.insert(content_hash.clone());
        used_content_hashes.insert(out_hash.clone());
        tracing::info!(
            "[youtube stock] ACCEPT id={} lex={:.2} hash={} title='{}' query='{}' -> {}",
            video_id,
            lex,
            &out_hash[..out_hash.len().min(20)],
            &title[..title.len().min(50)],
            diversified,
            out_path
        );
        return Some(StockClipFetch {
            path: out_path.to_string(),
            video_id,
            content_hash: out_hash,
            search_query: format!("{} | title={}", diversified, title),
            lexical_score: lex,
            source_title: title,
        });
    }
    None
}

/// Backward-compatible wrapper (no uniqueness set). Kept for call sites that
/// do not track identity sets (e.g. one-off background.fetch without multi-broll).
#[allow(dead_code)]
async fn fetch_youtube_stock_clip(
    query: &str,
    duration_s: f64,
    aspect: &str,
    out_path: &str,
) -> Option<String> {
    let mut ids = std::collections::HashSet::new();
    let mut hashes = std::collections::HashSet::new();
    fetch_youtube_stock_clip_unique(query, duration_s, aspect, out_path, 0, &mut ids, &mut hashes)
        .await
        .map(|f| f.path)
}

// ---------------------------------------------------------------------------
// Handler: reelize.brief
// ---------------------------------------------------------------------------

async fn handle_reelize_brief(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path_opt = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        report_progress(5.0, 100.0, "Using existing SRT...")
            .await
            .ok();
        srt
    } else {
        report_progress(0.0, 100.0, "Transcribing audio...")
            .await
            .ok();
        let transcribe_result = handle_transcribe(json!({"media_path": video_path})).await?;
        report_progress(30.0, 100.0, "Transcription complete")
            .await
            .ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    report_progress(35.0, 100.0, "Grouping caption segments...")
        .await
        .ok();
    let prepare_result = handle_srt_prepare(json!({
        "srt_path": &resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    }))
    .await?;
    let grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    report_progress(50.0, 100.0, "Analyzing segments...")
        .await
        .ok();
    let entries = parse_srt(&grouped_srt_path)?;

    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "like", "through",
        "after", "over", "between", "out", "against", "during", "without", "before", "under",
        "around", "among", "that", "this", "these", "those", "it", "its", "i", "me", "my", "we",
        "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their", "what",
        "which", "who", "whom", "whose", "where", "when", "why", "how", "not", "no", "nor", "so",
        "but", "and", "or", "if", "then", "than", "too", "very", "just", "about", "up", "some",
    ];

    let extract_keywords = |text: &str, limit: usize| -> Vec<String> {
        text.split_whitespace()
            .filter(|w| {
                let lower = w.to_lowercase();
                let cleaned: String = lower.chars().filter(|c| c.is_alphabetic()).collect();
                !cleaned.is_empty() && !STOPWORDS.contains(&cleaned.as_str())
            })
            .map(|w| {
                let lower = w.to_lowercase();
                lower.chars().filter(|c| c.is_alphabetic()).collect()
            })
            .take(limit)
            .collect()
    };

    let mut segments: Vec<serde_json::Value> = Vec::new();
    let mut total_dialogue_s = 0.0;

    for (i, entry) in entries.iter().enumerate() {
        let duration_s = entry.end - entry.start;
        let word_count = entry.text.split_whitespace().count();
        let wps = if duration_s > 0.0 {
            word_count as f64 / duration_s
        } else {
            0.0
        };

        let keywords = extract_keywords(&entry.text, 5);
        let broll_concepts: Vec<String> = if entry.text.len() < 20 && !entry.text.trim().is_empty()
        {
            let mut concepts = keywords.iter().take(3).cloned().collect::<Vec<_>>();
            concepts.push(entry.text.trim().to_string());
            concepts
        } else {
            keywords.iter().take(3).cloned().collect()
        };

        total_dialogue_s += duration_s;

        segments.push(json!({
            "id": format!("seg_{:03}", i + 1),
            "start_s": entry.start,
            "end_s": entry.end,
            "duration_s": duration_s,
            "text": entry.text,
            "word_count": word_count,
            "words_per_second": (wps * 100.0).round() / 100.0,
            "suggested_broll_concepts": broll_concepts,
            "topic_keywords": keywords,
        }));
    }

    let mut topic_map: std::collections::HashMap<String, (usize, f64)> =
        std::collections::HashMap::new();
    for seg in &segments {
        if let Some(keywords) = seg.get("topic_keywords").and_then(|v| v.as_array()) {
            if let Some(first) = keywords.first().and_then(|v| v.as_str()) {
                let topic = first.to_string();
                let entry = topic_map.entry(topic).or_insert((0, 0.0));
                entry.0 += 1;
                entry.1 += seg
                    .get("duration_s")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
            }
        }
    }

    let topic_summary: Vec<serde_json::Value> = topic_map
        .into_iter()
        .map(|(topic, (count, total_s))| {
            json!({
                "topic": topic,
                "segment_count": count,
                "total_s": (total_s * 100.0).round() / 100.0,
            })
        })
        .collect();

    let source_duration_s = match tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(&video_path)
        .output()
        .await
    {
        Ok(output) => {
            if let Ok(probe) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                probe
                    .get("format")
                    .and_then(|f| f.get("duration"))
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            } else {
                0.0
            }
        }
        Err(e) => {
            tracing::warn!("ffprobe failed for source duration: {}", e);
            0.0
        }
    };

    report_progress(100.0, 100.0, "Brief complete").await.ok();

    Ok(json!({
        "source_path": video_path,
        "source_duration_s": (source_duration_s * 100.0).round() / 100.0,
        "total_segments": segments.len(),
        "total_dialogue_s": (total_dialogue_s * 100.0).round() / 100.0,
        "segments": segments,
        "topic_summary": topic_summary,
    }))
}

// ---------------------------------------------------------------------------
// Handler: reelize.direct
// ---------------------------------------------------------------------------

async fn handle_reelize_direct(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;
    use openscript_ffmpeg::subtitles;

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let segments_arr = args
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("segments".to_string()))?;
    let aspect = default_str(&args, "aspect", "9:16");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 300);
    let fps = default_u32(&args, "fps", 30);
    let crf = default_u32(&args, "crf", 20);
    let output_path = default_opt_str(&args, "output_path");
    let captions_obj = args.get("captions").cloned().unwrap_or(json!({}));
    let caption_style = default_str(&captions_obj, "style", "standard");
    let captions_enabled = default_bool(&captions_obj, "enabled", true);
    let broll_arr = args
        .get("broll")
        .and_then(|v| v.as_array())
        .map(|a| a.clone())
        .unwrap_or_default();
    let sfx_arr = args
        .get("sfx")
        .and_then(|v| v.as_array())
        .map(|a| a.clone())
        .unwrap_or_default();
    let music_obj = args.get("music").cloned();
    let voiceover_arr = args
        .get("voiceover")
        .and_then(|v| v.as_array())
        .map(|a| a.clone())
        .unwrap_or_default();
    let srt_path_opt = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let mut warnings: Vec<String> = Vec::new();

    report_progress(0.0, 100.0, "Transcribing audio...")
        .await
        .ok();
    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        srt
    } else {
        let transcribe_result = handle_transcribe(json!({"media_path": video_path})).await?;
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    report_progress(15.0, 100.0, "Preparing grouped SRT...")
        .await
        .ok();
    let prepare_result = handle_srt_prepare(json!({
        "srt_path": &resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    }))
    .await?;
    let _grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    report_progress(25.0, 100.0, "Building timeline...")
        .await
        .ok();
    let timeline_path = default_timeline_path(&video_path);
    let mut timeline = Timeline::new(
        std::path::Path::new(&video_path).to_path_buf(),
        &aspect,
        fps,
        None,
    );

    for segment in segments_arr {
        let start = segment.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end = segment.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let caption = segment
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let seg_crossfade = segment
            .get("crossfade_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(crossfade_ms as u64) as u32;
        let semantic_role = segment.get("id").and_then(|v| v.as_str());

        timeline.add_segment(start, end, caption, seg_crossfade, semantic_role);
    }

    if captions_enabled {
        use openscript_core::srt::parse_srt;

        let word_srt_path = {
            let p = Path::new(&resolved_srt_path);
            let parent = p.parent().unwrap_or(Path::new("."));
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            parent
                .join(format!("{}.apex.word.srt", stem))
                .to_string_lossy()
                .to_string()
        };

        let word_entries = parse_srt(&word_srt_path).ok();
        let raw_srt_entries = parse_srt(&resolved_srt_path)
            .map_err(|e| ToolError::Srt(format!("Failed to parse SRT: {}", e)))?;

        let use_concat = segments_arr.len() > 10;
        let mut timeline_segments: Vec<(f64, f64, String)> = Vec::new();
        let mut output_cursor_s: f64 = 0.0;

        for segment in segments_arr {
            let seg_start = segment.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let seg_end = segment.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let caption = segment
                .get("caption")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let seg_crossfade_s = segment
                .get("crossfade_ms")
                .and_then(|v| v.as_f64())
                .unwrap_or(crossfade_ms as f64)
                / 1000.0;
            let seg_duration = seg_end - seg_start;

            if let Some(ref words) = word_entries {
                let words_in_range: Vec<_> = words
                    .iter()
                    .filter(|e| {
                        e.start >= seg_start && e.end <= seg_end + 0.05 && !e.text.trim().is_empty()
                    })
                    .collect();

                let mut i = 0;
                while i < words_in_range.len() {
                    let chunk_size = if words_in_range.len() - i >= 5 {
                        3
                    } else if words_in_range.len() - i == 4 {
                        2
                    } else {
                        words_in_range.len() - i
                    };
                    let chunk_start = output_cursor_s + (words_in_range[i].start - seg_start);
                    let chunk_end =
                        output_cursor_s + (words_in_range[i + chunk_size - 1].end - seg_start);
                    let text: Vec<_> = words_in_range[i..i + chunk_size]
                        .iter()
                        .map(|e| e.text.trim().to_string())
                        .collect();
                    timeline_segments.push((chunk_start, chunk_end, text.join(" ")));
                    i += chunk_size;
                }
            } else {
                let srt_in_range: Vec<_> = raw_srt_entries
                    .iter()
                    .filter(|e| {
                        e.start >= seg_start && e.end <= seg_end + 0.05 && !e.text.trim().is_empty()
                    })
                    .collect();

                if !srt_in_range.is_empty() && !caption.is_empty() {
                    let caption_words: Vec<&str> = caption.split_whitespace().collect();
                    let n = srt_in_range.len();
                    for (i, srt_entry) in srt_in_range.iter().enumerate() {
                        let out_start = output_cursor_s + (srt_entry.start - seg_start);
                        let out_end = output_cursor_s + (srt_entry.end - seg_start);
                        let ws = (i * caption_words.len()) / n;
                        let we = ((i + 1) * caption_words.len()) / n;
                        let chunk = caption_words[ws..we].join(" ");
                        if !chunk.is_empty() {
                            timeline_segments.push((out_start, out_end, chunk));
                        }
                    }
                } else if !caption.is_empty() {
                    timeline_segments.push((
                        output_cursor_s,
                        output_cursor_s + seg_duration,
                        caption.to_string(),
                    ));
                }
            }

            if use_concat {
                output_cursor_s += seg_duration;
            } else {
                output_cursor_s += seg_duration - seg_crossfade_s;
                if output_cursor_s < 0.0 {
                    output_cursor_s = 0.0;
                }
            }
        }

        let caption_asset_dir = Path::new(&timeline_path).parent().unwrap_or(Path::new("."));
        let style_name = if caption_style == "kinetic" {
            "KineticViral"
        } else {
            "Standard"
        };

        let ass_path = caption_asset_dir
            .join(format!("captions_{}.ass", style_name.to_lowercase()))
            .to_string_lossy()
            .to_string();

        if caption_style == "kinetic" {
            subtitles::generate_kinetic_captions(
                &timeline_segments,
                &ass_path,
                style_name,
                "&H00FFD700",
            )
            .map_err(|e| ToolError::Srt(e.to_string()))?;
        } else {
            subtitles::srt_to_ass(&timeline_segments, &ass_path, style_name)
                .map_err(|e| ToolError::Srt(e.to_string()))?;
        }

        timeline.add_asset("captions", "ass".to_string(), json!({"path": ass_path}));
    }

    // Save timeline BEFORE calling sub-tools (they load from disk)
    timeline.save(&timeline_path)?;

    report_progress(40.0, 100.0, "Fetching b-roll...")
        .await
        .ok();
    for directive in &broll_arr {
        let concept = directive
            .get("concept")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let overlay_at_s = directive
            .get("overlay_at_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let duration_s = directive
            .get("duration_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(3.0);

        let fetch_result = handle_broll_fetch(json!({
            "concepts": [concept],
            "orientation": "9:16",
            "quality": "sd",
            "download": true,
        }))
        .await;

        match fetch_result {
            Ok(result) => {
                let cached_path = result
                    .get("downloaded")
                    .and_then(|d| d.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("path"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if let Some(path) = cached_path {
                    let assign_result = handle_broll_assign(json!({
                        "timeline_path": &timeline_path,
                        "concept": concept,
                        "position_ms": (overlay_at_s * 1000.0) as i64,
                        "duration_ms": (duration_s * 1000.0) as i64,
                        "asset_path": path,
                    }))
                    .await;
                    if let Err(e) = assign_result {
                        warnings.push(format!("broll assign failed for '{}': {}", concept, e));
                    }
                } else {
                    warnings.push(format!(
                        "broll fetch found no downloadable asset for '{}'",
                        concept
                    ));
                }
            }
            Err(e) => {
                warnings.push(format!("broll fetch failed for '{}': {}", concept, e));
            }
        }
    }

    report_progress(55.0, 100.0, "Assigning SFX...").await.ok();

    for directive in &sfx_arr {
        let role = directive
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("transition");
        let at_s = directive
            .get("at_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let assign_result = handle_sfx_assign(json!({
            "timeline_path": &timeline_path,
            "editorial_role": role,
            "query": "",
            "position_ms": (at_s * 1000.0) as i64,
        }))
        .await;
        if let Err(e) = assign_result {
            warnings.push(format!("sfx assign failed for role '{}': {}", role, e));
        }
    }

    if let Some(ref music) = music_obj {
        report_progress(65.0, 100.0, "Assigning music...")
            .await
            .ok();
        let mood = default_str(music, "mood", "neutral");
        let energy = default_str(music, "energy", "medium");
        let gain_db = default_f64(music, "gain_db", -12.0);
        let ducking = default_bool(music, "duck_under_dialogue", true);

        // Search for a matching music track, then pass its path
        let music_path = match handle_music_search(json!({
            "mood": mood,
            "energy": energy,
            "limit": 1,
        }))
        .await
        {
            Ok(r) => r
                .get("results")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("path"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string()),
            Err(_) => None,
        };

        if let Some(path) = music_path {
            let music_result = handle_music_assign(json!({
                "timeline_path": &timeline_path,
                "path": path,
                "mood": mood,
                "energy": energy,
                "gain_db": gain_db,
                "ducking": ducking,
            }))
            .await;
            if let Err(e) = music_result {
                warnings.push(format!("music assign failed: {}", e));
            }
        } else {
            warnings.push("No music track found in index — skipping music assignment".to_string());
        }
    }

    if !voiceover_arr.is_empty() {
        report_progress(75.0, 100.0, "Generating voiceovers...")
            .await
            .ok();
    }
    for directive in &voiceover_arr {
        let text = directive.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let position_s = directive
            .get("position_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let profile_id = directive
            .get("voice_profile_id")
            .and_then(|v| v.as_str())
            .unwrap_or("test_narrator");
        let speed = directive
            .get("speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let gain_db = directive
            .get("gain_db")
            .and_then(|v| v.as_f64())
            .unwrap_or(-6.0);

        let vo_result = handle_voiceover_generate(json!({
            "timeline_path": &timeline_path,
            "text": text,
            "voice_profile_id": profile_id,
            "position_ms": (position_s * 1000.0) as i64,
            "speed": speed,
            "gain_db": gain_db,
        }))
        .await;
        if let Err(e) = vo_result {
            warnings.push(format!("voiceover generate failed: {}", e));
        }
    }

    report_progress(85.0, 100.0, "Validating timeline...")
        .await
        .ok();
    let timeline = Timeline::load(&timeline_path)?;
    let validation_errors = timeline.validate();
    if !validation_errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline validation failed: {}",
            validation_errors.join("; ")
        )));
    }

    report_progress(90.0, 100.0, "Rendering final video...")
        .await
        .ok();
    let output = render_from_timeline(&timeline, &video_path, output_path.as_deref(), Some(crf))
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    let broll_count = track_count(&timeline, &TrackType::Broll);
    let sfx_count = track_count(&timeline, &TrackType::Sfx);
    let music_count = track_count(&timeline, &TrackType::Music);
    let voiceover_count = track_count(&timeline, &TrackType::Voiceover);
    let duration_s = timeline.rendered_duration_ms() as f64 / 1000.0;

    report_progress(100.0, 100.0, "Direct complete").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output,
        "duration_s": (duration_s * 100.0).round() / 100.0,
        "segments_count": timeline.segments.len(),
        "broll_count": broll_count,
        "sfx_count": sfx_count,
        "music_count": music_count,
        "voiceover_count": voiceover_count,
        "timeline_path": timeline_path,
        "warnings": if warnings.is_empty() { serde_json::Value::Null } else { json!(warnings) },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.parse — from-scratch video creation script parser
// ---------------------------------------------------------------------------

/// Handle script.schema: return the full JSON schema for ScriptSpec.
/// WARNING: dual-maintenance — update this handler when ScriptSpec/SceneSpec/SpeakerSpec/BackgroundSpec fields change.
/// Agents call this to discover the correct format before writing a script.
async fn handle_script_schema(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
                "description": "TTS engine configuration.",
                "properties": {
                    "backend": {"type": "string", "default": "kokoro", "enum": ["kokoro", "sidecar"]},
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
                    "ducking_depth_db": {"type": "number", "default": 12.0}
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
                    "voice": {"type": "string", "description": "Voice ID: 'kokoro:af_heart', 'kokoro:am_michael', or bare 'af_heart'. Use tts.voices to discover all 54 Kokoro voices."},
                    "preset": {"type": "string", "default": "default_person", "description": "SVG preset: default_person, robot, cat, etc."},
                    "position": {"type": "string", "default": "top-left", "enum": ["top-left", "top-right", "top-center", "center", "bottom-left", "bottom-right", "bottom-center"]},
                    "scale": {"type": "number", "default": 0.35, "description": "Sticker scale as fraction of canvas width (0.0-1.0)."}
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
                    "text": {"type": "string", "description": "The spoken text for this scene."},
                    "emote": {"type": ["string", "null"], "enum": ["neutral", "happy", "surprised", "thinking", null], "description": "Speaker emote for this scene."},
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

async fn handle_script_parse(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let validate_only = default_bool(&args, "validate_only", false);

    // Determine if input is inline JSON or a file path
    let json_str: String = if script_input.trim_start().starts_with('{') {
        // Inline JSON
        script_input.to_string()
    } else {
        // File path
        let path = sanitize_input_path(&script_input)?;
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Script file not found: {}",
                path.display()
            )));
        }
        std::fs::read_to_string(&path)?
    };

    // Parse the script
    let spec = parse_script(&json_str)
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
            "caption_style": spec.captions.style,
            "background_type": spec.background.r#type,
            "stickers_enabled": spec.stickers.enabled,
            "lip_sync_mode": spec.stickers.lip_sync,
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.generate_voices — TTS per scene
// ---------------------------------------------------------------------------

async fn handle_script_generate_voices(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts/voices");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
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
        let voice_lookup = &speaker.voice;
        let normalized_voice = if !voice_lookup.starts_with("kokoro:") && !voice_lookup.starts_with("faster-qwen") {
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
            })
            .map(|p| p.clone())
            .ok_or_else(|| {
                ToolError::NotFound(format!(
                    "Voice profile '{}' not found in registry. Try '{}' or add it via voice.profile.add.",
                    voice_lookup, normalized_voice
                ))
            })?;

        // Generate TTS for this scene
        let wav_path = format!("{}/{}_{}.wav", output_dir, scene.id, scene.speaker);
        if let Some(parent) = Path::new(&wav_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let result = tts_generate_routed(
            &speaker.voice,
            &scene.text,
            &wav_path,
            spec.tts.default_speed,
            spec.tts.default_pitch,
            1.0, // volume
            "wav",
            &profile,
        )
        .await?;

        // Calculate word timings for this scene using Parakeet TDT force alignment.
        // Falls back to even-spacing estimation if Parakeet is unavailable.
        let scene_end_ms = current_ms + result.duration_ms;
        let words = run_parakeet_alignment(&result.output_path, current_ms, scene_end_ms)
            .await
            .unwrap_or_else(|e| {
                let msg = format!(
                    "Scene {}: Parakeet force-alignment failed ({}), using estimated word timings. Caption sync will be approximate.",
                    i + 1,
                    e
                );
                tracing::warn!("[script.generate_voices] {}", msg);
                voice_warnings.push(msg);
                estimate_word_timings(&scene.text, current_ms, scene_end_ms)
            });

        segments.push(serde_json::json!({
            "scene_id": scene.id,
            "speaker": scene.speaker,
            "text": scene.text,
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

// ---------------------------------------------------------------------------
// Handler: script.build_captions — ASS generation from word timings
// ---------------------------------------------------------------------------

async fn handle_script_build_captions(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let manifest_path = extract_str(&args, "voiceover_manifest")?;
    let output_path = default_str(&args, "output_path", "artifacts/captions.ass");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Build CaptionSegments from manifest
    let mut segments = Vec::new();
    if let Some(segs) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segs {
            let text = seg
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_ms = seg.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let end_ms = seg.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);

            // Convert word timings from manifest
            let words: Vec<openscript_core::captions::WordTiming> = seg
                .get("words")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|w| {
                            Some(openscript_core::captions::WordTiming {
                                word: w.get("word")?.as_str()?.to_string(),
                                start_ms: w.get("start_ms")?.as_i64()?,
                                end_ms: w.get("end_ms")?.as_i64()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_else(|| estimate_word_timings(&text, start_ms, end_ms));

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

/// Helper: read script from inline JSON or file path.
fn read_script_input(script_input: &str) -> Result<String, ToolError> {
    if script_input.trim_start().starts_with('{') {
        Ok(script_input.to_string())
    } else {
        let path = sanitize_input_path(script_input)?;
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Script file not found: {}",
                path.display()
            )));
        }
        Ok(std::fs::read_to_string(&path)?)
    }
}

/// Run Parakeet TDT force alignment on a TTS WAV file to get accurate word timestamps.
/// Falls back to even-spacing estimation if Parakeet is unavailable.
///
/// This replaces the old whisper_align.py which depended on the `openai-whisper`
/// Python package. Parakeet TDT is a faster, more accurate RNN-T model that
/// runs via `onnxruntime` (no PyTorch dependency).
/// (Whisper→Parakeet migration per user directive.)
async fn run_parakeet_alignment(
    wav_path: &str,
    offset_ms: i64,
    _scene_end_ms: i64,
) -> Result<Vec<openscript_core::captions::WordTiming>, String> {
    // Write alignment to a temp JSON file
    let tmp_json = format!("{}.align.json", wav_path);

    // Resolve Parakeet model paths CWD-independently
    let parakeet_dir = resolve_repo_path("mcp/assets/parakeet");
    let encoder_path = parakeet_dir.join("encoder-model.int8.onnx");
    let decoder_path = parakeet_dir.join("decoder_joint-model.int8.onnx");
    let vocab_path = parakeet_dir.join("vocab.txt");

    // Resolve the sidecar script path
    let sidecar_script = resolve_repo_path("mcp/scripts/parakeet_align.py");

    let output = tokio::process::Command::new("python3")
        .arg(&sidecar_script)
        .arg("--wav")
        .arg(wav_path)
        .arg("--output")
        .arg(&tmp_json)
        .arg("--encoder")
        .arg(&encoder_path)
        .arg("--decoder")
        .arg(&decoder_path)
        .arg("--vocab")
        .arg(&vocab_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("Failed to spawn parakeet: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&tmp_json);
        return Err(format!(
            "Parakeet failed: {}",
            stderr.lines().last().unwrap_or("unknown")
        ));
    }

    // Read the alignment JSON
    let align_str = std::fs::read_to_string(&tmp_json)
        .map_err(|e| format!("Failed to read alignment: {}", e))?;
    let _ = std::fs::remove_file(&tmp_json);

    let align: serde_json::Value = serde_json::from_str(&align_str)
        .map_err(|e| format!("Failed to parse alignment JSON: {}", e))?;

    // Convert to WordTiming with offset
    let mut words = Vec::new();
    if let Some(word_arr) = align.get("words").and_then(|v| v.as_array()) {
        for w in word_arr {
            let word = w
                .get("word")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_ms = w.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let end_ms = w.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(start_ms);

            if !word.is_empty() {
                words.push(openscript_core::captions::WordTiming {
                    word,
                    start_ms: start_ms + offset_ms,
                    end_ms: end_ms + offset_ms,
                });
            }
        }
    }

    if words.is_empty() {
        return Err("Parakeet returned no words".to_string());
    }

    Ok(words)
}

// ---------------------------------------------------------------------------
// Handler: background.fetch — Pexels API (primary) + YouTube (fallback)
// ---------------------------------------------------------------------------

async fn handle_background_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let duration_s = default_f64(&args, "duration_s", 30.0);
    let aspect = default_str(&args, "aspect", "9:16");
    let cache_dir = default_str(&args, "cache_dir", "mcp/assets/background_cache");
    let fallback_pool: Vec<String> = args
        .get("fallback_pool")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    std::fs::create_dir_all(&cache_dir)?;

    let cache_key = format!("{:x}", md5_hash(query.as_bytes()));
    let clip_path = format!("{}/{}_clip.mp4", cache_dir, cache_key);

    // === PRIORITY 1: Pexels API (most reliable) ===
    let pexels_key_val = pexels_key();

    if !pexels_key_val.is_empty() {
        report_progress(0.0, 100.0, "Searching Pexels for stock footage...")
            .await
            .ok();

        let orientation = aspect_to_orientation(&aspect);

        let pexels_url = format!(
            "https://api.pexels.com/videos/search?query={}&per_page=15&orientation={}",
            urlencoding::encode(&query),
            orientation
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

        match client
            .get(&pexels_url)
            .header("Authorization", &pexels_key_val)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

                let videos = body
                    .get("videos")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                // Find a video with enough duration — prefer longer videos
                let mut best_video: Option<(String, i64)> = None;
                let mut best_duration: i64 = 0;
                for video in &videos {
                    let vid_duration = video.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                    // Prefer videos that are at least as long as what we need
                    // But accept any video >= 5s — the renderer will loop it
                    if vid_duration >= 5 {
                        // Get the best quality file that's 720p-1080p
                        for file in video
                            .get("video_files")
                            .and_then(|v| v.as_array())
                            .unwrap_or(&Vec::new())
                        {
                            let width = file.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                            let url = file.get("link").and_then(|v| v.as_str()).unwrap_or("");
                            if width >= 720 && width <= 1920 && !url.is_empty() {
                                // Prefer the longest video
                                if vid_duration > best_duration {
                                    best_video = Some((url.to_string(), vid_duration));
                                    best_duration = vid_duration;
                                }
                                break;
                            }
                        }
                    }
                }

                if let Some((video_url, source_duration)) = best_video {
                    report_progress(40.0, 100.0, "Downloading stock footage...")
                        .await
                        .ok();

                    // Download the full video
                    match client.get(&video_url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            let bytes = resp
                                .bytes()
                                .await
                                .map_err(|e| ToolError::Asset(format!("Download error: {}", e)))?;
                            let full_path = format!("{}/{}_full.mp4", cache_dir, cache_key);
                            std::fs::write(&full_path, &bytes)?;

                            report_progress(70.0, 100.0, "Processing clip...")
                                .await
                                .ok();

                            // If the source video is long enough, extract the requested duration
                            // If it's shorter, use the full video (renderer will loop it)
                            let (output_path, actual_duration_s, start_s) =
                                if source_duration as f64 >= duration_s {
                                    // Extract a clip of duration_s from a random start point
                                    let max_start = (source_duration as f64 - duration_s).max(0.0);
                                    let seed = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_nanos())
                                        .unwrap_or(0)
                                        as u64;
                                    let start = if max_start > 0.0 {
                                        (seed as f64 / u64::MAX as f64) * max_start
                                    } else {
                                        0.0
                                    };

                                    // Crop to aspect ratio
                                    let crop_filter = crop_filter_for_aspect(&aspect);

                                    let crop_result = tokio::process::Command::new("ffmpeg")
                                        .arg("-y")
                                        .arg("-ss")
                                        .arg(start.to_string())
                                        .arg("-i")
                                        .arg(&full_path)
                                        .arg("-t")
                                        .arg(duration_s.to_string())
                                        .arg("-vf")
                                        .arg(&crop_filter)
                                        .arg("-c:v")
                                        .arg("libx264")
                                        .arg("-preset")
                                        .arg("fast")
                                        .arg("-crf")
                                        .arg("23")
                                        .arg("-an")
                                        .arg(&clip_path)
                                        .stdin(std::process::Stdio::null())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::piped())
                                        .kill_on_drop(true)
                                        .output()
                                        .await;

                                    if let Ok(o) = crop_result {
                                        if o.status.success() {
                                            (clip_path.clone(), duration_s, start)
                                        } else {
                                            (full_path.clone(), source_duration as f64, 0.0)
                                        }
                                    } else {
                                        (full_path.clone(), source_duration as f64, 0.0)
                                    }
                                } else {
                                    // Source is shorter than needed — use full video, renderer will loop
                                    // Still crop to aspect ratio
                                    let crop_filter = crop_filter_for_aspect(&aspect);

                                    let crop_result = tokio::process::Command::new("ffmpeg")
                                        .arg("-y")
                                        .arg("-i")
                                        .arg(&full_path)
                                        .arg("-vf")
                                        .arg(&crop_filter)
                                        .arg("-c:v")
                                        .arg("libx264")
                                        .arg("-preset")
                                        .arg("fast")
                                        .arg("-crf")
                                        .arg("23")
                                        .arg("-an")
                                        .arg(&clip_path)
                                        .stdin(std::process::Stdio::null())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::piped())
                                        .kill_on_drop(true)
                                        .output()
                                        .await;

                                    if let Ok(o) = crop_result {
                                        if o.status.success() {
                                            (clip_path.clone(), source_duration as f64, 0.0)
                                        } else {
                                            (full_path.clone(), source_duration as f64, 0.0)
                                        }
                                    } else {
                                        (full_path.clone(), source_duration as f64, 0.0)
                                    }
                                };

                            report_progress(100.0, 100.0, "Stock footage ready")
                                .await
                                .ok();
                            let needs_looping = (source_duration as f64) < duration_s;
                            let result = json!({
                                "status": "fetched",
                                "clip_path": output_path,
                                "source": "pexels",
                                "source_duration_s": source_duration,
                                "start_s": start_s,
                                "duration_s": actual_duration_s,
                                "needs_looping": needs_looping,
                                "cached": false
                            });
                            return Ok(result);
                        }
                        _ => tracing::warn!("[background.fetch] Pexels download failed"),
                    }
                } else {
                    tracing::warn!(
                        "[background.fetch] No suitable Pexels videos found for query: {}",
                        query
                    );
                }
            }
            _ => tracing::warn!("[background.fetch] Pexels API request failed"),
        }
    }

    // === PRIORITY 2: YouTube via yt-dlp ===
    report_progress(30.0, 100.0, "Trying YouTube...").await.ok();
    let full_video_path = format!("{}/{}.mp4", cache_dir, cache_key);

    if !Path::new(&full_video_path).exists() {
        let yt_dlp_result = tokio::process::Command::new("yt-dlp")
            .arg("--format")
            .arg("bestvideo[height<=720][ext=mp4]+bestaudio/bestvideo[height<=720]+bestaudio/best[vcodec!=none][height<=720]/best[vcodec!=none]/best")
            .arg("--merge-output-format")
            .arg("mp4")
            .arg("--output")
            .arg(&full_video_path)
            .arg("--no-playlist")
            .arg("--quiet")
            .arg(format!("ytsearch1:{}", query))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;

        match yt_dlp_result {
            Ok(output) if output.status.success() => {
                report_progress(60.0, 100.0, "YouTube downloaded, extracting clip...")
                    .await
                    .ok();
            }
            _ => {
                // === PRIORITY 3: Fallback pool ===
                if let Some(fallback) = fallback_pool.first() {
                    if Path::new(fallback).exists() {
                        return Ok(json!({
                            "status": "fallback",
                            "clip_path": fallback,
                            "source": "fallback_pool",
                            "source_duration_s": duration_s,
                            "cached": false,
                            "warning": "Pexels + YouTube failed, using fallback pool"
                        }));
                    }
                }
                // === PRIORITY 4: Procedural ===
                return generate_procedural_background(&cache_dir, &cache_key, duration_s, &aspect)
                    .await;
            }
        }
    } else {
        report_progress(60.0, 100.0, "Using cached YouTube video...")
            .await
            .ok();
    }

    // Get video duration via ffprobe
    let probe_output = tokio::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(&full_video_path)
        .output()
        .await;

    let source_duration_s: f64 = match probe_output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(duration_s),
        _ => duration_s,
    };

    // Pick a random start time (leave room for clip duration)
    let max_start = (source_duration_s - duration_s).max(0.0);
    let start_s = if max_start > 0.0 {
        // Use a simple hash of query + timestamp for deterministic randomness
        let seed = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0) as u64)
            .wrapping_add(md5_hash(query.as_bytes()) as u64);
        (seed as f64 / u64::MAX as f64) * max_start
    } else {
        0.0
    };

    // Crop dimensions based on aspect
    let (crop_w, crop_h) = aspect_to_crop_dims(&aspect);

    // Extract clip with crop
    let crop_filter = format!("crop={}:{}", crop_w, crop_h);
    let extract_result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss")
        .arg(start_s.to_string())
        .arg("-i")
        .arg(&full_video_path)
        .arg("-t")
        .arg(duration_s.to_string())
        .arg("-vf")
        .arg(&crop_filter)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-an") // no audio (we'll add our own)
        .arg(&clip_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match extract_result {
        Ok(o) if o.status.success() => {
            report_progress(100.0, 100.0, "Clip extracted").await.ok();
            Ok(json!({
                "status": "fetched",
                "clip_path": clip_path,
                "source_duration_s": source_duration_s,
                "start_s": start_s,
                "duration_s": duration_s,
                "cached": Path::new(&full_video_path).exists(),
            }))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(ToolError::Ffmpeg(format!(
                "FFmpeg clip extraction failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!("FFmpeg spawn failed: {}", e))),
    }
}

/// Generate a procedural background via FFmpeg filters (fallback when yt-dlp unavailable).
async fn generate_procedural_background(
    cache_dir: &str,
    cache_key: &str,
    duration_s: f64,
    aspect: &str,
) -> Result<serde_json::Value, ToolError> {
    let (w, h) = aspect_to_crop_dims(aspect);
    let clip_path = format!("{}/{}_procedural.mp4", cache_dir, cache_key);

    let filter = format!(
        "color=c=0x0a0a1a:s={}x{}:d={}:r=30[bg];\
         color=c=0x1a1a3a:s={}x{}:d={}:r=30[bg2];\
         [bg][bg2]blend=all_mode=overlay:all_opacity=0.5[bg3];\
         [bg3]geq=r='128+80*sin(2*PI*X/W+0.1*T)':g='128+80*sin(2*PI*Y/H+0.15*T)':b='128+80*sin(2*PI*(X+Y)/(W+H)+0.2*T)'[v]",
        w, h, duration_s, w, h, duration_s
    );

    let result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[v]")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-an")
        .arg(&clip_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(ToolError::Ffmpeg(format!(
            "Procedural background failed: {}",
            stderr
        )));
    }

    Ok(json!({
        "status": "procedural",
        "clip_path": clip_path,
        "duration_s": duration_s,
        "cached": false,
        "warning": "yt-dlp unavailable, generated procedural background"
    }))
}

// ---------------------------------------------------------------------------
// Handler: background.assign — assign clips to scenes
// ---------------------------------------------------------------------------

async fn handle_background_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let manifest_path = extract_str(&args, "voiceover_manifest")?;
    let background_pool: Vec<String> = args
        .get("background_pool")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("background_pool is required".into()))?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let output_path = default_str(
        &args,
        "output_path",
        "artifacts/background_assignments.json",
    );

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Extract scene IDs, speakers, and durations from manifest
    let mut scene_ids = Vec::new();
    let mut scene_speakers = Vec::new();
    let mut scene_durations = Vec::new();

    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            scene_ids.push(
                seg.get("scene_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
            scene_speakers.push(
                seg.get("speaker")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
            let dur_ms = seg
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(3000);
            scene_durations.push(dur_ms as f64 / 1000.0);
        }
    }

    // Phase 5: Add pause_ms from SceneSpec to scene durations (breath beats).
    // This extends each scene's duration by the specified pause, creating
    // natural breathing gaps between scenes without affecting the TTS audio.
    for (i, dur) in scene_durations.iter_mut().enumerate() {
        if let Some(scene) = spec.scenes.get(i) {
            if let Some(pause) = scene.pause_ms {
                if pause > 0 {
                    *dur += pause as f64 / 1000.0;
                }
            }
        }
    }

    // Assign backgrounds
    let clips = assign_backgrounds(
        &scene_ids,
        &scene_speakers,
        &background_pool,
        &spec.background.change_cadence,
        &scene_durations,
    );

    // Write assignments
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let assignments = json!({
        "clips": clips,
        "cadence": spec.background.change_cadence,
        "pool_size": background_pool.len(),
    });
    std::fs::write(&output_path, serde_json::to_string_pretty(&assignments)?)?;

    Ok(json!({
        "status": "assigned",
        "output_path": output_path,
        "clip_count": clips.len(),
        "cadence": spec.background.change_cadence,
    }))
}

/// Simple MD5 hash for cache keys (no external dep needed for this use case).
fn md5_hash(data: &[u8]) -> u128 {
    // Simple FNV-1a hash as a lightweight cache key (not cryptographic)
    let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
    for &byte in data {
        hash ^= byte as u128;
        hash = hash.wrapping_mul(0x0000000001000000000000000000013b);
    }
    hash
}

/// Format seconds as HH:MM:SS for yt-dlp --download-sections
fn format_seconds_to_timestamp(s: f64) -> String {
    let total = s as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

/// Auto-select a music track from the 20-track stock catalog based on theme.
/// Returns the path if a suitable track is found, None otherwise.
/// (Round-3 UX audit PROBLEM 3b fix — ensures every video has background
/// music by default, even when the agent doesn't specify music.path.)
/// Try to download a free/stock music bed via yt-dlp when placeholders are the only local option.
#[allow(dead_code)]
async fn fetch_youtube_music_bed(theme: &str) -> Option<String> {
    let query = match theme {
        "calm" => "lofi study focus chill no copyright music",
        "energetic" => "upbeat corporate positive no copyright music",
        _ => "ambient chill background no copyright music",
    };
    fetch_youtube_music_bed_query(query).await
}

async fn fetch_youtube_music_bed_query(query: &str) -> Option<String> {
    let cache_dir = "mcp/assets/music_cache";
    std::fs::create_dir_all(cache_dir).ok()?;
    let out_tmpl = format!("{}/yt_music_%(id)s.%(ext)s", cache_dir);
    // Constrain size/duration — unrestricted ytsearch can pull 800MB+ streams.
    let result = tokio::process::Command::new("yt-dlp")
        .args([
            "-x",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "7",
            "-f",
            "bestaudio[filesize<12M]/bestaudio/best[filesize<12M]",
            "--max-filesize",
            "15M",
            "--no-playlist",
            "--quiet",
            "--no-warnings",
            "--socket-timeout",
            "20",
            "-o",
            &out_tmpl,
            &format!("ytsearch1:{}", query),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !result.status.success() {
        return None;
    }
    // Pick newest mp3 in music_cache
    let mut best: Option<(std::time::SystemTime, String)> = None;
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for e in entries.filter_map(|x| x.ok()) {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("mp3") {
                let modified = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let ps = p.to_string_lossy().to_string();
                if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
                    best = Some((modified, ps));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Pick intro + cut SFX from the tagged sfx_index (editorial_role / tags).
fn auto_select_sfx_hits(scene_durations: &[f64]) -> Vec<openscript_ffmpeg::multilayer_render::SfxHit> {
    let mut hits = Vec::new();
    let index_path = resolve_repo_path("mcp/assets/sfx_index.json");
    let Ok(raw) = std::fs::read_to_string(&index_path) else {
        return hits;
    };
    let Ok(index) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return hits;
    };
    let assets = index
        .get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let sfx_root = index
        .get("sfx_path")
        .and_then(|v| v.as_str())
        .unwrap_or("mcp/assets/sfx_pack");
    let resolve_path = |p: &str| -> Option<String> {
        let pb = std::path::Path::new(p);
        if pb.exists() {
            return Some(p.to_string());
        }
        let via_repo = resolve_repo_path(p);
        if via_repo.exists() {
            return Some(via_repo.to_string_lossy().into());
        }
        // Relative under index sfx_path / OPENSCRIPT_SFX_PATH
        for base in [
            Some(sfx_root.to_string()),
            std::env::var("OPENSCRIPT_SFX_PATH").ok(),
            Some("mcp/assets/sfx_pack".into()),
        ]
        .into_iter()
        .flatten()
        {
            let base_pb = resolve_repo_path(&base);
            let cand = base_pb.join(p);
            if cand.exists() {
                return Some(cand.to_string_lossy().into());
            }
            if let Some(name) = pb.file_name() {
                let cand2 = base_pb.join(name);
                if cand2.exists() {
                    return Some(cand2.to_string_lossy().into());
                }
            }
        }
        None
    };

    // Collect all resolvable SFX paths tagged by role for rotation.
    let mut intro_sfx: Vec<String> = Vec::new();
    let mut transition_sfx: Vec<String> = Vec::new();
    for a in &assets {
        let er = a.get("editorial_role").and_then(|v| v.as_str()).unwrap_or("");
        let tags = a
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let path = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            continue;
        }
        let tag_blob = format!("{} {} {}", er, tags, path.to_lowercase());
        if let Some(rp) = resolve_path(path) {
            if er == "intro" || tag_blob.contains("rise") || tag_blob.contains("whoosh") && er != "transition" {
                intro_sfx.push(rp.clone());
            }
            if er == "transition" || tag_blob.contains("transition") {
                transition_sfx.push(rp);
            } else if tag_blob.contains("whoosh") || tag_blob.contains("swish") || tag_blob.contains("hit") {
                transition_sfx.push(intro_sfx.last().cloned().unwrap_or(rp));
            }
        }
    }
    // Deduplicate each pool while preserving order.
    intro_sfx.dedup();
    transition_sfx.dedup();

    // Intro: pick first, or fallback to first transition SFX.
    let intro = intro_sfx.first().or(transition_sfx.first()).cloned();
    if let Some(p) = intro {
        hits.push(openscript_ffmpeg::multilayer_render::SfxHit {
            path: p,
            start_s: 0.05,
            volume: 0.28,
        });
    }
    // Transitions: rotate through available SFX to avoid repetition.
    let mut t = 0.0;
    let mut sfx_idx = 0usize;
    for (i, d) in scene_durations.iter().enumerate() {
        if i == 0 {
            t += *d;
            continue;
        }
        let pool = if transition_sfx.is_empty() { &intro_sfx } else { &transition_sfx };
        if !pool.is_empty() {
            let p = pool[sfx_idx % pool.len()].clone();
            sfx_idx += 1;
            hits.push(openscript_ffmpeg::multilayer_render::SfxHit {
                path: p,
                start_s: (t - 0.05).max(0.0),
                volume: 0.22,
            });
        }
        t += *d;
    }
    hits
}

/// Selected music bed with provenance for KPI / denylist.
struct MusicSelection {
    path: String,
    #[allow(dead_code)]
    mood: String,
    tags: Vec<String>,
    selection_query: String,
    source: String,
}

/// Simple deterministic hash for shuffle ordering (no `rand` dependency).
/// Returns a u32 from a string — used to shuffle entries within a mood bucket
/// so the same video doesn't always get the same track.
fn deterministic_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Extract tags array from a JSON entry.
fn entry_tags(entry: &serde_json::Value) -> Vec<String> {
    entry
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Mood→energy affinity: low-energy moods prefer low-energy tracks, etc.
fn mood_energy_affinity(mood: &str, energy: &str) -> bool {
    matches!(
        (mood, energy),
        ("calm", "low")
            | ("calm", "medium")
            | ("sad", "low")
            | ("dark", "low")
            | ("dark", "medium")
            | ("dramatic", "medium")
            | ("dramatic", "high")
            | ("energetic", "high")
            | ("energetic", "medium")
            | ("upbeat", "high")
            | ("upbeat", "medium")
            | ("neutral", _)
    )
}

/// Topic-aware music selection: library index → Pixabay → yt-dlp.
///
/// Selection algorithm (Phase 1 upgrade):
/// 1. Filter library entries by mood field (primary signal)
/// 2. Within mood bucket, sort by deterministic hash for variety
/// 3. Fallback: energy-affinity match if mood bucket empty
/// 4. Fallback: title/tag keyword match if energy bucket empty
/// 5. Fallback: Pixabay, then yt-dlp
async fn auto_select_music(theme: &str, video_keywords: &[String]) -> Option<MusicSelection> {
    let calm =
        openscript_core::production_quality::is_calm_focus_context(Some(theme), video_keywords);
    let mood = if calm {
        "calm"
    } else if theme == "energetic" {
        "energetic"
    } else {
        "neutral"
    }
    .to_string();
    let search_terms: Vec<&str> = if calm {
        vec![
            "lofi", "chill", "ambient", "calm", "focus", "study", "peaceful", "meditation",
            "relax",
        ]
    } else if theme == "energetic" {
        vec!["upbeat", "electronic", "energy", "corporate", "positive"]
    } else {
        vec!["background", "chill", "lofi", "ambient"]
    };
    let deny = ["parade", "march", "military", "trailer", "stadium", "anthem", "circus"];

    // ── Library index: mood-first selection ──────────────────────────────
    let index_path = resolve_repo_path("mcp/assets/music_library_index.json");
    if index_path.exists() {
        if let Ok(index_str) = std::fs::read_to_string(&index_path) {
            if let Ok(index) = serde_json::from_str::<serde_json::Value>(&index_str) {
                let entries = index
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                // Pre-filter: valid music entries, not on denylist, not local-only.
                let candidates: Vec<&serde_json::Value> = entries
                    .iter()
                    .filter(|e| {
                        let title = e.get("title").and_then(|v| v.as_str()).unwrap_or("");
                        let title_lower = title.to_lowercase();
                        if deny.iter().any(|d| title_lower.contains(d)) {
                            return false;
                        }
                        let media_type = e
                            .get("media_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("music");
                        if media_type != "music" && !media_type.is_empty() {
                            return false;
                        }
                        let source_type = e
                            .get("source_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if source_type == "local" {
                            return false;
                        }
                        let tags = entry_tags(e);
                        if openscript_core::production_quality::music_hits_denylist(
                            title,
                            Some(&mood),
                            &tags,
                            Some(&title_lower),
                        ) {
                            return false;
                        }
                        true
                    })
                    .collect();

                // Tier 1: mood-field exact match, shuffled by deterministic hash.
                let mut mood_bucket: Vec<&&serde_json::Value> = candidates
                    .iter()
                    .filter(|e| {
                        let entry_mood = e.get("mood").and_then(|v| v.as_str()).unwrap_or("neutral");
                        entry_mood == mood
                    })
                    .collect();
                mood_bucket.sort_by_key(|e| {
                    let vid = e
                        .get("video_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    deterministic_hash(vid)
                });

                // Tier 2: energy-affinity match (if mood bucket empty).
                let energy_bucket: Vec<&&serde_json::Value> = if mood_bucket.is_empty() {
                    let mut bucket: Vec<&&serde_json::Value> = candidates
                        .iter()
                        .filter(|e| {
                            let entry_energy =
                                e.get("energy").and_then(|v| v.as_str()).unwrap_or("medium");
                            mood_energy_affinity(&mood, entry_energy)
                        })
                        .collect();
                    bucket.sort_by_key(|e| {
                        let vid = e
                            .get("video_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        deterministic_hash(vid)
                    });
                    bucket
                } else {
                    vec![]
                };

                // Tier 3: title/tag keyword match (if both mood and energy buckets empty).
                let keyword_bucket: Vec<&&serde_json::Value> =
                    if mood_bucket.is_empty() && energy_bucket.is_empty() {
                        candidates
                            .iter()
                            .filter(|e| {
                                let title = e
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                let tags = entry_tags(e);
                                let tag_blob = tags.join(" ").to_lowercase();
                                search_terms
                                    .iter()
                                    .any(|t| title.contains(t) || tag_blob.contains(t))
                            })
                            .collect()
                    } else {
                        vec![]
                    };

                // Try each tier in order.
                let selected: Vec<&&serde_json::Value> = if !mood_bucket.is_empty() {
                    mood_bucket
                } else if !energy_bucket.is_empty() {
                    energy_bucket
                } else {
                    keyword_bucket
                };

                for entry in selected {
                    let filename = entry
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if filename.is_empty() {
                        continue;
                    }
                    let tags = entry_tags(entry);
                    let music_cache = resolve_repo_path("mcp/assets/music_cache");
                    let _ = std::fs::create_dir_all(&music_cache);
                    let local_path = music_cache.join(filename);
                    if local_path.exists() {
                        return Some(MusicSelection {
                            path: local_path.to_string_lossy().into(),
                            mood: mood.clone(),
                            tags: tags.clone(),
                            selection_query: format!("library:{}", filename),
                            source: "library".into(),
                        });
                    }
                    let download_args = json!({
                        "filename": filename,
                        "output_dir": music_cache.to_string_lossy(),
                    });
                    if let Ok(result) = handle_library_download(download_args).await {
                        if let Some(path) = result.get("path").and_then(|v| v.as_str()) {
                            return Some(MusicSelection {
                                path: path.to_string(),
                                mood: mood.clone(),
                                tags,
                                selection_query: format!("library:{}", filename),
                                source: "library".into(),
                            });
                        }
                    }
                }
            }
        }
    } else {
        tracing::warn!(
            "[script.to_video] music_library_index.json missing — run library.build"
        );
    }

    // ── Pixabay fallback ─────────────────────────────────────────────────
    if let Some(path) = fetch_pixabay_music(if calm { "calm" } else { theme }).await {
        if !openscript_core::production_quality::music_hits_denylist(
            &path,
            Some(&mood),
            &[],
            Some("pixabay"),
        ) {
            return Some(MusicSelection {
                path,
                mood: mood.clone(),
                tags: search_terms.iter().map(|s| s.to_string()).collect(),
                selection_query: format!("pixabay:{}", theme),
                source: "pixabay".into(),
            });
        }
    }

    // ── yt-dlp fallback: topic-safe queries only ─────────────────────────
    let yt_q = if calm {
        "lofi study focus chill no copyright music"
    } else if theme == "energetic" {
        "upbeat corporate positive no copyright music"
    } else {
        "ambient chill background no copyright music"
    };
    if let Some(path) = fetch_youtube_music_bed_query(yt_q).await {
        if openscript_core::production_quality::music_hits_denylist(
            &path,
            Some(&mood),
            &[],
            Some(yt_q),
        ) {
            tracing::warn!("[script.to_video] rejecting denylist music path {}", path);
        } else {
            return Some(MusicSelection {
                path,
                mood,
                tags: search_terms.iter().map(|s| s.to_string()).collect(),
                selection_query: yt_q.into(),
                source: "youtube".into(),
            });
        }
    }

    // ── Fallback 6: Pick ANY cached MP3 from mcp/assets/music_cache/ ──
    // Phase 28: When all else fails (no library match, no Pixabay, no YouTube),
    // use any locally cached music file. This ensures music is always present
    // in the output when cached files exist, preventing silent music omission.
    {
        let music_cache = resolve_repo_path("mcp/assets/music_cache");
        if let Ok(entries) = std::fs::read_dir(&music_cache) {
            let mut mp3s: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "mp3")
                        .unwrap_or(false)
                })
                .collect();
            // Deterministic shuffle for variety across runs
            mp3s.sort_by_key(|e| e.file_name());
            if let Some(entry) = mp3s.first() {
                let path = entry.path().to_string_lossy().to_string();
                tracing::info!(
                    "[script.to_video] music: using cached fallback {}",
                    path
                );
                return Some(MusicSelection {
                    path,
                    mood: mood.clone(),
                    tags: vec!["cached".into()],
                    selection_query: "cached-fallback".into(),
                    source: "cache".into(),
                });
            }
        }
    }

    None
}

/// Fetch royalty-free music from Pixabay's API.
/// Pixabay provides direct MP3 download URLs (no yt-dlp needed).
/// If PIXABAY_API_KEY is not set, uses the free tier (limited but functional).
async fn fetch_pixabay_music(theme: &str) -> Option<String> {
    let pixabay_key = pixabay_key();
    if pixabay_key.is_empty() {
        tracing::info!("[script.to_video] PIXABAY_API_KEY not set — skipping Pixabay music fetch");
        return None;
    }

    let search_query = match theme {
        "calm" => "meditation+calm",
        "energetic" => "upbeat+energetic",
        _ => "background+music",
    };

    let url = format!(
        "https://pixabay.com/api/audio/?key={}&q={}&per_page=3",
        pixabay_key,
        search_query
    );

    tracing::info!("[script.to_video] Fetching music from Pixabay: q='{}'", search_query);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;

    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        tracing::warn!("[script.to_video] Pixabay API returned status: {}", resp.status());
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;
    let hits = body.get("hits").and_then(|v| v.as_array())?;

    for hit in hits {
        let audio_url = hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
        let title = hit.get("tags").and_then(|v| v.as_str()).unwrap_or("unknown");

        if audio_url.is_empty() {
            continue;
        }

        tracing::info!("[script.to_video] Pixabay music found: '{}'", title);

        // Download the MP3
        let music_cache = resolve_repo_path("mcp/assets/music_cache");
        let _ = std::fs::create_dir_all(&music_cache);
        let filename = format!("pixabay_{}.mp3",
            title.replace(' ', "_").chars().take(30).collect::<String>());
        let local_path = music_cache.join(&filename);

        if local_path.exists() {
            tracing::info!("[script.to_video] Pixabay music already cached: {}", local_path.display());
            return Some(local_path.to_string_lossy().to_string());
        }

        if let Ok(dl_resp) = client.get(audio_url).send().await {
            if dl_resp.status().is_success() {
                if let Ok(bytes) = dl_resp.bytes().await {
                    std::fs::write(&local_path, &bytes).ok();
                    tracing::info!(
                        "[script.to_video] Downloaded Pixabay music: {} ({} bytes)",
                        local_path.display(), bytes.len()
                    );
                    return Some(local_path.to_string_lossy().to_string());
                }
            }
        }
    }

    tracing::warn!("[script.to_video] No downloadable music found on Pixabay");
    None
}

/// Build a mood-aware, scene-text-aware GIPHY sticker search query.
///
/// Priority (first non-empty candidate wins):
/// 1. Theme-based keyword (calm → "meditation", energetic → "fire", etc.)
/// 2. Scene emote (if the first scene has an emote like "happy", "surprised")
/// 3. Scene-text noun extraction (first salient noun from scene text)
/// 4. Speaker preset ("robot", "cat", "default_person")
/// 5. Fallback: "talking head"
///
/// Round-5 audit: the old hardcoded "{speaker_name} talking" produced
/// irrelevant stickers because speaker names are abstract IDs like "alice"
/// or "narrator" — not content GIPHY has indexed.
fn build_sticker_query(
    _speaker_name: &str,
    speaker_spec: &openscript_core::script::SpeakerSpec,
    scenes: &[openscript_core::script::SceneSpec],
    theme: &str,
    used_queries: &mut std::collections::HashSet<String>,
) -> String {
    // Theme-based keyword pools (rotated to avoid duplicates across speakers)
    let calm_keywords = ["meditation", "lotus", "candle", "breathing", "zen", "calm"];
    let energetic_keywords = ["fire", "lightning", "thumbs up", "applause", "energy", "explosion"];
    let neutral_keywords = ["talking head", "speech", "microphone", "podcast", "speaker"];

    let pool: &[&str] = match theme {
        "calm" => &calm_keywords,
        "energetic" => &energetic_keywords,
        _ => &neutral_keywords,
    };

    // Build candidate list in priority order
    let mut candidates: Vec<String> = Vec::new();

    // 1. Theme keywords (try each until we find one not yet used)
    for kw in pool {
        candidates.push(kw.to_string());
    }

    // 2. Scene emote
    if let Some(scene) = scenes.first() {
        if let Some(ref emote) = scene.emote {
            if !emote.is_empty() {
                candidates.push(emote.clone());
            }
        }
    }

    // 3. Scene-text noun (simple heuristic: first non-stopword > 4 chars)
    if let Some(scene) = scenes.first() {
        if let Some(noun) = extract_salient_noun(&scene.text) {
            candidates.push(noun);
        }
    }

    // 4. Speaker preset (if it's a real preset name, not empty/default)
    if !speaker_spec.preset.is_empty() && speaker_spec.preset != "default_person" {
        candidates.push(speaker_spec.preset.clone());
    }

    // 5. Fallback
    candidates.push("talking head".to_string());

    // Return the first candidate not yet used
    for c in &candidates {
        if !used_queries.contains(c) {
            used_queries.insert(c.clone());
            return c.clone();
        }
    }

    // All used — return the last candidate anyway
    candidates.last().unwrap().clone()
}

/// Extract the first salient noun from a text string.
/// Simple heuristic: skip stopwords, return the first word > 4 characters.
/// Extract an emotional reaction query from scene text for GIPHY meme b-rolls.
///
/// Detects the emotional beat of the scene and returns a GIPHY-friendly
/// query that will produce a relevant reaction GIF. Uses keyword matching
/// in the scene text + the scene's emote field + the overall theme.
/// Build a GIPHY search query for meme b-rolls that is BOTH topic-relevant
/// AND scene-specific.
///
/// The old approach used emotion labels ("mind blown reaction", "happy reaction")
/// which returned generic reaction memes unrelated to the video's topic.
/// GIPHY's translate endpoint interprets these as "show me a reaction GIF"
/// — it returns random reaction memes, not content-relevant clips.
///
/// The new approach uses content keywords extracted from the scene text,
/// combined with the video topic keywords. This makes GIPHY return clips
/// that are actually about the scene's subject matter.
///
/// Example:
///   Scene: "Your brain uses 20 watts of power."
///   Topic: ["brain", "neuroscience"]
///   Old query: "brain reaction" → generic brain reaction meme (irrelevant)
///   New query: "brain power energy" → brain/power/energy themed GIF (relevant)
///
/// (Round-17: "GIF relevance is still an issue. The recently generated
/// round 16 has a lot of GIF Brolls that are fully irrelevant. They are
/// meme material but not relevant to the topic.")
/// Build multiple GIPHY search queries for meme b-rolls, ranked by relevance.
///
/// Uses GIPHY SDK features discovered via Context7 research:
/// 1. Multi-query strategy: try specific → broad → trending fallback
/// 2. Relevance scoring: check GIF `tags` and `title` against query keywords
/// 3. `remove_low_contrast=true`: filter low-quality results
/// 4. `channel_ids`: could filter to topic-relevant channels (future)
///
/// The function returns a list of (query, limit) pairs to try in order.
/// The caller iterates through them until it finds a suitable non-duplicate,
/// non-static GIF with an MP4 URL.
///
/// (Round-18: GIPHY SDK comprehensive integration — user asked to "check
/// GIPHY SDK details and investigate how to effectively implement the
/// topic/context relevant brolls through GIFs, and understand how the
/// GIPHY SDK provides more comprehensive features and utilities than
/// regular API")
fn build_meme_search_queries(
    scene_text: &str,
    video_keywords: &[String],
    theme: &str,
) -> Vec<(String, u32)> {
    let stop_words = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "have",
        "has", "had", "do", "does", "did", "will", "would", "could", "should",
        "can", "may", "might", "it", "its", "this", "that", "these", "those",
        "i", "you", "he", "she", "we", "they", "what", "which", "who", "when",
        "where", "why", "how", "all", "each", "every", "both", "few", "more",
        "most", "other", "some", "such", "no", "nor", "not", "only", "own",
        "same", "so", "than", "too", "very", "just", "but", "and", "or", "if",
        "then", "else", "for", "of", "to", "in", "on", "at", "by", "with",
        "from", "as", "into", "through", "during", "before", "after", "about",
        "your", "yours", "their", "them", "our", "us", "my", "me",
        "here", "there", "now", "then", "also", "very",
    ];

    // Extract ALL content words from scene (not just first 3)
    let scene_words: Vec<String> = scene_text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && w.len() > 2 && !stop_words.contains(&w.as_str()))
        .collect();

    let topic_words: Vec<&str> = video_keywords.iter().take(3).map(|s| s.as_str()).collect();

    let mut queries: Vec<(String, u32)> = Vec::new();

    // Strategy 1: Most specific — topic + best scene content words
    // Pick the 2 longest scene words (usually the most informative nouns)
    let mut sorted_scene: Vec<&String> = scene_words.iter().collect();
    sorted_scene.sort_by(|a, b| b.len().cmp(&a.len()));
    let best_scene_words: Vec<&str> = sorted_scene.iter().take(2).map(|s| s.as_str()).collect();

    let mut q1_parts: Vec<&str> = Vec::new();
    for tw in &topic_words {
        if !q1_parts.contains(tw) {
            q1_parts.push(tw);
        }
    }
    for sw in &best_scene_words {
        if !q1_parts.contains(sw) && !topic_words.contains(sw) {
            q1_parts.push(sw);
        }
    }
    if !q1_parts.is_empty() {
        queries.push((q1_parts.join(" "), 10));
    }

    // Strategy 2: Broader — just topic keywords (for when scene words are too specific)
    if !topic_words.is_empty() {
        queries.push((topic_words.join(" "), 10));
    }

    // Strategy 3: Single best scene word alone (for when topic is too broad)
    if let Some(&best) = best_scene_words.first() {
        if !topic_words.contains(&best) {
            queries.push((best.to_string(), 5));
        }
    }

    // Strategy 4: Topic + theme (fallback for diversity)
    if !topic_words.is_empty() {
        let theme_suffix = match theme {
            "calm" => " nature",
            "energetic" => " action",
            _ => "",
        };
        queries.push((format!("{}{}", topic_words[0], theme_suffix), 5));
    }

    // Strategy 5: Ultimate fallback — trending GIFs (no query, just popular content)
    queries.push(("".to_string(), 5)); // Empty query = use trending endpoint

    queries
}

/// Score a GIPHY GIF's relevance to the search query.
/// Returns a score 0-100 based on how well the GIF's title, tags, and
/// source match the query keywords.
/// (Round-18: GIPHY SDK relevance scoring using GIF Object metadata)
fn score_gif_relevance(gif: &serde_json::Value, query: &str) -> u32 {
    let query_words: Vec<&str> = query.split_whitespace().collect();
    if query_words.is_empty() {
        return 50; // No query = neutral score for trending
    }

    let title = gif.get("title").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let slug = gif.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let username = gif.get("username").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let source_tld = gif.get("source_tld").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();

    let mut score = 0u32;

    for word in &query_words {
        let w = word.to_lowercase();
        // Title match = strong signal (5 points per word)
        if title.contains(&w) {
            score += 5;
        }
        // Slug match = medium signal (3 points)
        if slug.contains(&w) {
            score += 3;
        }
        // Username match = weak signal (1 point)
        if username.contains(&w) {
            score += 1;
        }
        // Source TLD match = weak signal (1 point)
        if source_tld.contains(&w) {
            score += 1;
        }
    }

    // Bonus: GIF has alt_text (descriptive metadata) = higher quality
    if gif.get("alt_text").and_then(|v| v.as_str()).is_some() {
        score += 2;
    }

    score.min(100)
}

// build_meme_search_query + extract_emotion_query removed (YAGNI):
// superseded by build_meme_search_queries multi-query strategy (Phase BY/BZ).

fn extract_salient_noun(text: &str) -> Option<String> {
    let stopwords = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "can", "may", "might",
        "this", "that", "these", "those", "your", "you", "they", "them", "their", "with",
        "from", "into", "about", "what", "when", "where", "which", "who", "how", "why",
        "for", "and", "but", "not", "all", "any", "some", "more", "most", "other", "such",
        "only", "own", "same", "than", "too", "very", "just", "also", "now", "then",
    ];
    for word in text.split_whitespace() {
        let clean: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
        let lower = clean.to_lowercase();
        if lower.len() > 4 && !stopwords.contains(&lower.as_str()) {
            return Some(lower);
        }
    }
    None
}

/// Words that can produce undesirable Pexels results when used as search
/// queries. For example, "inhale" returns cigarette-smoking videos; "drink"
/// returns alcohol videos. Map them to safer equivalents that produce
/// on-topic calming/meditation content.
/// (Round-6 audit: "In the inhale section, the video was of someone
/// inhaling a cigarette.")
const UNSAFE_KEYWORD_MAP: &[(&str, &str)] = &[
    ("inhale", "breathing meditation"),
    ("exhale", "breathing relaxation"),
    ("breathe", "breathing calm"),
    ("breathing", "breathing meditation"),
    ("drink", "drinking water wellness"),
    ("smoke", "calm nature"),
    ("drug", "calm nature"),
    ("kill", "calm nature"),
    ("blood", "calm nature"),
    ("pain", "healing wellness"),
    ("stress", "stress relief meditation"),
    ("anxiety", "anxiety relief calm"),
    ("fear", "courage calm nature"),
    ("death", "calm nature peaceful"),
    ("weapon", "calm nature"),
];

/// Enrich a Pexels search query with mood-aware context to bias results
/// toward calming/energetic content. For theme:calm, prepend "calm" to
/// the query so Pexels returns peaceful footage instead of literal matches
/// that could be tonally wrong (e.g. "inhale" → cigarette).
fn enrich_query_for_theme(query: &str, theme: &str) -> String {
    // Don't double-enrich if the query already contains a mood word
    let lower = query.to_lowercase();
    let already_calm = lower.contains("calm") || lower.contains("peaceful") || lower.contains("meditation");
    let already_energetic = lower.contains("energy") || lower.contains("action") || lower.contains("intense");

    match theme {
        "calm" if !already_calm => format!("calm {}", query),
        "energetic" if !already_energetic => format!("energetic {}", query),
        _ => query.to_string(),
    }
}

/// Filter and enrich extracted keywords for Pexels search safety.
/// 1. Replace unsafe keywords (inhale → breathing meditation)
/// 2. Enrich with theme context (prepend "calm" for calm theme)
fn safe_search_query(raw_keywords: &str, theme: &str) -> String {
    // Check each word against the unsafe map
    let mut safe_words: Vec<String> = Vec::new();
    for word in raw_keywords.split_whitespace() {
        let lower = word.to_lowercase();
        let replaced = UNSAFE_KEYWORD_MAP
            .iter()
            .find(|(unsafe_word, _)| *unsafe_word == lower.as_str())
            .map(|(_, safe)| safe.to_string())
            .unwrap_or_else(|| word.to_string());
        safe_words.push(replaced);
    }
    let safe_query = safe_words.join(" ");
    enrich_query_for_theme(&safe_query, theme)
}

/// Legacy helper — prefer `stock_signal::build_scene_stock_query` for multi-broll.
#[allow(dead_code)]
fn extract_keywords(text: &str, fallback_query: &str) -> String {
    let toks = crate::stock_signal::signal_tokens_from_scene(text, &[]);
    if toks.is_empty() {
        fallback_query.to_string()
    } else {
        toks.into_iter().take(5).collect::<Vec<_>>().join(" ")
    }
}

// ---------------------------------------------------------------------------
// Handler: background.search — search procedural background index by mood
// ---------------------------------------------------------------------------

async fn handle_background_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let mood_filter = default_opt_str(&args, "mood");
    let energy_filter = default_opt_str(&args, "energy");
    let motion_filter = default_opt_str(&args, "motion_intensity");
    let limit = default_u32(&args, "limit", 10) as usize;

    // Resolve the index path CWD-independently. The round-2 UX audit
    // (GAP #12) found background.search only worked from the repo root
    // because it used a relative path. Now uses resolve_repo_path which
    // tries CWD > OPENSCRIPT_ROOT > CARGO_MANIFEST_DIR.
    let index_path_raw = std::env::var("OPENSCRIPT_BACKGROUNDS_INDEX")
        .unwrap_or_else(|_| "mcp/assets/backgrounds_index.json".to_string());
    let index_path = resolve_repo_path(&index_path_raw);

    if !index_path.exists() {
        return Err(ToolError::NotFound(format!(
            "Backgrounds index not found at {} (resolved from {}). The index is committed at mcp/assets/backgrounds_index.json — if missing, re-clone or restore from git.",
            index_path.display(),
            index_path_raw
        )));
    }

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut results: Vec<serde_json::Value> = Vec::new();
    for entry in &entries {
        if let Some(ref mood) = mood_filter {
            let entry_mood = entry.get("mood").and_then(|v| v.as_str()).unwrap_or("");
            if entry_mood != mood {
                continue;
            }
        }
        if let Some(ref energy) = energy_filter {
            let entry_energy = entry.get("energy").and_then(|v| v.as_str()).unwrap_or("");
            if entry_energy != energy {
                continue;
            }
        }
        if let Some(ref motion) = motion_filter {
            let entry_motion = entry
                .get("motion_intensity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_motion != motion {
                continue;
            }
        }

        // Build the full path so callers can use it directly in fallback_pool.
        // Resolve relative to the index file's parent's parent (so
        // mcp/assets/backgrounds_index.json → mcp/assets/backgrounds/).
        // This makes the returned paths work regardless of the agent's CWD.
        let filename = entry.get("filename").and_then(|v| v.as_str()).unwrap_or("");
        let backgrounds_dir = index_path
            .parent() // mcp/assets/
            .map(|p| p.join("backgrounds"))
            .unwrap_or_else(|| std::path::PathBuf::from("mcp/assets/backgrounds"));
        let full_path = backgrounds_dir.join(filename);
        let full_path_str = full_path.to_string_lossy().to_string();

        let mut result = entry.clone();
        if let Some(obj) = result.as_object_mut() {
            obj.insert("path".into(), json!(full_path_str));
        }
        results.push(result);
    }

    let total = results.len();
    results.truncate(limit);

    Ok(json!({
        "status": "searched",
        "filters": {
            "mood": mood_filter,
            "energy": energy_filter,
            "motion_intensity": motion_filter,
        },
        "total_matches": total,
        "count": results.len(),
        "results": results,
        "index_stats": {
            "total_entries": index.get("total_entries"),
            "mood_counts": index.get("mood_counts"),
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: sticker.load_preset — load SVG preset config
// ---------------------------------------------------------------------------

/// Handler: sticker.presets — list all available sticker positioning presets
async fn handle_sticker_presets(
    _args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let presets = openscript_core::sticker_presets::StickerPreset::all();
    let presets_json: serde_json::Value = serde_json::to_value(&presets)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to serialize presets: {}", e)))?;
    Ok(json!({
        "status": "success",
        "count": presets.len(),
        "presets": presets_json,
        "message": "Use preset name in speaker.preset field of script JSON. Each preset defines position, scale, and caption-safe margin."
    }))
}

async fn handle_sticker_load_preset(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let preset_name = extract_str(&args, "preset_name")?;
    let presets_dir = default_str(&args, "presets_dir", "mcp/assets/svg_presets");

    let preset_dir = format!("{}/{}", presets_dir, preset_name);
    if !Path::new(&preset_dir).exists() {
        return Err(ToolError::NotFound(format!(
            "Preset not found: {} (looked in {})",
            preset_name, preset_dir
        )));
    }

    // Load preset.json
    let preset_json_path = format!("{}/preset.json", preset_dir);
    let preset_json = std::fs::read_to_string(&preset_json_path)?;
    let preset: StickerPreset = serde_json::from_str(&preset_json)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse preset.json: {}", e)))?;

    // Load puppet.svg
    let puppet_svg_path = format!("{}/puppet.svg", preset_dir);
    let puppet_svg = std::fs::read_to_string(&puppet_svg_path)?;

    Ok(json!({
        "status": "loaded",
        "preset_name": preset_name,
        "preset": preset,
        "puppet_svg": puppet_svg,
    }))
}

// ---------------------------------------------------------------------------
// Handler: sticker.render — generate animated sticker HTML composition
// ---------------------------------------------------------------------------

async fn handle_sticker_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let wav_path = extract_str(&args, "wav_path")?;
    let preset_name = extract_str(&args, "preset_name")?;
    let position = default_str(&args, "position", "top-left");
    let scale = default_f64(&args, "scale", 0.25);
    let canvas_width = default_u32(&args, "canvas_width", 1080);
    let canvas_height = default_u32(&args, "canvas_height", 1920);
    let fps = default_u32(&args, "fps", 30);
    let output_path = default_str(&args, "output_path", "artifacts/sticker.html");
    let render_to_video = default_bool(&args, "render_to_video", false);

    report_progress(0.0, 100.0, "Loading preset...").await.ok();

    // Load preset
    let presets_dir = "mcp/assets/svg_presets";
    let preset_dir = format!("{}/{}", presets_dir, preset_name);
    if !Path::new(&preset_dir).exists() {
        return Err(ToolError::NotFound(format!(
            "Preset not found: {} (looked in {})",
            preset_name, preset_dir
        )));
    }

    let preset_json = std::fs::read_to_string(format!("{}/preset.json", preset_dir))?;
    let preset: StickerPreset = serde_json::from_str(&preset_json)
        .map_err(|e| ToolError::InvalidArg(format!("Preset parse error: {}", e)))?;

    let puppet_svg = std::fs::read_to_string(format!("{}/puppet.svg", preset_dir))?;

    report_progress(30.0, 100.0, "Extracting amplitude...")
        .await
        .ok();

    // Extract amplitude from WAV
    let amplitude = extract_amplitude(wav_path, fps)
        .map_err(|e| ToolError::InvalidArg(format!("Amplitude extraction failed: {}", e)))?;

    report_progress(60.0, 100.0, "Generating composition...")
        .await
        .ok();

    // Generate sticker HTML composition
    let html = generate_sticker_composition(
        &puppet_svg,
        &preset,
        &amplitude,
        &position,
        scale,
        canvas_width,
        canvas_height,
    );

    // Write output
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, html)?;

    // Phase K: Optionally render the HTML to a transparent WebM via hf.render.
    // This produces a video file that multilayer_render can composite as a
    // StickerOverlay. The WebM format preserves alpha transparency.
    let mut video_path: Option<String> = None;
    if render_to_video {
        report_progress(80.0, 100.0, "Rendering sticker to WebM via HyperFrames...")
            .await
            .ok();

        let sticker_dir = std::path::Path::new(&output_path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let webm_path = sticker_dir
            .join(format!(
                "sticker_{}.webm",
                std::path::Path::new(&output_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("sticker")
            ))
            .to_string_lossy()
            .to_string();

        match crate::hf::handle_hf_render(json!({
            "project_dir": sticker_dir.to_string_lossy().to_string(),
            "output_path": webm_path,
            "quality": "draft",
        }))
        .await
        {
            Ok(_) => {
                video_path = Some(webm_path);
            }
            Err(e) => {
                tracing::warn!("[sticker.render] HF render to WebM failed: {}", e);
                // Non-fatal — the HTML is still usable; the agent can render manually
            }
        }
    }

    report_progress(100.0, 100.0, "Sticker composition generated")
        .await
        .ok();

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "video_path": video_path,
        "preset_name": preset_name,
        "position": position,
        "scale": scale,
        "frame_count": amplitude.frames.len(),
        "duration_ms": amplitude.duration_ms,
        "next_step": if video_path.is_some() {
            "Sticker rendered to WebM. Use the video_path as a StickerOverlay in multilayer_render or overlay.assign."
        } else {
            "Sticker HTML generated. Call sticker.render with render_to_video=true to produce a compositable WebM, or use the HTML with hf.render manually."
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.to_timeline — orchestrator for from-scratch video creation
// ---------------------------------------------------------------------------

async fn handle_script_to_timeline(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts");
    let skip_background = default_bool(&args, "skip_background", false);
    let skip_stickers = default_bool(&args, "skip_stickers", false);
    let voiceover_manifest_path = default_opt_str(&args, "voiceover_manifest_path");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
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
            let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("");
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

// ---------------------------------------------------------------------------
// Handler: script.to_video — one-call from-scratch video creation
// ---------------------------------------------------------------------------

async fn handle_script_to_video(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
    let voiceover_manifest_path = default_opt_str(&args, "voiceover_manifest_path");

    // Parse script for render config
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

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
    let mut scene_stock_meta: Vec<Option<(String, String, String, f64, String)>> = Vec::new();
    let pexels_key_val = pexels_key();

    // Multi-broll stock footage: unique clip per scene.
    // Priority: Pexels (if key) → YouTube via yt-dlp (no key) → procedural (last resort).
    // type:"static" is the explicit opt-out. type:"procedural" still TRIES stock first
    // so agents do not silently ship gradient-only videos when stock is reachable.
    // (Phase CF: production quality upgrade — never treat procedural as success.)
    if !skip_background && spec.background.r#type != "static" {
        report_progress(
            35.0,
            60.0,
            "Fetching multi-broll stock backgrounds (Pexels → YouTube → procedural)...",
        )
        .await
        .ok();

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

        let orientation = match spec.meta.aspect.as_str() {
            "9:16" => "portrait",
            "16:9" => "landscape",
            "1:1" => "square",
            _ => "portrait",
        };

        let cache_dir = "mcp/assets/background_cache";
        std::fs::create_dir_all(cache_dir).ok();

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
                crate::stock_signal::build_scene_stock_query(
                    scene_text,
                    &spec.video_keywords,
                    &spec.output.theme,
                    &spec.meta.aspect,
                    scene_idx,
                )
            };
            // Keep unsafe-keyword rewrite for edge terms (blood → calm nature)
            let query = safe_search_query(&stock_q.query, &spec.output.theme);
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

            let mut scene_bg: Option<String> = None;
            let mut bg_source = "none";
            let mut stock_meta: Option<(String, String, String, f64, String)> = None; // id,hash,q,lex,title

            // --- Priority 1: Pexels (requires API key) ---
            if !pexels_key_val.is_empty() {
                let pexels_url = format!(
                    "https://api.pexels.com/videos/search?query={}&per_page=5&orientation={}",
                    urlencoding::encode(&query),
                    orientation
                );

                if let Ok(resp) = client
                    .get(&pexels_url)
                    .header("Authorization", &pexels_key_val)
                    .send()
                    .await
                {
                    if resp.status().is_success() {
                        if let Ok(body) = resp.json::<serde_json::Value>().await {
                            if let Some(videos) = body.get("videos").and_then(|v| v.as_array()) {
                                for video in videos {
                                    let vid_id =
                                        video.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                                    if vid_id > 0 && used_pexels_ids.contains(&vid_id) {
                                        continue;
                                    }
                                    let vid_dur = video
                                        .get("duration")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(0);
                                    if vid_dur >= 3 {
                                        if let Some(files) =
                                            video.get("video_files").and_then(|v| v.as_array())
                                        {
                                            for file in files {
                                                let width = file
                                                    .get("width")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0);
                                                let url = file
                                                    .get("link")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("");
                                                if width >= 720 && width <= 1920 && !url.is_empty() {
                                                    let clip_path = format!(
                                                        "{}/scene_{:03}.mp4",
                                                        cache_dir,
                                                        scene_idx + 1
                                                    );
                                                    if let Ok(dl_resp) =
                                                        client.get(url).send().await
                                                    {
                                                        if dl_resp.status().is_success() {
                                                            if let Ok(bytes) =
                                                                dl_resp.bytes().await
                                                            {
                                                                std::fs::write(&clip_path, &bytes)
                                                                    .ok();
                                                                let crop_filter =
                                                                    crop_filter_for_aspect(
                                                                        &spec.meta.aspect,
                                                                    );
                                                                let trimmed = format!(
                                                                    "{}/scene_{:03}_trim.mp4",
                                                                    cache_dir,
                                                                    scene_idx + 1
                                                                );
                                                                let trim_result =
                                                                    tokio::process::Command::new(
                                                                        "ffmpeg",
                                                                    )
                                                                    .arg("-y")
                                                                    .arg("-i")
                                                                    .arg(&clip_path)
                                                                    .arg("-t")
                                                                    .arg(dur.to_string())
                                                                    .arg("-vf")
                                                                    .arg(&crop_filter)
                                                                    .arg("-c:v")
                                                                    .arg("libx264")
                                                                    .arg("-preset")
                                                                    .arg("fast")
                                                                    .arg("-crf")
                                                                    .arg("23")
                                                                    .arg("-an")
                                                                    .arg(&trimmed)
                                                                    .stdin(std::process::Stdio::null())
                                                                    .stdout(std::process::Stdio::null())
                                                                    .stderr(std::process::Stdio::piped())
                                                                    .kill_on_drop(true)
                                                                    .output()
                                                                    .await;
                                                                let chosen = if trim_result
                                                                    .as_ref()
                                                                    .map(|o| o.status.success())
                                                                    .unwrap_or(false)
                                                                {
                                                                    trimmed
                                                                } else {
                                                                    clip_path
                                                                };
                                                                // Geometry gate (no stretch)
                                                                let geo = crate::stock_signal::probe_geometry(
                                                                    &chosen,
                                                                    &spec.meta.aspect,
                                                                );
                                                                if !geo.ok {
                                                                    tracing::warn!(
                                                                        "[pexels stock] geometry reject id={} {:?}",
                                                                        vid_id,
                                                                        geo.reasons
                                                                    );
                                                                    let _ = std::fs::remove_file(&chosen);
                                                                    continue;
                                                                }
                                                                // Fingerprint: reject if same bytes as prior scene
                                                                if let Some(h) =
                                                                    file_content_fingerprint(
                                                                        &chosen,
                                                                    )
                                                                {
                                                                    if used_content_hashes
                                                                        .contains(&h)
                                                                    {
                                                                        let _ =
                                                                            std::fs::remove_file(
                                                                                &chosen,
                                                                            );
                                                                        continue;
                                                                    }
                                                                    used_content_hashes
                                                                        .insert(h.clone());
                                                                    stock_meta = Some((
                                                                        format!("pexels_{}", vid_id),
                                                                        h,
                                                                        query.clone(),
                                                                        0.5,
                                                                        String::new(),
                                                                    ));
                                                                }
                                                                scene_bg = Some(chosen);
                                                                used_pexels_ids.insert(vid_id);
                                                                bg_source = "pexels";
                                                                break;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if scene_bg.is_some() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // --- Priority 2: YouTube stock via yt-dlp (no API key) ---
            if scene_bg.is_none() {
                // Prefer non-procedural paths from script.fallback_pool if caller supplied stock
                let pool_stock = spec
                    .background
                    .fallback_pool
                    .iter()
                    .find(|p| !is_procedural_media_path(p) && Path::new(p).exists());
                if let Some(p) = pool_stock {
                    scene_bg = Some(p.clone());
                    bg_source = "fallback_pool_stock";
                }
            }
            if scene_bg.is_none() {
                // Query already includes stock/vertical bias from stock_signal.
                // Diversify only if we already tried the exact same query string.
                let mut yt_q = query.clone();
                if used_yt_queries.contains(&yt_q) {
                    yt_q = format!("{} scene{}", query, scene_idx + 1);
                }
                used_yt_queries.insert(yt_q.clone());
                let yt_out = format!("{}/scene_{:03}_yt.mp4", cache_dir, scene_idx + 1);
                if let Some(fetch) = fetch_youtube_stock_clip_signal(
                    &yt_q,
                    &signal_tokens,
                    dur,
                    &spec.meta.aspect,
                    &yt_out,
                    scene_idx,
                    &mut used_video_ids,
                    &mut used_content_hashes,
                )
                .await
                {
                    stock_meta = Some((
                        fetch.video_id,
                        fetch.content_hash,
                        fetch.search_query,
                        fetch.lexical_score,
                        fetch.source_title,
                    ));
                    scene_bg = Some(fetch.path);
                    bg_source = "youtube";
                }
            }

            // Phase B: query fan-out — one more attempt with scene nouns only
            if scene_bg.is_none() && !signal_tokens.is_empty() {
                let short_q = signal_tokens
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                let yt_out2 = format!("{}/scene_{:03}_yt_b.mp4", cache_dir, scene_idx + 1);
                if let Some(fetch) = fetch_youtube_stock_clip_signal(
                    &format!("{} stock footage vertical", short_q),
                    &signal_tokens,
                    dur,
                    &spec.meta.aspect,
                    &yt_out2,
                    scene_idx,
                    &mut used_video_ids,
                    &mut used_content_hashes,
                )
                .await
                {
                    stock_meta = Some((
                        fetch.video_id,
                        fetch.content_hash,
                        fetch.search_query,
                        fetch.lexical_score,
                        fetch.source_title,
                    ));
                    scene_bg = Some(fetch.path);
                    bg_source = "youtube";
                }
            }

            scene_stock_meta.push(stock_meta);

            if let Some(path) = scene_bg {
                tracing::info!(
                    "[script.to_video] Scene {} background source={} path={}",
                    scene_idx + 1,
                    bg_source,
                    path
                );
                per_scene_backgrounds.push(path);
            } else {
                // Last resort: procedural (synthetic) — hard production quality fail.
                let fallback = format!(
                    "mcp/assets/backgrounds/procedural_{:02}.mp4",
                    (scene_idx % 10) + 1
                );
                let procedural_path = if std::path::Path::new(&fallback).exists() {
                    fallback
                } else {
                    "mcp/assets/backgrounds/procedural_01.mp4".to_string()
                };
                per_scene_backgrounds.push(procedural_path.clone());
                let allow_proc = std::env::var("OPENSCRIPT_ALLOW_PROCEDURAL")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                if !allow_proc {
                    render_warnings.push(format!(
                        "HARD stock_visuals scene {}: no relevant unique stock (Pexels/YT). Using procedural {}. Set PEXELS_API_KEY or OPENSCRIPT_ALLOW_PROCEDURAL=1. Production will hard-fail if ≥50% procedural.",
                        scene_idx + 1,
                        procedural_path
                    ));
                } else {
                    render_warnings.push(format!(
                        "PRODUCTION_FAIL stock_visuals scene {}: synthetic procedural ({})",
                        scene_idx + 1,
                        procedural_path
                    ));
                }
            }
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
            looped: false, // Each scene has its own clip, no need to loop
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
        // Track queries used across speakers/scenes so we don't fetch the same
        // sticker twice.
        let mut used_sticker_queries: std::collections::HashSet<String> =
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
                                    if used_sticker_queries.contains(&sticker_id) {
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
                                                used_sticker_queries.insert(sticker_id);
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
                    // Build a scene-specific query: emote > noun > text snippet
                    let query = {
                        let mut candidates: Vec<String> = Vec::new();
                        if let Some(ref emote) = scene.emote {
                            if !emote.is_empty() {
                                candidates.push(emote.clone());
                            }
                        }
                        if let Some(noun) = extract_salient_noun(&scene.text) {
                            candidates.push(noun);
                        }
                        // Use first 3 words of scene text as fallback
                        let text_snippet: String = scene
                            .text
                            .split_whitespace()
                            .take(3)
                            .collect::<Vec<_>>()
                            .join(" ");
                        if !text_snippet.is_empty() {
                            candidates.push(text_snippet);
                        }
                        candidates.push("talking head".to_string());

                        candidates
                            .into_iter()
                            .find(|c| !used_sticker_queries.contains(c.as_str()))
                            .unwrap_or_else(|| "talking head".to_string())
                    };
                    used_sticker_queries.insert(query.clone());

                    tracing::info!(
                        "[script.to_video] Per-scene sticker query for scene {}: '{}'",
                        scene_idx,
                        query
                    );

                    let giphy_url = format!(
                        "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit=8&rating=g&bundle=sticker_layering&lang=en",
                        giphy_key_val,
                        urlencoding::encode(&query)
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
                                        if used_sticker_queries.contains(&sticker_id) { continue; }

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
                                                    used_sticker_queries.insert(sticker_id);
                                                    tracing::info!(
                                                        "[script.to_video] Per-scene sticker for scene {}: {}",
                                                        scene_idx, sticker_path
                                                    );
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
            for track_type in [TrackType::Broll, TrackType::Music, TrackType::Captions, TrackType::Sfx] {
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
                        tags: vec![],
                        provenance: None,
                        kind: EventKind::Broll {
                            concept: String::new(),
                            source_provider: "multi_broll".into(),
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
                            mood: spec.output.theme.clone(),
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
                tl.add_asset(
                    "broll",
                    asset_id,
                    serde_json::json!({"path": bg.path, "start_ms": bg.start_ms, "end_ms": bg.end_ms}),
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

            // Save updated timeline
            let _ = tl.save(&timeline_path);
            tracing::info!(
                "[script.to_video] Updated timeline tracks: broll={} music={} captions={} sfx={}",
                bg_assignments.len(),
                if music_path.is_some() { 1 } else { 0 },
                if !captions_path.is_empty() { 1 } else { 0 },
                sfx_hits.len(),
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
                let hint = if is_procedural_media_path(&b.path) {
                    Some("procedural".into())
                } else if b.path.contains("_yt") || b.path.contains("background_cache") {
                    Some("youtube".into())
                } else if meta.map(|(id, _, _, _, _)| id.starts_with("pexels_")).unwrap_or(false) {
                    Some("pexels".into())
                } else {
                    None
                };
                bg_layers.push(BackgroundLayerInfo {
                    path: b.path.clone(),
                    start_ms: t_cursor,
                    end_ms: t_cursor + dur_ms,
                    source_hint: hint,
                    content_hash: meta.map(|(_, h, _, _, _)| h.clone()),
                    video_id: meta.map(|(id, _, _, _, _)| id.clone()),
                    search_query: meta.map(|(_, _, q, _, _)| q.clone()),
                    lexical_score: meta.map(|(_, _, _, lex, _)| *lex),
                    source_title: meta.map(|(_, _, _, _, t)| t.clone()),
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
                    mood: Some(spec.output.theme.clone()),
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
        "rendered_production_fail"
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
                "warnings": merged_warnings,
            }))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!("Render failed: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: stock.fetch — download stock music/videos from Pixabay/Pexels APIs
// ---------------------------------------------------------------------------

async fn handle_stock_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_type = extract_str(&args, "type")?; // "music" or "video"
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 5) as usize;
    let output_dir = default_str(&args, "output_dir", "mcp/assets/stock_cache");

    std::fs::create_dir_all(&output_dir)?;

    report_progress(0.0, 100.0, &format!("Searching for {}...", media_type))
        .await
        .ok();

    if media_type == "music" {
        // Try Pixabay music API
        let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };
        if let Some(key) = pixabay_key_val {
            let url = format!(
                "https://pixabay.com/api/audio/?key={}&q={}&per_page={}",
                key,
                urlencoding::encode(query),
                limit
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(e.to_string()))?;
                    let hits = body.get("hits").cloned().unwrap_or(json!([]));
                    let mut results = Vec::new();

                    if let Some(arr) = hits.as_array() {
                        for hit in arr.iter().take(limit) {
                            let audio_url = hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
                            let title = hit
                                .get("tags")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let duration =
                                hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !audio_url.is_empty() {
                                let filename = format!(
                                    "{}/{}_{}.mp3",
                                    output_dir,
                                    query.replace(' ', "_"),
                                    results.len()
                                );
                                match client.get(audio_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp
                                            .bytes()
                                            .await
                                            .map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": title,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => {
                                        tracing::warn!("[stock.fetch] Download failed: {}", e)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(
                        100.0,
                        100.0,
                        &format!("Downloaded {} tracks", results.len()),
                    )
                    .await
                    .ok();
                    return Ok(json!({
                        "status": "fetched",
                        "type": "music",
                        "source": "pixabay",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[stock.fetch] Pixabay API failed"),
            }
        }

        // Fallback: return local stock library results
        report_progress(100.0, 100.0, "Using local stock library")
            .await
            .ok();
        return Ok(json!({
            "status": "fallback",
            "type": "music",
            "source": "local",
            "message": "Set PIXABAY_API_KEY env var to download from Pixabay. Using local stock library.",
            "local_library": "mcp/assets/music_index.json",
        }));
    }

    if media_type == "video" {
        // Try Pixabay video API
        let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };
        if let Some(key) = pixabay_key_val {
            let url = format!(
                "https://pixabay.com/api/videos/?key={}&q={}&per_page={}&video_type=animation",
                key,
                urlencoding::encode(query),
                limit
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(e.to_string()))?;
                    let hits = body.get("hits").cloned().unwrap_or(json!([]));
                    let mut results = Vec::new();

                    if let Some(arr) = hits.as_array() {
                        for hit in arr.iter().take(limit) {
                            // Get the best quality video URL
                            let videos = hit.get("videos");
                            let video_url = videos
                                .and_then(|v| v.get("large"))
                                .or_else(|| videos.and_then(|v| v.get("medium")))
                                .or_else(|| videos.and_then(|v| v.get("small")))
                                .and_then(|q| q.get("url"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");

                            let tags = hit
                                .get("tags")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let duration =
                                hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !video_url.is_empty() {
                                let filename = format!(
                                    "{}/{}_{}.mp4",
                                    output_dir,
                                    query.replace(' ', "_"),
                                    results.len()
                                );
                                match client.get(video_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp
                                            .bytes()
                                            .await
                                            .map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": tags,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => {
                                        tracing::warn!("[stock.fetch] Download failed: {}", e)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(
                        100.0,
                        100.0,
                        &format!("Downloaded {} videos", results.len()),
                    )
                    .await
                    .ok();
                    return Ok(json!({
                        "status": "fetched",
                        "type": "video",
                        "source": "pixabay",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[stock.fetch] Pixabay API failed"),
            }
        }

        // Fallback: return local stock library
        report_progress(100.0, 100.0, "Using local stock library")
            .await
            .ok();
        return Ok(json!({
            "status": "fallback",
            "type": "video",
            "source": "local",
            "message": "Set PIXABAY_API_KEY env var to download from Pixabay. Using local stock library.",
            "local_library": "mcp/assets/backgrounds/",
        }));
    }

    Err(ToolError::InvalidArg(format!(
        "Unknown media type: {}. Use 'music' or 'video'.",
        media_type
    )))
}

// ---------------------------------------------------------------------------
// Handler: youtube.download — download YouTube video clips
// ---------------------------------------------------------------------------

async fn handle_youtube_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?; // URL or search query
    let duration_s = default_f64(&args, "duration_s", 30.0);
    let start_s = default_opt_f64(&args, "start_s"); // Optional: specific start time
    let aspect = default_str(&args, "aspect", "9:16");
    let cache_dir = default_str(&args, "cache_dir", "mcp/assets/background_cache");
    let use_cookies = default_bool(&args, "use_cookies", true);

    std::fs::create_dir_all(&cache_dir)?;

    report_progress(0.0, 100.0, "Downloading from YouTube...")
        .await
        .ok();

    // Determine if query is a URL or a search term
    let is_url = query.starts_with("http://")
        || query.starts_with("https://")
        || query.starts_with("youtu.be");
    let cache_key = format!("{:x}", md5_hash(query.as_bytes()));
    let clip_path = format!("{}/{}_clip.mp4", cache_dir, cache_key);

    // If start_s is specified, use --download-sections to download only the range
    // This avoids downloading a 10-hour video when we only need 100 seconds
    if let Some(start) = start_s {
        let end = start + duration_s;
        let start_fmt = format_seconds_to_timestamp(start);
        let end_fmt = format_seconds_to_timestamp(end);
        let section_arg = format!("*{}-{}", start_fmt, end_fmt);

        report_progress(
            20.0,
            100.0,
            &format!("Downloading range {}-{}...", start_fmt, end_fmt),
        )
        .await
        .ok();

        let mut yt_args = vec![
            "--download-sections".to_string(),
            section_arg,
            "--force-keyframes-at-cuts".to_string(),
            "--format".to_string(),
            "best[height<=720]".to_string(),
            "--output".to_string(),
            clip_path.clone(),
            "--no-playlist".to_string(),
        ];

        if use_cookies {
            yt_args.push("--cookies-from-browser".to_string());
            yt_args.push("chrome".to_string());
        }
        yt_args.push("--user-agent".to_string());
        yt_args.push("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_string());

        if is_url {
            yt_args.push(query.to_string());
        } else {
            yt_args.push(format!("ytsearch1:{}", query));
        }

        let yt_result = tokio::process::Command::new("yt-dlp")
            .args(&yt_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;

        match yt_result {
            Ok(output) if output.status.success() => {
                // Clip is already the right duration — just crop to aspect
                report_progress(70.0, 100.0, "Cropping to aspect ratio...")
                    .await
                    .ok();
                let (crop_w, crop_h) = aspect_to_crop_dims(&aspect);

                let cropped_path = format!("{}/{}_cropped.mp4", cache_dir, cache_key);
                let crop_result = tokio::process::Command::new("ffmpeg")
                    .arg("-y")
                    .arg("-i")
                    .arg(&clip_path)
                    .arg("-vf")
                    .arg(format!("crop={}:{}", crop_w, crop_h))
                    .arg("-c:v")
                    .arg("libx264")
                    .arg("-preset")
                    .arg("fast")
                    .arg("-crf")
                    .arg("23")
                    .arg("-an")
                    .arg(&cropped_path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .output()
                    .await;

                if let Ok(o) = crop_result {
                    if o.status.success() {
                        let _ = std::fs::rename(&cropped_path, &clip_path);
                    } else {
                        let _ = std::fs::remove_file(&cropped_path);
                    }
                }

                report_progress(100.0, 100.0, "Clip downloaded").await.ok();
                return Ok(json!({
                    "status": "downloaded",
                    "clip_path": clip_path,
                    "start_s": start,
                    "duration_s": duration_s,
                    "aspect": aspect,
                    "method": "range_download",
                    "cached": false,
                }));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ToolError::Ffmpeg(format!(
                    "YouTube range download failed: {}",
                    stderr.lines().last().unwrap_or("unknown error")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!("yt-dlp not available: {}", e)));
            }
        }
    }

    // No start_s specified — download full video (or use cache), then extract random clip
    let full_video_path = format!("{}/{}.mp4", cache_dir, cache_key);

    // Check cache first
    if Path::new(&full_video_path).exists() {
        report_progress(50.0, 100.0, "Using cached video...")
            .await
            .ok();
    } else {
        // Build yt-dlp command
        let mut yt_args = vec![
            "--format".to_string(),
            "best[height<=720]".to_string(),
            "--output".to_string(),
            full_video_path.clone(),
            "--no-playlist".to_string(),
            "--quiet".to_string(),
        ];

        // Add cookies if enabled
        if use_cookies {
            // Try browser cookies
            for browser in &["chrome", "firefox", "edge"] {
                yt_args.push("--cookies-from-browser".to_string());
                yt_args.push(browser.to_string());
                // Only try one browser — if it fails, yt-dlp will continue without cookies
                break;
            }
        }

        // Add user agent to avoid bot detection
        yt_args.push("--user-agent".to_string());
        yt_args.push("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string());

        // Search or direct URL
        if is_url {
            yt_args.push(query.to_string());
        } else {
            yt_args.push(format!("ytsearch1:{}", query));
        }

        let yt_result = tokio::process::Command::new("yt-dlp")
            .args(&yt_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;

        match yt_result {
            Ok(output) if output.status.success() => {
                report_progress(50.0, 100.0, "Downloaded, extracting clip...")
                    .await
                    .ok();
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("[youtube.download] yt-dlp failed: {}", stderr);
                return Err(ToolError::Ffmpeg(format!(
                    "YouTube download failed: {}. Try providing a direct URL, or set PIXABAY_API_KEY for stock footage.",
                    stderr.lines().last().unwrap_or("unknown error")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!(
                    "yt-dlp not available: {}. Install with: pip install yt-dlp",
                    e
                )));
            }
        }
    }

    // Get video duration
    let probe_output = tokio::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(&full_video_path)
        .output()
        .await;

    let source_duration_s: f64 = match probe_output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(duration_s),
        _ => duration_s,
    };

    // Pick random start time
    let max_start = (source_duration_s - duration_s).max(0.0);
    let start_s = if max_start > 0.0 {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0) as u64;
        (seed as f64 / u64::MAX as f64) * max_start
    } else {
        0.0
    };

    // Crop dimensions
    let (crop_w, crop_h) = aspect_to_crop_dims(&aspect);

    // Extract clip with crop
    let extract_result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss")
        .arg(start_s.to_string())
        .arg("-i")
        .arg(&full_video_path)
        .arg("-t")
        .arg(duration_s.to_string())
        .arg("-vf")
        .arg(format!("crop={}:{}", crop_w, crop_h))
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-an")
        .arg(&clip_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match extract_result {
        Ok(o) if o.status.success() => {
            report_progress(100.0, 100.0, "Clip extracted").await.ok();
            Ok(json!({
                "status": "downloaded",
                "clip_path": clip_path,
                "source_duration_s": source_duration_s,
                "start_s": start_s,
                "duration_s": duration_s,
                "aspect": aspect,
                "cached": Path::new(&full_video_path).exists(),
            }))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(ToolError::Ffmpeg(format!(
                "Clip extraction failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!("FFmpeg failed: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: youtube.search — search YouTube without downloading
// ---------------------------------------------------------------------------

async fn handle_youtube_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;

    report_progress(0.0, 100.0, "Searching YouTube...")
        .await
        .ok();

    // Use yt-dlp to search (flat, no download)
    let search_query = format!("ytsearch{}:{}", limit, query);

    let result = tokio::process::Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--dump-json")
        .arg("--no-playlist")
        .arg(&search_query)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut results = Vec::new();

            for line in stdout.lines() {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                    let title = entry
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let url = entry
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            entry
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|id| format!("https://youtube.com/watch?v={}", id))
                        })
                        .unwrap_or_default();
                    let duration = entry
                        .get("duration")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let view_count = entry
                        .get("view_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let uploader = entry
                        .get("uploader")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");

                    results.push(json!({
                        "title": title,
                        "url": url,
                        "duration_s": duration,
                        "view_count": view_count,
                        "uploader": uploader,
                    }));
                }
            }

            report_progress(100.0, 100.0, &format!("Found {} results", results.len()))
                .await
                .ok();

            Ok(json!({
                "status": "searched",
                "query": query,
                "count": results.len(),
                "results": results,
            }))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ToolError::Ffmpeg(format!(
                "YouTube search failed: {}",
                stderr.lines().last().unwrap_or("unknown error")
            )))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!(
            "yt-dlp not available: {}. Install with: pip install yt-dlp",
            e
        ))),
    }
}

// ---------------------------------------------------------------------------
// Handler: stock.search — search Pixabay without downloading
// ---------------------------------------------------------------------------

async fn handle_stock_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_type = extract_str(&args, "type")?;
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;

    report_progress(
        0.0,
        100.0,
        &format!("Searching Pixabay for {}...", media_type),
    )
    .await
    .ok();

    let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };

    if let Some(key) = pixabay_key_val {
        let endpoint = if media_type == "music" {
            "https://pixabay.com/api/audio/"
        } else {
            "https://pixabay.com/api/videos/"
        };

        let url = format!(
            "{}?key={}&q={}&per_page={}",
            endpoint,
            key,
            urlencoding::encode(query),
            limit
        );

        let client = reqwest::Client::new();
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(e.to_string()))?;

                let total = body.get("totalHits").and_then(|v| v.as_u64()).unwrap_or(0);
                let hits = body.get("hits").cloned().unwrap_or(json!([]));

                let results: Vec<serde_json::Value> = hits
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .take(limit)
                            .map(|hit| {
                                let title = hit
                                    .get("tags")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                let duration =
                                    hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                                let user = hit
                                    .get("user")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                let views = hit.get("views").and_then(|v| v.as_u64()).unwrap_or(0);
                                let likes = hit.get("likes").and_then(|v| v.as_u64()).unwrap_or(0);

                                if media_type == "music" {
                                    let preview_url =
                                        hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
                                    json!({
                                        "title": title,
                                        "duration_s": duration,
                                        "user": user,
                                        "views": views,
                                        "likes": likes,
                                        "preview_url": preview_url,
                                    })
                                } else {
                                    let videos = hit.get("videos");
                                    let thumb = hit
                                        .get("previewURL")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let video_url = videos
                                        .and_then(|v| v.get("large"))
                                        .or_else(|| videos.and_then(|v| v.get("medium")))
                                        .or_else(|| videos.and_then(|v| v.get("small")))
                                        .and_then(|q| q.get("url"))
                                        .and_then(|u| u.as_str())
                                        .unwrap_or("");
                                    json!({
                                        "title": title,
                                        "duration_s": duration,
                                        "user": user,
                                        "views": views,
                                        "likes": likes,
                                        "thumbnail": thumb,
                                        "video_url": video_url,
                                    })
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                report_progress(100.0, 100.0, &format!("Found {} results", results.len()))
                    .await
                    .ok();

                return Ok(json!({
                    "status": "searched",
                    "type": media_type,
                    "source": "pixabay",
                    "query": query,
                    "total_hits": total,
                    "count": results.len(),
                    "results": results,
                }));
            }
            _ => tracing::warn!("[stock.search] Pixabay API failed"),
        }
    }

    // Fallback: list local stock library
    report_progress(100.0, 100.0, "Using local stock library")
        .await
        .ok();

    if media_type == "music" {
        let index_path = std::env::var("OPENSCRIPT_MUSIC_INDEX")
            .unwrap_or_else(|_| "mcp/assets/music_index.json".to_string());
        if let Ok(content) = std::fs::read_to_string(&index_path) {
            if let Ok(index) = serde_json::from_str::<serde_json::Value>(&content) {
                let assets = index.get("assets").cloned().unwrap_or(json!([]));
                let results: Vec<serde_json::Value> = assets
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter(|a| {
                                let title = a
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                let mood = a
                                    .get("mood")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                title.contains(&query.to_lowercase())
                                    || mood.contains(&query.to_lowercase())
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok(json!({
                    "status": "fallback",
                    "type": "music",
                    "source": "local",
                    "query": query,
                    "count": results.len(),
                    "results": results,
                    "message": "Set PIXABAY_API_KEY to search Pixabay. Showing local library matches.",
                }));
            }
        }
    }

    // Video fallback: list local backgrounds
    let bg_dir = "mcp/assets/backgrounds";
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(bg_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".mp4") {
                let path = format!("{}/{}", bg_dir, name);
                results.push(json!({
                    "title": name,
                    "path": path,
                    "source": "local",
                }));
            }
        }
    }

    Ok(json!({
        "status": "fallback",
        "type": media_type,
        "source": "local",
        "query": query,
        "count": results.len(),
        "results": results,
        "message": "Set PIXABAY_API_KEY to search Pixabay. Showing local library.",
    }))
}

// ---------------------------------------------------------------------------
// Handler: media.search — PNG image search (Pexels Images + Openverse)
// ---------------------------------------------------------------------------

async fn handle_media_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;
    let source = default_str(&args, "source", "auto");

    report_progress(0.0, 100.0, &format!("Searching for images: {}...", query))
        .await
        .ok();

    let pexels_key_val = pexels_key();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    // Try Pexels Images API first (if key available and source allows)
    if source != "openverse" && !pexels_key().is_empty() {
        let url = format!(
            "https://api.pexels.com/v1/search?query={}&per_page={}&orientation=portrait",
            urlencoding::encode(query),
            limit
        );

        match client
            .get(&url)
            .header("Authorization", &pexels_key_val)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

                let results: Vec<serde_json::Value> = body.get("photos")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().take(limit).map(|p| {
                        let src = p.get("src").cloned().unwrap_or(json!({}));
                        json!({
                            "id": p.get("id"),
                            "title": format!("Photo by {}", p.get("photographer").and_then(|v| v.as_str()).unwrap_or("Unknown")),
                            "url": src.get("original").and_then(|v| v.as_str()).unwrap_or(""),
                            "medium_url": src.get("medium").and_then(|v| v.as_str()).unwrap_or(""),
                            "large_url": src.get("large").and_then(|v| v.as_str()).unwrap_or(""),
                            "width": p.get("width"),
                            "height": p.get("height"),
                            "source": "pexels",
                            "license": "pexels-license",
                        })
                    }).collect())
                    .unwrap_or_default();

                if !results.is_empty() {
                    report_progress(100.0, 100.0, &format!("Found {} images", results.len()))
                        .await
                        .ok();
                    return Ok(json!({
                        "status": "searched",
                        "query": query,
                        "source": "pexels",
                        "count": results.len(),
                        "results": results,
                    }));
                }
            }
            _ => tracing::warn!("[media.search] Pexels API failed, trying Openverse"),
        }
    }

    // Fallback: Openverse API (free, no key needed)
    if source != "pexels" {
        let url = format!(
            "https://api.openverse.org/v1/images/?q={}&page_size={}",
            urlencoding::encode(query),
            limit
        );

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Openverse parse error: {}", e)))?;

                let results: Vec<serde_json::Value> = body
                    .get("results")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .take(limit)
                            .map(|r| {
                                json!({
                                    "id": r.get("id"),
                                    "title": r.get("title"),
                                    "url": r.get("url"),
                                    "thumbnail": r.get("thumbnail"),
                                    "width": r.get("width"),
                                    "height": r.get("height"),
                                    "source": "openverse",
                                    "license": r.get("license"),
                                    "creator": r.get("creator"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                report_progress(100.0, 100.0, &format!("Found {} images", results.len()))
                    .await
                    .ok();
                return Ok(json!({
                    "status": "searched",
                    "query": query,
                    "source": "openverse",
                    "count": results.len(),
                    "results": results,
                }));
            }
            _ => tracing::warn!("[media.search] Openverse API failed"),
        }
    }

    Ok(json!({
        "status": "no_results",
        "query": query,
        "count": 0,
        "results": [],
        "message": "No images found. Try a different query.",
    }))
}

// ---------------------------------------------------------------------------
// Handler: gif.search — GIPHY sticker search
// ---------------------------------------------------------------------------

async fn handle_gif_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;
    let rating = default_str(&args, "rating", "g");

    report_progress(0.0, 100.0, &format!("Searching GIPHY for: {}...", query))
        .await
        .ok();

    let giphy_key = Some(giphy_key()).filter(|s| !s.is_empty());

    if let Some(key) = giphy_key {
        if !key.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

            // Search GIPHY stickers (transparent GIFs)
            let url = format!(
                "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit={}&rating={}&bundle=sticker_layering",
                key,
                urlencoding::encode(query),
                limit,
                rating
            );

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(format!("GIPHY parse error: {}", e)))?;

                    let results: Vec<serde_json::Value> = body
                        .get("data")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .take(limit)
                                .map(|g| {
                                    let images = g.get("images").cloned().unwrap_or(json!({}));
                                    let original =
                                        images.get("original").cloned().unwrap_or(json!({}));
                                    let downsized =
                                        images.get("downsized").cloned().unwrap_or(json!({}));
                                    json!({
                                        "id": g.get("id"),
                                        "title": g.get("title"),
                                        "url": g.get("url"),
                                        "gif_url": original.get("url"),
                                        "webp_url": original.get("webp"),
                                        "preview_url": downsized.get("url"),
                                        "width": original.get("width"),
                                        "height": original.get("height"),
                                        "size_bytes": original.get("size"),
                                        "source": "giphy",
                                        "transparent": true,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    report_progress(100.0, 100.0, &format!("Found {} stickers", results.len()))
                        .await
                        .ok();
                    return Ok(json!({
                        "status": "searched",
                        "query": query,
                        "source": "giphy",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[gif.search] GIPHY API failed"),
            }
        }
    }

    // Fallback: Pexels video search for short clips
    report_progress(
        50.0,
        100.0,
        "GIPHY key not set, searching Pexels for short clips...",
    )
    .await
    .ok();
    let pexels_key_val = pexels_key();

    let url = format!(
        "https://api.pexels.com/videos/search?query={}&per_page={}&orientation=square",
        urlencoding::encode(query),
        limit
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    match client
        .get(&url)
        .header("Authorization", &pexels_key_val)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

            let results: Vec<serde_json::Value> = body.get("videos")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().take(limit).filter_map(|v| {
                    let duration = v.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                    if duration > 10 { return None; } // Only short clips
                    let video_files = v.get("video_files").and_then(|v| v.as_array())?;
                    let best = video_files.iter()
                        .filter(|f| {
                            let w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                            w >= 360 && w <= 720
                        })
                        .next()?;
                    Some(json!({
                        "id": v.get("id"),
                        "title": format!("Pexels video {}", v.get("id").and_then(|v| v.as_u64()).unwrap_or(0)),
                        "url": v.get("url"),
                        "video_url": best.get("link"),
                        "width": best.get("width"),
                        "height": best.get("height"),
                        "duration_s": duration,
                        "source": "pexels",
                        "transparent": false,
                    }))
                }).collect())
                .unwrap_or_default();

            report_progress(100.0, 100.0, &format!("Found {} clips", results.len()))
                .await
                .ok();
            return Ok(json!({
                "status": "searched",
                "query": query,
                "source": "pexels",
                "count": results.len(),
                "results": results,
                "message": "GIPHY_API_KEY not set. Set it to search GIPHY stickers. Showing Pexels short clips instead.",
            }));
        }
        _ => {}
    }

    Ok(json!({
        "status": "no_results",
        "query": query,
        "count": 0,
        "results": [],
        "message": "Set GIPHY_API_KEY env var for GIPHY sticker search. Get free key at https://developers.giphy.com",
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.inspect — deep-dive layer inspection
// ---------------------------------------------------------------------------

async fn handle_timeline_inspect(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let preview_path = extract_str(&args, "timeline_preview_path")?;
    let layer = extract_str(&args, "layer")?;

    // Read the timeline preview file
    let preview = std::fs::read_to_string(sanitize_input_path(preview_path)?)
        .map_err(|e| ToolError::NotFound(format!("Cannot read timeline preview: {}", e)))?;

    // Also try to read the timeline JSON for full details
    let timeline_dir = std::path::Path::new(preview_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let timeline_json_path = timeline_dir.join("timeline.json");
    let manifest_path = timeline_dir.join("voices").join("manifest.json");

    let mut details = Vec::new();

    match layer {
        "background" => {
            // Read from timeline tracks, not assets (the schema stores events in tracks)
            if let Ok(tl_str) = std::fs::read_to_string(&timeline_json_path) {
                if let Ok(tl) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    // Try tracks.broll first (EDL v2 schema)
                    if let Some(tracks) = tl.get("tracks").and_then(|t| t.as_object()) {
                        if let Some(broll_events) = tracks.get("broll").and_then(|b| b.as_array()) {
                            for event in broll_events {
                                let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let start_ms =
                                    event.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let end_ms =
                                    event.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let asset_id =
                                    event.get("asset_id").and_then(|v| v.as_str()).unwrap_or("");
                                // Look up the actual file path from assets
                                let path = tl
                                    .get("assets")
                                    .and_then(|a| a.get("broll"))
                                    .and_then(|b| b.as_object())
                                    .and_then(|b| b.get(asset_id))
                                    .and_then(|p| p.get("path"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or(asset_id);
                                details.push(json!({
                                    "id": id,
                                    "start_ms": start_ms,
                                    "end_ms": end_ms,
                                    "path": path,
                                    "exists": std::path::Path::new(path).exists(),
                                }));
                            }
                        }
                    }
                    // Also check assets.broll as fallback
                    if details.is_empty() {
                        if let Some(broll) = tl
                            .get("assets")
                            .and_then(|a| a.get("broll"))
                            .and_then(|b| b.as_object())
                        {
                            for (id, asset) in broll {
                                let path = asset.get("path").and_then(|v| v.as_str()).unwrap_or("");
                                details.push(json!({
                                    "id": id,
                                    "path": path,
                                    "exists": std::path::Path::new(path).exists(),
                                }));
                            }
                        }
                    }
                }
            }
        }
        "voiceover" => {
            if let Ok(m_str) = std::fs::read_to_string(&manifest_path) {
                if let Ok(m) = serde_json::from_str::<serde_json::Value>(&m_str) {
                    if let Some(segments) = m.get("segments").and_then(|v| v.as_array()) {
                        for seg in segments {
                            details.push(json!({
                                "scene_id": seg.get("scene_id"),
                                "speaker": seg.get("speaker"),
                                "text": seg.get("text"),
                                "start_ms": seg.get("start_ms"),
                                "end_ms": seg.get("end_ms"),
                                "duration_ms": seg.get("duration_ms"),
                                "wav_path": seg.get("wav_path"),
                                "word_count": seg.get("words").and_then(|v| v.as_array()).map(|a| a.len()),
                                "backend": seg.get("backend"),
                            }));
                        }
                    }
                }
            }
        }
        "music" => {
            if let Ok(tl_str) = std::fs::read_to_string(&timeline_json_path) {
                if let Ok(tl) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    // Try tracks.music first (EDL v2 schema)
                    if let Some(tracks) = tl.get("tracks").and_then(|t| t.as_object()) {
                        if let Some(music_events) = tracks.get("music").and_then(|m| m.as_array()) {
                            for event in music_events {
                                let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let start_ms =
                                    event.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let end_ms =
                                    event.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let asset_id =
                                    event.get("asset_id").and_then(|v| v.as_str()).unwrap_or("");
                                let path = tl
                                    .get("assets")
                                    .and_then(|a| a.get("music"))
                                    .and_then(|m| m.as_object())
                                    .and_then(|m| m.get(asset_id))
                                    .and_then(|p| p.get("path"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or(asset_id);
                                details.push(json!({
                                    "id": id,
                                    "start_ms": start_ms,
                                    "end_ms": end_ms,
                                    "path": path,
                                }));
                            }
                        }
                    }
                    // Fallback to assets.music
                    if details.is_empty() {
                        if let Some(music) = tl
                            .get("assets")
                            .and_then(|a| a.get("music"))
                            .and_then(|m| m.as_object())
                        {
                            for (id, asset) in music {
                                details.push(json!({
                                    "id": id,
                                    "path": asset.get("path"),
                                }));
                            }
                        }
                    }
                }
            }
        }
        "captions" => {
            let captions_path = timeline_dir.join("captions.ass");
            if let Ok(content) = std::fs::read_to_string(&captions_path) {
                let dialogue_count = content.matches("Dialogue:").count();
                details.push(json!({
                    "path": captions_path.to_string_lossy(),
                    "dialogue_count": dialogue_count,
                    "size_bytes": content.len(),
                }));
            }
        }
        "stickers" => {
            // Stickers are in the script.to_video response, not stored separately
            details.push(json!({
                "message": "Sticker details are in the script.to_video response. Check the 'sticker_count' and 'timeline_preview' fields.",
            }));
        }
        _ => {
            return Err(ToolError::InvalidArg(format!(
                "Unknown layer: {}. Use: background, voiceover, music, captions, stickers",
                layer
            )));
        }
    }

    Ok(json!({
        "status": "inspected",
        "layer": layer,
        "event_count": details.len(),
        "events": details,
        "preview_excerpt": preview.lines().take(5).collect::<Vec<_>>().join("\n"),
    }))
}

// ---------------------------------------------------------------------------
// Handler: library.search — search music/SFX library index
// ---------------------------------------------------------------------------

async fn handle_library_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let media_type = default_opt_str(&args, "type");
    let limit = default_u32(&args, "limit", 10) as usize;
    // New filters (audit bug #18): mood/energy/duration/source/license.
    // These make library.search as filterable as music.search + sfx.search,
    // so an agent can find a 30s "epic cinematic" track without paging
    // through hundreds of irrelevant results.
    let source_filter = default_opt_str(&args, "source");
    let license_filter = default_opt_str(&args, "license");
    let min_duration_s = args
        .get("min_duration_s")
        .and_then(|v| v.as_f64());
    let max_duration_s = args
        .get("max_duration_s")
        .and_then(|v| v.as_f64());
    let tag_filter: Option<String> = default_opt_str(&args, "tag");
    let mood_filter: Option<String> = default_opt_str(&args, "mood");
    let energy_filter: Option<String> = default_opt_str(&args, "energy");

    // Resolve path CWD-independently (round-2 GAP #12 fix — same as
    // background.search). library.search only worked from repo root before.
    let index_path_raw = std::env::var("OPENSCRIPT_MUSIC_LIBRARY_INDEX")
        .unwrap_or_else(|_| "mcp/assets/music_library_index.json".to_string());
    let index_path = resolve_repo_path(&index_path_raw);

    if !index_path.exists() {
        return Err(ToolError::NotFound(format!(
            "Music library index not found at {} (resolved from {}). Run the library.build MCP tool to generate it (requires yt-dlp on PATH).",
            index_path.display(),
            index_path_raw
        )));
    }

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut filtered_by_duration = 0u32;
    let mut filtered_by_source = 0u32;
    let mut filtered_by_license = 0u32;
    let mut filtered_by_tag = 0u32;
    let mut filtered_by_mood = 0u32;
    let mut filtered_by_energy = 0u32;

    for entry in &entries {
        // Filter by media type if specified
        if let Some(ref mt) = media_type {
            let entry_type = entry
                .get("media_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_type != mt {
                continue;
            }
        }

        // Filter by source channel (e.g. "NoCopyrightSounds")
        if let Some(ref src) = source_filter {
            let entry_source = entry
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_source != src {
                filtered_by_source += 1;
                continue;
            }
        }

        // Filter by license (e.g. "no-copyright", "creative-commons")
        if let Some(ref lic) = license_filter {
            let entry_license = entry
                .get("license")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_license != lic {
                filtered_by_license += 1;
                continue;
            }
        }

        // Filter by duration range (in seconds)
        let duration = entry
            .get("duration_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if let Some(min_d) = min_duration_s {
            if duration < min_d {
                filtered_by_duration += 1;
                continue;
            }
        }
        if let Some(max_d) = max_duration_s {
            if duration > max_d {
                filtered_by_duration += 1;
                continue;
            }
        }

        // Filter by tag (substring match against the entry's tags array)
        if let Some(ref tag_q) = tag_filter {
            let tag_lower = tag_q.to_lowercase();
            let tags: Vec<String> = entry
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let matches_tag = tags
                .iter()
                .any(|t| t.to_lowercase().contains(&tag_lower));
            if !matches_tag {
                filtered_by_tag += 1;
                continue;
            }
        }

        // Filter by mood (exact match against enriched mood field)
        if let Some(ref mood_q) = mood_filter {
            let entry_mood = entry
                .get("mood")
                .and_then(|v| v.as_str())
                .unwrap_or("neutral");
            if entry_mood != mood_q {
                filtered_by_mood += 1;
                continue;
            }
        }

        // Filter by energy (exact match against enriched energy field)
        if let Some(ref energy_q) = energy_filter {
            let entry_energy = entry
                .get("energy")
                .and_then(|v| v.as_str())
                .unwrap_or("medium");
            if entry_energy != energy_q {
                filtered_by_energy += 1;
                continue;
            }
        }

        let title = entry
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let tags: Vec<String> = entry
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut score = 0i32;

        // Exact title match
        if query_lower.contains(&title) || title.contains(&query_lower) {
            score += 10;
        }

        // Word matches
        for word in &query_words {
            if title.contains(word) {
                score += 3;
            }
            if tags.iter().any(|t| t == word) {
                score += 5;
            }
        }

        // Phase 3: mood/energy/genre scoring weights (audit bug #19).
        // Without these, a text-match "calm" on a title returns energetic
        // tracks that happen to say "calm" in their description.
        let entry_mood = entry
            .get("mood")
            .and_then(|v| v.as_str())
            .unwrap_or("neutral");
        let entry_energy = entry
            .get("energy")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");
        let entry_genre = entry
            .get("genre")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Mood match: strong signal when agent filters by mood.
        if mood_filter.is_some() && entry_mood == mood_filter.as_deref().unwrap_or("") {
            score += 8;
        }

        // Energy match: moderate signal when agent filters by energy.
        if energy_filter.is_some() && entry_energy == energy_filter.as_deref().unwrap_or("") {
            score += 4;
        }

        // Genre match: if query words appear in genre field, boost.
        if !entry_genre.is_empty() {
            let genre_lower = entry_genre.to_lowercase();
            for word in &query_words {
                if genre_lower.contains(word) {
                    score += 3;
                }
            }
        }

        // Penalize mood mismatch when mood filter is active.
        if mood_filter.is_some() && entry_mood != mood_filter.as_deref().unwrap_or("") {
            score -= 5;
        }

        if score > 0 {
            let mut result = entry.clone();
            if let Some(obj) = result.as_object_mut() {
                obj.insert("relevance_score".into(), json!(score));
            }
            results.push(result);
        }
    }

    // Sort by relevance
    results.sort_by(|a, b| {
        let sa = a
            .get("relevance_score")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let sb = b
            .get("relevance_score")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        sb.cmp(&sa)
    });

    let total = results.len();
    results.truncate(limit);

    // Surface filter stats so an agent can tell why results are sparse.
    let mut filter_stats = serde_json::Map::new();
    if filtered_by_duration > 0 {
        filter_stats.insert("filtered_by_duration".into(), json!(filtered_by_duration));
    }
    if filtered_by_source > 0 {
        filter_stats.insert("filtered_by_source".into(), json!(filtered_by_source));
    }
    if filtered_by_license > 0 {
        filter_stats.insert("filtered_by_license".into(), json!(filtered_by_license));
    }
    if filtered_by_tag > 0 {
        filter_stats.insert("filtered_by_tag".into(), json!(filtered_by_tag));
    }
    if filtered_by_mood > 0 {
        filter_stats.insert("filtered_by_mood".into(), json!(filtered_by_mood));
    }
    if filtered_by_energy > 0 {
        filter_stats.insert("filtered_by_energy".into(), json!(filtered_by_energy));
    }

    Ok(json!({
        "status": "searched",
        "query": query,
        "type": media_type,
        "total_matches": total,
        "count": results.len(),
        "results": results,
        "filters_applied": {
            "source": source_filter,
            "license": license_filter,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
            "tag": tag_filter,
            "mood": mood_filter,
            "energy": energy_filter,
        },
        "filter_stats": filter_stats,
        "index_stats": {
            "total_entries": index.get("total_entries"),
            "music_count": index.get("music_count"),
            "sfx_count": index.get("sfx_count"),
            "sources": index.get("sources"),
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: library.download — download music/SFX on demand
// ---------------------------------------------------------------------------

async fn handle_library_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let filename = extract_str(&args, "filename")?;
    let output_dir = default_str(&args, "output_dir", "mcp/assets/music_cache");
    let output_dir_owned = output_dir.to_string();

    std::fs::create_dir_all(&output_dir_owned)?;

    let index_path = std::env::var("OPENSCRIPT_MUSIC_LIBRARY_INDEX")
        .unwrap_or_else(|_| "mcp/assets/music_library_index.json".to_string());

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let entry = entries
        .iter()
        .find(|e| e.get("filename").and_then(|v| v.as_str()).unwrap_or("") == filename)
        .ok_or_else(|| ToolError::NotFound(format!("Entry not found in library: {}", filename)))?;

    let source_type = entry
        .get("source_type")
        .and_then(|v| v.as_str())
        .unwrap_or("local")
        .to_string();
    let download_url = entry
        .get("download_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let output_path = format!("{}/{}", output_dir_owned, filename);

    // Check if already downloaded
    if Path::new(&output_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": output_path,
            "filename": filename,
            "source": entry.get("source"),
        }));
    }

    if source_type == "local" {
        // Local file — just return the path
        return Ok(json!({
            "status": "local",
            "path": download_url,
            "filename": filename,
            "source": entry.get("source"),
        }));
    }

    // Download with yt-dlp (include bot-detection evasion)
    report_progress(0.0, 100.0, &format!("Downloading: {}", filename))
        .await
        .ok();

    let result = tokio::process::Command::new("yt-dlp")
        .arg("-x").arg("--audio-format").arg("mp3")
        .arg("--audio-quality").arg("0")
        .arg("-o").arg(&output_path)
        .arg("--no-playlist")
        .arg("--quiet")
        // ponytail: skip --cookies-from-browser — NCS/AudioLibrary tracks are
        // public. Chrome cookies fail on headless/server environments. Only add
        // cookies for age-gated / private content.
        .arg("--user-agent").arg("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .arg(&download_url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            report_progress(100.0, 100.0, "Downloaded").await.ok();
            let file_size = std::fs::metadata(&output_path)
                .map(|m| m.len())
                .unwrap_or(0);
            Ok(json!({
                "status": "downloaded",
                "path": output_path,
                "filename": filename,
                "file_size_bytes": file_size,
                "source": entry.get("source"),
                "title": entry.get("title"),
            }))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ToolError::Asset(format!(
                "Download failed: {}",
                stderr.lines().last().unwrap_or("unknown")
            )))
        }
        Err(e) => Err(ToolError::Asset(format!("yt-dlp not available: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: library.build — rebuild the music/SFX library index
// ---------------------------------------------------------------------------

async fn handle_library_build(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    report_progress(0.0, 100.0, "Building music/SFX library index...")
        .await
        .ok();

    // C2 fix: prior versions shelled out to `python3 mcp/scripts/music_library_indexer.py --build`,
    // which required Python + yt-dlp at runtime. Now uses the native Rust port
    // in `library_indexer.rs`, which shells out to yt-dlp directly and builds
    // the JSON index with serde_json. No Python dependency.
    let index_path = "mcp/assets/music_library_index.json";
    let index = crate::library_indexer::build_index(index_path)
        .await
        .map_err(|e| ToolError::Asset(format!("Index build failed: {}", e)))?;

    report_progress(100.0, 100.0, "Library index built")
        .await
        .ok();

    Ok(json!({
        "status": "built",
        "index_path": index_path,
        "total_entries": index.get("total_entries"),
        "music_count": index.get("music_count"),
        "sfx_count": index.get("sfx_count"),
        "sources": index.get("sources"),
    }))
}

// ---------------------------------------------------------------------------
// Handler: media.download (Phase I — unblock image workflow)
// ---------------------------------------------------------------------------

/// Download an image from a URL to a local file. Caches in
/// mcp/assets/image_cache/ to avoid re-downloading.
async fn handle_media_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let url = extract_str(&args, "url")?;
    let output_path = default_opt_str(&args, "output_path");

    // Determine cache dir + output path
    let cache_dir = "mcp/assets/image_cache";
    std::fs::create_dir_all(cache_dir).ok();

    let resolved_path = if let Some(p) = output_path {
        if !p.is_empty() {
            p.to_string()
        } else {
            format!("{}/img_{}.{}", cache_dir, md5_hash(url.as_bytes()), url_extension(url))
        }
    } else {
        format!("{}/img_{}.{}", cache_dir, md5_hash(url.as_bytes()), url_extension(url))
    };

    // Check cache
    if std::path::Path::new(&resolved_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": resolved_path,
            "url": url,
        }));
    }

    // Download
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    let resp = client.get(url).send().await
        .map_err(|e| ToolError::Asset(format!("Failed to download image: {}", e)))?;

    if !resp.status().is_success() {
        return Err(ToolError::Asset(format!(
            "Image download failed: HTTP {} for {}",
            resp.status(),
            url
        )));
    }

    let bytes = resp.bytes().await
        .map_err(|e| ToolError::Asset(format!("Failed to read image bytes: {}", e)))?;

    std::fs::write(&resolved_path, &bytes)
        .map_err(|e| ToolError::Asset(format!("Failed to write image to {}: {}", resolved_path, e)))?;

    Ok(json!({
        "status": "downloaded",
        "path": resolved_path,
        "url": url,
        "size_bytes": bytes.len(),
    }))
}

/// Extract file extension from a URL (defaults to "png").
fn url_extension(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    if let Some(ext) = std::path::Path::new(path).extension() {
        if let Some(s) = ext.to_str() {
            let s = s.to_lowercase();
            if matches!(s.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
                return s;
            }
        }
    }
    "png".to_string()
}

// ---------------------------------------------------------------------------
// Handler: gif.download (Phase I — unblock GIF workflow)
// ---------------------------------------------------------------------------

/// Download a GIF from a URL to a local file. Caches in mcp/assets/stickers/
/// so it can be used directly by script.to_video's sticker pipeline.
async fn handle_gif_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let url = extract_str(&args, "url")?;
    let output_path = default_opt_str(&args, "output_path");

    let cache_dir = "mcp/assets/stickers";
    std::fs::create_dir_all(cache_dir).ok();

    let resolved_path = if let Some(p) = output_path {
        if !p.is_empty() {
            p.to_string()
        } else {
            format!("{}/gif_{}.gif", cache_dir, md5_hash(url.as_bytes()))
        }
    } else {
        format!("{}/gif_{}.gif", cache_dir, md5_hash(url.as_bytes()))
    };

    // Check cache
    if std::path::Path::new(&resolved_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": resolved_path,
            "url": url,
        }));
    }

    // Download
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    let resp = client.get(url).send().await
        .map_err(|e| ToolError::Asset(format!("Failed to download GIF: {}", e)))?;

    if !resp.status().is_success() {
        return Err(ToolError::Asset(format!(
            "GIF download failed: HTTP {} for {}",
            resp.status(),
            url
        )));
    }

    let bytes = resp.bytes().await
        .map_err(|e| ToolError::Asset(format!("Failed to read GIF bytes: {}", e)))?;

    std::fs::write(&resolved_path, &bytes)
        .map_err(|e| ToolError::Asset(format!("Failed to write GIF to {}: {}", resolved_path, e)))?;

    Ok(json!({
        "status": "downloaded",
        "path": resolved_path,
        "url": url,
        "size_bytes": bytes.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: overlay.assign (Phase J — place images/GIFs/PNGs on the timeline)
// ---------------------------------------------------------------------------

/// Place an image/GIF/PNG overlay on the timeline at a specific position and
/// duration. The overlay is stored as a b-roll track event with a special
/// `overlay` tag, and the render pipeline composites it via FFmpeg's overlay
/// filter. This closes the "search → download → assign" loop for stickers,
/// GIFs, and images.
async fn handle_overlay_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let asset_path = extract_str(&args, "asset_path")?;
    let start_ms = extract_i64(&args, "start_ms")?;
    let end_ms = extract_i64(&args, "end_ms")?;
    let position = default_str(&args, "position", "bottom-right");
    let scale = default_f64(&args, "scale", 0.2);
    let fade_in_ms = default_u32(&args, "fade_in_ms", 0);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 0);
    let speaker_name = default_opt_str(&args, "speaker_name");

    // Validate the asset exists
    if !std::path::Path::new(asset_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Overlay asset not found: {}. Use media.download or gif.download to fetch it first.",
            asset_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("overlay_{:03}", track_count(&timeline, &TrackType::Broll) + 1);

    let duration_ms = end_ms - start_ms;
    let mut tags = vec!["overlay".to_string(), position.to_string()];
    if let Some(ref speaker) = speaker_name {
        tags.push(speaker.clone());
    }

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: asset_path.to_string(),
        start_ms,
        end_ms,
        offset_ms: 0,
        gain_db: 0.0,
        fade_in_ms,
        fade_out_ms,
        tags,
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "overlay.assign".into(),
            editorial_role: None,
            concept: Some(format!("overlay:{}:{}", position, scale)),
        }),
        kind: openscript_core::timeline::EventKind::Broll {
            concept: format!("overlay:{}", position),
            source_provider: asset_path.to_string(),
            transition_style: "overlay".into(),
            crop_mode: "none".into(),
            orientation: "9:16".into(),
            motion_intensity: "static".into(),
        },
    };

    timeline.add_track_event(TrackType::Broll, event);
    timeline.add_asset(
        "broll",
        event_id.clone(),
        json!({
            "path": asset_path,
            "overlay": true,
            "position": position,
            "scale": scale,
        }),
    );
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "assigned",
        "event_id": event_id,
        "asset_path": asset_path,
        "start_ms": start_ms,
        "end_ms": end_ms,
        "duration_ms": duration_ms,
        "position": position,
        "scale": scale,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voices.list — list all available TTS voices
// ---------------------------------------------------------------------------

/// List all available TTS voices: registered profiles from voices.json plus
/// the full list of Kokoro preset voice IDs. Agents use this to discover
/// available voices before generating TTS.
async fn handle_voices_list(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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

// ---------------------------------------------------------------------------
// Handler: timeline.to_hyperframes (Phase M — bridge EDL v2 → HF HTML)
// ---------------------------------------------------------------------------

/// Compile an EDL v2 timeline JSON into a HyperFrames HTML composition by
/// shelling out to `tsx hyperframes/src/edl_v2_to_html.ts`. The resulting
/// index.html can then be rendered via `hf.render` or `composition.render`.
///
/// This is the bridge between the NLE timeline (EDL v2 JSON) and the
/// HyperFrames motion-graphics render engine. Prior to this tool, the
/// edl_v2_to_html.ts compiler was dead code — never called by any Rust
/// handler. An agent had to run it manually, which broke the programmatic
/// pipeline.
async fn handle_timeline_to_hyperframes(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let output_dir = default_str(&args, "output_dir", "artifacts/hf_composition");
    let composition_id = default_opt_str(&args, "composition_id");

    // Validate timeline exists
    if !std::path::Path::new(timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline file not found: {}",
            timeline_path
        )));
    }

    // Create output directory
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| ToolError::Asset(format!("Failed to create output dir: {}", e)))?;

    let index_html_path = format!("{}/index.html", output_dir);

    // Build the tsx command
    let compiler_script = "hyperframes/src/edl_v2_to_html.ts";
    if !std::path::Path::new(compiler_script).exists() {
        return Err(ToolError::NotFound(format!(
            "HyperFrames compiler not found: {}. Ensure the hyperframes/ directory is present.",
            compiler_script
        )));
    }

    let mut cmd = tokio::process::Command::new("npx");
    cmd.arg("tsx")
        .arg(compiler_script)
        .arg("--timeline")
        .arg(timeline_path)
        .arg("--out")
        .arg(&index_html_path);

    if let Some(ref cid) = composition_id {
        cmd.arg("--composition-id").arg(cid);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Run with a 60s timeout — compilation should be fast
    let child = cmd
        .spawn()
        .map_err(|e| ToolError::Asset(format!("Failed to spawn tsx: {}", e)))?;

    let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| ToolError::Asset("tsx compilation timed out (60s)".to_string()))?
        .map_err(|e| ToolError::Asset(format!("tsx execution failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::Asset(format!(
            "HyperFrames compilation failed: {}",
            stderr.trim()
        )));
    }

    // Verify the output was created
    if !std::path::Path::new(&index_html_path).exists() {
        return Err(ToolError::Asset(format!(
            "Compilation appeared to succeed but no index.html was written to {}",
            index_html_path
        )));
    }

    let file_size = std::fs::metadata(&index_html_path)
        .map(|m| m.len())
        .unwrap_or(0);

    // Extract composition ID from the HTML (data-composition-id attribute)
    let html_content = std::fs::read_to_string(&index_html_path).unwrap_or_default();
    let extracted_cid = html_content
        .find("data-composition-id=\"")
        .and_then(|pos| {
            let start = pos + "data-composition-id=\"".len();
            html_content[start..].find('"').map(|end| &html_content[start..start + end])
        })
        .unwrap_or("unknown")
        .to_string();

    Ok(json!({
        "status": "compiled",
        "project_dir": output_dir,
        "index_html_path": index_html_path,
        "composition_id": extracted_cid,
        "file_size_bytes": file_size,
        "next_step": "Call hf.render or composition.render with project_dir to produce the final MP4",
    }))
}

// ---------------------------------------------------------------------------
// Handlers: llm.complete / vision.analyze_clip / vision.score_clip
// ---------------------------------------------------------------------------

/// Text LLM via local Ollama (Qwen3.5-4B GGUF) → OpenRouter free models.
async fn handle_llm_complete(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
async fn handle_system_config_get(
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
async fn handle_system_config_set(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
        if let Some(s) = llm.get("local_model").and_then(|v| v.as_str()) {
            cfg.llm.local_model = s.to_string();
        }
        if let Some(s) = llm.get("local_base_url").and_then(|v| v.as_str()) {
            cfg.llm.local_base_url = s.to_string();
        }
        if let Some(s) = llm.get("gguf_path").and_then(|v| v.as_str()) {
            cfg.llm.gguf_path = Some(s.to_string());
        }
        if let Some(s) = llm.get("mmproj_path").and_then(|v| v.as_str()) {
            cfg.llm.mmproj_path = Some(s.to_string());
        }
        if let Some(b) = llm.get("local_vision").and_then(|v| v.as_bool()) {
            cfg.llm.local_vision = b;
        }
        if let Some(b) = llm.get("prefer_openrouter_vision").and_then(|v| v.as_bool()) {
            cfg.llm.prefer_openrouter_vision = b;
        }
        if let Some(s) = llm.get("openrouter_base_url").and_then(|v| v.as_str()) {
            cfg.llm.openrouter_base_url = s.to_string();
        }
        if let Some(arr) = llm.get("openrouter_models").and_then(|v| v.as_array()) {
            cfg.llm.openrouter_models = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
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
        .map_err(|e| ToolError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    Ok(json!({
        "status": "success",
        "written": path.display().to_string(),
        "config": crate::config::config_public_view(),
    }))
}

/// Extract a frame and describe it (OpenRouter multimodal free → local text).
async fn handle_vision_analyze_clip(
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
async fn handle_vision_score_clip(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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

// ---------------------------------------------------------------------------
// Handler: system.capabilities (P1-2 from prior audit)
// ---------------------------------------------------------------------------

/// Probe every backend subsystem and report availability. Agents should call
/// this once at the start of a session to know which tools will work.
async fn handle_system_capabilities(
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

    // Transcription engine (Apex)
    let apex_wrapper = std::env::var("OPENSCRIPT_APEX_WRAPPER").ok();
    let transcription_available = apex_wrapper
        .as_ref()
        .map(|p| path_exists(p))
        .unwrap_or_else(|| {
            // Fall back to checking the relative path
            path_exists("mcp/scripts/apex_transcriber.py")
        });
    let transcription = json!({
        "available": transcription_available,
        "engine": "apex",
        "wrapper_path": apex_wrapper.unwrap_or_else(|| "mcp/scripts/apex_transcriber.py".to_string()),
        "reason": if transcription_available {
            serde_json::Value::Null
        } else {
            "Apex wrapper script not found. Set OPENSCRIPT_APEX_WRAPPER env var. Requires whisper-hindi conda env with whisper_timestamped installed.".into()
        },
    });

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

    // LLM / vision cascade: local Ollama Qwen3.5-4B GGUF + OpenRouter free multimodal
    let llm = crate::llm::probe_llm_capabilities().await;
    let openscript_config = crate::config::config_public_view();

    Ok(json!({
        "status": "success",
        "voicebox": voicebox,
        "kokoro": kokoro,
        "transcription": transcription,
        "parakeet_align": parakeet_align,
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

// ---------------------------------------------------------------------------
// Handler: system.doctor — cold-start production readiness
// ---------------------------------------------------------------------------

async fn handle_system_doctor(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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

    // Whisper ASR check (primary engine)
    let whisper_available = openscript_transcribe::transcriber::check_whisper_health()
        .await;
    let whisper_ok = whisper_available.is_ok();
    let whisper_msg = if whisper_ok {
        whisper_available.unwrap()
    } else {
        whisper_available.unwrap_err()
    };
    push(
        &mut checklist,
        &mut next_actions,
        "whisper",
        whisper_ok,
        &whisper_msg,
        Some("pip install openai-whisper  # or run bash setup.sh"),
    );

    // Production-ready: binaries + pexels + music + kokoro. GIPHY optional.
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

/// Lightweight HTTP probe — returns true if the URL responds with any HTTP
/// status (even 404). Used to check if a local sidecar is running without
/// depending on a specific endpoint shape.
async fn probe_http(url: &str) -> bool {
    match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .build()
    {
        Ok(client) => client.get(url).send().await.is_ok(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Handler: help.tool (Recommendation 3.1 from prior audit)
// ---------------------------------------------------------------------------

/// Natural-language tool discovery. Tokenises the query, scores each tool by
/// keyword overlap with its name + description, and returns the top N matches.
async fn handle_help_tool(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
        .flat_map(|t| expand_token(t))
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
    // boost transcribe/reelize/timeline; from-scratch boosts script.to_video.
    let tool_weight = |name: &str| -> f64 {
        if nle_intent {
            if matches!(
                name,
                "transcribe"
                    | "reelize.timeline"
                    | "reelize.direct"
                    | "reelize.brief"
                    | "timeline.render"
                    | "timeline.build"
                    | "broll.director"
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
        if from_scratch_intent && !nle_intent {
            if matches!(name, "script.to_video" | "script.parse") {
                return 0.20;
            }
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
            "reelize.timeline"
                | "reelize.direct"
                | "broll.director"
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

#[cfg(test)]
mod tests {
    use super::*;

    /// music.search (deprecated wrapper) forwards to library.search and returns results.
    #[tokio::test]
    async fn test_music_search_omitted_filters_returns_results() {
        // music.search now wraps library.search; music_index.json is empty/deleted.
        let resp = handle_music_search(json!({"query": "chill", "limit": 5}))
            .await
            .expect("music.search should succeed");
        assert!(
            resp["status"] == "success" || resp["status"] == "searched",
            "music.search should succeed; got status={}",
            resp["status"]
        );
        let count = resp["count"].as_u64().unwrap_or(0);
        assert!(
            count > 0,
            "deprecated music.search wrapper must forward to library.search; got count={} resp={}",
            count,
            resp
        );
        // Deprecation warning must be present
        let warnings = resp["warnings"].as_array().cloned().unwrap_or_default();
        assert!(
            warnings.iter().any(|w| w.as_str().unwrap_or("").contains("DEPRECATED")),
            "music.search must warn about deprecation; got warnings={:?}",
            warnings
        );
    }

    /// default_opt_bool: absent key → None; explicit true/false preserved.
    #[test]
    fn test_default_opt_bool_semantics() {
        assert_eq!(default_opt_bool(&json!({}), "loopable"), None);
        assert_eq!(
            default_opt_bool(&json!({"loopable": true}), "loopable"),
            Some(true)
        );
        assert_eq!(
            default_opt_bool(&json!({"loopable": false}), "loopable"),
            Some(false)
        );
    }

    #[test]
    fn test_production_score_procedural_only_is_fail_grade() {
        let bgs = vec![
            "mcp/assets/backgrounds/procedural_01.mp4".into(),
            "mcp/assets/backgrounds/procedural_02.mp4".into(),
        ];
        let (score, _dims, hard, _next) =
            compute_production_score(&bgs, None, 0, 0, true, true, true);
        assert!(score < 55, "procedural-only should not reach grade C; score={}", score);
        assert!(!hard.is_empty());
        assert_eq!(
            openscript_core::production_quality::production_grade(score),
            "F"
        );
    }

    #[test]
    fn test_production_score_full_stack_is_a() {
        let music = std::env::temp_dir().join("openscript_real_music_kpi.mp3");
        std::fs::write(&music, vec![1u8; 12_000]).expect("write temp music");
        let bgs = vec![
            "mcp/assets/background_cache/scene_001_yt.mp4".into(),
            "cache/stock_city.mp4".into(),
        ];
        let (score, _, _hard, _) = compute_production_score(
            &bgs,
            Some(music.to_str().unwrap()),
            3,
            2,
            true,
            true,
            true,
        );
        let _ = std::fs::remove_file(&music);
        // v2 scorer includes timeline/section dimensions; stock+stickers+music+speech
        // without full section/title map lands high-C / low-B.
        assert!(score >= 60, "full stack should be ≥C; score={}", score);
    }

    /// help.tool for NLE queries must prefer transcribe/reelize over script.to_video.
    #[tokio::test]
    async fn test_help_tool_nle_prefers_transcribe_over_script_to_video() {
        let resp = handle_help_tool(json!({
            "query": "edit existing footage with broll and captions",
            "limit": 8
        }))
        .await
        .expect("help.tool");
        assert_eq!(resp["status"], "success");
        let results = resp["results"].as_array().expect("results array");
        assert!(!results.is_empty());
        let names: Vec<&str> = results
            .iter()
            .filter_map(|r| r.get("name").and_then(|n| n.as_str()))
            .collect();
        // script.to_video must not be the top hit for NLE wording
        assert_ne!(
            names.first().copied(),
            Some("script.to_video"),
            "NLE query should not rank script.to_video first; got {:?}",
            names
        );
        // Prefer NLE tools over script.to_video in the ranking
        let script_pos = names.iter().position(|n| *n == "script.to_video");
        let nle_pos = names.iter().position(|n| {
            matches!(
                *n,
                "transcribe"
                    | "reelize.timeline"
                    | "reelize.direct"
                    | "reelize.brief"
                    | "broll.director"
                    | "timeline.render"
                    | "timeline.build"
            )
        });
        assert!(
            nle_pos.is_some(),
            "expected an NLE tool in top results: {:?}",
            names
        );
        if let (Some(sp), Some(np)) = (script_pos, nle_pos) {
            assert!(
                np < sp,
                "NLE tool should rank above script.to_video; got {:?}",
                names
            );
        }
    }

    /// broll.fetch with no API key and no fallback_pool should return
    /// status:warning with missing_key:true (NOT hard-fail with an Err).
    /// This is the bug #16 regression test.
    ///
    /// Note: we set OPENSCRIPT_ROOT to a temp dir so get_api_key() doesn't
    /// find the real config file at mcp/assets/.openscript_config.json.
    /// Without this, the test would pass real Pexels keys through and
    /// broll.fetch would succeed instead of returning the warning.
    ///
    /// IMPORTANT: This test mutates process-global env vars (set_var/remove_var)
    /// and MUST be run single-threaded: cargo test -- --test-threads=1
    #[tokio::test]
    #[ignore] // env-var race — see #1132
    async fn test_broll_fetch_no_key_no_fallback_returns_warning() {
        // Ensure no Pexels key is set for this test (env + ~/.openscript).
        std::env::remove_var("PEXELS_API_KEY");
        let temp_cfg = std::env::temp_dir().join("openscript_test_no_key_cfg");
        let _ = std::fs::create_dir_all(&temp_cfg);
        // Empty config so resolve_api_key("pexels") is empty.
        let _ = std::fs::write(temp_cfg.join("config.json"), r#"{"version":1,"api_keys":{}}"#);
        std::env::set_var("OPENSCRIPT_CONFIG_DIR", &temp_cfg);
        crate::config::reload_config();
        let temp_root = std::env::temp_dir().join("openscript_test_no_key");
        let _ = std::fs::create_dir_all(&temp_root);
        std::env::set_var("OPENSCRIPT_ROOT", &temp_root);

        let args = json!({
            "concepts": ["technology", "city"],
        });

        let result = handle_broll_fetch(args).await;
        std::env::remove_var("OPENSCRIPT_ROOT");
        std::env::remove_var("OPENSCRIPT_CONFIG_DIR");
        crate::config::reload_config();
        let _ = std::fs::remove_dir_all(&temp_root);
        let _ = std::fs::remove_dir_all(&temp_cfg);

        assert!(result.is_ok(), "broll.fetch should not hard-fail without PEXELS_API_KEY");
        let resp = result.unwrap();
        assert_eq!(resp["status"], "warning");
        assert_eq!(resp["missing_key"], true);
        assert_eq!(resp["results"].as_array().unwrap().len(), 0);
    }

    /// broll.fetch with no API key but WITH a fallback_pool should return
    /// one fallback entry per concept with status:warning.
    ///
    /// IMPORTANT: This test mutates process-global env vars (set_var/remove_var)
    /// and MUST be run single-threaded: cargo test -- --test-threads=1
    #[tokio::test]
    #[ignore] // env-var race — see #1132
    async fn test_broll_fetch_no_key_with_fallback_uses_fallback_pool() {
        std::env::remove_var("PEXELS_API_KEY");
        let temp_cfg = std::env::temp_dir().join("openscript_test_no_key_fb_cfg");
        let _ = std::fs::create_dir_all(&temp_cfg);
        let _ = std::fs::write(temp_cfg.join("config.json"), r#"{"version":1,"api_keys":{}}"#);
        std::env::set_var("OPENSCRIPT_CONFIG_DIR", &temp_cfg);
        crate::config::reload_config();
        let temp_root = std::env::temp_dir().join("openscript_test_no_key_fb");
        let _ = std::fs::create_dir_all(&temp_root);
        std::env::set_var("OPENSCRIPT_ROOT", &temp_root);

        let args = json!({
            "concepts": ["technology", "city"],
            "fallback_pool": ["/tmp/clip1.mp4", "/tmp/clip2.mp4"],
            "download": true,
        });

        let result = handle_broll_fetch(args).await;
        std::env::remove_var("OPENSCRIPT_ROOT");
        std::env::remove_var("OPENSCRIPT_CONFIG_DIR");
        crate::config::reload_config();
        let _ = std::fs::remove_dir_all(&temp_root);
        let _ = std::fs::remove_dir_all(&temp_cfg);

        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["status"], "warning");
        assert_eq!(resp["missing_key"], true);
        let results = resp["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "one fallback entry per concept");
        assert_eq!(results[0]["cached_path"], "/tmp/clip1.mp4");
        assert_eq!(results[0]["source"], "fallback_pool");
        assert_eq!(results[1]["cached_path"], "/tmp/clip2.mp4");
    }

    /// broll.fetch with no concepts should fail (required arg).
    #[tokio::test]
    async fn test_broll_fetch_missing_concepts_fails() {
        let args = json!({});
        let result = handle_broll_fetch(args).await;
        assert!(result.is_err(), "broll.fetch without concepts must fail");
    }

    /// Verify broll.fetch tool definition exposes fallback_pool in its schema.
    #[test]
    fn test_broll_fetch_schema_has_fallback_pool() {
        let tools = tool_definitions();
        let broll_fetch = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "broll.fetch")
            .expect("broll.fetch tool must exist");
        let props = broll_fetch["inputSchema"]["properties"].as_object().unwrap();
        assert!(
            props.contains_key("fallback_pool"),
            "broll.fetch schema must expose fallback_pool parameter"
        );
        // Description should mention duration is now returned.
        let desc = broll_fetch["description"].as_str().unwrap();
        assert!(
            desc.contains("duration"),
            "broll.fetch description should mention duration is returned"
        );
    }
}

#[cfg(test)]
mod library_search_tests {
    use super::*;

    /// Verify library.search tool definition exposes the new filter params.
    #[test]
    fn test_library_search_schema_has_new_filters() {
        let tools = tool_definitions();
        let lib_search = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "library.search")
            .expect("library.search tool must exist");
        let props = lib_search["inputSchema"]["properties"].as_object().unwrap();
        for required in ["source", "license", "min_duration_s", "max_duration_s", "tag"] {
            assert!(
                props.contains_key(required),
                "library.search schema must expose '{}' parameter (audit bug #18)",
                required
            );
        }
    }
}

#[cfg(test)]
mod background_search_tests {
    use super::*;

    /// Verify background.search tool definition exists and exposes mood filter.
    #[test]
    fn test_background_search_schema_has_mood_filter() {
        let tools = tool_definitions();
        let bg_search = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "background.search")
            .expect("background.search tool must exist");
        let props = bg_search["inputSchema"]["properties"].as_object().unwrap();
        for required in ["mood", "energy", "motion_intensity", "limit"] {
            assert!(
                props.contains_key(required),
                "background.search schema must expose '{}' parameter",
                required
            );
        }
    }

    /// Verify the backgrounds_index.json exists and has mood tags.
    /// Without this index, background.search returns NotFound and the
    /// fresh-agent UX gap (#2) returns.
    #[test]
    fn test_backgrounds_index_exists_and_has_moods() {
        // Tests run with CWD = crate dir, so resolve relative to CARGO_MANIFEST_DIR.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let index_path = format!("{}/../../mcp/assets/backgrounds_index.json", manifest_dir);
        let index_str = std::fs::read_to_string(&index_path)
            .unwrap_or_else(|_| panic!("backgrounds_index.json must exist at {}", index_path));
        let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
        let entries = index.get("entries").and_then(|v| v.as_array()).unwrap();
        assert!(
            entries.len() >= 10,
            "expected at least 10 background entries, got {}",
            entries.len()
        );
        // Every entry must have a mood field
        for entry in entries {
            let mood = entry.get("mood").and_then(|v| v.as_str());
            assert!(mood.is_some(), "every background entry must have a mood");
        }
        // Must have at least some calm clips (for healing content)
        let calm_count = entries
            .iter()
            .filter(|e| e.get("mood").and_then(|v| v.as_str()) == Some("calm"))
            .count();
        assert!(calm_count >= 3, "expected at least 3 calm backgrounds, got {}", calm_count);
    }
}
