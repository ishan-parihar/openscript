# Fresh-Agent UX Audit — Run #14

**Date:** 2026-07-22 15:16
**Topic:** How Neural Networks Learn (agent freely chosen)
**Trajectory:** A (From-Scratch Video via script.parse → script.to_video)

## Tool Call Log

| Step | Status | Detail |
|------|--------|--------|
| initialize | ✅ | 0 tools in 0.0s |
| system.capabilities | ✅ | 2.7s |
| script.schema | ✅ | 0.0s |
| script.parse | ✅ | 0.0s |
| script.to_video | ✅ | 79s |
| verify.production | ✅ | 2.3s |
| file_check | ✅ | 20.5 MB |

## Score: 7/7

## Output Video

Path: `/home/ishanp/Documents/GitHub/MY-PROJECTS/CONTENT-CREATION/openscript/artifacts/run14/video.mp4`

## Issues Found

No issues found.

## UX Friction Points

1. Schema discovery: Agent must call script.schema before script.parse
2. Voice ID format: Agent must know to use "kokoro:af_heart" not "af_heart"
3. Stock query: Agent must provide stock_query for gameplay backgrounds
4. Render time: ~50s for 4 scenes with TTS + backgrounds + captions
