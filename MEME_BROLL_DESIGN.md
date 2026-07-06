# Meme B-Roll from GIPHY GIFs — Design Document

## Concept

Meme b-roll = short contextual reaction GIF clips that pop in during specific moments of a scene, like TikTok reaction videos. Unlike speaker stickers (which persist for the whole scene), meme b-rolls are **brief** (2-3s), **emotional** (reaction to what's being said), and **dynamic** (pop-in animation, then disappear).

## Current System (Sticker Mode)

- GIPHY sticker downloaded **per speaker**, placed as persistent overlay
- Stays on screen for the entire speaker's segment
- Search query is **mood-aware** (calm → meditation, energetic → fire)
- Position: corner (top-left, bottom-right, etc.)

## New System (Meme B-Roll Mode)

- GIPHY GIF downloaded **per scene**, plays for 2-3 seconds at a specific moment
- Search query is **emotion-aware** (derived from scene text + emote)
- Position: center or center-bottom (distinct from corner stickers)
- Animation: pop-in (scale 0→1 in 200ms) + pop-out (fade out in 300ms)
- Can run alongside sticker mode (stickers = speaker identity, memes = reactions)

## GIPHY SDK Features to Leverage

1. **`/v1/gifs/translate`** — Returns the single best-match GIF for a phrase. Perfect for "translate this scene's emotional beat into a reaction GIF."
2. **`/v1/gifs/search`** with `lang=en` — Search for reaction GIFs by emotional keyword.
3. **`/v1/stickers/trending`** — Fallback when search returns nothing.
4. **`random_id`** parameter — Per-session anonymized ID for better trending personalization.

## Implementation Plan

### 1. New ScriptSpec fields

```json
{
  "meme_brolls": {
    "enabled": true,
    "position": "center-bottom",
    "scale": 0.35,
    "duration_s": 2.5,
    "offset_s": 0.3,
    "query_strategy": "translate"
  }
}
```

- `position`: "center", "center-bottom", "center-top" (default "center-bottom")
- `scale`: fraction of canvas width (default 0.35 = 35%)
- `duration_s`: how long each meme plays (default 2.5s)
- `offset_s`: delay after scene start before meme appears (default 0.3s)
- `query_strategy`: "translate" (GIPHY translate endpoint, 1 best match) or "search" (GIPHY search, pick from results)

### 2. Emotion extraction from scene text

Extract the emotional beat of each scene:
- "surprising" → "shocked reaction", "mind blown", "wait what"
- "funny" → "laughing", "lol reaction", "funny"
- "motivational" → "hype", "let's go", "motivated"
- "sad" → "sad reaction", "crying", "emotional"
- "confused" → "confused reaction", "what", "thinking"
- "excited" → "excited", "yes", "celebration"
- Default → "reaction" (generic)

Detection: keyword matching in scene text + scene.emote field.

### 3. GIPHY translate endpoint integration

```
GET /v1/gifs/translate?api_key=KEY&s={emotion_query}
→ Returns single best-match GIF
```

This is better than search for meme b-rolls because:
- Returns exactly 1 result (no filtering needed)
- GIPHY's algorithm picks the most culturally relevant reaction
- Faster (less data transferred)

### 4. FFmpeg rendering with pop-in animation

```ffmpeg
[meme_input]scale=W*0.35:-1,fade=in:0:5,fade=out:st=2.2:d=0.3[meme_scaled];
[base][meme_scaled]overlay=x=(W-w)/2:y=(H-h)*0.7:enable='between(t,start+offset,start+offset+duration)'
```

- Scale to 35% of canvas width
- Fade in over 5 frames (~167ms at 30fps)
- Fade out over 0.3s before disappearing
- Positioned at 70% height (center-bottom)
- Only visible during the `enable` window

### 5. ScriptSpec schema changes

```rust
// New struct in script.rs
pub struct MemeBrollSpec {
    pub enabled: bool,
    pub position: String,      // "center-bottom" default
    pub scale: f64,            // 0.35 default
    pub duration_s: f64,       // 2.5 default
    pub offset_s: f64,         // 0.3 default
    pub query_strategy: String, // "translate" default
}

// Add to ScriptSpec
pub meme_brolls: Option<MemeBrollSpec>,
```

### 6. Integration in handle_script_to_video

After sticker download, before render:
1. If `meme_brolls.enabled`, for each scene:
   - Extract emotion from scene text
   - Call GIPHY translate/search
   - Download GIF
   - Create a `MemeBrollOverlay` with start/end times
2. Add `meme_brolls: Vec<MemeBrollOverlay>` to `MultiLayerRenderSpec`
3. In `multilayer_render.rs`, add meme b-roll overlay loop (similar to stickers but with fade + enable window)

### 7. Agent experience

An agent enables meme b-rolls by adding one field to the script:
```json
{
  "meme_brolls": {"enabled": true},
  "output": {"theme": "energetic"}
}
```

The system automatically:
- Detects each scene's emotional beat
- Fetches a contextual reaction GIF from GIPHY
- Places it center-bottom with a pop-in/fade-out animation
- Times it to appear 0.3s after the scene starts and play for 2.5s

### 8. Use cases

- **Energetic/edu content**: "Did you know X?" → mind-blown reaction GIF
- **Motivational content**: "Start today!" → hype/let's-go reaction GIF
- **Comedy content**: punchline → laughing reaction GIF
- **Tutorial content**: "This is important!" → attention/notice reaction GIF

## Phased Implementation

| Phase | Description | Effort |
|-------|-------------|--------|
| BE-1 | Add MemeBrollSpec to ScriptSpec + schema | Low |
| BE-2 | Add emotion extraction from scene text | Medium |
| BE-3 | Add GIPHY translate endpoint integration | Medium |
| BE-4 | Add meme broll overlay to multilayer_render | High |
| BE-5 | Add CLI mirror + AGENT_GUIDE docs | Low |
| BE-6 | Test with fresh-agent simulation | — |
