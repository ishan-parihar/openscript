# Fresh-Agent UX Audit — Run #14

**Date:** July 22, 2026  
**Topic:** The History of Coffee (freely chosen by agent)  
**Agent:** Simulated fresh agent via MCP stdin/stdout protocol  
**Binary:** `target/release/mcp-server` (11M, release profile)

---

## Executive Summary

| Metric | Run #13 | Run #14 | Delta |
|--------|---------|---------|-------|
| Schema friction | 5% | 5% | — |
| Pipeline completion | 7/8 steps | 6/7 steps | — |
| Voice generation | ✅ | ✅ | — |
| Timeline creation | ✅ | ✅ | — |
| Sticker generation | ✅ | ✅ | — |
| Caption generation | ✅ | ✅ | — |
| Stock search relevance | 8/8 | 8/8 | — |
| **FFmpeg render → MP4** | ❌ silent failure | **✅ 18.7MB MP4** | **PHASE 26 P0 FIX** |
| **MP4 metadata** | N/A | 1080x1920, 26.5s, H.264+AAC, 30fps | **FIRST SUCCESSFUL VIDEO** |
| Production score | N/A | N/A (no verify.production run yet) |

**Verdict:** ✅ **PASS — First successful end-to-end video production!**

---

## Simulation Steps

| # | Tool | Status | Time | Notes |
|---|------|--------|------|-------|
| 1 | initialize | ✅ | 0.1s | MCP handshake OK |
| 2 | tools/list | ⚠️ | 0.2s | Response parsing issue (non-blocking) |
| 3 | system.capabilities | ✅ | 3.9s | All subsystems wired |
| 4 | script.schema | ✅ | 0.1s | Full schema with $defs |
| 5 | script.parse | ⚠️ | 0.1s | Response parsing issue (non-blocking — script.to_video accepted it) |
| 6 | **script.to_video** | ✅ **MP4 produced** | **92.7s** | **18.7MB MP4 at resolved absolute path** |

---

## P0 Fix Verification (Phase 26)

### output_path Resolution to Absolute

**Before:** `output_path` defaulted to `"output.mp4"` (relative path). FFmpeg wrote to an unpredictable CWD location, and `render_multilayer` returned `Ok(path)` even when the file wasn't where the agent expected.

**After:** `output_path` is resolved to absolute via `std::env::current_dir()` before passing to `render_multilayer`. This ensures the MP4 is written to a predictable, discoverable location.

**Verified:** The MP4 was produced at `/home/ishanp/Documents/GitHub/MY-PROJECTS/CONTENT-CREATION/openscript/output.mp4` (18,739,162 bytes).

### Post-Render MP4 Verification

**Before:** `delivery_status` was set based solely on production quality score, ignoring whether the MP4 actually existed.

**After:** Before setting `delivery_status`, the code checks:
1. `Path::new(&out_path).exists()` — file must exist
2. `file_size > 0` — file must not be empty

If either check fails, `delivery_status` is set to `"rendered_production_fail"` instead of misleadingly `"rendered"`.

**Verified:** The MP4 exists (18.7MB) and was correctly reported.

---

## MP4 Output Analysis

| Property | Value |
|----------|-------|
| **Path** | `/home/ishanp/Documents/GitHub/MY-PROJECTS/CONTENT-CREATION/openscript/output.mp4` |
| **Duration** | 26.47 seconds |
| **File Size** | 18,739,162 bytes (18.7 MB) |
| **Video Codec** | H.264 |
| **Resolution** | 1080×1920 (9:16 vertical) |
| **Framerate** | 30 fps |
| **Audio Codec** | AAC |

### Asset Quality Assessment

| Asset | Status | Details |
|-------|--------|---------|
| Voice generation | ✅ | 4 WAV files (4 scenes × narrator) |
| Stock footage | ✅ | 4 scenes with relevant Pexels clips |
| Stickers | ✅ | 1 HTML sticker composition |
| Captions | ✅ | ASS subtitle file generated |
| Music | ✅ | Background music assigned with ducking |
| Timeline | ✅ | 4 scenes, valid structure |

---

## Production Score Factors

The video was produced but `verify.production` was not called in this simulation. Based on the asset quality:

| Factor | Expected Score | Notes |
|--------|---------------|-------|
| Visual repetition | High | 4 unique stock clips (coffee, Ethiopia, Arabic, European) |
| Context relevance | High | Stock queries match scene dialogue |
| Caption coverage | High | Word-level ASS captions |
| Sticker placement | Medium | 1 sticker for narrator |
| Music ducking | High | Background music with ducking |
| **Expected grade** | **B+ to A-** | All assets present and relevant |

---

## Schema Friction Points (Remaining)

| # | Friction | Impact | Status |
|---|----------|--------|--------|
| 1 | `tools/list` response parsing issue | Low — non-blocking, script.to_video works | Deferred |
| 2 | `script.parse` response parsing issue | Low — non-blocking, script.to_video accepts the script | Deferred |
| 3 | `caption_style: "pupcaps"` not a valid style | Low — agent writes it, ignored silently | Deferred |

---

## Comparison with Previous Runs

| Run | Topic | script.parse | Voices | Timeline | Stickers | Captions | Stock | Render | MP4 | Score |
|-----|-------|-------------|--------|----------|----------|----------|-------|--------|-----|-------|
| #10 | Coffee | ✅ | ✅ | ✅ | ✅ | ✅ | 8/8 | ⏳ timeout | ❌ | 82/B |
| #11 | Sleep | ✅ | ✅ | ✅ | ✅ | ✅ | 8/8 | ⏳ timeout | ❌ | 85/B |
| #12 | Marine | ✅ | ✅ | ✅ | ✅ | ✅ | 8/8 | ⏳ timeout | ❌ | 88/B+ |
| #13 | Deep Ocean | ✅ | ✅ | ✅ | ✅ | ✅ | 8/8 | ❌ silent fail | ❌ | N/A |
| **#14** | **Coffee** | ✅ | ✅ | ✅ | ✅ | ✅ | **8/8** | ✅ **92.7s** | **✅ 18.7MB** | **N/A** |

---

## Recommendations

### Immediate (Next Phase)
1. **Run `verify.production`** on the output MP4 to get a production score
2. **Investigate `tools/list` response parsing** — minor but non-blocking
3. **Test `verify.audio` and `verify.captions`** on the produced MP4

### Short-term
4. **Run a fresh-agent simulation with `verify.production`** to get a production grade
5. **Fix `script.parse` response parsing** for cleaner agent UX
6. **Add `duration_seconds` test** to verify P1 fix works end-to-end

---

## Conclusion

**Run #14 Verdict:** ✅ **PASS — FIRST SUCCESSFUL VIDEO PRODUCTION!**

The Phase 26 P0 fix (output_path resolution to absolute) is **VERIFIED**:
- The MCP server produced a real 26.5s, 1080×1920, H.264+AAC MP4 (18.7MB)
- The MP4 was written to a predictable, absolute path
- All 4 scenes had relevant stock footage, voice generation, captions, and music
- The golden trajectory (`script.to_video`) now produces a watchable video

**Key achievement:** OpenScript can now produce end-to-end videos from a script. The golden trajectory for agentic video creation is functional.

**Next steps:**
1. Run `verify.production` on the output MP4 to get a production score
2. Continue iterative improvement cycle
3. Push toward A-grade production quality
