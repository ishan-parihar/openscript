# Brainstorm: Monolithic `audio.to_video` — Architectural & Implementation Gaps

**Date:** July 25, 2026  
**Trigger:** User observed the monolith output — looping solid-color placeholder video, no audio passthrough, no relevant b-roll for Hindi audio  
**Code analyzed:** `crates/openscript-mcp/src/tools.rs` (handler: lines 5494–5980), `crates/openscript-ffmpeg/src/multilayer_render.rs`, `crates/openscript-mcp/src/stock_signal.rs`

---

## BUG-1: MEDIUM — Audio Present But Potentially Wrong Content

**Root Cause:** The input audio path is passed as `voiceover_paths: vec![audio_path.to_string()]` (line 5889). The `render_multilayer` function treats this as a VOICEOVER — it writes a concat file and runs it through `concat=n=N` filter.

**Evidence (ffprobe on `/tmp/audio_to_video_1784914088.mp4`):**
- Audio stream EXISTS: aac, mono, 143.5s
- mean_volume: -11.3 dB (LOUDER than input at -21.9 dB)
- max_volume: -0.3 dB
- The audio is NOT silent — it has proper volume levels

**Conclusion:** The audio passthrough WORKS. The user may have tested a different file, or the audio content is correct but the user expected different audio (e.g., TTS voiceover instead of original speech).

**Remaining Risk:** No verification that the output audio matches the input audio content (only volume is checked). Add audio fingerprint comparison if needed.

---

## BUG-2: CRITICAL — Looping Solid-Color Placeholder Instead of B-Roll

**Root Cause:** When `PEXELS_API_KEY` is not set (or returns no results), the handler falls back to generating a solid-color MP4 using `ffmpeg -f lavfi -i color=c=0x1a1a2e` (line 5736–5746). This placeholder is then set to `looped: true` and used as the background.

**Evidence:** The output video shows a static dark blue (`#1a1a2e`) frame for the entire duration.

**Fix:**
- When no stock footage is available, the tool should **FAIL with a clear error** instead of silently producing a useless placeholder
- OR: Use `system.capabilities` to check for PEXELS_API_KEY before starting the pipeline
- OR: Generate procedural backgrounds (abstract motion, gradients) instead of flat color

---

## BUG-3: HIGH — Hindi/Hinglish Keyword Extraction Fails

**Root Cause:** The `stock_signal::build_scene_stock_query` function extracts keywords from the SRT text. The `NOISE_TOKENS` constant (line 32) only contains English stop words and listicle fillers. For Hinglish audio:

1. The transcription produces Latin-script Hinglish text (e.g., "yeh bahut important hai")
2. The keyword extraction picks up Hinglish words that are meaningless for Pexels search
3. The `build_scene_stock_query` doesn't translate Hinglish → English visual concepts
4. Fallback keywords ("abstract motion", "city timelapse") are generic and irrelevant

**Evidence:** The b-roll search queries would be garbled Hinglish phrases that Pexels can't match.

**Fix:**
- Add `stock_signal::hinglish_to_english_concepts()` function that maps common Hinglish phrases to English visual concepts (e.g., "sarkar" → "government", "inquilab" → "revolution protest")
- Fall back to `llm.complete` for unknown phrases
- Add Hinglish noise tokens to `NOISE_TOKENS` (e.g., "hai", "ho", "ka", "ki", "ke", "ko", "se", "mein", "par")
- This is a one-function change in `stock_signal.rs`, not a pipeline redesign

---

## BUG-4: HIGH — No Voiceover/TTS Generation

**Root Cause:** The `audio.to_video` handler treats the INPUT audio as the voiceover. It does NOT:
- Generate TTS voiceover from the transcript
- Add intro/outro narration
- Add transition commentary

**This means:** The output video has the original audio as "voiceover" — which is correct for A2V. But the tool description says it creates a "reel" which implies production value. The monolith doesn't add ANY production value beyond stock footage and captions.

**Architecture Gap:** The monolith can't make agentic decisions like:
- "This segment needs a hook voiceover at the start"
- "This transition needs a whoosh SFX"
- "This topic needs a different music mood"

---

## BUG-5: MEDIUM — `timeline.validate` Reports Valid on Empty Timeline

**Root Cause:** The validate function checks structural correctness but doesn't check if segments are actually populated. An empty timeline with no segments passes validation.

**Fix:** Add check: `timeline.segments.len() > 0` as a hard requirement.

---

## BUG-6: MEDIUM — `broll.plan` Returns Empty When Timeline Has No Segments

**Root Cause:** `broll.plan` reads segments from the timeline JSON. If the timeline is empty (which it always is after `timeline.build`), it returns 0 segments with no actionable error message.

**Fix:** Return error: "Timeline has no segments. Call timeline.add_segment first or use srt.to_timeline."

---

## ARCHITECTURAL GAP-1: No `srt.to_timeline` Tool

**Problem:** After `timeline.build`, the agent must call `timeline.add_segment` N times (once per SRT entry) to populate the timeline. This is:
- O(N) tool calls for N segments
- Requires the agent to parse SRT timestamps manually
- No batching mechanism exists

**Solution:** Add `srt.to_timeline` tool that reads an SRT file and populates a timeline with segments in one call.

---

## ARCHITECTURAL GAP-2: Monolith Hides Failures Silently

**Problem:** The monolith catches errors and continues with placeholders/fallbacks:
- Transcription fails → continues with empty SRT
- B-roll fetch fails → uses solid-color placeholder
- Music search fails → renders without music
- SFX search fails → renders without SFX

This means the output is ALWAYS produced, but it might be garbage.

**Solution:** The monolith should fail-fast on critical errors (transcription, render) and warn on non-critical ones (SFX, music).

---

## ARCHITECTURAL GAP-3: No Agent Decision Points

**Problem:** The monolith hardcodes every decision:
- Scene duration = SCENE_SIZE (4 SRT entries)
- Music mood = "neutral"
- Music energy = "medium"
- SFX = enabled by default
- Caption style = "word_highlight"
- No intro/outro voiceover
- No transition SFX

An agent would make BETTER decisions based on the content:
- Energetic content → upbeat music, fast cuts
- Serious content → calm music, slow transitions
- Hindi audio → English visual keywords for b-roll
- Long monologue → add transition SFX between topics

**Solution:** Keep the monolith as an escape hatch, but make the atomic tool chain the primary path.

---

## ARCHITECTURAL GAP-4: No Audio Verification

**Problem:** After rendering, there's no check that the output video actually has an audio stream. The `verify.render` tool checks technical integrity but doesn't verify audio presence.

**Solution:** Add audio stream check to `verify.render` and `render_multilayer`.

---

## Priority Fix Order

| # | Fix | Impact | Effort |
|---|-----|--------|--------|
| 1 | Add `srt.to_timeline` tool | Unblocks atomic chain | Medium |
| 2 | Fix audio passthrough in `render_multilayer` | Fixes silent output | Low |
| 3 | Fail-fast when no PEXELS_API_KEY | Prevents garbage output | Low |
| 4 | Add Hinglish → English keyword mapping | Fixes b-roll relevance | Medium |
| 5 | Fix `timeline.validate` to reject empty timelines | Prevents false positive | Low |
| 6 | Add audio verification to `verify.render` | Catches audio bugs | Low |
| 7 | Delete monolithic `audio.to_video` | Removes YAGNI deadweight | Low |

---

## Recommendation

**Don't delete `audio.to_video` yet.** Audit #22 scored atomic tools at 2.9/10 vs monolithic at 5.0/10. The monolith produces *some* output; the atomic chain produces *no* output. Instead:

1. **Fix the monolith's bugs** (keyword extraction, placeholder fallback) so it produces USEFUL output
2. **Add `srt.to_timeline` tool** so the atomic chain can also produce output
3. **Only then** delete the monolith once the atomic chain is proven

The monolith is a crutch, not a solution — but we need the crutch until the replacement works.
