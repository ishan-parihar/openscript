// ---------------------------------------------------------------------------
// content_formats — Content-format playbook registry (harness surface).
//
// Each format bundles an ANATOMY (scene-structure rules), a SPEAKER BLUEPRINT
// (archetypes with gender + ready-to-use voice.design instructs), PACING +
// REACTION guidance, correlated SCRIPT DEFAULTS, and a WORKED EXAMPLE script
// draft. The harness feeds these to the agent three ways:
//   1. MCP   — director.format {type, topic} returns the playbook JSON
//   2. CLI   — openscript video new --format podcast --topic "..." scaffolds
//   3. Skill — skills/content-formats/<type>/SKILL.md (agent-loadable)
//
// The speaker blueprint always suggests a male/female alternation for
// dialogic formats; the scene lists in each worked example alternate
// speakers so the resulting video is engaging and stimulating.
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
];

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
                "recommended_speakers": p["recommended_speakers"].clone(),
                "alternation": p["alternation"].clone(),
            })
        })
        .collect();
    json!(formats)
}

/// Build a playbook for a format type and topic. Unknown types fall back to
/// the presentation playbook (backward compatible with the linear format).
pub fn playbook(r#type: &str, topic: &str) -> Value {
    let t = topic.trim();
    match r#type {
        "podcast" => podcast(t),
        "dialogue" => dialogue(t),
        "comedy_sketch" => comedy_sketch(t),
        "romcom" => romcom(t),
        "meme_reel" => meme_reel(t),
        "documentary" => documentary(t),
        _ => presentation(t),
    }
}

/// Build a worked-example ScriptSpec draft with alternating speakers.
/// `speakers` is a list of (id, role, gender, voice_profile_id) and `scenes`
/// is a list of (speaker_id, text) — the harness substitutes the topic into
/// each scene text so the draft is immediately readable.
fn example_script(
    format_type: &str,
    alternation: &str,
    title: &str,
    speakers: &[(&str, &str, &str, &str)],
    scenes: &[(&str, String)],
) -> Value {
    // The worked example mirrors the format's correlated defaults so the
    // draft validates with script.format.validate and renders with the right
    // reaction/sticker behavior out of the box.
    let (reaction_memes, sticker_mode) = match format_type {
        "meme_reel" => (true, "reaction"),
        "podcast" | "comedy_sketch" => (true, "character"),
        "documentary" => (false, "none"),
        _ => (false, "character"),
    };
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
            "default_speed": 0.0,
            "reaction_memes": reaction_memes,
            "sticker_mode": sticker_mode,
            "music_mood": "neutral",
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
// presentation — the default linear format
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
    json!({
        "type": "presentation",
        "title": "Presentation",
        "summary": "Linear single-narrator explainer — the historical default. One speaker walks the audience through a topic in order.",
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
        "pacing": {"default_speed": 1.0, "default_temperature": 0.8},
        "reactions": {"reaction_memes": false, "sticker_mode": "character"},
        "music_mood": "neutral",
        "defaults": {
            "type": "presentation", "alternation": "none",
            "default_speed": 1.0, "default_temperature": 0.8,
            "reaction_memes": false, "sticker_mode": "character", "music_mood": "neutral"
        },
        "next_steps": "Write each scene as ONE idea. Use emote to vary delivery. For single-speaker content this is the fastest path.",
        "example_script": example_script("presentation", "none", &format!("{} — Presentation", topic_title), &[("narrator", "Presenter", "auto", "presentation_narrator")], &scenes),
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
    json!({
        "type": "podcast",
        "title": "Podcast",
        "summary": "Host + guest conversation. Alternating turns (M/F recommended) with a hook, topic rounds, takeaway and CTA. Use reaction memes sparingly at punchlines.",
        "anatomy": "Hook (host) → introduce guest → 3-5 topic rounds as Q/A pairs → takeaway → CTA. Scenes alternate host/guest; never give one speaker 3+ consecutive scenes.",
        "scene_structure": {"hook": 1, "body_min": 6, "body_max": 12, "closing": 2, "pair_based": true},
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
        "pacing": {"default_speed": 1.02, "default_temperature": 0.88},
        "reactions": {"reaction_memes": true, "sticker_mode": "character"},
        "music_mood": "energetic",
        "defaults": {
            "type": "podcast", "alternation": "male_female",
            "min_speakers": 2, "max_speakers": 4,
            "default_speed": 1.02, "default_temperature": 0.88,
            "reaction_memes": true, "sticker_mode": "character", "music_mood": "energetic"
        },
        "next_steps": format!("Create the two voice profiles with voice.design (use the instructs above; they synthesize DIRECTLY on Qwen3 VoiceDesign, no cloning). Then fill scene texts with real substance on '{}'. Keep lines 8-15s.", t),
        "example_script": example_script("podcast", "male_female", &format!("Podcast — {}", t), &[("host", "Host", "male", host_v), ("guest", "Guest", "female", guest_v)], &scenes),
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
    json!({
        "type": "dialogue",
        "title": "Interactive Session",
        "summary": "Interviewer + expert back-and-forth. Tight alternating Q/A rounds; shorter lines than a podcast, higher temperature for more inflection.",
        "anatomy": "Opening → 3-4 Q/A exchange rounds (interviewer asks, expert answers) → actionable closing. Scenes alternate; keep lines 5-10s.",
        "scene_structure": {"hook": 1, "body_min": 6, "body_max": 10, "closing": 1, "pair_based": true},
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
        "pacing": {"default_speed": 1.0, "default_temperature": 0.9},
        "reactions": {"reaction_memes": false, "sticker_mode": "character"},
        "music_mood": "neutral",
        "defaults": {
            "type": "dialogue", "alternation": "male_female",
            "min_speakers": 2, "max_speakers": 2,
            "default_speed": 1.0, "default_temperature": 0.9,
            "reaction_memes": false, "sticker_mode": "character", "music_mood": "neutral"
        },
        "next_steps": "Two speakers only. Every other scene changes speaker. Questions end with '?' and hand the turn back — keep the rhythm visible in the text.",
        "example_script": example_script("dialogue", "male_female", &format!("Session — {}", t), &[("interviewer", "Interviewer", "male", "dialogue_interviewer"), ("expert", "Expert", "female", "dialogue_expert")], &scenes),
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
    json!({
        "type": "comedy_sketch",
        "title": "Comedy Sketch",
        "summary": "Straight man + comic. Setup → escalation → punchline, with a reaction meme GIF landing ON the punchline scene. Higher speed, animated delivery.",
        "anatomy": "Setup (straight man sets the premise) → 2-3 escalation beats → punchline (short, clipped) → reaction. The punchline scene should carry emote 'surprised'/'excited' and land the meme.",
        "scene_structure": {"hook": 1, "body_min": 6, "body_max": 10, "closing": 1, "pair_based": true},
        "recommended_speakers": {"min": 2, "max": 2, "default": 2},
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
        "pacing": {"default_speed": 1.05, "default_temperature": 0.9},
        "reactions": {"reaction_memes": true, "sticker_mode": "character"},
        "music_mood": "energetic",
        "defaults": {
            "type": "comedy_sketch", "alternation": "male_female",
            "min_speakers": 2, "max_speakers": 3,
            "default_speed": 1.05, "default_temperature": 0.9,
            "reaction_memes": true, "sticker_mode": "character", "music_mood": "energetic"
        },
        "next_steps": "Short setup lines (5-8s), punchlines clipped (3-5s). Put a reaction meme on the punchline scene. Deadpan contrast between the two voices IS the comedy.",
        "example_script": example_script("comedy_sketch", "male_female", &format!("Sketch — {}", t), &[("straight", "Straight Man", "male", "comedy_straight"), ("comic", "Comic", "female", "comedy_comic")], &scenes),
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
    json!({
        "type": "romcom",
        "title": "Romcom",
        "summary": "Two leads (M/F). Meet-cute → banter → tension → warm resolution. Emotional emote pairs carry the chemistry.",
        "anatomy": "Meet-cute (scene 1-2) → banter escalation (3-5) → misunderstanding/tension beat (6) → resolution + warm close (7-8). Alternate M/F every scene.",
        "scene_structure": {"hook": 2, "body_min": 4, "body_max": 8, "closing": 2, "pair_based": true},
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
        "pacing": {"default_speed": 1.0, "default_temperature": 0.9},
        "reactions": {"reaction_memes": false, "sticker_mode": "character"},
        "music_mood": "calm",
        "defaults": {
            "type": "romcom", "alternation": "male_female",
            "min_speakers": 2, "max_speakers": 2,
            "default_speed": 1.0, "default_temperature": 0.9,
            "reaction_memes": false, "sticker_mode": "character", "music_mood": "calm"
        },
        "next_steps": "Chemistry lives in the emote map: pair 'flirty' with 'shy', 'nervous' with 'tender'. Keep the tension beat short so the warm resolution lands.",
        "example_script": example_script("romcom", "male_female", &format!("Romcom — {}", t), &[("lead_m", "Male Lead", "male", "romcom_lead_m"), ("lead_f", "Female Lead", "female", "romcom_lead_f")], &scenes),
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
    json!({
        "type": "meme_reel",
        "title": "Meme Reel",
        "summary": "Single fast narrator, short punchy takes, heavy reaction memes (GIPHY pop-ins). Highest pacing, snappy delivery.",
        "anatomy": "Hook (one line, zero context) → 3-4 rapid takes → punchline → reaction meme. Every line 3-7s.",
        "scene_structure": {"hook": 1, "body_min": 3, "body_max": 6, "closing": 1, "pair_based": false},
        "recommended_speakers": {"min": 1, "max": 2, "default": 1},
        "alternation": "none",
        "speaker_blueprint": [
            {
                "id": "narrator",
                "role": "Meme Narrator",
                "gender": "auto",
                "voice_design_instruct": "meme narrator, quick snappy delivery, slightly sarcastic, high energy male voice",
                "emote_vocab": ["sarcastic", "shocked", "amused", "deadpan"]
            }
        ],
        "pacing": {"default_speed": 1.1, "default_temperature": 0.85},
        "reactions": {"reaction_memes": true, "sticker_mode": "reaction"},
        "music_mood": "energetic",
        "defaults": {
            "type": "meme_reel", "alternation": "none",
            "default_speed": 1.1, "default_temperature": 0.85,
            "reaction_memes": true, "sticker_mode": "reaction", "music_mood": "energetic"
        },
        "next_steps": "Shorter lines than any other format. Reaction memes land on punchlines. sarcastic/deadpan delivery beats earnest here.",
        "example_script": example_script("meme_reel", "none", &format!("{} — Meme Reel", t), &[("narrator", "Meme Narrator", "auto", "meme_narrator")], &scenes),
    })
}

// ---------------------------------------------------------------------------
// documentary — measured single/two narrator, evidence chapters
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
    json!({
        "type": "documentary",
        "title": "Documentary",
        "summary": "Measured narrator(s). Longer sentences, lower temperature, evidence chapters, calm music. Authoritative and somber.",
        "anatomy": "Opening thesis → 3-5 evidence chapters → synthesis → closing reflection. Slower pacing, deeper tone.",
        "scene_structure": {"hook": 1, "body_min": 4, "body_max": 8, "closing": 1, "pair_based": false},
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
        "pacing": {"default_speed": 0.95, "default_temperature": 0.75},
        "reactions": {"reaction_memes": false, "sticker_mode": "none"},
        "music_mood": "calm",
        "defaults": {
            "type": "documentary", "alternation": "none",
            "default_speed": 0.95, "default_temperature": 0.75,
            "reaction_memes": false, "sticker_mode": "none", "music_mood": "calm"
        },
        "next_steps": "Longer sentences, chapter markers in the text, minimal stickers. Lower temperature keeps the delivery steady and serious.",
        "example_script": example_script("documentary", "none", &format!("{} — Documentary", t), &[("narrator", "Narrator", "auto", "documentary_narrator")], &scenes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(p["pacing"]["default_speed"], 1.1);
        assert_eq!(p["reactions"]["reaction_memes"], true);
        assert_eq!(p["reactions"]["sticker_mode"], "reaction");
        let ex = p["example_script"].as_object().unwrap();
        assert_eq!(ex["format"]["sticker_mode"], "reaction");
        assert_eq!(ex["format"]["reaction_memes"], true);
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
    }
}
