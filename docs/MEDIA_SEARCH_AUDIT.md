# Media Search Infrastructure Audit — Pexels / Pixabay / YouTube

**Date:** 2026-08-06
**Purpose:** Investigate how OpenScript leverages its three stock-footage engines, test each engine live, and provide sample downloads so the b-roll architecture can be planned.

---

## 1. Engine wiring (as-built)

| Engine | Where used | Key needed | Status on this machine |
|--------|-----------|-----------|------------------------|
| **Pexels** | `background.fetch` (Priority 1), `broll.fetch`, `script.to_video` (Priority 1) | `PEXELS_API_KEY` | ✅ **SET — primary b-roll source** |
| **YouTube** (yt-dlp) | `background.fetch` (Priority 2, **blind**), `script.to_video` (Priority 2, **signal-ranked**), `youtube.search` / `youtube.download`, music library | none | ✅ **Works** (yt-dlp installed) |
| **Pixabay** | `stock.search` / `stock.fetch` — **music only**; video path forces `video_type=animation` | `PIXABAY_API_KEY` | ❌ **DEAD — key unset + video never wired into any b-roll path** |

### Key code paths
- `pexels_search_url()` — Pexels URL builder with `min_duration`/`max_duration` filters (SEGMENTATION_ARCHITECTURE).
- `fetch_youtube_stock_clip_signal()` — the **good** YouTube path: 12 candidates via `ytsearch`, ranked by `stock_signal::rank_and_filter_candidates` (lexical gate), video-only format, video-ID + content-hash dedup. Used by `script.to_video` Priority 2.
- `handle_background_fetch` Priority 2 — the **weak** YouTube path: `ytsearch1:{query}` — first hit, no ranking, no duration filter, no stock phrasing.
- `handle_stock_search` / `handle_stock_fetch` — Pixabay music + animation-only video; fall back to local library without a key.

---

## 2. Live probe — 10 keywords, portrait orientation

Method: `docs/../output/media_probe_samples/probe_report.json` (full JSON), 10 sample clips downloaded to `output/media_probe_samples/`, visual contact sheet at `output/media_probe_samples/contact_sheet.jpg`.

### Pexels (key set) — **excellent**

| Keyword | Results | Top hit (relevance) |
|---------|---------|---------------------|
| crowd protest rally | 4,875 | "people protesting on the street" (40s) |
| man silhouette alone | 8,000 | "solitary sunset silhouette walk" (60s, 1080×1920) |
| courtroom justice gavel | 2,320 | "judge striking gavel in courtroom setting" |
| statistics charts data | ~8,000 | chart/data visualization footage |
| people walking city street | 8,000 | "pedestrians crossing the street" (11s) |
| family parent child home | 8,000 | "father making a braid on his daughter" (15s) |
| interview conversation office | 8,000 | "man conducting a job interview" (25s) |
| newspaper reading morning | 8,000 | "person opening newspaper" (5s) |
| dark moody thinking | 7,601 | "a problematic man" (57s) |
| society community group | 8,000 | "aerial view of busy urban crowd movement" |

**Verdict:** Pexels with plain English keywords returns relevant, portrait, duration-tagged stock — the pipeline's primary choice is correct. All 10 sample clips downloaded successfully (0.3–2.5 MB, 5–60s).

### YouTube (yt-dlp, no key) — **query formulation is everything**

| Keyword | Plain `ytsearch` top hit | `"{kw} stock footage vertical"` top hit |
|---------|--------------------------|------------------------------------------|
| crowd protest rally | NSUI protest news (politically irrelevant) | — (news dominates both) |
| man silhouette alone | "Silhouette of Man Walking Out Door" ✅ | "silhouette of man walking through smoky studio" ✅ |
| courtroom justice gavel | "My Husband's Guilty & the broken gavel!" ❌ | "Why Judges Break the Gavel in Court?" ❌ |
| statistics charts data | 6-hour statistics lecture ❌ | "Free Stock Footage [Sale Growth Data Chart Diagram]" ✅ |
| people walking city street | "Free City Street Footage – Royalty Free" ✅ | "people walking on street \| free footage" ✅ |
| family parent child home | "Old Age – The New Curse of India" ❌ | "Happy Family Playing At Sunset \| Premium Video Footage" ✅ |
| interview conversation office | English job-interview practice lesson ❌ | "Human Resource Interviewing Male Candidate Stock Video" ✅ |
| newspaper reading morning | The Hindu newspaper analysis ❌ | "Old Newspaper 4K – Stock Footage Free Background" ✅ |
| dark moody thinking | jazz playlist / type beat ❌ | "Man Walking towards Dark – No Copyright Video – Free Stock Footage" ✅ |
| society community group | sociology lecture ❌ | "Team Group People Faces Smiling Happy Bonding" ✅ |

**Verdict:** plain keywords → news/lectures/music (bad b-roll). Adding **"stock footage"** phrasing flips ~8/10 keywords to real b-roll. The `stock_signal` module already encodes this bias for `script.to_video`; `background.fetch`'s blind `ytsearch1` does not.

### Pixabay (no key) — **dead**

- Every call → `HTTP 400 Bad Request` (API requires a key).
- Even with a key, `stock.fetch video` requests `video_type=animation` → motion graphics, **not real footage** — a design bug for b-roll use.
- No b-roll path (background/broll/to_video) calls Pixabay at all today.

---

## 3. Implementation gaps (why "clip relevance" suffers)

1. **`background.fetch` YouTube fallback is a blind grab.** `ytsearch1:{query}` takes the *first* search hit with no lexical gate, no duration check, no stock phrasing, no dedup. `script.to_video` already has the ranked path — `background.fetch` should reuse it.
2. **Pixabay video is not a b-roll provider.** It's music-only, requires a key that isn't set, and its video endpoint is pinned to `animation`. No pipeline path can currently produce Pixabay *footage*.
3. **`youtube.search` returns unranked, unbounded results.** No duration filter, no lexical prefilter — agents get whatever yt-dlp returns (news/lectures).
4. **No unified candidate model across engines.** Pexels → `PexelsVideo`, YouTube → yt-dlp JSON, Pixabay → hits. Ranking (`stock_signal`), dedup (id/hash), and duration-coverage logic can't be shared across engines because the response shapes differ. Only the to_video YouTube path gets the full treatment.
5. **Pexels requires a custom User-Agent.** Plain clients (default urllib) get Cloudflare 403 (1010). The Rust client sets `OpenScript/1.0` — fine in-pipeline; any new probe/test code must set it too.

---

## 4. Recommendations for the b-roll architecture plan

1. **Unify into a candidate pool.** Fetch N candidates per keyword from every enabled engine, normalize into one `StockCandidate { provider, id, title, duration_s, width, height, thumbnail, url }`, then rank *once* with `stock_signal` (lexical) + duration-coverage + orientation, and dedup by video ID **across engines** (same clip can exist on both Pexels and YouTube).
2. **Engine priority:** Pexels (relevance + duration API) → YouTube with stock phrasing + lexical gate → Pixabay footage (needs key + drop `video_type=animation`).
3. **Fix `background.fetch`:** replace `ytsearch1` with the existing `fetch_youtube_stock_clip_signal` (or at minimum append " stock footage" to the query and rank the top 5 by lexical score).
4. **Enable Pixabay:** obtain a free `PIXABAY_API_KEY`, remove `video_type=animation` (or make it a param), and add Pixabay as a second stock provider behind Pexels.
5. **`youtube.search` upgrade:** optional `min_duration_s`/`max_duration_s` bounds + lexical relevance prefilter so agents never see news/lecture noise.
6. **Add a probe/QA tool:** a `broll.probe` MCP tool that returns the unified candidate pool for a keyword (all engines, ranked) — turns this audit into a permanent diagnostic.

---

## 5. Sample downloads (for inspection)

```
output/media_probe_samples/
├── 01_crowd_protest_rally_15886346.mp4      (Pexels, 6s)
├── 02_man_silhouette_alone_35798906.mp4     (Pexels, 60s)
├── 03_courtroom_justice_gavel_34536748.mp4  (Pexels, 5s)
├── 04_statistics_charts_data_analysi_36879789.mp4 (Pexels, 26s)
├── 05_people_walking_city_street_10531835.mp4      (Pexels, 9s)
├── 06_family_parent_child_home_5894605.mp4         (Pexels, 15s)
├── 07_interview_conversation_office_7643444.mp4    (Pexels, 24s)
├── 08_newspaper_reading_morning_10449873.mp4       (Pexels, 4s)
├── 09_dark_moody_thinking_7277919.mp4              (Pexels, 57s)
├── 10_society_community_group_37307217.mp4         (Pexels, 9s)
├── contact_sheet.jpg          (2×5 visual grid of all 10 clips)
└── probe_report.json          (full per-engine search results)
```

*Note: `output/` is gitignored — these samples are local inspection artifacts, not committed.*
