// ---------------------------------------------------------------------------
// content_formats — Content-format playbook registry (harness surface).
//
// Each format bundles an ANATOMY (scene-structure rules), a SPEAKER BLUEPRINT
// (archetypes with gender + ready-to-use voice.design instructs), CORRELATED
// DEFAULTS (the single canonical object the engine consumes), and a WORKED
// EXAMPLE script draft. The harness feeds these to the agent three ways:
//   1. MCP   — director.format {type, topic} returns the playbook JSON
//   2. CLI   — openscript video new --format podcast --topic "..." scaffolds
//   3. Skill — skills/content-formats/<type>/SKILL.md (agent-loadable)
//
// UNIQUENESS CONTRACT: every format must have a PAIRWISE-DISTINCT signature
// (structure_kind, speaker min/max, alternation, pacing, reaction behavior,
// sticker mode, music mood). test playbook_signatures_are_pairwise_distinct
// enforces this — a new format that duplicates an existing one FAILS CI.
// The worked example is generated FROM the canonical `defaults` object so the
// two can never drift apart.
// ---------------------------------------------------------------------------

use serde_json::{json, Value};

/// Valid format types (must match openscript_core::script ContentFormatSpec).
pub const FORMAT_TYPES: &[&str] = &[
    "presentation",
    "podcast",
    "dialogue",
    "comedy_sketch",
    "romcom",
    "meme_reel",
    "documentary",
    "how_to",
];

/// Music moods understood by the library/music pipeline — canonical set lives
/// in openscript-core (validate_script enforces it); re-exported here so the
/// registry and the engine can never disagree.
pub use openscript_core::script::VALID_MUSIC_MOODS;

pub fn is_valid_format(r#type: &str) -> bool {
    FORMAT_TYPES.contains(&r#type)
}

/// Summaries of every format (for `director.format {type:"list"}` + CLI).
pub fn format_list() -> Value {
    let formats: Vec<Value> = FORMAT_TYPES
        .iter()
        .map(|t| {
            let p = playbook(t, "");
            json!({
                "type": p["type"].clone(),
                "title": p["title"].clone(),
                "summary": p["summary"].clone(),
                "differentiator": p["differentiator"].clone(),
                "family": p["family"].clone(),
                "structure_kind": p["structure_kind"].clone(),
                "recommended_speakers": p["recommended_speakers"].clone(),
                "alternation": p["alternation"].clone(),
            })
        })
        .collect();
    json!(formats)
}

/// Build a playbook for a format type and topic. Unknown types fall back to
/// the presentation playbook — this is the BACKWARD-COMPATIBLE DEFAULT for
/// scripts written before formats existed, so it is intentionally overloaded
/// (the default IS the linear presentation shape).
pub fn playbook(r#type: &str, topic: &str) -> Value {
    let t = topic.trim();
    match r#type {
        "podcast" => podcast(t),
        "dialogue" => dialogue(t),
        "comedy_sketch" => comedy_sketch(t),
        "romcom" => romcom(t),
        "meme_reel" => meme_reel(t),
        "documentary" => documentary(t),
        "how_to" => how_to(t),
        _ => presentation(t),
    }
}

/// Build a worked-example ScriptSpec draft. `speakers` is a list of
/// (id, role, gender, voice_profile_id) and `scenes` is a list of
/// (speaker_id, text). The example is generated FROM the format's canonical
/// `defaults` object — the draft always carries the same correlated defaults
/// the playbook advertises (single source of truth, no drift).
fn example_script(
    format_type: &str,
    alternation: &str,
    title: &str,
    speakers: &[(&str, &str, &str, &str)],
    scenes: &[(&str, String)],
    defaults: &Value,
) -> Value {
    let reaction_memes = defaults["reaction_memes"].as_bool().unwrap_or(false);
    let sticker_mode = defaults["sticker_mode"]
        .as_str()
        .unwrap_or("character")
        .to_string();
    let music_mood = defaults["music_mood"].as_str().unwrap_or("neutral");
    let default_speed = defaults["default_speed"].as_f64().unwrap_or(1.0);
    let default_temperature = defaults["default_temperature"].as_f64().unwrap_or(0.8);
    let min_speakers = defaults["min_speakers"].as_u64().unwrap_or(1);
    let max_speakers = defaults["max_speakers"].as_u64().unwrap_or(2);
    let min_scenes = defaults["min_scenes"].as_u64().unwrap_or(4);
    let max_scenes = defaults["max_scenes"].as_u64().unwrap_or(10);

    let mut speakers_map = serde_json::Map::new();
    for (i, (id, _role, gender, voice)) in speakers.iter().enumerate() {
        speakers_map.insert(
            id.to_string(),
            json!({
                "voice": voice,
                "gender": gender,
                "preset": "default_person",
                // Alternate speaker positions so multi-speaker formats get a
                // left/right visual rhythm instead of stacked overlays.
                "position": if i % 2 == 0 { "top-left" } else { "top-right" },
            }),
        );
    }
    let scenes_arr: Vec<Value> = scenes
        .iter()
        .map(|(sid, text)| {
            json!({
                "speaker": sid,
                "text": text,
                "emote": "neutral",
            })
        })
        .collect();
    json!({
        "schema": "openscript-video/v1",
        "title": title,
        "video_keywords": [],
        "format": {
            "type": format_type,
            "alternation": alternation,
            "min_speakers": min_speakers,
            "max_speakers": max_speakers,
            "min_scenes": min_scenes,
            "max_scenes": max_scenes,
            "default_speed": default_speed,
            "default_temperature": default_temperature,
            "reaction_memes": reaction_memes,
            "sticker_mode": sticker_mode,
            "music_mood": music_mood,
        },
        "meta": {"aspect": "9:16", "fps": 30},
        "tts": {"backend": "voicedesign"},
        "speakers": Value::Object(speakers_map),
        "background": {"type": "procedural", "change_cadence": "speaker"},
        "captions": {"style": "word_highlight"},
        "stickers": {"enabled": sticker_mode != "none"},
        "meme_brolls": {"enabled": reaction_memes},
        "scenes": scenes_arr,
        "output": {"theme": "neutral"},
    })
}

// ---------------------------------------------------------------------------
// presentation — the default linear format (solo persuasive explainer)
// ---------------------------------------------------------------------------
fn presentation(topic: &str) -> Value {
    let topic_title = if topic.is_empty() {
        "Your Topic".to_string()
    } else {
        topic.to_string()
    };
    let narrator_text = |i: usize| {
        format!(
            "Point {} about {}: keep each line one idea, 8-12 seconds of speech.",
            i + 1,
            if topic.is_empty() { "the topic" } else { topic }
        )
    };
    let scenes = vec![
        ("narrator", format!("Let's talk about {}.", topic_title)),
        ("narrator", narrator_text(0)),
        ("narrator", narrator_text(1)),
        ("narrator", narrator_text(2)),
        ("narrator", format!("That's the picture on {}.", topic_title)),
    ];
    let defaults = json!({
        "type": "presentation", "alternation": "none",
        "min_speakers": 1, "max_speakers": 2,
        "min_scenes": 5, "max_scenes": 8,
        "default_speed": 1.0, "default_temperature": 0.8,
        "reaction_memes": false, "sticker_mode": "character", "music_mood": "neutral"
    });
    json!({
        "type": "presentation",
        "title": "Presentation",
        "summary": "Linear single-narrator explainer — the historical default. One speaker walks the audience through a topic in order.",
        "differentiator": "≠ documentary: SHORT-FORM persuasive explainer, one idea per line (8-12s), neutral-to-warm delivery. Documentary is the chaptered long-form.",
        "family": "solo_narrated",
        "structure_kind": "persuasive_points",
        "anatomy": "Hook (scene 1) → 3-5 evidence points → recap. Every scene is one idea.",
        "scene_structure": {"hook": 1, "body_min": 3, "body_max": 6, "closing": 1, "pair_based": false},
        "recommended_speakers": {"min": 1, "max": 2, "default": 1},
        "alternation": "none",
        "speaker_blueprint": [
            {
                "id": "narrator",
                "role": "Presenter",
                "gender": "auto",
                "voice_design_instruct": "clear, confident presenter, warm authoritative tone, measured delivery",
                "emote_vocab": ["neutral", "confident", "curious", "emphatic"]
            }
        ],
        "defaults": defaults,
        "next_steps": "Write each scene as ONE idea. Use emote to vary delivery. For single-speaker content this is the fastest path.",
        "example_script": example_script("presentation", "none", &format!("{} — Presentation", topic_title), &[("narrator", "Presenter", "auto", "presentation_narrator")], &scenes, &defaults),
    })
}

// ---------------------------------------------------------------------------
// podcast — host + guest, alternating turns, male/female by default
// ---------------------------------------------------------------------------
fn podcast(topic: &str) -> Value {
    let t = if topic.is_empty() { "your topic" } else { topic };
    let host_v = "podcast_host";
    let guest_v = "podcast_guest";
    let scenes = vec![
        ("host", format!("Welcome to the show — today we're digging into {}.", t)),
        ("guest", format!("And I'm genuinely excited to talk {} — most people get this completely wrong.", t)),
        ("host", format!("Before we dive in: what does '{}' bring to mind for you?", t)),
        ("guest", format!("Honestly? The obvious angle. But the real story behind {} is much stranger.", t)),
        ("host", format!("Okay, you have to unpack that. Where does {} actually begin?", t)),
        ("guest", format!("It starts about a decade ago, when a handful of people noticed something that didn't fit the standard model of {}.", t)),
        ("host", format!("So the takeaway for our listeners — what should they remember about {}?", t)),
        ("guest", format!("One line: {} isn't what we were told, and understanding it changes how you see everything downstream.", t)),
    ];
    let defaults = json!({
        "type": "podcast", "alternation": "male_female",
        "min_speakers": 2, "max_speakers": 4,
        "min_scenes": 6, "max_scenes": 14,
        "default_speed": 1.02, "default_temperature": 0.88,
        "reaction_memes": true, "sticker_mode": "character", "music_mood": "energetic"
    });
    json!({
        "type": "podcast",
        "title": "Podcast",
        "summary": "Host + guest conversation. Alternating turns (M/F recommended) with a hook, topic rounds, takeaway and CTA. Use reaction memes sparingly at punchlines.",
        "differentiator": "≠ dialogue: INFORMAL entertainment roundtable (2-4 speakers, 8-15s lines, reaction memes on punchlines, energetic). Dialogue is the formal interviewer/expert Q&A with no memes.",
        "family": "duo_conversational",
        "structure_kind": "conversation_rounds",
        "anatomy": "Hook (host) → introduce guest → 3-5 topic rounds as Q/A pairs → takeaway → CTA. Scenes alternate host/guest; never give one speaker 3+ consecutive scenes.",
        "scene_structure": {"hook": 1, "body_min": 4, "body_max": 12, "closing": 1, "pair_based": true},
        "recommended_speakers": {"min": 2, "max": 4, "default": 2},
        "alternation": "male_female",
        "speaker_blueprint": [
            {
                "id": "host",
                "role": "Host",
                "gender": "male",
                "voice_design_instruct": "warm, energetic podcast host, natural conversational cadence, bright male voice",
                "emote_vocab": ["warm", "curious", "amused", "serious"]
            },
            {
                "id": "guest",
                "role": "Guest",
                "gender": "female",
                "voice_design_instruct": "calm, articulate podcast guest, thoughtful female voice, measured and precise",
                "emote_vocab": ["thoughtful", "excited", "sincere", "surprised"]
            }
        ],
        "defaults": defaults,
        "next_steps": format!("Create the two voice profiles with voice.design (use the instructs above; they synthesize DIRECTLY on Qwen3 VoiceDesign, no cloning). Then fill scene texts with real substance on '{}'. Keep lines 8-15s.", t),
        "example_script": example_script("podcast", "male_female", &format!("Podcast — {}", t), &[("host", "Host", "male", host_v), ("guest", "Guest", "female", guest_v)], &scenes, &defaults),
    })
}

// ---------------------------------------------------------------------------
// dialogue — interactive session / Q&A, tight back-and-forth
// ---------------------------------------------------------------------------
fn dialogue(topic: &str) -> Value {
    let t = if topic.is_empty() { "your topic" } else { topic };
    let scenes = vec![
        ("interviewer", format!("Thanks for joining. Let's get straight into {} — what's the one thing everyone gets wrong?", t)),
        ("expert", format!("That it's complicated. {} is actually simple once you see the pattern underneath.", t)),
        ("interviewer", format!("Okay — walk me through the pattern, piece by piece.")),
        ("expert", format!("Step one: notice it. Step two: name it. Step three: test it against your own experience of {}.", t)),
        ("interviewer", format!("And when that test fails? What does that tell you?")),
        ("expert", format!("It tells you the map is not the territory — and that's the beginning of actually understanding {}.", t)),
        ("interviewer", format!("Brilliant. If someone takes one action after this session, what should it be?")),
        ("expert", format!("Write down your own one-line model of {} tonight — and change it next month when you know better.", t)),
    ];
    let defaults = json!({
        "type": "dialogue", "alternation": "male_female",
        "min_speakers": 2, "max_speakers": 2,
        "min_scenes": 6, "max_scenes": 10,
        "default_speed": 1.0, "default_temperature": 0.9,
        "reaction_memes": false, "sticker_mode": "character", "music_mood": "neutral"
    });
    json!({
        "type": "dialogue",
        "title": "Interactive Session",
        "summary": "Interviewer + expert back-and-forth. Tight alternating Q/A rounds; shorter lines than a podcast, higher temperature for more inflection.",
        "differentiator": "≠ podcast: FORMAL interviewer/expert session — exactly 2 speakers, short 5-10s lines, NO reaction memes, neutral music. Podcast is the informal 2-4 speaker roundtable with memes.",
        "family": "duo_conversational",
        "structure_kind": "qa_exchange",
        "anatomy": "Opening → 3-4 Q/A exchange rounds (interviewer asks, expert answers) → actionable closing. Scenes alternate; keep lines 5-10s.",
        "scene_structure": {"hook": 1, "body_min": 4, "body_max": 8, "closing": 1, "pair_based": true},
        "recommended_speakers": {"min": 2, "max": 2, "default": 2},
        "alternation": "male_female",
        "speaker_blueprint": [
            {
                "id": "interviewer",
                "role": "Interviewer",
                "gender": "male",
                "voice_design_instruct": "engaging interviewer, curious male voice, quick and attentive",
                "emote_vocab": ["curious", "playful", "serious", "reassuring"]
            },
            {
                "id": "expert",
                "role": "Expert",
                "gender": "female",
                "voice_design_instruct": "warm expert voice, clear female delivery, precise and confident",
                "emote_vocab": ["confident", "thoughtful", "emphatic", "warm"]
            }
        ],
        "defaults": defaults,
        "next_steps": "Two speakers only. Every other scene changes speaker. Questions end with '?' and hand the turn back — keep the rhythm visible in the text.",
        "example_script": example_script("dialogue", "male_female", &format!("Session — {}", t), &[("interviewer", "Interviewer", "male", "dialogue_interviewer"), ("expert", "Expert", "female", "dialogue_expert")], &scenes, &defaults),
    })
}

// ---------------------------------------------------------------------------
// comedy_sketch — straight man + comic, setup → punchline + reaction
// ---------------------------------------------------------------------------
fn comedy_sketch(topic: &str) -> Value {
    let t = if topic.is_empty() { "the absurd side of everyday life" } else { topic };
    let scenes = vec![
        ("straight", format!("Okay, so you're telling me that {} is actually a serious problem?", t)),
        ("comic", format!("Serious? It's a CRISIS. I saw a man lose his entire morning to it. He never recovered.")),
        ("straight", format!("That sounds... extreme. Give me an example.")),
        ("comic", format!("He opened his phone, looked at {}, and just — sighed. For four hours.", t)),
        ("straight", format!("So the solution would be what, exactly?")),
        ("comic", format!("Simple. We ban {}, and if that fails, we blame the weather.", t)),
        ("straight", format!("That is not a solution.")),
        ("comic", format!("It's better than what the experts came up with. Trust me, I read one article.")),
    ];
    let defaults = json!({
        "type": "comedy_sketch", "alternation": "male_female",
        "min_speakers": 2, "max_speakers": 3,
        "min_scenes": 6, "max_scenes": 10,
        "default_speed": 1.05, "default_temperature": 0.9,
        "reaction_memes": true, "sticker_mode": "character", "music_mood": "energetic"
    });
    json!({
        "type": "comedy_sketch",
        "title": "Comedy Sketch",
        "summary": "Straight man + comic. Setup → escalation → punchline, with a reaction meme GIF landing ON the punchline scene. Higher speed, animated delivery.",
        "differentiator": "≠ meme_reel: TWO-speaker setup→escalation→punchline sketch with a reaction meme landing on the punchline beat. Meme_reel is a solo rapid-fire narrator.",
        "family": "duo_comedic",
        "structure_kind": "setup_punchline",
        "anatomy": "Setup (straight man sets the premise) → 2-3 escalation beats → punchline (short, clipped) → reaction. The punchline scene should carry emote 'surprised'/'excited' and land the meme.",
        "scene_structure": {"hook": 1, "body_min": 4, "body_max": 8, "closing": 1, "pair_based": true},
        "recommended_speakers": {"min": 2, "max": 3, "default": 2},
        "alternation": "male_female",
        "speaker_blueprint": [
            {
                "id": "straight",
                "role": "Straight Man",
                "gender": "male",
                "voice_design_instruct": "deadpan comedic straight man, dry male voice, monotone under pressure",
                "emote_vocab": ["deadpan", "exasperated", "flat", "resigned"]
            },
            {
                "id": "comic",
                "role": "Comic",
                "gender": "female",
                "voice_design_instruct": "energetic comedian, animated female voice, quick delivery, big energy",
                "emote_vocab": ["excited", "shocked", "triumphant", "mocking"]
            }
        ],
        "defaults": defaults,
        "next_steps": "Short setup lines (5-8s), punchlines clipped (3-5s). Put a reaction meme on the punchline scene. Deadpan contrast between the two voices IS the comedy.",
        "example_script": example_script("comedy_sketch", "male_female", &format!("Sketch — {}", t), &[("straight", "Straight Man", "male", "comedy_straight"), ("comic", "Comic", "female", "comedy_comic")], &scenes, &defaults),
    })
}

// ---------------------------------------------------------------------------
// romcom — two leads, beat structure: meet-cute → tension → resolution
// ---------------------------------------------------------------------------
fn romcom(topic: &str) -> Value {
    let t = if topic.is_empty() { "a chance encounter" } else { topic };
    let scenes = vec![
        ("lead_m", format!("I wasn't looking for {} that day. Nobody ever is.", t)),
        ("lead_f", format!("And yet there we were — me, a coffee, and the worst pickup line I'd ever heard.")),
        ("lead_m", format!("Worst? I'll have you know that line was carefully workshopped.")),
        ("lead_f", format!("By whom? A committee of pigeons?")),
        ("lead_m", format!("Okay, fair. But you laughed.")),
        ("lead_f", format!("I laughed AT you. There's a difference.")),
        ("lead_m", format!("Still a laugh. That's the first date sorted.")),
        ("lead_f", format!("...You know what? Fine. One coffee. And you're telling me the real story behind {}.", t)),
    ];
    let defaults = json!({
        "type": "romcom", "alternation": "male_female",
        "min_speakers": 2, "max_speakers": 2,
        "min_scenes": 8, "max_scenes": 10,
        "default_speed": 1.0, "default_temperature": 0.9,
        "reaction_memes": false, "sticker_mode": "character", "music_mood": "calm"
    });
    json!({
        "type": "romcom",
        "title": "Romcom",
        "summary": "Two leads (M/F). Meet-cute → banter → tension → warm resolution. Emotional emote pairs carry the chemistry.",
        "differentiator": "≠ dialogue: EMOTIONAL beat structure (meet-cute→banter→tension→resolution) driven by emote pairs, calm music. Dialogue is intellectual Q&A.",
        "family": "duo_dramatic",
        "structure_kind": "romantic_beats",
        "anatomy": "Meet-cute (scene 1-2) → banter escalation (3-5) → misunderstanding/tension beat (6) → resolution + warm close (7-8). Alternate M/F every scene.",
        "scene_structure": {"hook": 2, "body_min": 4, "body_max": 6, "closing": 2, "pair_based": true},
        "recommended_speakers": {"min": 2, "max": 2, "default": 2},
        "alternation": "male_female",
        "speaker_blueprint": [
            {
                "id": "lead_m",
                "role": "Male Lead",
                "gender": "male",
                "voice_design_instruct": "charming romantic lead, warm sincere male voice, slight playful edge",
                "emote_vocab": ["flirty", "nervous", "hopeful", "sincere"]
            },
            {
                "id": "lead_f",
                "role": "Female Lead",
                "gender": "female",
                "voice_design_instruct": "bright romantic lead, soft warm female voice, quick wit",
                "emote_vocab": ["shy", "amused", "warm", "tender"]
            }
        ],
        "defaults": defaults,
        "next_steps": "Chemistry lives in the emote map: pair 'flirty' with 'shy', 'nervous' with 'tender'. Keep the tension beat short so the warm resolution lands.",
        "example_script": example_script("romcom", "male_female", &format!("Romcom — {}", t), &[("lead_m", "Male Lead", "male", "romcom_lead_m"), ("lead_f", "Female Lead", "female", "romcom_lead_f")], &scenes, &defaults),
    })
}

// ---------------------------------------------------------------------------
// meme_reel — single narrator, rapid punchy takes + reaction memes
// ---------------------------------------------------------------------------
fn meme_reel(topic: &str) -> Value {
    let t = if topic.is_empty() { "the topic" } else { topic };
    let scenes = vec![
        ("narrator", format!("Everyone's lying to you about {}.", t)),
        ("narrator", format!("The 'experts'? They read one article. I read ZERO articles, and I'm right.")),
        ("narrator", format!("Point one: it's not complicated. Point two: nobody wants it to be simple.")),
        ("narrator", format!("Point three — and this is the spicy one — the fix is obvious and nobody will say it.")),
        ("narrator", format!("So here's the takeaway: ignore the noise on {}, follow the pattern, and you'll see it too.", t)),
    ];
    let defaults = json!({
        "type": "meme_reel", "alternation": "none",
        "min_speakers": 1, "max_speakers": 2,
        "min_scenes": 4, "max_scenes": 7,
        "default_speed": 1.1, "default_temperature": 0.85,
        "reaction_memes": true, "sticker_mode": "reaction", "music_mood": "energetic"
    });
    json!({
        "type": "meme_reel",
        "title": "Meme Reel",
        "summary": "Single fast narrator, short punchy takes, heavy reaction memes (GIPHY pop-ins). Highest pacing, snappy delivery.",
        "differentiator": "≠ comedy_sketch: ONE fast narrator, 3-7s takes, reaction-driven stickers, no character arc. Sketch needs a duo and a setup→punchline beat.",
        "family": "solo_comedic",
        "structure_kind": "rapid_takes",
        "anatomy": "Hook (one line, zero context) → 3-4 rapid takes → punchline → reaction meme. Every line 3-7s.",
        "scene_structure": {"hook": 1, "body_min": 2, "body_max": 5, "closing": 1, "pair_based": false},
        "recommended_speakers": {"min": 1, "max": 2, "default": 1},
        "alternation": "none",
        "speaker_blueprint": [
            {
                "id": "narrator",
                "role": "Meme Narrator",
                "gender": "auto",
                // Gender is intentionally absent: auto means the agent picks.
                // Append "male voice" or "female voice" to the instruct to fix it.
                "voice_design_instruct": "meme narrator, quick snappy delivery, slightly sarcastic, high energy",
                "emote_vocab": ["sarcastic", "shocked", "amused", "deadpan"]
            }
        ],
        "defaults": defaults,
        "next_steps": "Shorter lines than any other format. Reaction memes land on punchlines. sarcastic/deadpan delivery beats earnest here. To fix the narrator gender, append 'male voice' or 'female voice' to the voice_design_instruct.",
        "example_script": example_script("meme_reel", "none", &format!("{} — Meme Reel", t), &[("narrator", "Meme Narrator", "auto", "meme_narrator")], &scenes, &defaults),
    })
}

// ---------------------------------------------------------------------------
// documentary — measured narrator, evidence chapters (long-form contrast)
// ---------------------------------------------------------------------------
fn documentary(topic: &str) -> Value {
    let t = if topic.is_empty() { "the topic" } else { topic };
    let scenes = vec![
        ("narrator", format!("There's a story hidden inside {} that almost nobody was told.", t)),
        ("narrator", format!("To understand it, we have to go back — to the moment the pattern first appeared.")),
        ("narrator", format!("Chapter one: the evidence nobody collected. It was there all along, in plain sight.")),
        ("narrator", format!("Chapter two: the institutions that looked away, and why they did.")),
        ("narrator", format!("Chapter three: the people who saw it clearly anyway — and what they did next.")),
        ("narrator", format!("The conclusion writes itself: {} was never one story. It was a thousand.", t)),
    ];
    let defaults = json!({
        "type": "documentary", "alternation": "none",
        "min_speakers": 1, "max_speakers": 2,
        "min_scenes": 5, "max_scenes": 8,
        "default_speed": 0.95, "default_temperature": 0.75,
        "reaction_memes": false, "sticker_mode": "none", "music_mood": "calm"
    });
    json!({
        "type": "documentary",
        "title": "Documentary",
        "summary": "Measured narrator(s). Longer sentences, lower temperature, evidence chapters, calm music. Authoritative and somber.",
        "differentiator": "≠ presentation: CHAPTERED long-form evidence narrative, 3-5 clause lines, grave/somber emotes, no stickers, calm music. Presentation is the short persuasive explainer.",
        "family": "solo_narrated",
        "structure_kind": "evidence_chapters",
        "anatomy": "Opening thesis → 3-5 evidence chapters → synthesis → closing reflection. Slower pacing, deeper tone.",
        "scene_structure": {"hook": 1, "body_min": 3, "body_max": 6, "closing": 1, "pair_based": false},
        "recommended_speakers": {"min": 1, "max": 2, "default": 1},
        "alternation": "none",
        "speaker_blueprint": [
            {
                "id": "narrator",
                "role": "Documentary Narrator",
                "gender": "auto",
                "voice_design_instruct": "calm authoritative documentary narrator, deep measured voice, serious but warm undertone",
                "emote_vocab": ["neutral", "grave", "hopeful", "somber"]
            }
        ],
        "defaults": defaults,
        "next_steps": "Longer sentences, chapter markers in the text, minimal stickers. Lower temperature keeps the delivery steady and serious.",
        "example_script": example_script("documentary", "none", &format!("{} — Documentary", t), &[("narrator", "Narrator", "auto", "documentary_narrator")], &scenes, &defaults),
    })
}

// ---------------------------------------------------------------------------
// how_to — instructional steps (single narrator, numbered directives)
// ---------------------------------------------------------------------------
fn how_to(topic: &str) -> Value {
    let t = if topic.is_empty() { "the skill" } else { topic };
    let scenes = vec![
        ("narrator", format!("Here's how to get good at {} — no fluff, just the steps that actually work.", t)),
        ("narrator", format!("Step one: get your tools ready. You need three things, and one of them is probably already on your desk.")),
        ("narrator", format!("Step two: set up the foundation. Ten minutes now saves an hour later.")),
        ("narrator", format!("Step three: run a first pass and write down what breaks.")),
        ("narrator", format!("Step four: fix the two things that matter and ignore the rest.")),
        ("narrator", format!("That's it. Do those steps tonight and you're already ahead of most people on {}.", t)),
    ];
    let defaults = json!({
        "type": "how_to", "alternation": "none",
        "min_speakers": 1, "max_speakers": 1,
        "min_scenes": 4, "max_scenes": 9,
        "default_speed": 1.0, "default_temperature": 0.8,
        "reaction_memes": false, "sticker_mode": "character", "music_mood": "neutral"
    });
    json!({
        "type": "how_to",
        "title": "How-To / Tutorial",
        "summary": "Single narrator, numbered actionable steps. Direct commands, encouraging delivery. The instructional counterpart to the persuasive explainer.",
        "differentiator": "≠ presentation: NUMBERED actionable steps with direct commands ('Step one: do X') — presentation persuades, how_to instructs.",
        "family": "solo_narrated",
        "structure_kind": "instructional_steps",
        "anatomy": "Hook (what you'll learn) → 3-6 numbered steps → recap that chains the steps into one action.",
        "scene_structure": {"hook": 1, "body_min": 3, "body_max": 7, "closing": 1, "pair_based": false},
        "recommended_speakers": {"min": 1, "max": 1, "default": 1},
        "alternation": "none",
        "speaker_blueprint": [
            {
                "id": "narrator",
                "role": "Instructor",
                "gender": "auto",
                "voice_design_instruct": "clear instructional guide voice, direct and encouraging, friendly measured delivery",
                "emote_vocab": ["neutral", "confident", "encouraging", "emphatic"]
            }
        ],
        "defaults": defaults,
        "next_steps": "Number the steps out loud in the text ('Step one: ...'). Keep each step one concrete action. End with a recap that chains steps into a single sentence.",
        "example_script": example_script("how_to", "none", &format!("{} — How-To", t), &[("narrator", "Instructor", "auto", "how_to_narrator")], &scenes, &defaults),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signature tuple used to enforce the pairwise-uniqueness contract.
    /// Every format must have a distinct (structure_kind, speaker range,
    /// alternation, pacing, reaction, sticker, mood) combination.
    fn signature(p: &Value) -> (String, u64, u64, String, String, String, bool, String, String) {
        let d = &p["defaults"];
        (
            p["structure_kind"].as_str().unwrap_or("").to_string(),
            d["min_speakers"].as_u64().unwrap_or(0),
            d["max_speakers"].as_u64().unwrap_or(0),
            d["alternation"].as_str().unwrap_or("").to_string(),
            format!("{:.2}", d["default_speed"].as_f64().unwrap_or(0.0)),
            format!("{:.2}", d["default_temperature"].as_f64().unwrap_or(0.0)),
            d["reaction_memes"].as_bool().unwrap_or(false),
            d["sticker_mode"].as_str().unwrap_or("").to_string(),
            d["music_mood"].as_str().unwrap_or("").to_string(),
        )
    }

    #[test]
    fn playbook_signatures_are_pairwise_distinct() {
        let mut seen: Vec<(String, (String, u64, u64, String, String, String, bool, String, String))> = Vec::new();
        for t in FORMAT_TYPES {
            let p = playbook(t, "test");
            assert_eq!(p["type"].as_str().unwrap(), *t, "playbook type mismatch");
            let sig = signature(&p);
            assert!(
                !seen.iter().any(|(_, s)| s == &sig),
                "format '{t}' duplicates the signature of '{}': {:?}",
                seen.iter().find(|(_, s)| s == &sig).map(|(n, _)| n.as_str()).unwrap_or("?"),
                sig
            );
            seen.push((t.to_string(), sig));
        }
        assert_eq!(seen.len(), FORMAT_TYPES.len());
    }

    #[test]
    fn playbook_example_script_matches_correlated_defaults() {
        // The worked example must carry the SAME correlated defaults the
        // playbook advertises (single source of truth, no drift).
        for t in FORMAT_TYPES {
            let p = playbook(t, "ai agents");
            let ex = p["example_script"].as_object().unwrap();
            let f = &ex["format"];
            assert_eq!(f["type"], p["defaults"]["type"], "format {t}: type drift");
            assert_eq!(f["alternation"], p["defaults"]["alternation"], "format {t}: alternation drift");
            assert_eq!(f["default_speed"], p["defaults"]["default_speed"], "format {t}: speed drift");
            assert_eq!(f["default_temperature"], p["defaults"]["default_temperature"], "format {t}: temp drift");
            assert_eq!(f["reaction_memes"], p["defaults"]["reaction_memes"], "format {t}: reaction drift");
            assert_eq!(f["sticker_mode"], p["defaults"]["sticker_mode"], "format {t}: sticker drift");
            assert_eq!(f["music_mood"], p["defaults"]["music_mood"], "format {t}: mood drift");
            assert_eq!(f["min_speakers"], p["defaults"]["min_speakers"], "format {t}: min_speakers drift");
            assert_eq!(f["max_speakers"], p["defaults"]["max_speakers"], "format {t}: max_speakers drift");
            assert_eq!(f["min_scenes"], p["defaults"]["min_scenes"], "format {t}: min_scenes drift");
            assert_eq!(f["max_scenes"], p["defaults"]["max_scenes"], "format {t}: max_scenes drift");
            // The example must satisfy its own min/max scene contract.
            let n = ex["scenes"].as_array().unwrap().len() as u64;
            assert!(n >= f["min_scenes"].as_u64().unwrap(), "format {t}: {n} scenes < min {}", f["min_scenes"]);
            assert!(n <= f["max_scenes"].as_u64().unwrap(), "format {t}: {n} scenes > max {}", f["max_scenes"]);
        }
    }

    #[test]
    fn playbook_podcast_has_alternating_blueprint() {
        let p = playbook("podcast", "ai agents");
        assert_eq!(p["type"], "podcast");
        assert_eq!(p["alternation"], "male_female");
        let bp = p["speaker_blueprint"].as_array().unwrap();
        assert_eq!(bp.len(), 2);
        assert_eq!(bp[0]["gender"], "male");
        assert_eq!(bp[1]["gender"], "female");
        // Worked example alternates speakers.
        let ex = p["example_script"].as_object().unwrap();
        let scenes = ex["scenes"].as_array().unwrap();
        let ids: Vec<&str> = scenes
            .iter()
            .map(|s| s["speaker"].as_str().unwrap())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] != w[1]), "scenes must alternate: {:?}", ids);
        assert_eq!(ids.first(), Some(&"host"));
        assert_eq!(ids.last(), Some(&"guest"));
    }

    #[test]
    fn playbook_meme_reel_has_fast_pacing_and_reactions() {
        let p = playbook("meme_reel", "");
        assert_eq!(p["defaults"]["default_speed"], 1.1);
        assert_eq!(p["defaults"]["reaction_memes"], true);
        assert_eq!(p["defaults"]["sticker_mode"], "reaction");
        let ex = p["example_script"].as_object().unwrap();
        assert_eq!(ex["format"]["sticker_mode"], "reaction");
        assert_eq!(ex["format"]["reaction_memes"], true);
    }

    #[test]
    fn playbook_how_to_has_numbered_steps_and_self_consistent_limits() {
        let p = playbook("how_to", "cooking");
        assert_eq!(p["type"], "how_to");
        assert_eq!(p["structure_kind"], "instructional_steps");
        let d = &p["defaults"];
        assert_eq!(d["min_speakers"], 1);
        assert_eq!(d["max_speakers"], 1);
        assert_eq!(d["reaction_memes"], false);
        // Every worked-example scene should embed an audible step marker
        // ("Step one/two/...") so the structure is audible, not just visual.
        let ex = p["example_script"].as_object().unwrap();
        let texts: Vec<&str> = ex["scenes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["text"].as_str().unwrap())
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("Step one")),
            "how_to example should number its steps: {:?}",
            texts
        );
    }

    #[test]
    fn playbook_unknown_falls_back_to_presentation() {
        let p = playbook("fireside", "");
        assert_eq!(p["type"], "presentation");
    }

    #[test]
    fn format_list_covers_all_types() {
        let list_value = format_list();
        let list = list_value.as_array().unwrap();
        assert_eq!(list.len(), FORMAT_TYPES.len());
        // Discovery metadata present for agent decision-making.
        for entry in list {
            assert!(entry["differentiator"].is_string() && !entry["differentiator"].as_str().unwrap().is_empty());
            assert!(entry["family"].is_string() && !entry["family"].as_str().unwrap().is_empty());
            assert!(entry["structure_kind"].is_string() && !entry["structure_kind"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn all_music_moods_are_valid() {
        for t in FORMAT_TYPES {
            let p = playbook(t, "");
            let mood = p["defaults"]["music_mood"].as_str().unwrap();
            assert!(
                VALID_MUSIC_MOODS.contains(&mood),
                "format {t}: music_mood '{mood}' is not in the known mood set"
            );
        }
    }
}
