use openscript_core::amplitude::extract_amplitude;
use openscript_core::captions::{estimate_word_timings, generate_ass, CaptionSegment};
use openscript_core::background::{assign_backgrounds, BackgroundClip};
use openscript_core::script::{parse_script, validate_script};
use openscript_core::sticker::{generate_sticker_composition, StickerPreset};
use openscript_core::srt::{analyze_srt, build_edl, group_entries, parse_srt, write_srt};
use openscript_core::timeline::Timeline;
use openscript_core::types::TrackType;
use openscript_transcribe::transcriber::transcribe;
use serde_json::json;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::error::ToolError;
use crate::server::report_progress;

// ---------------------------------------------------------------------------
// Tool definitions (62 tools: 43 original + 5 HyperFrames hf.* tools + 1 composition.render + 3 script.* + 2 background.* + 2 sticker.* + 1 script.to_timeline + 1 script.to_video + 1 stock.fetch + 1 youtube.download + 1 youtube.search + 1 stock.search)
// ---------------------------------------------------------------------------

pub fn tool_definitions() -> serde_json::Value {
    let mut tools = json!([
        // ===================================================================
        // GROUP 1: CORE PIPELINE — Transcribe, caption, and render
        // ===================================================================
        {
            "name": "transcribe",
            "description": "Convert spoken audio to word-level SRT subtitles. Uses Apex (Oriserve/Whisper-Hindi2Hinglish-Apex) — the ONLY transcription model in OpenScript. No fallbacks, no alternatives. Requires whisper-hindi conda env. ALWAYS call this first on any raw video — it produces the SRT that every other tool depends on. Returns: output_srt_path, entry_count, phrase_srt_path, word_srt_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "media_path": {"type": "string", "description": "Path to video or audio file to transcribe"},
                    "output_srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional output SRT path. Auto-generated if omitted."}
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
            "description": "Scan the SFX library directory and build a searchable index JSON. Run once when SFX library changes. The index enables sfx.search and sfx.assign. Default path: /home/ishanp/Videos/Assets/SFX. Returns: output_path, count of indexed files.",
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
            "description": "Scan music directories and build a searchable index JSON. Run once when adding new music files. Default path: /home/ishanp/Videos/Assets/Music. Returns: output_path, count of indexed files.",
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
            "description": "Search the music index by mood, energy, and structural properties. Mood: 'energetic', 'calm', 'dramatic', 'neutral'. Energy: 'high', 'medium', 'low'. intro_friendly tracks have a clean opening for voiceover. cta_friendly tracks build to a natural ending for call-to-action. loopable tracks repeat seamlessly. Returns: results with title, artist, path, duration_ms, mood, energy.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "default": "", "description": "Keyword search in title/artist"},
                    "mood": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Emotional mood: 'energetic', 'calm', 'dramatic', 'neutral', 'uplifting'"},
                    "energy": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Intensity level: 'high', 'medium', 'low'"},
                    "intro_friendly": {"type": "boolean", "default": false, "description": "Has a clean opening suitable for voiceover intro"},
                    "cta_friendly": {"type": "boolean", "default": false, "description": "Builds to a natural ending suitable for call-to-action"},
                    "loopable": {"type": "boolean", "default": false, "description": "Can loop seamlessly for background use"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results to return"}
                }
            }
        },
        {
            "name": "music.assign",
            "description": "Assign background music to the timeline's music track. Requires a music file path — use music.search first to find tracks, then pass the path here. Automatically spans the full timeline duration, applies ducking (lowers music during dialogue/voiceover), and sets gain. Use after building segments — the music provides emotional context beneath the spoken content. Default: -12dB with auto-ducking enabled. Returns: event_id, start_ms, end_ms, asset_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "path": {"type": "string", "description": "Path to the music audio file (MP3/WAV). Use music.search to find tracks and get their path."},
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
            "description": "Search Pexels for b-roll videos matching given concepts. Set download=true to actually download videos to the cache directory. Use BEFORE broll.assign — this finds the footage, broll.assign places it on the timeline. Requires PEXELS_API_KEY env var. Returns: results with concept, videos (id, width, height, url), cached_path if downloaded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "concepts": {"type": "array", "items": {"type": "string"}, "description": "Visual concepts to search for (e.g., ['city skyline', 'technology', 'nature'])"},
                    "asset_dir": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Cache directory for downloaded videos"},
                    "orientation": {"type": "string", "default": "9:16", "description": "Video orientation: '9:16' (vertical), '16:9' (horizontal)"},
                    "quality": {"type": "string", "default": "sd", "description": "Video quality: 'sd', 'hd', '4k'"},
                    "download": {"type": "boolean", "default": false, "description": "Actually download the top result to cache"}
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
            "description": "ONE-CALL pipeline: raw video → complete 9:16 reel. Orchestrates: (1) Transcribe with Apex, (2) Group captions, (3) Build timeline with segments, (4) B-roll director (Pexels search + download + assign), (5) Assign background music with ducking, (6) Assign SFX (hook, transitions, highlights), (7) Generate ASS captions with Bebas Neue, (8) Render final video. Use when you want a fully-produced reel from a single raw video with minimal manual intervention. All sub-steps are configurable via broll/music/sfx objects. Returns: output_path, file_size_bytes, timeline_path, segments_count, tracks_rendered, preset.",
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
            "description": "Verify caption synchronization in a rendered video. Compares caption timing from the source SRT/ASS against the actual video duration to check: (1) Coverage — do captions span the full speaking duration? (2) Gaps — are there sections without captions that should have them? (3) Overlap — do any captions overlap incorrectly? (4) Duration — are individual captions readable (not too fast)? Use AFTER rendering to ensure captions are properly burned in and timed. Returns: caption_count, coverage_percent, gaps, overlaps, avg_caption_duration_ms, readability_score (0-100).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered video"},
                    "srt_path": {"type": "string", "description": "Source SRT file used for caption burn-in"},
                    "min_caption_duration_ms": {"type": "integer", "default": 300, "description": "Minimum readable caption duration in ms"},
                    "max_caption_duration_ms": {"type": "integer", "default": 5000, "description": "Maximum caption duration before flagging"}
                },
                "required": ["video_path", "srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "verify.render",
            "description": "Verify a rendered video matches its source timeline. Compares the output video against the timeline JSON to check: (1) Duration — does output match expected timeline duration? (2) Segment count — are all timeline segments present? (3) Resolution — does output match target aspect ratio and resolution? (4) File integrity — is the file valid and non-corrupt? (5) Track completeness — were all expected tracks (broll, music, sfx, voiceover) rendered? Use AFTER timeline.render as the final quality gate before delivery. Returns: duration_match (boolean), expected_duration_ms, actual_duration_ms, segment_count_match, resolution, file_size_bytes, tracks_present, issues (array), overall_score (0-100).",
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
            "name": "script.parse",
            "description": "Parse and validate a from-scratch video creation script (JSON). The script is the single source of truth for AI-agent-driven video creation — it describes speakers, scenes, backgrounds, captions, music, and output. Returns the parsed ScriptSpec with defaults applied, plus validation errors (if any). Use BEFORE script.to_timeline / script.to_video to catch schema issues early. See openscript-core/src/script.rs for the full schema. Kokoro is the default TTS backend.",
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
            "description": "Fetch a background video clip from YouTube via yt-dlp. Searches for copyright-free gameplay footage (Minecraft, Subway Surfers, etc.), downloads, extracts a random clip of the desired duration, and crops to the target aspect ratio. Caches downloaded videos for reuse. Falls back to procedural FFmpeg background if yt-dlp is unavailable. Returns: clip_path, source_duration_s, cached.",
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
            "description": "Render an animated sticker overlay (HyperFrames HTML composition) for a speaker's voiceover. Extracts per-frame amplitude from the WAV, generates GSAP timeline that animates the SVG puppet's mouth scaleY in sync with audio. Produces an HTML file that can be rendered via hf.render to a transparent WebM. Use AFTER script.generate_voices and sticker.load_preset.",
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
                    "output_path": {"type": "string", "default": "artifacts/sticker.html", "description": "Output HTML composition path"}
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
                    "skip_stickers": {"type": "boolean", "default": false, "description": "Skip sticker rendering (no animated overlays)"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.to_video",
            "description": "ONE-CALL from-scratch video creation: script JSON → MP4. Calls script.to_timeline (which orchestrates TTS, captions, backgrounds, stickers) then renders via the from-scratch render path (background + voiceover + music + captions). This is the simplest entry point for AI agents — provide a script, get a video. Returns output_path + file_size + timeline_path + warnings. Use script.parse first to validate the script.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "output_path": {"type": "string", "default": "output.mp4", "description": "Output MP4 path"},
                    "output_dir": {"type": "string", "default": "artifacts", "description": "Directory for intermediate assets"},
                    "skip_background": {"type": "boolean", "default": false, "description": "Skip background fetching"},
                    "skip_stickers": {"type": "boolean", "default": false, "description": "Skip sticker rendering"},
                    "preview_mode": {"type": "boolean", "default": false, "description": "If true, use draft quality for faster iteration"}
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
        }
    ]);

    // Append HyperFrames tools (hf.*) — wrappers around `npx hyperframes` CLI
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
        "script.parse" => Box::pin(handle_script_parse(args)),
        "script.generate_voices" => Box::pin(handle_script_generate_voices(args)),
        "script.build_captions" => Box::pin(handle_script_build_captions(args)),
        "background.fetch" => Box::pin(handle_background_fetch(args)),
        "background.assign" => Box::pin(handle_background_assign(args)),
        "sticker.load_preset" => Box::pin(handle_sticker_load_preset(args)),
        "sticker.render" => Box::pin(handle_sticker_render(args)),
        "script.to_timeline" => Box::pin(handle_script_to_timeline(args)),
        "script.to_video" => Box::pin(handle_script_to_video(args)),
        "stock.fetch" => Box::pin(handle_stock_fetch(args)),
        "youtube.download" => Box::pin(handle_youtube_download(args)),
        "youtube.search" => Box::pin(handle_youtube_search(args)),
        "stock.search" => Box::pin(handle_stock_search(args)),
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
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
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
    timeline.tracks.get(track_type).map(|v: &Vec<openscript_core::timeline::TimelineEvent>| v.len()).unwrap_or(0)
}

fn sanitize_input_path<P: AsRef<std::path::Path>>(path: P) -> Result<std::path::PathBuf, ToolError> {
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
        path.canonicalize().map_err(|e| ToolError::InvalidArg(format!("Cannot resolve path: {}", e)))?
    } else {
        path.to_path_buf()
    };

    // If OPENSCRIPT_WORKSPACE_ROOT is set, reject paths that resolve outside it.
    // This is a defense-in-depth measure — the MCP server trusts the agent by default,
    // but operators can opt into workspace confinement via this env var.
    if let Ok(workspace_root) = std::env::var("OPENSCRIPT_WORKSPACE_ROOT") {
        let root = std::path::Path::new(&workspace_root)
            .canonicalize()
            .map_err(|e| ToolError::InvalidArg(format!("Invalid OPENSCRIPT_WORKSPACE_ROOT: {}", e)))?;
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
    let stem = path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
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
    let media_path = sanitize_input_path(extract_str(&args, "media_path")?)?.to_string_lossy().to_string();
    let output_srt_path = default_opt_str(&args, "output_srt_path")
        .unwrap_or_else(|| {
            let p = Path::new(&media_path);
            let parent = p.parent().unwrap_or(Path::new("."));
            let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
            parent.join(format!("{}.srt", stem)).to_string_lossy().to_string()
        });

    if !Path::new(&media_path).exists() {
        return Err(ToolError::NotFound(format!("Media file not found: {}", media_path)));
    }

    report_progress(0.0, 100.0, "Starting transcription...").await.ok();

    let result = transcribe(&media_path, &output_srt_path)
        .await
        .map_err(|e| ToolError::Srt(e.to_string()))?;

    report_progress(100.0, 100.0, "Transcription complete").await.ok();

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
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?.to_string_lossy().to_string();
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
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?.to_string_lossy().to_string();
    let max_words = default_u32(&args, "max_words", 10) as usize;
    let max_chars = default_u32(&args, "max_chars", 64) as usize;
    let max_gap = default_f64(&args, "max_gap", 0.6);

    let entries = parse_srt(&srt_path)?;
    let groups = group_entries(&entries, max_words, max_chars, max_gap);

    let out_srt_path = {
        let p = Path::new(&srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
        parent.join(format!("{}.grouped.srt", stem))
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

    report_progress(0.0, 100.0, "Parsing edited SRT...").await.ok();

    let edited_entries = parse_srt(edited_srt_path)
        .map_err(|e| ToolError::Srt(e.to_string()))?;

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
        let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
        parent.join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };
    let edl_json = serde_json::to_string_pretty(&edl)
        .map_err(|e| ToolError::Json(e))?;
    std::fs::write(&edl_path, edl_json)
        .map_err(|e| ToolError::Io(e))?;

    // Generate ASS subtitles if burn_captions
    let ass_path = if burn_captions {
        report_progress(20.0, 100.0, "Generating subtitle styles...").await.ok();
        let orig_srt = segments.clone();
        let retimed = retime_srt(
            &orig_srt,
            &segments.iter().map(|(s, e, _)| (*s, *e)).collect::<Vec<_>>(),
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
    report_progress(40.0, 100.0, "Rendering edited video...").await.ok();
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

    report_progress(100.0, 100.0, "Edit applied and rendered").await.ok();

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
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?.to_string_lossy().to_string();
    let strategy = default_str(&args, "strategy", "keep");
    let max_duration = default_opt_f64(&args, "max_duration");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 120);
    let analysis_path = default_opt_str(&args, "analysis_path");
    let aspect = default_str(&args, "aspect", "9:16");

    let entries = parse_srt(&srt_path).map_err(|e| ToolError::Srt(e.to_string()))?;

    let groups = group_entries(&entries, 10, 64, 0.6);

    let analysis = analyze_srt(&groups);

    if let Some(ap) = &analysis_path {
        let analysis_json = serde_json::to_string_pretty(&analysis)
            .map_err(|e| ToolError::Json(e))?;
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
        let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
        parent.join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };

    let edl_json = serde_json::to_string_pretty(&edl)
        .map_err(|e| ToolError::Json(e))?;
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

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?.to_string_lossy().to_string();
    let edl_path = sanitize_input_path(extract_str(&args, "edl_path")?)?.to_string_lossy().to_string();
    let burn_captions = default_bool(&args, "burn_captions", true);
    let srt_path = default_opt_str(&args, "srt_path");
    let ass_path = default_opt_str(&args, "ass_path");
    let aspect = default_str(&args, "aspect", "9:16");
    let crf = default_u32(&args, "crf", 20);
    let fps = default_u32(&args, "fps", 30);

    report_progress(0.0, 100.0, "Preparing render...").await.ok();

    let resolved_ass_path = if burn_captions && ass_path.is_none() {
        if let Some(srt) = &srt_path {
            if Path::new(srt).exists() {
                report_progress(10.0, 100.0, "Converting subtitles...").await.ok();
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

    report_progress(20.0, 100.0, "Rendering video with FFmpeg...").await.ok();

    let output_path = render(config).await.map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

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
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?.to_string_lossy().to_string();
    let srt_path = default_opt_str(&args, "srt_path").map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string())).transpose()?;
    let preset = default_str(&args, "preset", "Balanced");
    let max_duration = default_opt_f64(&args, "max_duration");
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!("Video not found: {}", video_path)));
    }

    // Step 1: Transcribe (if no SRT provided)
    let resolved_srt_path = if let Some(srt) = srt_path {
        report_progress(5.0, 100.0, "Using existing SRT...").await.ok();
        srt.to_string()
    } else {
        report_progress(0.0, 100.0, "Step 1/4: Transcribing audio...").await.ok();
        let transcribe_args = json!({
            "media_path": video_path,
        });
        let transcribe_result = handle_transcribe(transcribe_args).await?;
        report_progress(25.0, 100.0, "Transcription complete").await.ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    // Step 2: SRT prepare (group word-per-line)
    report_progress(30.0, 100.0, "Step 2/4: Grouping captions...").await.ok();
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
    report_progress(50.0, 100.0, "Step 3/4: Building edit decision list...").await.ok();
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
    report_progress(70.0, 100.0, "Step 4/4: Rendering final video...").await.ok();
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

    let total_segments = edl_result.get("segments_count").and_then(|v| v.as_u64()).unwrap_or(0);
    let total_duration = edl_result.get("total_duration_s").and_then(|v| v.as_f64()).unwrap_or(0.0);

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

async fn handle_overlay_generate(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let srt_path = extract_str(&args, "srt_path")?;
    let _edl_path = extract_str(&args, "edl_path")?;
    let out_path = default_opt_str(&args, "out_path").unwrap_or_else(|| {
        let p = Path::new(&srt_path);
        let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
        format!("{}.overlay.mov", stem)
    });
    let width = default_u32(&args, "width", 1080);
    let height = default_u32(&args, "height", 1920);
    let fps = default_u32(&args, "fps", 30);
    let animate = default_bool(&args, "animate", false);
    let style = default_str(&args, "style", "pupcaps_center");
    let timeline_path = default_opt_str(&args, "timeline_path");

    report_progress(0.0, 100.0, "Generating caption overlay...").await.ok();

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
                        timeline.add_asset("captions", "overlay_mov".to_string(), json!({"path": out_path}));
                        timeline.save(tl_path).ok();
                    }
                }
            }
            report_progress(100.0, 100.0, "Overlay generated").await.ok();
            Ok(json!({
                "status": "generated",
                "output_path": out_path,
            }))
        }
        Ok(o) => Err(ToolError::Ffmpeg(format!(
            "overlay.generate failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ))),
        Err(e) => Err(ToolError::Ffmpeg(format!(
            "overlay.generate error: {}",
            e
        ))),
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
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?.to_string_lossy().to_string();
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

async fn handle_timeline_validate(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?.to_string_lossy().to_string();
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
    let edl_v1_path = sanitize_input_path(extract_str(&args, "edl_v1_path")?)?.to_string_lossy().to_string();
    let output_path = default_opt_str(&args, "output_path");

    let data = std::fs::read_to_string(&edl_v1_path)?;
    let v1: serde_json::Value = serde_json::from_str(&data)?;
    let timeline = Timeline::from_edl_v1(&v1)?;

    let out_path = output_path.unwrap_or_else(|| {
        let p = Path::new(&edl_v1_path);
        let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
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
    let segment_id = timeline.add_segment(start, end, caption, crossfade_ms, semantic_role.as_deref());
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

    let track_type: TrackType = track_type_str
        .parse()
        .map_err(|e| ToolError::Timeline(e))?;

    let mut timeline = Timeline::load(timeline_path)?;

    let event_obj: openscript_core::timeline::TimelineEvent = serde_json::from_value(event.clone())
        .map_err(|e| ToolError::Json(e))?;

    timeline.add_track_event(track_type, event_obj);
    timeline.save(timeline_path)?;

    let event_id = event.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");

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
                    let profile_id = v.get("id").and_then(|x| x.as_str())
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

#[allow(dead_code)]
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
    let cache_dir = std::env::var("OPENSCRIPT_TTS_CACHE")
        .unwrap_or_else(|_| "artifacts/tts".to_string());

    // Route to Kokoro backend if the profile's provider is "kokoro" and the
    // feature is enabled. Otherwise fall through to the sidecar.
    #[cfg(feature = "kokoro")]
    if profile.provider == "kokoro" {
        use openscript_tts::kokoro::{KokoroClient, KokoroConfig};

        let model_dir = std::env::var("KOKORO_MODEL_DIR")
            .unwrap_or_else(|_| "mcp/assets/kokoro".to_string());
        let model_variant = std::env::var("KOKORO_MODEL_VARIANT")
            .unwrap_or_else(|_| "kokoro-v1.0.onnx".to_string());
        let default_voice = std::env::var("KOKORO_DEFAULT_VOICE")
            .unwrap_or_else(|_| "af_heart".to_string());

        let cfg = KokoroConfig {
            model_dir: std::path::PathBuf::from(&model_dir),
            model_variant,
            default_voice,
            cache_dir: std::path::PathBuf::from(&cache_dir),
        };
        let kokoro_client = KokoroClient::new(cfg);

        let result = kokoro_client
            .generate(voice_profile_id, text, output_path, speed, pitch, volume, format, profile)
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

    if !client.health_check().await.map_err(|e| ToolError::Tts(e.to_string()))? {
        return Err(ToolError::Tts(format!(
            "TTS sidecar server is not reachable at {}. \
             Start the faster-qwen3-tts server or set OPENSCRIPT_TTS_URL.",
            tts_url
        )));
    }

    let result = client
        .generate(voice_profile_id, text, output_path, speed, pitch, volume, format, profile)
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

    report_progress(0.0, 100.0, "Generating speech...").await.ok();

    let profiles_path = ".openscript/voice_profiles.json";
    let registry = VoiceProfileRegistry::new(profiles_path)
        .map_err(|e| ToolError::Tts(e.to_string()))?;
    let profile = registry
        .get(voice_profile_id)
        .ok_or_else(|| {
            ToolError::NotFound(format!(
                "Voice profile not found: {}",
                voice_profile_id
            ))
        })?
        .clone();

    let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());
    let cache_dir = std::env::var("OPENSCRIPT_TTS_CACHE")
        .unwrap_or_else(|_| "artifacts/tts".to_string());

    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Route to Kokoro backend if the profile's provider is "kokoro" and the
    // feature is enabled. Otherwise fall through to the sidecar (faster-qwen3-tts).
    #[cfg(feature = "kokoro")]
    if profile.provider == "kokoro" {
        use openscript_tts::kokoro::{KokoroClient, KokoroConfig};

        let model_dir = std::env::var("KOKORO_MODEL_DIR")
            .unwrap_or_else(|_| "mcp/assets/kokoro".to_string());
        let model_variant = std::env::var("KOKORO_MODEL_VARIANT")
            .unwrap_or_else(|_| "model_q8f16.onnx".to_string());
        let default_voice = std::env::var("KOKORO_DEFAULT_VOICE")
            .unwrap_or_else(|_| "af_heart".to_string());

        let cfg = KokoroConfig {
            model_dir: std::path::PathBuf::from(&model_dir),
            model_variant,
            default_voice,
            cache_dir: std::path::PathBuf::from(&cache_dir),
        };
        let kokoro_client = KokoroClient::new(cfg);

        let result = kokoro_client
            .generate(voice_profile_id, text, output_path, speed, pitch, volume, &format, &profile)
            .await
            .map_err(|e| ToolError::Tts(e.to_string()))?;

        report_progress(100.0, 100.0, "Speech generated (Kokoro)").await.ok();

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
    if !client.health_check().await.map_err(|e| ToolError::Tts(e.to_string()))? {
        return Err(ToolError::Tts(format!(
            "TTS sidecar server is not reachable at {}. \
             Start the faster-qwen3-tts server or set OPENSCRIPT_TTS_URL.",
            tts_url
        )));
    }

    let result = client
        .generate(voice_profile_id, text, output_path, speed, pitch, volume, &format, &profile)
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

    let sfx_path = default_opt_str(&args, "sfx_path")
        .or_else(|| std::env::var("OPENSCRIPT_SFX_PATH").ok())
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{}/Videos/Assets/SFX", h)))
        .unwrap_or_else(|| "./mcp/assets/sfx".to_string());
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| "mcp/assets/sfx_index.json".to_string());

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    report_progress(0.0, 100.0, "Scanning SFX directory...").await.ok();

    let index = SfxIndex::scan_directory(&sfx_path)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(80.0, 100.0, "Saving index...").await.ok();

    index
        .save(&output_path)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(100.0, 100.0, "SFX index complete").await.ok();

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

    let index = SfxIndex::load(Some(&index_path))
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    let results = index.search(&query, editorial_role.as_deref(), category.as_deref(), limit);

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

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("sfx_{:03}", track_count(&timeline, &TrackType::Sfx) + 1);

    let index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
        .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());
    let sfx_path = SfxIndex::load(Some(&index_path))
        .ok()
        .and_then(|idx| idx.search(&query, Some(editorial_role), None, 1)
            .first()
            .map(|a| a.path.clone()));

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: sfx_path.clone().unwrap_or_else(|| query.to_string()),
        start_ms: position_ms,
        end_ms: position_ms + 1000,
        offset_ms: 0,
        gain_db,
        fade_in_ms: 50,
        fade_out_ms: 50,
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
            duration_ms: 1000,
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

    Ok(json!({
        "status": "assigned",
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

    let default_paths = vec![
        std::env::var("OPENSCRIPT_MUSIC_PATH").ok()
            .or_else(|| std::env::var("HOME").ok().map(|h| format!("{}/Videos/Assets/Music", h)))
            .unwrap_or_else(|| "./mcp/assets/music".to_string())
    ];
    let paths = music_paths.as_deref().unwrap_or(&default_paths);

    report_progress(0.0, 100.0, "Scanning music directories...").await.ok();

    let index = MusicIndex::scan_directories(paths)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(80.0, 100.0, "Saving index...").await.ok();

    index
        .save(&output_path)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(100.0, 100.0, "Music index complete").await.ok();

    Ok(json!({
        "status": "indexed",
        "output_path": output_path,
        "count": index.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: music.search (native via openscript-assets)
// ---------------------------------------------------------------------------

async fn handle_music_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::music::MusicIndex;

    let query = default_str(&args, "query", "");
    let mood = default_opt_str(&args, "mood");
    let energy = default_opt_str(&args, "energy");
    let intro_friendly = default_bool(&args, "intro_friendly", false);
    let cta_friendly = default_bool(&args, "cta_friendly", false);
    let loopable = default_bool(&args, "loopable", false);
    let limit = default_u32(&args, "limit", 10) as usize;

    let index_path = std::env::var("OPENSCRIPT_MUSIC_INDEX")
        .unwrap_or_else(|_| "mcp/assets/music_index.json".to_string());

    let index = MusicIndex::load(Some(&index_path))
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    let results = index.search(
        &query,
        mood.as_deref(),
        energy.as_deref(),
        Some(intro_friendly),
        Some(cta_friendly),
        Some(loopable),
        limit,
    );

    let result_json: Vec<serde_json::Value> = results
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "title": m.title,
                "artist": m.artist,
                "path": m.path,
                "duration_ms": m.duration_ms,
                "mood": m.mood,
                "energy": m.energy,
                "loopability": m.loopability,
                "intro_friendly": m.intro_friendly,
                "cta_friendly": m.cta_friendly,
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

    if ducking {
        timeline.add_ducking_directive("dialogue_active", "music", 10.0, 50, 200);
    }

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: event_id.clone(),
        start_ms: start_ms,
        end_ms: end,
        offset_ms: 0,
        gain_db,
        fade_in_ms: 500,
        fade_out_ms: 500,
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

        if duration_ms > cadence_ms * 2 {
            let mut t = 0i64;
            while t < duration_ms {
                let slot_duration = cadence_ms.min(duration_ms - t);
                suggestions.push(json!({
                    "position_ms": position_ms + t,
                    "duration_ms": slot_duration,
                    "concept": "b-roll",
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
    let asset_dir = default_opt_str(&args, "asset_dir")
        .unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    let download = args.get("download").and_then(|v| v.as_bool()).unwrap_or(false);

    let api_key = std::env::var("PEXELS_API_KEY")
        .map_err(|_| ToolError::Asset("PEXELS_API_KEY not set".to_string()))?;

    let total = concepts.len();
    report_progress(0.0, total as f64, "Fetching b-roll...").await.ok();

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    let mut all_results = Vec::new();
    let mut downloaded = Vec::new();

    for (i, concept) in concepts.iter().enumerate() {
        report_progress(i as f64, total as f64, &format!("Searching: {}", concept)).await.ok();

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
                    Err(e) => tracing::warn!("[broll.fetch] Download failed for {}: {}", concept, e),
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
        all_results.push(result);
    }

    report_progress(total as f64, total as f64, "B-roll fetch complete").await.ok();

    let mut resp = json!({
        "status": "fetched",
        "results": all_results,
        "total_concepts": concepts.len(),
    });
    if !downloaded.is_empty() {
        resp["downloaded"] = json!(downloaded.iter().map(|(c, p)| json!({"concept": c, "path": p})).collect::<Vec<_>>());
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
    let (asset_id, asset_registry_path) = if resolved_path.is_empty()
        || resolved_path.contains("placeholder")
        || !std::path::Path::new(&resolved_path).exists()
    {
        ("placeholder".to_string(), "placeholder".to_string())
    } else {
        (resolved_path.clone(), resolved_path.clone())
    };

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: asset_id.clone(),
        start_ms: position_ms,
        end_ms: position_ms + duration_ms,
        offset_ms: 0,
        gain_db: 0.0,
        fade_in_ms: 0,
        fade_out_ms: 0,
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
    timeline.add_asset("broll", event_id.clone(), json!({"path": asset_registry_path}));
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "assigned",
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
    let registry = VoiceProfileRegistry::new(profiles_path)
        .map_err(|e| ToolError::Tts(e.to_string()))?;
    let profile = registry
        .get(voice_profile_id)
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile not found: {}", voice_profile_id))
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

    report_progress(0.0, 100.0, "Generating voiceover...").await.ok();

    let result = tts_generate_routed(
        voice_profile_id, text, &output_path, speed, pitch, volume, "wav", &profile
    ).await?;

    let duration_ms = result.duration_ms;

    timeline.add_asset("voices", event_id.clone(), json!({
        "path": output_path.clone(),
        "voice_profile_id": voice_profile_id,
        "text": text,
    }));

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

    report_progress(100.0, 100.0, "Voiceover generated").await.ok();

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

async fn handle_tts_commentary(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
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
    let registry = VoiceProfileRegistry::new(profiles_path)
        .map_err(|e| ToolError::Tts(e.to_string()))?;
    let profile = registry
        .get(voice_profile_id)
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile not found: {}", voice_profile_id))
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

        let result = tts_generate_routed(
            voice_profile_id, &text, &output_path, speed, 1.0, 1.0, "wav", &profile
        ).await?;

        let duration_ms = result.duration_ms;

        timeline.add_asset("voices", event_id.clone(), json!({
            "path": output_path.clone(),
            "voice_profile_id": voice_profile_id,
            "text": text.clone(),
        }));

        let event = openscript_core::timeline::TimelineEvent {
            id: event_id.clone(),
            asset_id: output_path.clone(),
            start_ms: 0,
            end_ms: duration_ms,
            offset_ms: 0,
            gain_db: -6.0,
            fade_in_ms: 50,
            fade_out_ms: 50,
            tags: vec!["commentary".to_string(), "intro".to_string()],
            provenance: Some(openscript_core::timeline::Provenance {
                tool: "tts.commentary".into(),
                editorial_role: None,
                concept: Some("intro".to_string()),
            }),
            kind: openscript_core::timeline::EventKind::Voiceover {
                voice_profile_id: voice_profile_id.to_string(),
                text,
                estimated_duration_ms: duration_ms,
            },
        };

        timeline.add_track_event(TrackType::Voiceover, event);
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
            ).await.ok();

            let seg_start_ms = (seg.start * 1000.0) as i64;
            if seg_start_ms <= 0 {
                continue;
            }
            let concept = seg.caption.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
            let text = format!("Now, let's look at {}.", concept);
            let event_id = format!(
                "voiceover_{:03}",
                track_count(&timeline, &TrackType::Voiceover) + generated.len() + 1
            );
            let output_path = timeline_dir
                .join(format!("voiceover_{}.wav", event_id))
                .to_string_lossy()
                .to_string();

            if let Some(parent) = Path::new(&output_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }

            let result = tts_generate_routed(
                voice_profile_id, &text, &output_path, speed, 1.0, 1.0, "wav", &profile
            ).await?;

            let duration_ms = result.duration_ms;

            timeline.add_asset("voices", event_id.clone(), json!({
                "path": output_path.clone(),
                "voice_profile_id": voice_profile_id,
                "text": text.clone(),
            }));

            let event = openscript_core::timeline::TimelineEvent {
                id: event_id.clone(),
                asset_id: output_path.clone(),
                start_ms: seg_start_ms,
                end_ms: seg_start_ms + duration_ms,
                offset_ms: 0,
                gain_db: -6.0,
                fade_in_ms: 50,
                fade_out_ms: 50,
                tags: vec!["commentary".to_string(), "transition".to_string()],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "tts.commentary".into(),
                    editorial_role: None,
                    concept: Some("transition".to_string()),
                }),
                kind: openscript_core::timeline::EventKind::Voiceover {
                    voice_profile_id: voice_profile_id.to_string(),
                    text,
                    estimated_duration_ms: duration_ms,
                },
            };

            timeline.add_track_event(TrackType::Voiceover, event);
            generated.push(event_id);
            positions.push(seg_start_ms);
        }
    }

    if do_outro {
        let text = outro_text.unwrap_or_else(|| "Thanks for watching!".to_string());
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

        let result = tts_generate_routed(
            voice_profile_id, &text, &output_path, speed, 1.0, 1.0, "wav", &profile
        ).await?;

        let duration_ms = result.duration_ms;

        timeline.add_asset("voices", event_id.clone(), json!({
            "path": output_path.clone(),
            "voice_profile_id": voice_profile_id,
            "text": text.clone(),
        }));

        let event = openscript_core::timeline::TimelineEvent {
            id: event_id.clone(),
            asset_id: output_path.clone(),
            start_ms: total_ms,
            end_ms: total_ms + duration_ms,
            offset_ms: 0,
            gain_db: -6.0,
            fade_in_ms: 50,
            fade_out_ms: 50,
            tags: vec!["commentary".to_string(), "outro".to_string()],
            provenance: Some(openscript_core::timeline::Provenance {
                tool: "tts.commentary".into(),
                editorial_role: None,
                concept: Some("outro".to_string()),
            }),
            kind: openscript_core::timeline::EventKind::Voiceover {
                voice_profile_id: voice_profile_id.to_string(),
                text,
                estimated_duration_ms: duration_ms,
            },
        };

        timeline.add_track_event(TrackType::Voiceover, event);
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

    let added: Vec<&str> = seg_ids_b.difference(&seg_ids_a).copied().collect();
    let removed: Vec<&str> = seg_ids_a.difference(&seg_ids_b).copied().collect();

    let mut modified = Vec::new();
    for seg_a in &a.segments {
        if seg_ids_b.contains(seg_a.id.as_str()) {
            if let Some(seg_b) = b.segments.iter().find(|s| s.id == seg_a.id) {
                if seg_a.start != seg_b.start
                    || seg_a.end != seg_b.end
                    || seg_a.caption != seg_b.caption
                {
                    modified.push(&seg_a.id);
                }
            }
        }
    }

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
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?.to_string_lossy().to_string();
    let timeline = Timeline::load(&timeline_path)?;

    let total_duration_ms = timeline.total_duration_ms();
    let segments_info: Vec<serde_json::Value> = timeline
        .segments
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "start": s.start,
                "end": s.end,
                "caption": s.caption.chars().take(60).collect::<String>(),
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
    let dialogue = timeline.tracks.get(&TrackType::Dialogue).cloned().unwrap_or_default();
    let voiceover = timeline.tracks.get(&TrackType::Voiceover).cloned().unwrap_or_default();

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

    report_progress(0.0, max_gaps as f64, "Auto-filling b-roll slots...").await.ok();

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
                .map(|s| s.caption.split_whitespace().take(2).collect::<Vec<_>>().join("_"))
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
                report_progress(count as f64, max_gaps as f64, &format!("Filled {} b-roll slots", count)).await.ok();
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

    let timeline = Timeline::load(timeline_path)?;
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

    report_progress(20.0, 100.0, "Building filter graph...").await.ok();

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
        Err(e) => Err(ToolError::Ffmpeg(e.to_string())),
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

    let api_key = std::env::var("PEXELS_API_KEY")
        .map_err(|_| ToolError::Asset(
            "PEXELS_API_KEY environment variable not set. Set it to use broll.director. Get a free key at https://www.pexels.com/api/".to_string()
        ))?;

    let mut timeline = Timeline::load(timeline_path)?;

    report_progress(0.0, 100.0, "Analyzing script and creating b-roll slots...").await.ok();

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
            let concept = event.tags.first().cloned().unwrap_or_else(|| "general".into());
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
        ).await.ok();

        let search_result = client.search_for_slot(concept, &orientation, &quality).await;
        match search_result {
            Ok(Some(video)) => {
                match client.download_best(&video, concept).await {
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
                }
            }
Ok(None) => tracing::warn!("[broll.director] No video found for concept: {}", concept),
Err(e) => tracing::warn!("[broll.director] Search failed for {}: {}", concept, e),
        }
    }

    timeline.save(timeline_path)?;

    report_progress(100.0, 100.0, "B-roll director complete").await.ok();

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

async fn handle_reelize_timeline(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
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
        return Err(ToolError::NotFound(format!("Video not found: {}", video_path)));
    }

    let mut warnings: Vec<String> = Vec::new();

    // Environment diagnostics — warn about missing capabilities early
    if std::env::var("PEXELS_API_KEY").is_err() {
        warnings.push("PEXELS_API_KEY not set — b-roll will be skipped".into());
    }
    let tts_available = std::env::var("OPENSCRIPT_TTS_URL").is_ok();
    if !tts_available {
        warnings.push("No TTS server configured (OPENSCRIPT_TTS_URL) — voiceover unavailable".into());
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
    let music_gain_db = default_f64(&music_obj, "gain_db", -12.0);

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
    report_progress(0.0, 100.0, "Step 1/7: Transcribing audio...").await.ok();
    let transcribe_args = json!({ "media_path": video_path });
    let transcribe_result = handle_transcribe(transcribe_args).await?;
    let srt_path = transcribe_result
        .get("output_srt_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
        .to_string();
    report_progress(15.0, 100.0, "Transcription complete").await.ok();

    // Step 2/7: SRT prepare → grouped SRT
    report_progress(15.0, 100.0, "Step 2/7: Grouping captions...").await.ok();
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
    report_progress(25.0, 100.0, "Caption grouping complete").await.ok();

    // Step 3/7: Build timeline + populate segments from SRT
    report_progress(25.0, 100.0, "Step 3/7: Building timeline...").await.ok();
    let mut timeline = Timeline::new(video_path.into(), &aspect, 30, max_duration);
    let segment_count = timeline.populate_segments_from_srt(&grouped_srt_path, crossfade_ms)
        .map_err(|e| ToolError::Timeline(e))?;

    if segment_count == 0 {
        return Err(ToolError::Timeline("No segments created from SRT — transcript may be empty".to_string()));
    }

    // Generate ASS subtitles with Bebas Neue styling for burn-in
    let ass_path = {
        let p = Path::new(&grouped_srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
        parent.join(format!("{}.ass", stem)).to_string_lossy().to_string()
    };
    match openscript_core::srt::parse_srt(&grouped_srt_path) {
        Ok(entries) => {
            let ass_entries: Vec<(f64, f64, String)> = entries
                .iter()
                .map(|e| (e.start, e.end, e.text.clone()))
                .collect();
            match openscript_ffmpeg::subtitles::srt_to_ass(&ass_entries, &ass_path, "Default") {
                Ok(()) => {
                    timeline.assets.captions.insert(
                        "ass".into(),
                        json!({"path": ass_path.clone()}),
                    );
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
    report_progress(40.0, 100.0, &format!("Timeline built with {} segments", segment_count)).await.ok();

    // Step 4/7: B-roll director (if enabled)
    if broll_enabled {
        report_progress(40.0, 100.0, "Step 4/7: B-roll director...").await.ok();
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
                let filled = r.get("broll_slots_filled").and_then(|v| v.as_u64()).unwrap_or(0);
                report_progress(55.0, 100.0, &format!("B-roll: {} slots filled", filled)).await.ok();

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
        report_progress(55.0, 100.0, "B-roll disabled, skipping").await.ok();
    }

    // Step 5/7: Music + SFX
    report_progress(55.0, 100.0, "Step 5/7: Assigning music and SFX...").await.ok();

    if music_enabled {
        // Search for a matching music track first, then pass its path to music.assign
        let music_search_args = json!({
            "mood": music_mood,
            "energy": music_energy,
            "limit": 1,
        });
        let music_path = match handle_music_search(music_search_args).await {
            Ok(r) => r.get("results")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("path"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string()),
            Err(e) => {
                warnings.push(format!("Music search failed: {}", e));
                None
            }
        };

        if let Some(path) = music_path {
            let music_args = json!({
                "timeline_path": &timeline_path,
                "path": path,
                "mood": music_mood,
                "energy": music_energy,
                "gain_db": music_gain_db,
                "ducking": true,
            });
            let music_result = handle_music_assign(music_args).await;
            match music_result {
                Ok(_r) => {
                    if let Ok(t) = Timeline::load(&timeline_path) {
                        let music_count = t.tracks.get(&TrackType::Music)
                            .map(|v| v.len()).unwrap_or(0);
                        report_progress(60.0, 100.0, &format!("Music assigned ({} track(s))", music_count)).await.ok();
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
                all_transitions.into_iter().step_by(step).take(max_transitions).collect()
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
                let highlight_id = format!("sfx_{:03}", track_count(&timeline, &TrackType::Sfx) + 1);
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
    report_progress(70.0, 100.0, &format!("Music and SFX assigned ({} SFX events)", sfx_count)).await.ok();

    // Step 6/7: Animated captions overlay
    if animated_captions && burn_captions {
        report_progress(70.0, 100.0, "Step 6/7: Generating animated caption overlay...").await.ok();
        // Need an EDL path for overlay.generate — create a minimal one
        let edl_path = {
            let p = Path::new(&timeline_path);
            let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
            p.parent()
                .unwrap_or(Path::new("."))
                .join(format!("{}.edl.json", stem))
                .to_string_lossy()
                .to_string()
        };
        let t = Timeline::load(&timeline_path)
            .map_err(|e| ToolError::Timeline(e.to_string()))?;
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
        std::fs::write(&edl_path, serde_json::to_string_pretty(&edl).unwrap_or_default()).ok();

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
        report_progress(85.0, 100.0, "Animated captions generated").await.ok();
    } else {
        report_progress(85.0, 100.0, "Static captions (burn-in)").await.ok();
    }

    // Step 7/7: Validate + render
    report_progress(85.0, 100.0, "Step 7/7: Validating and rendering...").await.ok();

    let timeline = Timeline::load(&timeline_path)
        .map_err(|e| ToolError::Timeline(e.to_string()))?;
    let errors = timeline.validate();
    if !errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline validation failed before render: {:?}",
            errors
        )));
    }

    let broll_count = timeline.tracks.get(&TrackType::Broll).map(|v| v.len()).unwrap_or(0);
    let music_count = timeline.tracks.get(&TrackType::Music).map(|v| v.len()).unwrap_or(0);
    let sfx_count = timeline.tracks.get(&TrackType::Sfx).map(|v| v.len()).unwrap_or(0);
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
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?.to_string_lossy().to_string();
    let expected_has_voice = default_bool(&args, "expected_has_voice", true);
    let max_silence_seconds = default_f64(&args, "max_silence_seconds", 3.0);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!("Video not found: {}", video_path)));
    }

    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "a:0",
            "-show_entries", "stream=codec_name,sample_rate,channels,duration",
            "-of", "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !output.status.success() {
        return Err(ToolError::Ffmpeg(format!("ffprobe failed: {}", String::from_utf8_lossy(&output.stderr))));
    }

    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| ToolError::Json(e))?;

    let streams = probe.get("streams").and_then(|v| v.as_array()).cloned().unwrap_or_default();
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
        .args([
            "-i", &video_path,
            "-af", "volumedetect",
            "-f", "null",
            "-",
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("volumedetect failed: {}", e)))?;

    if !vol_output.status.success() {
        return Err(ToolError::Ffmpeg(format!("volumedetect failed: {}", String::from_utf8_lossy(&vol_output.stderr))));
    }

    let stderr = String::from_utf8_lossy(&vol_output.stderr);
    let mean_volume = stderr.lines()
        .find(|l| l.contains("mean_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());
    let max_volume = stderr.lines()
        .find(|l| l.contains("max_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());

    let silence_output = tokio::process::Command::new("ffmpeg")
        .args([
            "-i", &video_path,
            "-af", &format!("silencedetect=noise=-30dB:d={}", max_silence_seconds),
            "-f", "null",
            "-",
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("silencedetect failed: {}", e)))?;

    if !silence_output.status.success() {
        return Err(ToolError::Ffmpeg(format!("silencedetect failed: {}", String::from_utf8_lossy(&silence_output.stderr))));
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
        if has_audio { score += 25; }
        if has_good_level { score += 25; }
        if has_no_clipping { score += 25; }
        if no_long_silence { score += 25; }
        score
    } else {
        if has_audio { 50 } else { 100 }
    };

    let mut issues: Vec<String> = Vec::new();
    if !has_audio { issues.push("No audio stream".into()); }
    if !has_good_level && has_audio { issues.push(format!("Audio level unhealthy: RMS {} dB (expected -30 to -12 dB)", rms)); }
    if !has_no_clipping { issues.push(format!("Audio clipping detected: peak {} dB", peak)); }
    if !no_long_silence { issues.push(format!("{} silence gaps detected (>{})", silence_segments.len(), max_silence_seconds)); }

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

// ---------------------------------------------------------------------------
// Handler: verify.captions
// ---------------------------------------------------------------------------

async fn handle_verify_captions(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?.to_string_lossy().to_string();
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?.to_string_lossy().to_string();
    let min_caption_duration_ms = default_i64(&args, "min_caption_duration_ms", 300);
    let max_caption_duration_ms = default_i64(&args, "max_caption_duration_ms", 5000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!("Video not found: {}", video_path)));
    }
    if !Path::new(&srt_path).exists() {
        return Err(ToolError::NotFound(format!("SRT not found: {}", srt_path)));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!("ffprobe failed: {}", String::from_utf8_lossy(&probe_output.stderr))));
    }

    let probe: serde_json::Value = serde_json::from_slice(&probe_output.stdout)
        .map_err(|e| ToolError::Json(e))?;
    let video_duration_s: f64 = probe.get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let video_duration_ms = (video_duration_s * 1000.0) as i64;

    let entries = openscript_core::srt::parse_srt(srt_path)
        .map_err(|e| ToolError::Srt(e.to_string()))?;

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

    let avg_duration = if !entries.is_empty() { total_caption_ms / entries.len() as i64 } else { 0 };
    let coverage = if video_duration_ms > 0 { (total_caption_ms as f64 / video_duration_ms as f64) * 100.0 } else { 0.0 };

    let mut issues: Vec<String> = Vec::new();
    if !gaps.is_empty() { issues.push(format!("{} caption gaps > 2s", gaps.len())); }
    if !overlaps.is_empty() { issues.push(format!("{} caption overlaps", overlaps.len())); }
    if !too_fast.is_empty() { issues.push(format!("{} captions too fast (<{}ms)", too_fast.len(), min_caption_duration_ms)); }
    if !too_slow.is_empty() { issues.push(format!("{} captions too slow (>{})", too_slow.len(), max_caption_duration_ms)); }

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
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?.to_string_lossy().to_string();
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?.to_string_lossy().to_string();
    let expected_aspect = default_str(&args, "expected_aspect", "9:16");
    let duration_tolerance_ms = default_i64(&args, "duration_tolerance_ms", 2000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!("Video not found: {}", video_path)));
    }
    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!("Timeline not found: {}", timeline_path)));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height,r_frame_rate,duration",
            "-show_entries", "format=duration,size",
            "-of", "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!("ffprobe failed: {}", String::from_utf8_lossy(&probe_output.stderr))));
    }

    let probe: serde_json::Value = serde_json::from_slice(&probe_output.stdout)
        .map_err(|e| ToolError::Json(e))?;

    let streams = probe.get("streams").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let format_info = probe.get("format").cloned().unwrap_or(json!({}));

    let width = streams.first().and_then(|s| s.get("width")).and_then(|v| v.as_u64()).unwrap_or(0);
    let height = streams.first().and_then(|s| s.get("height")).and_then(|v| v.as_u64()).unwrap_or(0);
    let file_size = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);

    let actual_duration_s: f64 = format_info.get("duration")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| streams.first().and_then(|s| s.get("duration")).and_then(|v| v.as_str()).and_then(|v| v.parse::<f64>().ok()))
        .unwrap_or(0.0);
    let actual_duration_ms = (actual_duration_s * 1000.0) as i64;

    let timeline = Timeline::load(timeline_path)
        .map_err(|e| ToolError::Timeline(e.to_string()))?;
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
    let actual_ratio = if height > 0 { width as f64 / height as f64 } else { 0.0 };
    let aspect_match = (actual_ratio - expected_ratio).abs() < 0.05;

    let tracks_present: serde_json::Map<String, serde_json::Value> = timeline.tracks.iter()
        .map(|(track, events)| {
            let track = track as &TrackType;
            let events = events as &Vec<openscript_core::timeline::TimelineEvent>;
            (track.to_string(), json!({"count": events.len(), "rendered": !events.is_empty()}))
        })
        .collect();

    let total_tracks = timeline.tracks.values().filter(|v| !v.is_empty()).count();
    let has_audio = total_tracks > 1;

    let mut issues: Vec<String> = Vec::new();
    if !duration_match { issues.push(format!("Duration mismatch: expected {}ms, got {}ms (delta: {}ms)", expected_duration_ms, actual_duration_ms, duration_delta)); }
    if !aspect_match { issues.push(format!("Aspect ratio mismatch: expected {}, got {}x{} (ratio: {:.3})", expected_aspect, width, height, actual_ratio)); }
    if file_size == 0 { issues.push("File size is 0 bytes — render may have failed".into()); }
    if width == 0 || height == 0 { issues.push("Could not determine video resolution".into()); }

    let mut score = 100;
    if !duration_match { score -= 30; }
    if !aspect_match { score -= 25; }
    if file_size == 0 { score -= 45; }
    if !has_audio && total_tracks > 1 { score -= 15; }
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
    }))
}

// ---------------------------------------------------------------------------
// Handler: reelize.brief
// ---------------------------------------------------------------------------

async fn handle_reelize_brief(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?.to_string_lossy().to_string();
    let srt_path_opt = default_opt_str(&args, "srt_path").map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string())).transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!("Video not found: {}", video_path)));
    }

    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        report_progress(5.0, 100.0, "Using existing SRT...").await.ok();
        srt
    } else {
        report_progress(0.0, 100.0, "Transcribing audio...").await.ok();
        let transcribe_result = handle_transcribe(json!({"media_path": video_path})).await?;
        report_progress(30.0, 100.0, "Transcription complete").await.ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    report_progress(35.0, 100.0, "Grouping caption segments...").await.ok();
    let prepare_result = handle_srt_prepare(json!({
        "srt_path": &resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    })).await?;
    let grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    report_progress(50.0, 100.0, "Analyzing segments...").await.ok();
    let entries = parse_srt(&grouped_srt_path)?;

    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "can", "shall", "to", "of", "in", "for",
        "on", "with", "at", "by", "from", "as", "into", "like", "through",
        "after", "over", "between", "out", "against", "during", "without",
        "before", "under", "around", "among", "that", "this", "these",
        "those", "it", "its", "i", "me", "my", "we", "our", "you", "your",
        "he", "him", "his", "she", "her", "they", "them", "their", "what",
        "which", "who", "whom", "whose", "where", "when", "why", "how",
        "not", "no", "nor", "so", "but", "and", "or", "if", "then", "than",
        "too", "very", "just", "about", "up", "some",
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
        let broll_concepts: Vec<String> = if entry.text.len() < 20 && !entry.text.trim().is_empty() {
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

    let mut topic_map: std::collections::HashMap<String, (usize, f64)> = std::collections::HashMap::new();
    for seg in &segments {
        if let Some(keywords) = seg.get("topic_keywords").and_then(|v| v.as_array()) {
            if let Some(first) = keywords.first().and_then(|v| v.as_str()) {
                let topic = first.to_string();
                let entry = topic_map.entry(topic).or_insert((0, 0.0));
                entry.0 += 1;
                entry.1 += seg.get("duration_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            }
        }
    }

    let topic_summary: Vec<serde_json::Value> = topic_map
        .into_iter()
        .map(|(topic, (count, total_s))| json!({
            "topic": topic,
            "segment_count": count,
            "total_s": (total_s * 100.0).round() / 100.0,
        }))
        .collect();

    let source_duration_s = match tokio::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "json"])
        .arg(&video_path)
        .output()
        .await
    {
        Ok(output) => {
            if let Ok(probe) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                probe.get("format")
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

async fn handle_reelize_direct(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;
    use openscript_ffmpeg::subtitles;

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?.to_string_lossy().to_string();
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
    let srt_path_opt = default_opt_str(&args, "srt_path").map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string())).transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let mut warnings: Vec<String> = Vec::new();

    report_progress(0.0, 100.0, "Transcribing audio...").await.ok();
    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        srt
    } else {
        let transcribe_result =
            handle_transcribe(json!({"media_path": video_path})).await?;
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::Srt("Transcription did not return output path".to_string())
            })?
            .to_string()
    };

    report_progress(15.0, 100.0, "Preparing grouped SRT...").await.ok();
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

    report_progress(25.0, 100.0, "Building timeline...").await.ok();
    let timeline_path = default_timeline_path(&video_path);
    let mut timeline =
        Timeline::new(std::path::Path::new(&video_path).to_path_buf(), &aspect, fps, None);

    for segment in segments_arr {
        let start = segment
            .get("start")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let end = segment
            .get("end")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let caption = segment
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let seg_crossfade = segment
            .get("crossfade_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(crossfade_ms as u64) as u32;
        let semantic_role = segment
            .get("id")
            .and_then(|v| v.as_str());

        timeline.add_segment(start, end, caption, seg_crossfade, semantic_role);
    }

    if captions_enabled {
        use openscript_core::srt::parse_srt;

        let word_srt_path = {
            let p = Path::new(&resolved_srt_path);
            let parent = p.parent().unwrap_or(Path::new("."));
            let stem = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
            parent.join(format!("{}.apex.word.srt", stem))
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
            let seg_start = segment
                .get("start")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let seg_end = segment
                .get("end")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let caption = segment
                .get("caption")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let seg_crossfade_s = segment
                .get("crossfade_ms")
                .and_then(|v| v.as_f64())
                .unwrap_or(crossfade_ms as f64) / 1000.0;
            let seg_duration = seg_end - seg_start;

            if let Some(ref words) = word_entries {
                let words_in_range: Vec<_> = words.iter()
                    .filter(|e| e.start >= seg_start && e.end <= seg_end + 0.05
                        && !e.text.trim().is_empty())
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
                    let chunk_end = output_cursor_s + (words_in_range[i + chunk_size - 1].end - seg_start);
                    let text: Vec<_> = words_in_range[i..i + chunk_size].iter()
                        .map(|e| e.text.trim().to_string()).collect();
                    timeline_segments.push((chunk_start, chunk_end, text.join(" ")));
                    i += chunk_size;
                }
            } else {
                let srt_in_range: Vec<_> = raw_srt_entries.iter()
                    .filter(|e| e.start >= seg_start && e.end <= seg_end + 0.05
                        && !e.text.trim().is_empty())
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
                    timeline_segments.push((output_cursor_s, output_cursor_s + seg_duration, caption.to_string()));
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

        let caption_asset_dir = Path::new(&timeline_path)
            .parent()
            .unwrap_or(Path::new("."));
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

    report_progress(40.0, 100.0, "Fetching b-roll...").await.ok();
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
                    warnings.push(format!("broll fetch found no downloadable asset for '{}'", concept));
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
        report_progress(65.0, 100.0, "Assigning music...").await.ok();
        let mood = default_str(music, "mood", "neutral");
        let energy = default_str(music, "energy", "medium");
        let gain_db = default_f64(music, "gain_db", -12.0);
        let ducking = default_bool(music, "duck_under_dialogue", true);

        // Search for a matching music track, then pass its path
        let music_path = match handle_music_search(json!({
            "mood": mood,
            "energy": energy,
            "limit": 1,
        })).await {
            Ok(r) => r.get("results")
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
        report_progress(75.0, 100.0, "Generating voiceovers...").await.ok();
    }
    for directive in &voiceover_arr {
        let text = directive
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
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

    report_progress(85.0, 100.0, "Validating timeline...").await.ok();
    let timeline = Timeline::load(&timeline_path)?;
    let validation_errors = timeline.validate();
    if !validation_errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline validation failed: {}",
            validation_errors.join("; ")
        )));
    }

    report_progress(90.0, 100.0, "Rendering final video...").await.ok();
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
    let spec = parse_script(&json_str).map_err(|e| {
        ToolError::InvalidArg(format!("Failed to parse script JSON: {}", e))
    })?;

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
            "speaker_layout": format!("{:?}", spec.speaker_layout).to_lowercase(),
            "background_type": spec.background.r#type,
            "stickers_enabled": spec.stickers.enabled,
            "lip_sync_mode": spec.stickers.lip_sync,
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.generate_voices — TTS per scene
// ---------------------------------------------------------------------------

async fn handle_script_generate_voices(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts/voices");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str).map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;
    let errors = validate_script(&spec);
    if !errors.is_empty() {
        return Err(ToolError::InvalidArg(format!("Script validation failed: {} errors", errors.len())));
    }

    // Create output directory
    std::fs::create_dir_all(&output_dir)?;

    report_progress(0.0, 100.0, "Generating voices...").await.ok();

    let total_scenes = spec.scenes.len();
    let mut segments = Vec::new();
    let mut current_ms = 0i64;

    for (i, scene) in spec.scenes.iter().enumerate() {
        report_progress(
            (i as f64 / total_scenes as f64) * 100.0,
            100.0,
            &format!("Voice {}/{}: {}", i + 1, total_scenes, scene.speaker),
        ).await.ok();

        // Get speaker's voice profile
        let speaker = spec.speakers.get(&scene.speaker)
            .ok_or_else(|| ToolError::NotFound(format!("Speaker not found: {}", scene.speaker)))?;

        // Load voice profile from registry
        let profiles_path = ".openscript/voice_profiles.json";
        let registry = openscript_tts::profiles::VoiceProfileRegistry::new(profiles_path)
            .map_err(|e| ToolError::Tts(e.to_string()))?;

        // Try to find the voice profile by ID or by voice field
        let profile = registry.get(&speaker.voice)
            .or_else(|| {
                // If voice is "kokoro:af_heart", try to find a profile with that model
                registry.list().iter().find(|p| p.model == speaker.voice).cloned()
            })
            .map(|p| p.clone())
            .ok_or_else(|| {
                ToolError::NotFound(format!(
                    "Voice profile '{}' not found in registry. Add it via voice.profile.add.",
                    speaker.voice
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
        ).await?;

        // Calculate word timings for this scene using whisper force alignment
        let scene_end_ms = current_ms + result.duration_ms;
        let words = run_whisper_alignment(&result.output_path, current_ms, scene_end_ms)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("[script.generate_voices] Whisper alignment failed ({}), using estimate", e);
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
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.build_captions — ASS generation from word timings
// ---------------------------------------------------------------------------

async fn handle_script_build_captions(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let manifest_path = extract_str(&args, "voiceover_manifest")?;
    let output_path = default_str(&args, "output_path", "artifacts/captions.ass");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str).map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Build CaptionSegments from manifest
    let mut segments = Vec::new();
    if let Some(segs) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segs {
            let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
            return Err(ToolError::NotFound(format!("Script file not found: {}", path.display())));
        }
        Ok(std::fs::read_to_string(&path)?)
    }
}

/// Run whisper force alignment on a TTS WAV file to get accurate word timestamps.
/// Falls back to even-spacing estimation if whisper is unavailable.
async fn run_whisper_alignment(
    wav_path: &str,
    offset_ms: i64,
    scene_end_ms: i64,
) -> Result<Vec<openscript_core::captions::WordTiming>, String> {
    // Write alignment to a temp JSON file
    let tmp_json = format!("{}.align.json", wav_path);

    let output = tokio::process::Command::new("python3")
        .arg("mcp/scripts/whisper_align.py")
        .arg("--wav").arg(wav_path)
        .arg("--output").arg(&tmp_json)
        .arg("--model").arg("tiny")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("Failed to spawn whisper: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&tmp_json);
        return Err(format!("Whisper failed: {}", stderr.lines().last().unwrap_or("unknown")));
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
            let word = w.get("word").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
        return Err("Whisper returned no words".to_string());
    }

    Ok(words)
}

// ---------------------------------------------------------------------------
// Handler: background.fetch — YouTube auto-download via yt-dlp
// ---------------------------------------------------------------------------

async fn handle_background_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let duration_s = default_f64(&args, "duration_s", 30.0);
    let aspect = default_str(&args, "aspect", "9:16");
    let cache_dir = default_str(&args, "cache_dir", "mcp/assets/background_cache");
    let fallback_pool: Vec<String> = args
        .get("fallback_pool")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    std::fs::create_dir_all(&cache_dir)?;

    report_progress(0.0, 100.0, "Searching YouTube...").await.ok();

    // Try yt-dlp
    let cache_key = format!("{:x}", md5_hash(query.as_bytes()));
    let full_video_path = format!("{}/{}.mp4", cache_dir, cache_key);
    let clip_path = format!("{}/{}_clip.mp4", cache_dir, cache_key);

    // Check if we already have the full video cached
    let full_cached = Path::new(&full_video_path).exists();

    if !full_cached {
        // Download via yt-dlp
        let yt_dlp_result = tokio::process::Command::new("yt-dlp")
            .arg("--format").arg("best[height<=720]")
            .arg("--output").arg(&full_video_path)
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
                report_progress(50.0, 100.0, "Downloaded, extracting clip...").await.ok();
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("[background.fetch] yt-dlp failed: {}", stderr);
                // Try fallback pool
                if let Some(fallback) = fallback_pool.first() {
                    if Path::new(fallback).exists() {
                        return Ok(json!({
                            "status": "fallback",
                            "clip_path": fallback,
                            "source_duration_s": duration_s,
                            "cached": false,
                            "warning": "yt-dlp failed, using fallback pool"
                        }));
                    }
                }
                // No fallback — generate procedural background
                return generate_procedural_background(&cache_dir, &cache_key, duration_s, &aspect).await;
            }
            Err(e) => {
                tracing::warn!("[background.fetch] yt-dlp not available: {}", e);
                if let Some(fallback) = fallback_pool.first() {
                    if Path::new(fallback).exists() {
                        return Ok(json!({
                            "status": "fallback",
                            "clip_path": fallback,
                            "source_duration_s": duration_s,
                            "cached": false,
                            "warning": "yt-dlp not available, using fallback pool"
                        }));
                    }
                }
                return generate_procedural_background(&cache_dir, &cache_key, duration_s, &aspect).await;
            }
        }
    } else {
        report_progress(50.0, 100.0, "Using cached video, extracting clip...").await.ok();
    }

    // Get video duration via ffprobe
    let probe_output = tokio::process::Command::new("ffprobe")
        .arg("-v").arg("error")
        .arg("-show_entries").arg("format=duration")
        .arg("-of").arg("default=noprint_wrappers=1:nokey=1")
        .arg(&full_video_path)
        .output()
        .await;

    let source_duration_s: f64 = match probe_output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(duration_s)
        }
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
    let (crop_w, crop_h) = match aspect.as_str() {
        "9:16" => (720, 1280),   // vertical
        "16:9" => (1280, 720),   // horizontal
        "1:1" => (1080, 1080),   // square
        _ => (720, 1280),
    };

    // Extract clip with crop
    let crop_filter = format!("crop={}:{}", crop_w, crop_h);
    let extract_result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss").arg(start_s.to_string())
        .arg("-i").arg(&full_video_path)
        .arg("-t").arg(duration_s.to_string())
        .arg("-vf").arg(&crop_filter)
        .arg("-c:v").arg("libx264")
        .arg("-preset").arg("fast")
        .arg("-crf").arg("23")
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
                "cached": full_cached,
            }))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(ToolError::Ffmpeg(format!("FFmpeg clip extraction failed: {}", stderr)))
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
    let (w, h) = match aspect {
        "9:16" => (1080, 1920),
        "16:9" => (1920, 1080),
        "1:1" => (1080, 1080),
        _ => (1080, 1920),
    };
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
        .arg("-filter_complex").arg(&filter)
        .arg("-map").arg("[v]")
        .arg("-c:v").arg("libx264")
        .arg("-preset").arg("fast")
        .arg("-crf").arg("23")
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
        return Err(ToolError::Ffmpeg(format!("Procedural background failed: {}", stderr)));
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
    let output_path = default_str(&args, "output_path", "artifacts/background_assignments.json");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str).map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Extract scene IDs, speakers, and durations from manifest
    let mut scene_ids = Vec::new();
    let mut scene_speakers = Vec::new();
    let mut scene_durations = Vec::new();

    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            scene_ids.push(seg.get("scene_id").and_then(|v| v.as_str()).unwrap_or("").to_string());
            scene_speakers.push(seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("").to_string());
            let dur_ms = seg.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(3000);
            scene_durations.push(dur_ms as f64 / 1000.0);
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

// ---------------------------------------------------------------------------
// Handler: sticker.load_preset — load SVG preset config
// ---------------------------------------------------------------------------

async fn handle_sticker_load_preset(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
    let preset: StickerPreset = serde_json::from_str(&preset_json).map_err(|e| {
        ToolError::InvalidArg(format!("Failed to parse preset.json: {}", e))
    })?;

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

    report_progress(30.0, 100.0, "Extracting amplitude...").await.ok();

    // Extract amplitude from WAV
    let amplitude = extract_amplitude(wav_path, fps).map_err(|e| {
        ToolError::InvalidArg(format!("Amplitude extraction failed: {}", e))
    })?;

    report_progress(60.0, 100.0, "Generating composition...").await.ok();

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

    report_progress(100.0, 100.0, "Sticker composition generated").await.ok();

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "preset_name": preset_name,
        "position": position,
        "scale": scale,
        "frame_count": amplitude.frames.len(),
        "duration_ms": amplitude.duration_ms,
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.to_timeline — orchestrator for from-scratch video creation
// ---------------------------------------------------------------------------

async fn handle_script_to_timeline(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts");
    let skip_background = default_bool(&args, "skip_background", false);
    let skip_stickers = default_bool(&args, "skip_stickers", false);

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str).map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;
    let errors = validate_script(&spec);
    if !errors.is_empty() {
        return Err(ToolError::InvalidArg(format!("Script validation failed: {} errors", errors.len())));
    }

    let voices_dir = format!("{}/voices", output_dir);
    let stickers_dir = format!("{}/stickers", output_dir);
    std::fs::create_dir_all(&voices_dir)?;
    std::fs::create_dir_all(&stickers_dir)?;

    let mut warnings = Vec::new();

    // Step 1: Generate voices
    report_progress(0.0, 100.0, "Step 1/5: Generating voices...").await.ok();
    let voices_result = handle_script_generate_voices(json!({
        "script": script_input,
        "output_dir": voices_dir,
    })).await?;

    let manifest_path = voices_result.get("manifest_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArg("No manifest_path in voices result".into()))?
        .to_string();
    let total_duration_ms = voices_result.get("total_duration_ms")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Step 2: Build captions
    report_progress(20.0, 100.0, "Step 2/5: Building captions...").await.ok();
    let captions_path = format!("{}/captions.ass", output_dir);
    let _captions_result = handle_script_build_captions(json!({
        "script": script_input,
        "voiceover_manifest": manifest_path,
        "output_path": captions_path,
    })).await?;

    // Step 3: Fetch + assign backgrounds
    report_progress(40.0, 100.0, "Step 3/5: Fetching backgrounds...").await.ok();
    let mut background_pool: Vec<String> = spec.background.fallback_pool.clone();

    if !skip_background && spec.background.r#type == "gameplay" && !spec.background.query.is_empty() {
        // Fetch a background clip
        let fetch_result = handle_background_fetch(json!({
            "query": spec.background.query,
            "duration_s": total_duration_ms as f64 / 1000.0,
            "aspect": spec.meta.aspect,
            "fallback_pool": spec.background.fallback_pool,
        })).await;

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
        })).await?;
    }

    // Step 4: Render stickers (if enabled)
    report_progress(60.0, 100.0, "Step 4/5: Rendering stickers...").await.ok();
    let mut sticker_paths: Vec<serde_json::Value> = Vec::new();

    if !skip_stickers && spec.stickers.enabled {
        let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;

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
                let preset_name = speaker_spec.map(|s| s.preset.clone()).unwrap_or_else(|| "default_person".to_string());
                let position = speaker_spec.map(|s| s.position.clone()).unwrap_or_else(|| "top-left".to_string());
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
                })).await;

                match sticker_result {
                    Ok(r) => {
                        sticker_paths.push(json!({
                            "speaker": speaker,
                            "start_ms": start_ms,
                            "html_path": r.get("output_path").and_then(|v| v.as_str()).unwrap_or(""),
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
    report_progress(80.0, 100.0, "Step 5/5: Assembling timeline...").await.ok();
    let timeline_path = format!("{}/timeline.json", output_dir);

    // Build a proper Timeline struct — use the first background as the "source" video
    // (for from-scratch videos, the background IS the source)
    let bg_source = background_pool.first().cloned()
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
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    let mut current_ms = 0i64;
    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            let scene_id = seg.get("scene_id").and_then(|v| v.as_str()).unwrap_or("");
            let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let dur_ms = seg.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(3000);
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
                    voice_profile_id: seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("").to_string(),
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
    timeline.add_asset("captions", "ass".to_string(), json!({"path": captions_path}));

    // Add music if specified
    if let Some(ref music) = spec.music {
        if !music.path.is_empty() {
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
                    ducking_policy: if music.ducking { "auto".to_string() } else { "none".to_string() },
                },
            };
            timeline.add_track_event(TrackType::Music, music_event);
            timeline.add_asset("music", "music_bg".to_string(), json!({"path": music.path}));

            // Add ducking directive
            if music.ducking {
                timeline.add_ducking_directive("voiceover", "music", music.ducking_depth_db, 50, 200);
            }
        }
    }

    // Save timeline
    timeline.save(&timeline_path)?;

    report_progress(100.0, 100.0, "Timeline assembled").await.ok();

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
    let output_path = default_str(&args, "output_path", "output.mp4");
    let output_dir = default_str(&args, "output_dir", "artifacts");
    let skip_background = default_bool(&args, "skip_background", false);
    let skip_stickers = default_bool(&args, "skip_stickers", false);
    let preview_mode = default_bool(&args, "preview_mode", false);

    // Parse script for render config
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str).map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    report_progress(0.0, 100.0, "Phase 1/3: Building timeline...").await.ok();

    // Step 1: Build the timeline
    let timeline_result = handle_script_to_timeline(json!({
        "script": script_input,
        "output_dir": output_dir,
        "skip_background": skip_background,
        "skip_stickers": skip_stickers,
    })).await?;

    let timeline_path = timeline_result.get("timeline_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArg("No timeline_path in result".into()))?
        .to_string();
    let warnings = timeline_result.get("warnings").cloned().unwrap_or(serde_json::Value::Null);

    report_progress(40.0, 100.0, "Phase 2/3: Building layered composition...").await.ok();

    // Load manifest
    let manifest_path = timeline_result.get("voiceover_manifest")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let captions_path = timeline_result.get("captions_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let total_duration_ms = timeline_result.get("total_duration_ms")
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
            let dur_ms = seg.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(3000);
            scene_durations.push(dur_ms as f64 / 1000.0);
        }
    }

    // Build per-scene background clips using change_cadence
    let fallback_pool = if !spec.background.fallback_pool.is_empty() {
        spec.background.fallback_pool.clone()
    } else {
        // Scan the backgrounds directory for available clips
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

    // Assign backgrounds per scene based on cadence
    let mut backgrounds: Vec<openscript_ffmpeg::multilayer_render::BackgroundClip> = Vec::new();
    let mut pool_idx = 0usize;
    let mut last_speaker = String::new();

    for (i, &dur) in scene_durations.iter().enumerate() {
        let speaker = manifest.get("segments")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.get(i))
            .and_then(|s| s.get("speaker"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let should_change = match spec.background.change_cadence.as_str() {
            "speaker" => speaker != last_speaker,
            "fixed" => i == 0,
            _ => true, // "scene"
        };

        if should_change || backgrounds.is_empty() {
            if !backgrounds.is_empty() {
                pool_idx = (pool_idx + 1) % fallback_pool.len();
            }
            last_speaker = speaker.to_string();
        }

        let bg_path = if spec.background.change_cadence == "fixed" {
            fallback_pool[0].clone()
        } else {
            fallback_pool[pool_idx].clone()
        };

        backgrounds.push(openscript_ffmpeg::multilayer_render::BackgroundClip {
            path: bg_path,
            duration_s: dur,
            looped: true,
        });
    }

    // Build sticker overlays (from script spec speakers)
    let mut stickers: Vec<openscript_ffmpeg::multilayer_render::StickerOverlay> = Vec::new();
    if !skip_stickers {
        let mut current_ms = 0i64;
        if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
            for seg in segments {
                let speaker_name = seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
                let end_ms = seg.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(current_ms + 3000);

                if let Some(speaker_spec) = spec.speakers.get(speaker_name) {
                    // Look for sticker PNG: mcp/assets/stickers/speaker_{name}_{position}.png
                    let position_parts: Vec<&str> = speaker_spec.position.split('-').collect();
                    let facing = position_parts.last().unwrap_or(&"left");
                    let sticker_path = format!("mcp/assets/stickers/speaker_{}_{}.png", speaker_name, facing);

                    if std::path::Path::new(&sticker_path).exists() {
                        stickers.push(openscript_ffmpeg::multilayer_render::StickerOverlay {
                            path: sticker_path,
                            start_s: current_ms as f64 / 1000.0,
                            end_s: end_ms as f64 / 1000.0,
                            position: speaker_spec.position.clone(),
                            scale: speaker_spec.scale,
                        });
                    }
                }

                current_ms = end_ms;
            }
        }
    }

    // Get music path
    let timeline_json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&timeline_path)?)?;
    let music_path = spec.music.as_ref()
        .map(|m| m.path.clone())
        .filter(|p| std::path::Path::new(p).exists());

    // Build timeline preview for agent inspection
    let bg_assignments: Vec<openscript_core::timeline_preview::BackgroundClipAssignment> = backgrounds.iter()
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

    let sticker_assignments: Vec<openscript_core::timeline_preview::StickerAssignment> = stickers.iter()
        .map(|s| openscript_core::timeline_preview::StickerAssignment {
            start_ms: (s.start_s * 1000.0) as i64,
            end_ms: (s.end_s * 1000.0) as i64,
            path: s.path.clone(),
            position: s.position.clone(),
            scale: s.scale,
            speaker: String::new(),
        })
        .collect();

    let layered_timeline = openscript_core::timeline_preview::build_layered_timeline(
        &manifest,
        &bg_assignments,
        music_path.as_deref(),
        spec.music.as_ref().map(|m| m.ducking).unwrap_or(false),
        &sticker_assignments,
        Some(captions_path),
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

    report_progress(60.0, 100.0, "Phase 3/3: Rendering multi-layer video...").await.ok();

    // Build multi-layer render spec
    use openscript_ffmpeg::multilayer_render::{render_multilayer, MultiLayerRenderSpec};
    let render_spec = MultiLayerRenderSpec {
        backgrounds,
        voiceover_paths,
        stickers,
        music_path,
        music_volume: 10f64.powf(spec.music.as_ref().map(|m| m.gain_db).unwrap_or(-18.0) / 20.0),
        ducking: spec.music.as_ref().map(|m| m.ducking).unwrap_or(false),
        ducking_depth_db: spec.music.as_ref().map(|m| m.ducking_depth_db).unwrap_or(12.0),
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
        preset: if preview_mode { "ultrafast".to_string() } else { "fast".to_string() },
        total_duration_s,
    };

    let render_result = render_multilayer(&render_spec).await;

    match render_result {
        Ok(out_path) => {
            let file_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            report_progress(100.0, 100.0, "Video created").await.ok();

            // Include timeline preview in the response for agent inspection
            Ok(json!({
                "status": "rendered",
                "output_path": out_path,
                "file_size_bytes": file_size,
                "timeline_path": timeline_path,
                "timeline_preview_path": preview_path,
                "timeline_preview": timeline_preview,
                "timeline_summary": timeline_summary,
                "timeline_issues": if timeline_issues.is_empty() { serde_json::Value::Null } else { json!(timeline_issues) },
                "voiceover_manifest": manifest_path,
                "captions_path": captions_path,
                "total_duration_ms": total_duration_ms,
                "scene_count": timeline_result.get("scene_count"),
                "speaker_count": timeline_result.get("speaker_count"),
                "background_count": render_spec.backgrounds.len(),
                "sticker_count": render_spec.stickers.len(),
                "warnings": warnings,
            }))
        }
        Err(e) => {
            Err(ToolError::Ffmpeg(format!("Render failed: {}", e)))
        }
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

    report_progress(0.0, 100.0, &format!("Searching for {}...", media_type)).await.ok();

    if media_type == "music" {
        // Try Pixabay music API
        let pixabay_key = std::env::var("PIXABAY_API_KEY").ok();
        if let Some(key) = pixabay_key {
            let url = format!(
                "https://pixabay.com/api/audio/?key={}&q={}&per_page={}",
                key,
                urlencoding::encode(query),
                limit
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.map_err(|e| ToolError::Asset(e.to_string()))?;
                    let hits = body.get("hits").cloned().unwrap_or(json!([]));
                    let mut results = Vec::new();

                    if let Some(arr) = hits.as_array() {
                        for hit in arr.iter().take(limit) {
                            let audio_url = hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
                            let title = hit.get("tags").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let duration = hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !audio_url.is_empty() {
                                let filename = format!("{}/{}_{}.mp3", output_dir, query.replace(' ', "_"), results.len());
                                match client.get(audio_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp.bytes().await.map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": title,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => tracing::warn!("[stock.fetch] Download failed: {}", e),
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(100.0, 100.0, &format!("Downloaded {} tracks", results.len())).await.ok();
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
        report_progress(100.0, 100.0, "Using local stock library").await.ok();
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
        let pixabay_key = std::env::var("PIXABAY_API_KEY").ok();
        if let Some(key) = pixabay_key {
            let url = format!(
                "https://pixabay.com/api/videos/?key={}&q={}&per_page={}&video_type=animation",
                key,
                urlencoding::encode(query),
                limit
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.map_err(|e| ToolError::Asset(e.to_string()))?;
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

                            let tags = hit.get("tags").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let duration = hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !video_url.is_empty() {
                                let filename = format!("{}/{}_{}.mp4", output_dir, query.replace(' ', "_"), results.len());
                                match client.get(video_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp.bytes().await.map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": tags,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => tracing::warn!("[stock.fetch] Download failed: {}", e),
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(100.0, 100.0, &format!("Downloaded {} videos", results.len())).await.ok();
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
        report_progress(100.0, 100.0, "Using local stock library").await.ok();
        return Ok(json!({
            "status": "fallback",
            "type": "video",
            "source": "local",
            "message": "Set PIXABAY_API_KEY env var to download from Pixabay. Using local stock library.",
            "local_library": "mcp/assets/backgrounds/",
        }));
    }

    Err(ToolError::InvalidArg(format!("Unknown media type: {}. Use 'music' or 'video'.", media_type)))
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

    report_progress(0.0, 100.0, "Downloading from YouTube...").await.ok();

    // Determine if query is a URL or a search term
    let is_url = query.starts_with("http://") || query.starts_with("https://") || query.starts_with("youtu.be");
    let cache_key = format!("{:x}", md5_hash(query.as_bytes()));
    let clip_path = format!("{}/{}_clip.mp4", cache_dir, cache_key);

    // If start_s is specified, use --download-sections to download only the range
    // This avoids downloading a 10-hour video when we only need 100 seconds
    if let Some(start) = start_s {
        let end = start + duration_s;
        let start_fmt = format_seconds_to_timestamp(start);
        let end_fmt = format_seconds_to_timestamp(end);
        let section_arg = format!("*{}-{}", start_fmt, end_fmt);

        report_progress(20.0, 100.0, &format!("Downloading range {}-{}...", start_fmt, end_fmt)).await.ok();

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
                report_progress(70.0, 100.0, "Cropping to aspect ratio...").await.ok();
                let (crop_w, crop_h) = match aspect.as_str() {
                    "9:16" => (720, 1280),
                    "16:9" => (1280, 720),
                    "1:1" => (1080, 1080),
                    _ => (720, 1280),
                };

                let cropped_path = format!("{}/{}_cropped.mp4", cache_dir, cache_key);
                let crop_result = tokio::process::Command::new("ffmpeg")
                    .arg("-y")
                    .arg("-i").arg(&clip_path)
                    .arg("-vf").arg(format!("crop={}:{}", crop_w, crop_h))
                    .arg("-c:v").arg("libx264")
                    .arg("-preset").arg("fast")
                    .arg("-crf").arg("23")
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
                return Err(ToolError::Ffmpeg(format!(
                    "yt-dlp not available: {}", e
                )));
            }
        }
    }

    // No start_s specified — download full video (or use cache), then extract random clip
    let full_video_path = format!("{}/{}.mp4", cache_dir, cache_key);

    // Check cache first
    if Path::new(&full_video_path).exists() {
        report_progress(50.0, 100.0, "Using cached video...").await.ok();
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
                report_progress(50.0, 100.0, "Downloaded, extracting clip...").await.ok();
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
        .arg("-v").arg("error")
        .arg("-show_entries").arg("format=duration")
        .arg("-of").arg("default=noprint_wrappers=1:nokey=1")
        .arg(&full_video_path)
        .output()
        .await;

    let source_duration_s: f64 = match probe_output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(duration_s)
        }
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
    let (crop_w, crop_h) = match aspect.as_str() {
        "9:16" => (720, 1280),
        "16:9" => (1280, 720),
        "1:1" => (1080, 1080),
        _ => (720, 1280),
    };

    // Extract clip with crop
    let extract_result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss").arg(start_s.to_string())
        .arg("-i").arg(&full_video_path)
        .arg("-t").arg(duration_s.to_string())
        .arg("-vf").arg(format!("crop={}:{}", crop_w, crop_h))
        .arg("-c:v").arg("libx264")
        .arg("-preset").arg("fast")
        .arg("-crf").arg("23")
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
            Err(ToolError::Ffmpeg(format!("Clip extraction failed: {}", stderr)))
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

    report_progress(0.0, 100.0, "Searching YouTube...").await.ok();

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
                    let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown");
                    let url = entry.get("url").and_then(|v| v.as_str()).map(|s| s.to_string())
                        .or_else(|| entry.get("id").and_then(|v| v.as_str()).map(|id| format!("https://youtube.com/watch?v={}", id)))
                        .unwrap_or_default();
                    let duration = entry.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let view_count = entry.get("view_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let uploader = entry.get("uploader").and_then(|v| v.as_str()).unwrap_or("Unknown");

                    results.push(json!({
                        "title": title,
                        "url": url,
                        "duration_s": duration,
                        "view_count": view_count,
                        "uploader": uploader,
                    }));
                }
            }

            report_progress(100.0, 100.0, &format!("Found {} results", results.len())).await.ok();

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

    report_progress(0.0, 100.0, &format!("Searching Pixabay for {}...", media_type)).await.ok();

    let pixabay_key = std::env::var("PIXABAY_API_KEY").ok();

    if let Some(key) = pixabay_key {
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
                let body: serde_json::Value = resp.json().await
                    .map_err(|e| ToolError::Asset(e.to_string()))?;

                let total = body.get("totalHits").and_then(|v| v.as_u64()).unwrap_or(0);
                let hits = body.get("hits").cloned().unwrap_or(json!([]));

                let results: Vec<serde_json::Value> = hits.as_array()
                    .map(|arr| {
                        arr.iter().take(limit).map(|hit| {
                            let title = hit.get("tags").and_then(|v| v.as_str()).unwrap_or("Unknown");
                            let duration = hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                            let user = hit.get("user").and_then(|v| v.as_str()).unwrap_or("Unknown");
                            let views = hit.get("views").and_then(|v| v.as_u64()).unwrap_or(0);
                            let likes = hit.get("likes").and_then(|v| v.as_u64()).unwrap_or(0);

                            if media_type == "music" {
                                let preview_url = hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
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
                                let thumb = hit.get("previewURL").and_then(|v| v.as_str()).unwrap_or("");
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
                        }).collect()
                    })
                    .unwrap_or_default();

                report_progress(100.0, 100.0, &format!("Found {} results", results.len())).await.ok();

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
    report_progress(100.0, 100.0, "Using local stock library").await.ok();

    if media_type == "music" {
        let index_path = std::env::var("OPENSCRIPT_MUSIC_INDEX")
            .unwrap_or_else(|_| "mcp/assets/music_index.json".to_string());
        if let Ok(content) = std::fs::read_to_string(&index_path) {
            if let Ok(index) = serde_json::from_str::<serde_json::Value>(&content) {
                let assets = index.get("assets").cloned().unwrap_or(json!([]));
                let results: Vec<serde_json::Value> = assets.as_array()
                    .map(|arr| arr.iter().filter(|a| {
                        let title = a.get("title").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        let mood = a.get("mood").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        title.contains(&query.to_lowercase()) || mood.contains(&query.to_lowercase())
                    }).cloned().collect())
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
