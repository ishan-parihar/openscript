# Video Search Architecture Upgrade

## Problem
Current search extracts keywords from individual scene text only:
- "Did you know octopuses have three hearts?" → searches "did you know octopuses three hearts"
- This is contextually correct for the SENTENCE but not for the VIDEO TOPIC
- A brain video scene about "breathing" gets generic breathing clips, not neuroscience breathing

## Solution: Topic-Aware Keyword System

### New ScriptSpec field: `video_keywords`
```json
{
  "title": "3 Surprising Facts About the Human Brain",
  "video_keywords": ["brain", "neuroscience", "neurons", "science", "mind"],
  "scenes": [...]
}
```

The agent (or auto-extraction from title) provides 3-5 topic keywords that
represent the WHOLE video. These are prepended to every scene's Pexels/GIPHY
search to bias results toward the video's topic.

### How it works (two-pass)

**Pass 1 — Script generation (agent)**:
The agent writes the script with `video_keywords` at the top level.

**Pass 2 — Video search (OpenScript)**:
For each scene, OpenScript builds a search query by combining:
1. Video topic keywords (from `video_keywords`) — provides CONTEXT
2. Scene-specific keywords (from `extract_keywords(scene_text)`) — provides SPECIFICITY  
3. Theme context (from `output.theme`) — provides MOOD

Example:
- Video topic: "brain neuroscience neurons"
- Scene text: "Did you know your brain has 86 billion neurons?"
- Old query: "did you know brain 86 billion neurons" (too long, mixed)
- New query: "brain neurons" (topic keywords) + "86 billion" (scene keywords) = "brain neurons 86 billion"
- Pexels returns: neuroscience/neuron footage, not generic "did you know" clips

### For GIPHY memes:
- Current: `extract_emotion_query(scene_text)` → "mind blown reaction" (generic)
- New: `extract_emotion_query(scene_text)` + topic context → "brain mind blown reaction" 
- GIPHY returns: science/smart reaction GIFs, not generic mind-blown memes

### For GIPHY stickers:
- Current: `build_sticker_query(theme)` → "meditation" or "fire" (theme-only)
- New: `build_sticker_query(theme, video_keywords)` → "brain science" (topic-aware)
- GIPHY returns: brain/science stickers, not generic meditation clips

### Implementation

1. Add `video_keywords: Vec<String>` to `ScriptSpec` (optional, defaults to empty)
2. Auto-extract topic keywords from `title` if `video_keywords` is not provided
3. Prepend topic keywords to every Pexels/GIPHY search query
4. Log the full query construction for debugging
