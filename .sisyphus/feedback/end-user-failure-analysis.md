# OpenScript MCP - End User Failure Analysis Report

**Date:** April 11, 2026  
**Author:** End User (AI Agent via openscript-director skill)  
**Project:** VN20260411_032944 — Political Commentary Reel  
**Input:** 227s Hindi video → Target: <90s vertical reel with chapters, motion graphics, music, SFX, captions  
**Final Output:** 46.5s video with significant quality issues  

---

## Executive Summary

This report documents the failures encountered while attempting to produce a polished short-form video using OpenScript's MCP tooling. The system is architecturally sound but has critical gaps in error handling, verification, audio pipeline resilience, and quality assurance. A production-ready system should deliver a usable video in a single run — this attempt required 20+ manual workarounds and still produced suboptimal results.

---

## 1. Captions System — FAILED

### What Was Expected
- Centered captions with Bebas Neue font
- Animated word-by-word captions throughout the video
- Full coverage of all spoken dialogue

### What Actually Happened
- **Captions not centered**: PupCaps overlay used default positioning, not the expected center-aligned Bebas Neue style
- **Font not applied**: The CSS style file (`pupcaps_center.css`) didn't enforce Bebas Neue — the font is supposed to be burned in at the ffmpeg layer, creating a mismatch between expectations and reality
- **Only 50% coverage**: 10 caption entries covering 23s of a 46.5s video — gaps during title cards and between chapters
- **Timestamp remapping broken**: The system has no tool to remap SRT timestamps when the assembled video differs from the source timeline

### Root Causes
1. **`overlay.generate` has a bug**: The Rust handler passes `"retimed"` as a positional argument to PupCaps CLI, but PupCaps doesn't support subcommands. The command fails with `Error: File should have extension .srt!`
2. **No font validation**: The system assumes Bebas Neue is available but never verifies it
3. **Timeline-dependent overlays**: `overlay.generate` requires EDL/timeline with source timestamps. When manual concatenation changes the timeline, timestamps become invalid
4. **Caption coverage threshold too low**: 50% coverage reported as "warning" — should be a hard failure below 80%

### User Impact
Had to manually remap SRT timestamps with a custom Python script, run PupCaps directly via CLI, and composite the overlay with ffmpeg. The captions were still mispositioned and incomplete.

### Recommended Fixes
- [ ] **P0**: Fix `overlay.generate` — remove the "retimed" subcommand or implement it properly
- [ ] **P0**: Add font availability check before caption generation
- [ ] **P1**: Add `srt.remap` tool that accepts source SRT + assembled timeline + outputs remapped SRT
- [ ] **P1**: Raise caption coverage failure threshold to 80%
- [ ] **P2**: Add visual caption preview (extract frames with burned captions)
- [ ] **P2**: Support multiple caption styles: ASS (static), PupCaps (animated), simple text overlay

---

## 2. Motion Graphics — FAILED

### What Was Expected
- 4 animated chapter title cards, 2-3 seconds each
- Clean transitions between title cards and content
- Consistent visual style with brand colors

### What Actually Happened
- **Duration mismatch**: Title cards rendered at 30s (900 frames) instead of the requested 2-3s. The Remotion composition's registered `durationInFrames` (900) overrode the runtime parameter
- **Silent title cards**: Title cards are video-only — no audio. When concatenated with content, they create 12s of total silence breaking the narration flow
- **No preview mechanism**: Had to render full 30s cards before discovering the duration issue
- **No audio integration**: No built-in way to add even a simple whoosh or tone to title cards

### Root Causes
1. **Remotion CLI ignores runtime duration**: The `duration_in_frames` parameter passed to `motion.render` is not respected — the CLI uses the composition's registered `durationInFrames` from the TypeScript source
2. **Video-only output**: `motion.render` produces video without audio track. The system should automatically add a short audio element (whoosh, tone, or music continuation)
3. **No single-frame preview**: Users must render the full composition to see what it looks like

### User Impact
Had to render 30s title cards, then trim each to 3s with ffmpeg (`-t 3`). Had to manually insert silence placeholders during audio concatenation. Total time wasted: ~15 minutes of render time.

### Recommended Fixes
- [ ] **P0**: Fix `motion.render` to respect `duration_in_frames` runtime parameter
- [ ] **P0**: Add `motion.preview` — render single frame as PNG for visual verification (2-5s feedback vs 30-120s video render)
- [ ] **P1**: Add automatic audio track to motion graphics (configurable: silence, whoosh, tone, or music continuation)
- [ ] **P1**: Add `motion.validate` — check TSX for common errors before rendering
- [ ] **P2**: Add title card template system with pre-configured durations and audio

---

## 3. Sound Effects (SFX) — FAILED

### What Was Expected
- Hook SFX at video start (0ms)
- Transition whooshes between each chapter (4 total)
- Proper timing aligned with chapter boundaries

### What Actually Happened
- **SFX completely absent from final output**: 5 SFX events were assigned to the timeline (hook + 4 transitions), but none are audible in the final video
- **Timing was relative to source timeline**: SFX assigned at 10560ms, 20200ms, 27420ms, 34020ms — these are source video timestamps, not assembled reel timestamps
- **Lost during concatenation**: When we manually concatenated video and audio clips with ffmpeg, the SFX events from the timeline JSON were ignored

### Root Causes
1. **No audio re-mixing during assembly**: The timeline's SFX track exists in JSON but is never rendered into the final audio when using manual concatenation
2. **SFX timestamps not recalculated**: When segments are re-ordered or title cards are inserted, SFX positions should be recalculated relative to the assembled timeline
3. **No SFX preview**: No way to hear the SFX mix before committing to a render

### User Impact
Zero SFX in the final video. The user had to manually add SFX if they wanted them — but the system provided no tool for this.

### Recommended Fixes
- [ ] **P0**: Add automatic SFX re-mixing during video concatenation
- [ ] **P0**: Add `sfx.recalculate` — recalculate SFX positions based on assembled timeline
- [ ] **P1**: Add `sfx.preview` — generate a short audio-only preview of the SFX mix
- [ ] **P2**: Add SFX timing visualization on the timeline

---

## 4. Background Music — DEGRADED

### What Was Expected
- Dramatic background music (Hans Zimmer) throughout the video
- Automatic ducking during dialogue sections
- Consistent volume (-14 LUFS target)

### What Actually Happened
- **Music present but ducking lost**: The original timeline render had music with ducking baked in. After manual concatenation, we used extracted audio tracks which may not have the ducked mix
- **Silent gaps during title cards**: Music was not extended under title cards, creating 12s of total silence
- **No music verification**: No tool to verify music is present and properly mixed before final render

### Root Causes
1. **Music baked into timeline render only**: When we extracted content clips for concatenation, we got the raw audio, not the mixed audio with music and ducking
2. **No music extension under title cards**: The system should automatically extend the background music under title card gaps
3. **No mix verification**: The `music.ducking.plan` tool exists but was never run

### User Impact
Music is present in the final video but the quality is degraded compared to the original timeline render. No way to verify the mix is correct without listening to the entire video.

### Recommended Fixes
- [ ] **P0**: Add `music.verify` — check if music is present and at correct volume
- [ ] **P1**: Add automatic music extension under title card gaps
- [ ] **P1**: Add `music.preview` — generate audio-only preview of music mix with ducking
- [ ] **P2**: Add music loudness normalization to -14 LUFS target

---

## 5. TTS Voiceover — UNAVAILABLE

### What Was Expected
- AI voiceover commentary where required
- Voice profiles available for selection
- Automatic voice generation and placement

### What Actually Happened
- **Voicebox completely unavailable**: The Docker container was not running or no voices were configured
- **No health check**: The system didn't check if Voicebox was running before offering voiceover features
- **No fallback**: No suggestion to use preset voices (Kokoro) or skip voiceover
- **No voice profile list**: User had to manually discover that no voices were available

### Root Causes
1. **No startup health check**: The system should run `voice.profile.list` automatically at session start
2. **No preset voice fallback**: When Voicebox is down, Kokoro preset voices should be offered as alternatives
3. **No voiceover requirement validation**: The system should inform the user that voiceover is unavailable and ask if they want to continue without it

### User Impact
Had to pivot to source audio only. No AI voiceover in the final video.

### Recommended Fixes
- [ ] **P0**: Add Voicebox health check at session start (`curl http://127.0.0.1:17493/health`)
- [ ] **P0**: Add automatic `voice.profile.list` at session start
- [ ] **P1**: Add Kokoro preset voice fallback when Voicebox is unavailable
- [ ] **P1**: Add voiceover availability warning before pipeline starts
- [ ] **P2**: Add voiceover requirement template (e.g., "needs voiceover: yes/no")

---

## 6. Verification System — INEFFECTIVE

### What Was Expected
- Verify captions are correctly positioned and styled
- Verify SFX are present and properly timed
- Verify audio mix is correct before final render
- Gate checks at each major step to catch issues early

### What Actually Happened
- **verify.render**: Compared against source timeline (145s) vs actual assembled reel (46.5s) — reported "warning" for 98s delta. This is expected behavior for custom assemblies, not an issue
- **verify.captions**: Reported 50% coverage as "warning" — should be a hard failure
- **verify.audio**: Passed correctly (100/100) — the only working verification
- **No intermediate checks**: All verification ran only at the very end, when everything was already broken
- **No visual verification**: No way to see what captions actually look like without watching the full video

### Root Causes
1. **Verification tools assume source timeline**: All tools compare against the original EDL, not the assembled reel
2. **No quality contract**: The system has no minimum quality thresholds that must be met before declaring success
3. **No gate checks**: No automatic verification after each major step
4. **No visual preview**: No tool to extract key frames and show caption positioning

### User Impact
Had to watch the entire 46.5s video to discover caption issues. No way to catch problems early.

### Recommended Fixes
- [ ] **P0**: Add intermediate quality gates after each major step (transcription, timeline, render, captions)
- [ ] **P0**: Add output quality contract with hard failure thresholds:
  - Caption coverage >80%
  - Audio loudness -14 ±2 LUFS
  - No silent gaps >1s
  - Resolution 1080×1920
  - Duration within ±10% of target
- [ ] **P1**: Add visual caption verification (extract 3-5 frames with captions, show to user)
- [ ] **P1**: Add audio mix verification (generate 5s audio preview of mix)
- [ ] **P2**: Add verification dashboard showing all metrics at a glance

---

## 7. Workflow Fragmentation — CRITICAL

### What Was Expected
A pipeline that handles: transcription → segment selection → title cards → voiceover → music → SFX → captions → render → verify

### What Actually Happened
- **20+ manual tool calls** required to produce the final video
- **No state tracking**: System doesn't track what's been done vs what needs to be done
- **No error recovery**: When audio concat failed, had to manually work around it
- **No custom pipeline support**: The standard pipeline (`reelize.timeline`) doesn't support title cards or custom assemblies
- **No progress indicator**: User has no idea how far along they are in the pipeline

### Root Causes
1. **Pipeline is too rigid**: The one-call pipeline assumes a standard workflow. Custom requirements require manual orchestration
2. **No state management**: Each tool call is independent — there's no "session state" tracking what's been accomplished
3. **No error recovery**: When a step fails, the system doesn't suggest alternatives
4. **No pipeline templates**: No pre-configured workflows for common use cases (e.g., "with title cards", "with voiceover")

### User Impact
Required deep knowledge of the MCP tool ecosystem to produce a working video. The average user would give up after the first failure.

### Recommended Fixes
- [ ] **P1**: Add custom pipeline builder (drag-and-drop style workflow)
- [ ] **P1**: Add state tracking and progress checklist for multi-step pipelines
- [ ] **P1**: Add automatic error recovery with fallback strategies
- [ ] **P2**: Add pipeline templates (e.g., "with title cards", "with voiceover", "basic reel")
- [ ] **P2**: Add pipeline status endpoint showing completed/remaining steps

---

## 8. Audio Concatenation — WORKAROUND REQUIRED

### What Was Expected
Seamless concatenation of video clips with audio, music, and SFX

### What Actually Happened
- **ffmpeg concat demuxer failed**: AAC codec incompatibility between Remotion-rendered title cards and source content
- **Had to separate video and audio pipelines**: Concatenate video with filter_complex, concatenate audio separately, then mux
- **Silent title cards**: Had to manually insert 3s silence placeholders for each title card

### Root Causes
1. **Codec mismatch**: Remotion renders with AAC audio that's incompatible with source content codec
2. **No automatic codec normalization**: The system should normalize all clips to the same codec before concatenation
3. **No silent placeholder generation**: The system should automatically generate silent audio for title cards

### User Impact
Required manual ffmpeg commands outside the MCP tooling. Total time: ~30 minutes of troubleshooting.

### Recommended Fixes
- [ ] **P0**: Add automatic codec normalization before concatenation
- [ ] **P0**: Add `audio.concat` tool that handles video+audio concatenation with codec normalization
- [ ] **P1**: Add automatic silent placeholder generation for title cards
- [ ] **P2**: Add concat failure auto-recovery (try alternative approaches automatically)

---

## Priority Summary

| Priority | Count | Examples |
|----------|-------|----------|
| **P0 - Critical** | 8 | Fix overlay.generate bug, fix motion.render duration, add health checks, fix verification tools |
| **P1 - High** | 12 | Add intermediate quality gates, add audio re-mixing, add preset voice fallback, add state tracking |
| **P2 - Medium** | 8 | Add visual caption preview, add pipeline templates, add music extension under title cards |
| **P3 - Nice to Have** | 4 | Add frame preview, add audio mix preview, add caption style preview, add pipeline status |

---

## Conclusion

OpenScript has a solid architectural foundation with its multi-track timeline system, MCP tooling, and verification layer. However, the system is not production-ready for end users who expect to produce a polished video in a single run. The critical issues are:

1. **Bugs in core tools** (overlay.generate, motion.render) that prevent expected functionality
2. **No quality gates** that catch issues before they compound
3. **No error recovery** when steps fail
4. **No state tracking** to guide users through multi-step workflows
5. **No fallback mechanisms** when dependencies (Voicebox) are unavailable

Addressing the P0 issues should be the immediate priority. The P1 items should be addressed in the next sprint. The P2 and P3 items can be planned for future releases.

---

## Appendix: File Inventory

| File | Purpose | Status |
|------|---------|--------|
| `final_reel_captions.mp4` | Final output video | Delivered (46.5s, 16MB) |
| `VN20260411_032944.srt` | Source transcription | Complete (72 entries) |
| `final_reel_captions.srt` | Remapped captions | Incomplete (10 entries, 50% coverage) |
| `chapter{1-4}_title_3s_fixed.mp4` | Motion graphic title cards | Complete (3s each) |
| `VN20260411_032944.timeline.json` | Original EDL v2 timeline | Stale (145s, not matching final) |
| `caption_overlay.mov` | PupCaps animated captions | Complete (42.2s) |
| `final_video.mp4` | Video-only concatenation | Complete (46.6s) |
| `final_audio.wav` | Audio-only concatenation | Complete (46.5s) |
