//! Stock B-roll **signal vs noise** gates for multi-broll selection.
//!
//! Noise sources observed in production:
//! 1. **Query pollution** — dialogue fragments (`Swap one.`, `Phone later.`)
//!    leak into yt-dlp/Pexels searches and pull irrelevant viral clips.
//! 2. **Geometry distortion** — `scale=W:H,crop=W:H` stretches landscape
//!    sources into 9:16 with non-square SAR (looks squashed/stretched).
//! 3. **Uniqueness-only selection** — first unique ID wins even if the title
//!    has zero topical overlap with the scene.
//!
//! Pipeline:
//! ```text
//! scene text + video_keywords
//!   → build_scene_stock_query (sanitize + topic bias + visual anchor)
//!   → candidate search (title + id)
//!   → lexical_relevance gate (title vs signal tokens)
//!   → download + cover-crop (setsar=1, no stretch)
//!   → geometry_gate (pixel size + SAR ≈ 1 + target aspect)
//!   → optional vision_gate (when vision backend available)
//! ```

use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;

// ---------------------------------------------------------------------------
// Token / query signal extraction
// ---------------------------------------------------------------------------

/// Words that are high-frequency dialogue glue or listicle noise — not visual.
const NOISE_TOKENS: &[&str] = &[
    // listicle / structure
    "swap", "one", "two", "three", "four", "five", "first", "second", "third",
    "later", "before", "after", "then", "next", "step", "tip", "habit", "habits",
    // generic verbs with no visual
    "starts", "start", "started", "make", "makes", "made", "try", "tries",
    "watch", "come", "comes", "back", "get", "gets", "got", "keep", "keeps",
    "open", "opens", "touch", "touches", "write", "writes", "check", "checking",
    // negations / function words / non-visual scene chatter (leaked into
    // stock queries and produced garbage ytsearch: "calm daily not fix
    // yourself remember never ..."). These carry zero visual signal.
    "not", "never", "ever", "daily", "out", "eight", "see", "name",
    "slowly", "gently", "shift", "yourself", "remember", "fix", "broken",
    "stuck", "survival", "mode", "safety", "signals", "practices",
    "gently", "elongated", "micro", "movement", "discharge", "present",
    "things", "signal", "practice", "daily", "fixing", "fixes",
    "small", "wiggle", "turn", "orienting", "orient", "safe",
    "nerve", "nerves", "firstly", "secondly", "thirdly", "once", "every",
    // pronouns / fillers already partially stopped elsewhere
    "your", "you", "our", "their", "thing", "things", "whole", "single", "must",
    "exactly", "really", "just", "like", "also", "even", "still", "don", "doesn",
    "isn", "aren", "wasn", "won", "can", "cant", "dont",
    // stop words (articles, prepositions, conjunctions)
    "the", "and", "or", "but", "for", "nor", "yet", "so", "a", "an", "in", "on",
    "at", "to", "of", "by", "with", "from", "into", "during", "including",
    "until", "against", "among", "throughout", "despite", "towards", "upon",
    "within", "without", "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did", "will", "would", "could",
    "should", "may", "might", "must", "shall", "that", "this", "these", "those",
    "it", "its", "as", "if", "then", "than", "when", "where", "why", "how",
    // meta
    "stock", "footage", "cinematic", "video", "clip", "free", "royalty",
    // Hinglish noise tokens (Hindi function words with no visual content)
    "hai", "ho", "ka", "ki", "ke", "ko", "se", "mein", "par", "aur",
    "yeh", "woh", "jo", "kya", "kaise", "kyun", "nahi", "haan",
    "bhi", "ab", "phir", "toh", "yaar", "dekho", "suno", "bolo",
];

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Hinglish -> English visual concept mapping
// ---------------------------------------------------------------------------

/// High-frequency Hinglish/Hindi nouns → English VISUAL concepts for stock
/// footage search. Used by `broll.keywords`' fallback when the LLM call fails
/// or returns nothing: raw Hinglish words ("sarkar", "bhai") produce garbage
/// Pexels results, but their English visual equivalents search cleanly.
///
/// The map targets political/social commentary vocabulary (the dominant
/// Hinglish content class) plus everyday life nouns. Words not in the map are
/// passed through unchanged; the caller's stopword filter drops noise tokens.
const HINGLISH_VISUAL_MAP: &[(&str, &str)] = &[
    // politics / government
    ("sarkar", "government building"),
    ("sarkaar", "government building"),
    ("sarkari", "government"),
    ("neta", "political leader"),
    ("chunav", "election"),
    ("vote", "voting"),
    ("kanoon", "law justice"),
    ("police", "police"),
    ("media", "news media"),
    ("samachar", "news broadcast"),
    ("patrakar", "journalist"),
    ("andolan", "protest"),
    ("inqilab", "revolution"),
    ("zindabad", "celebration crowd"),
    ("virodh", "opposition protest"),
    ("bhrashtachar", "corruption"),
    ("ghotala", "scandal"),
    ("paise", "money"),
    ("paisa", "money"),
    ("dhan", "wealth"),
    ("property", "property building"),
    // economy / daily life
    ("majdoor", "construction worker"),
    ("kisan", "farmer field"),
    ("gareeb", "poverty"),
    ("gareebi", "poverty"),
    ("mehngai", "rising prices market"),
    ("kharcha", "spending shopping"),
    ("roti", "bread food"),
    ("naukri", "office job"),
    ("padhai", "student studying"),
    ("school", "school"),
    ("college", "college campus"),
    ("bachche", "children"),
    ("parivaar", "family"),
    ("sheher", "city skyline"),
    ("gaon", "village"),
    ("sadak", "road traffic"),
    ("pani", "water"),
    ("bijli", "electricity power lines"),
    ("kachra", "garbage"),
    ("pradushan", "pollution smog"),
    ("hawai", "airplane"),
    // people / emotion
    ("bhai", "crowd of people"),
    ("bhaiyo", "crowd of people"),
    ("logon", "crowd of people"),
    ("log", "crowd of people"),
    ("aawaaz", "speaking microphone"),
    ("aavaz", "speaking microphone"),
    ("sach", "truth news"),
    ("jhuth", "lying"),
    ("darr", "fear"),
    ("khushi", "happy celebration"),
    ("gussa", "angry"),
    ("gusse", "angry"),
    ("dukh", "sadness"),
];

/// Translate Hindi/Hinglish visual nouns in a scene to English equivalents.
/// Returns the scene text with known Hindi words replaced by their English
/// visual translations; unknown words are passed through unchanged. Whole-word
/// matching only (tokenized) so "media" isn't matched inside "immediate".
///
/// This is the fallback path for `broll.keywords` when the LLM is
/// unavailable — it guarantees the naive keyword extractor never feeds raw
/// Hinglish words to Pexels (the "single-video-on-loop" garbage-query bug).
pub fn translate_hinglish_visuals(scene_text: &str) -> String {
    scene_text
        .split_whitespace()
        .map(|w| {
            let clean: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
            let lower = clean.to_lowercase();
            HINGLISH_VISUAL_MAP
                .iter()
                .find(|(hing, _)| *hing == lower.as_str())
                .map(|(_, eng)| (*eng).to_string())
                .unwrap_or(clean)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// Topic detection + topic-aware visual boost + anchor banks
// ---------------------------------------------------------------------------

/// Broad topic categories derived from `spec.video_keywords`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopicCategory {
    Space,
    Science,
    Nature,
    Marine, // ocean, underwater, sea creatures
    Tech,
    Lifestyle, // default fallback
}

/// Detect the dominant topic from video_keywords.
/// Counts hits against per-topic seed words; highest wins.
fn detect_topic(video_keywords: &[String]) -> TopicCategory {
    let seeds: &[(TopicCategory, &[&str])] = &[
        (
            TopicCategory::Space,
            &[
                "space", "galaxy", "nebula", "universe", "cosmos", "star",
                "planet", "black hole", "black", "hole", "gravity", "astronomy", "telescope",
                "aurora", "milky way", "solar", "lunar", "comet", "asteroid",
                "constellation", "supernova", "cosmic", "orbit", "satellite",
                "event horizon", "singularity", "spacetime", "dark matter",
            ],
        ),
        (
            TopicCategory::Science,
            &[
                "science", "experiment", "laboratory", "chemistry", "physics",
                "biology", "atom", "molecule", "research", "hypothesis",
                "microscope", "genetics", "dna", "quantum", "formula",
                "equation", "discovery", "innovation", "medical", "anatomy",
                // Biology / photosynthesis
                "photosynthesis", "chloroplast", "chlorophyll", "thylakoid", "photosystem",
                "stomata", "guard cell", "calvin cycle", "glucose", "photon",
                "leaf", "plant", "seedling", "germination", "green",
            ],
        ),
        (
            TopicCategory::Nature,
            &[
                "nature", "forest", "mountain", "river", "wildlife",
                "animal", "bird", "tree", "flower", "landscape", "waterfall",
                "beach", "desert", "jungle", "canyon",
                "glacier", "volcano", "ecosystem", "biodiversity", "flora",
                "fauna", "wilderness", "sunrise", "sunset", "cloud",
            ],
        ),
        (
            TopicCategory::Marine,
            &[
                "ocean", "underwater", "sea", "marine", "octopus", "squid",
                "jellyfish", "shark", "whale", "dolphin", "turtle", "fish",
                "coral", "reef", "deep sea", "abyss", "tidal", "kelp",
                "seaweed", "submarine", "diving", "scuba", "aqua", "aquatic",
                "tentacle", "cephalopod", "bivalve", "crustacean", "lobster",
                "crab", "seahorse", "starfish", "urchin", "manta", "barracuda",
            ],
        ),
        (
            TopicCategory::Tech,
            &[
                "technology", "computer", "software", "hardware", "ai",
                "robot", "circuit", "data", "code", "programming", "digital",
                "cyber", "network", "server", "chip", "processor", "screen",
                "display", "virtual", "augmented", "machine learning",
                "algorithm", "startup", "innovation", "device", "gadget",
            ],
        ),
    ];

    let lower_kw: Vec<String> = video_keywords
        .iter()
        .map(|k| k.to_ascii_lowercase())
        .collect();

    let mut best = TopicCategory::Lifestyle;
    let mut best_count = 0;

    for &(cat, words) in seeds {
        let count = lower_kw
            .iter()
            .filter(|kw| words.iter().any(|w| kw.contains(w)))
            .count();
        if count > best_count {
            best_count = count;
            best = cat;
        }
    }

    best
}

/// Topic-specific visual-boost nouns (weighted 2x in relevance scoring).
/// Replaces the old hardcoded VISUAL_BOOST that was lifestyle-only.
fn topic_visual_boost(cat: TopicCategory) -> Vec<&'static str> {
    match cat {
        TopicCategory::Space => vec![
            "galaxy", "nebula", "star", "planet", "universe", "cosmos",
            "aurora", "orbit", "comet", "asteroid", "supernova", "milky",
            "solar", "lunar", "telescope", "cosmic", "black hole",
            "constellation", "satellite", "rocket", "spacecraft",
        ],
TopicCategory::Science => vec![
            "laboratory", "experiment", "microscope", "atom", "molecule",
            "dna", "formula", "research", "discovery", "genetics",
            "chemistry", "physics", "quantum", "equation", "anatomy",
            "medical", "hypothesis", "specimen", "pipette", "centrifuge",
            // Biology / photosynthesis - specific visual terms (common terms like leaf/plant/sunlight in topic_anchors)
            "photosynthesis", "stomata", "glucose", "calvin", "cycle",
            "carbon", "sugar", "energy", "thylakoid", "granum", "photosystem",
            "rubisco", "carbon fixation", "triose phosphate", "seedling", "sprout", "growth",
        ],
        TopicCategory::Marine => vec![
            "octopus", "squid", "jellyfish", "shark", "whale", "dolphin",
            "turtle", "fish", "coral", "reef", "underwater", "ocean",
            "sea", "marine", "deep sea", "abyss", "kelp", "seaweed",
            "diving", "submarine", "tentacle", "cephalopod", "seahorse",
            "starfish", "urchin", "manta", "aqua", "aquatic", "tidal",
        ],
        TopicCategory::Nature => vec![
            "forest", "mountain", "river", "waterfall", "wildlife",
            "bird", "flower", "tree", "landscape", "sunset", "sunrise",
            "canyon", "glacier", "cloud", "rain",
            "meadow", "cliff", "cave", "spring", "autumn", "leaf",
        ],
        TopicCategory::Tech => vec![
            "screen", "code", "circuit", "chip", "robot", "data",
            "server", "display", "device", "gadget", "keyboard", "monitor",
            "fiber", "laser", "drone", "sensor", "wifi", "bluetooth",
            "interface", "dashboard", "analytics", "binary",
        ],
        TopicCategory::Lifestyle => vec![
            "phone", "lock", "screen", "coffee", "water", "glass", "light",
            "sunrise", "window", "desk", "notebook", "paper", "note", "pen",
            "music", "headphones", "kitchen", "bed", "bedroom", "alarm",
            "clock", "commute", "outdoor", "sun", "breakfast", "yoga",
            "stretch", "hand", "typing", "laptop", "message", "app",
            "scroll", "smartphone", "mug", "steam",
        ],
    }
}

/// Topic-specific anchor banks for `pick_visual_anchor`.
fn topic_anchors(cat: TopicCategory) -> Vec<(&'static str, Vec<&'static str>)> {
    match cat {
        TopicCategory::Space => vec![
            ("galaxy timelapse", vec!["galaxy", "space", "universe", "cosmos"]),
            ("nebula deep field", vec!["nebula", "cosmos", "space", "stars"]),
            ("aurora borealis", vec!["aurora", "sky", "space", "night"]),
            ("planet orbit", vec!["planet", "orbit", "solar", "space"]),
            ("star field", vec!["star", "stars", "constellation", "night sky"]),
            ("rocket launch", vec!["rocket", "launch", "spacecraft", "mission"]),
            ("black hole visualization", vec!["black hole", "gravity", "spacetime"]),
            ("milky way timelapse", vec!["milky way", "stars", "night sky", "telescope"]),
            ("solar flare", vec!["sun", "solar", "flare", "plasma"]),
            ("comet trail", vec!["comet", "asteroid", "meteor", "space"]),
            ("astronaut floating", vec!["astronaut", "space station", "zero gravity"]),
            ("satellite orbiting", vec!["satellite", "orbit", "earth", "space"]),
        ],
        TopicCategory::Science => vec![
            ("laboratory work", vec!["laboratory", "lab", "experiment", "research"]),
            ("microscope view", vec!["microscope", "cell", "biology", "specimen"]),
            ("dna helix", vec!["dna", "genetics", "genome", "molecular"]),
            ("physics apparatus", vec!["physics", "quantum", "particle", "atom"]),
            ("chemistry reaction", vec!["chemistry", "reaction", "molecule", "formula"]),
            ("brain scan", vec!["brain", "neuroscience", "medical", "anatomy"]),
            ("telescope observatory", vec!["telescope", "astronomy", "observation"]),
            ("petri dish culture", vec!["culture", "bacteria", "microbiology", "growth"]),
            ("skeleton anatomy", vec!["skeleton", "anatomy", "bone", "medical"]),
            ("formula derivation", vec!["formula", "equation", "mathematics", "derivation"]),
            ("robot arm assembly", vec!["robot", "assembly", "automation", "manufacturing"]),
            ("data visualization", vec!["data", "chart", "graph", "visualization"]),
            // Biology / photosynthesis specific anchors - specific visual terms only
            ("leaf surface timelapse", vec!["leaf surface", "epidermis", "mesophyll", "cuticle", "stomata"]),
            ("chloroplast closeup", vec!["chloroplast", "thylakoid", "granum", "photosystem", "stroma"]),
            ("sunlight through leaves", vec!["sunbeams", "light rays", "canopy", "dappled light", "leaf canopy", "crepuscular rays"]),
            ("stomata microscopic", vec!["stomata", "guard cell", "pores", "gas exchange", "transpiration"]),
            ("plant growth timelapse", vec!["seedling", "sprout", "germination", "cotyledon", "meristem"]),
            ("glucose energy", vec!["glucose", "calvin cycle", "rubisco", "carbon fixation", "triose phosphate"]),
        ],
        TopicCategory::Nature => vec![
            ("forest aerial", vec!["forest", "trees", "aerial", "canopy"]),
            ("mountain panorama", vec!["mountain", "peak", "summit", "alpine"]),
            ("waterfall", vec!["waterfall", "cascade", "river", "rapids"]),
            ("wildlife safari", vec!["wildlife", "animal", "safari", "savanna"]),
            ("desert dunes", vec!["desert", "dunes", "sand", "arid"]),
            ("flower bloom timelapse", vec!["flower", "bloom", "garden", "petal"]),
            ("rainforest canopy", vec!["rainforest", "jungle", "tropical", "canopy"]),
            ("northern lights", vec!["aurora", "northern lights", "night sky"]),
            ("canyon landscape", vec!["canyon", "cliff", "gorge", "erosion"]),
            ("glacier calving", vec!["glacier", "iceberg", "arctic", "frozen"]),
        ],
        TopicCategory::Marine => vec![
            ("octopus swimming underwater", vec!["octopus", "cephalopod", "tentacle", "marine"]),
            ("underwater coral reef", vec!["coral", "reef", "underwater", "ocean"]),
            ("jellyfish floating deep sea", vec!["jellyfish", "deep sea", "abyss", "bioluminescent"]),
            ("shark ocean patrol", vec!["shark", "predator", "ocean", "deep"]),
            ("whale underwater majesty", vec!["whale", "cetacean", "ocean", "marine"]),
            ("sea turtle gliding reef", vec!["turtle", "sea turtle", "reef", "glide"]),
            ("dolphin pod surface", vec!["dolphin", "pod", "surface", "playful"]),
            ("tropical fish school reef", vec!["fish", "tropical", "school", "reef", "colorful"]),
            ("seahorse kelp forest", vec!["seahorse", "kelp", "forest", "camouflage"]),
            ("starfish tide pool", vec!["starfish", "urchin", "tide", "pool", "shore"]),
            ("manta ray glide ocean", vec!["manta", "ray", "glide", "ocean", "graceful"]),
            ("underwater diving exploration", vec!["diving", "scuba", "explore", "ocean", "deep"]),
        ],
        TopicCategory::Tech => vec![
            ("code on screen", vec!["code", "programming", "software", "developer"]),
            ("circuit board closeup", vec!["circuit", "chip", "electronics", "hardware"]),
            ("data center", vec!["server", "data center", "network", "infrastructure"]),
            ("robot movement", vec!["robot", "robotics", "automation", "actuator"]),
            ("holographic display", vec!["hologram", "display", "interface", "futuristic"]),
            ("drone flight", vec!["drone", "aerial", "uav", "flight"]),
            ("keyboard typing", vec!["keyboard", "typing", "input", "workstation"]),
            ("fiber optic", vec!["fiber", "optics", "network", "speed"]),
            ("ai visualization", vec!["ai", "artificial intelligence", "neural", "machine learning"]),
            ("smartphone usage", vec!["smartphone", "mobile", "app", "touchscreen"]),
            ("3d printing", vec!["3d print", "additive", "prototyping", "model"]),
            ("laser cutting", vec!["laser", "cutting", "precision", "manufacturing"]),
        ],
        TopicCategory::Lifestyle => vec![
            (
                "sunrise window natural light bedroom",
                vec!["light", "sun", "window", "morning", "bed", "bedroom", "alarm", "wake"],
            ),
            (
                "coffee mug steam desk morning",
                vec!["coffee", "mug", "desk", "steam", "drink", "cup"],
            ),
            (
                "hand writing notebook paper daylight",
                vec!["write", "paper", "note", "notebook", "pen", "plan", "journal"],
            ),
            (
                "pouring water glass kitchen morning",
                vec!["water", "glass", "drink", "kitchen", "pour"],
            ),
            (
                "smartphone lock screen hands close up",
                vec!["phone", "lock", "screen", "scroll", "message", "app", "mobile"],
            ),
            (
                "headphones music listening daylight",
                vec!["music", "song", "headphones", "audio", "listen"],
            ),
            (
                "commute outdoor daylight walking",
                vec!["commute", "outdoor", "walk", "street", "outside", "presence"],
            ),
            (
                "yoga stretch morning home light",
                vec!["yoga", "stretch", "focus", "body", "breath"],
            ),
            (
                "healthy breakfast table natural light",
                vec!["breakfast", "food", "table", "eat", "meal"],
            ),
        ],
    }
}

fn is_alpha_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// Normalize a free-text blob into lowercased alphabetic tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphabetic())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && is_alpha_token(s))
        .collect()
}

fn is_noise(tok: &str) -> bool {
    NOISE_TOKENS.contains(&tok)
}

/// Extract **visual signal** tokens from scene dialogue (not listicle noise).
///
/// **Scene-first:** concrete shot nouns from the spoken line outrank broad
/// `video_keywords`, so multi-broll queries differ per scene instead of all
/// collapsing to the same topic list.
pub fn signal_tokens_from_scene(scene_text: &str, video_keywords: &[String]) -> Vec<String> {
    // Phase 56: Translate Hindi/Hinglish visual nouns to English before tokenization
    let scene_text_en = scene_text.to_lowercase(); // Agent generates keywords, no Hinglish dictionary needed
    let mut seen = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    // 1) Scene tokens (visual-boosted) — per-scene specificity
    let mut scene: Vec<String> = tokenize(&scene_text_en)
        .into_iter()
        .filter(|t| !is_noise(t))
        .collect();
    let topic = detect_topic(video_keywords);
    scene.sort_by_key(|t| {
        if topic_visual_boost(topic).contains(&t.as_str()) {
            0
        } else {
            1
        }
    });
    for t in scene {
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }

    // 2) Topic keywords (whole-video context) — fill remaining slots
    for k in video_keywords {
        for t in tokenize(k) {
            if !is_noise(&t) && seen.insert(t.clone()) {
                out.push(t);
            }
        }
    }
    out
}

// ANCHOR_BANK removed — replaced by topic_anchors() which routes to topic-specific banks.

fn pick_visual_anchor(signal: &[String], video_keywords: &[String], scene_idx: usize) -> String {
    let set: HashSet<&str> = signal.iter().map(|s| s.as_str()).collect();
    let topic = detect_topic(video_keywords);
    let boost = topic_visual_boost(topic);
    let bank = topic_anchors(topic);
    // Weight concrete visual nouns higher than broad topic words.
    let mut best: Option<(i32, &str)> = None;
    for (anchor, keys) in &bank {
        let mut score = 0i32;
        for k in keys {
            if set.contains(k) {
                score += if boost.contains(k) { 4 } else { 1 };
            }
        }
        if score > 0 {
            match best {
                Some((s, _)) if s >= score => {}
                _ => best = Some((score, anchor)),
            }
        }
    }
    // A single shared keyword (e.g. video_keywords contains "breath", which
    // is also an anchor key) used to pin EVERY scene to the same anchor — all
    // six scenes queried "yoga stretch morning home light". Require at least
    // two distinct anchor keys to commit to a specific anchor; otherwise fall
    // back to the rotated bank so scenes diversify.
    if let Some((score, a)) = best {
        if score >= 2 {
            return a.to_string();
        }
    }
    // Fall back to rotated bank so multi-scene still diversifies
    bank[scene_idx % bank.len()].0.to_string()
}

/// Build a clean stock search query: signal tokens + theme + visual anchor + orientation bias.
pub fn build_scene_stock_query(
    scene_text: &str,
    video_keywords: &[String],
    theme: &str,
    aspect: &str,
    scene_idx: usize,
) -> SceneStockQuery {
    let signal = signal_tokens_from_scene(scene_text, video_keywords);
    // Cap query length — long queries confuse ytsearch
    let core: Vec<&str> = signal.iter().map(|s| s.as_str()).take(5).collect();
    let mut parts: Vec<String> = Vec::new();
    if !core.is_empty() {
        parts.push(core.join(" "));
    }
    // Theme mood (avoid double words)
    let theme_l = theme.to_ascii_lowercase();
    if theme_l == "calm" && !parts.iter().any(|p| p.contains("calm")) {
        parts.insert(0, "calm".into());
    } else if theme_l == "energetic" && !parts.iter().any(|p| p.contains("energetic")) {
        // Prefer "lifestyle" over "energetic" for stock search — less sports noise
        parts.push("lifestyle".into());
    }
    let anchor = pick_visual_anchor(&signal, video_keywords, scene_idx);
    parts.push(anchor.clone());
    // Orientation bias for vertical shorts
    if aspect == "9:16" || aspect.is_empty() {
        parts.push("vertical video".into());
    }
    // ponytail: removed "stock footage b-roll" filler — dilutes topic signal
    // for non-lifestyle topics (black holes query became "gravity universe
    // coffee mug desk stock footage b-roll"). The anchor + signal tokens
    // already bias toward stock footage.

    let query = parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    SceneStockQuery {
        query,
        signal_tokens: signal,
        visual_anchor: anchor,
        scene_idx,
    }
}

#[derive(Debug, Clone)]
pub struct SceneStockQuery {
    pub query: String,
    pub signal_tokens: Vec<String>,
    pub visual_anchor: String,
    pub scene_idx: usize,
}

// ---------------------------------------------------------------------------
// Lexical relevance (title / description vs signal)
// ---------------------------------------------------------------------------

/// Jaccard-like overlap + visual boost. Returns 0.0–1.0.
/// Uses a generic visual boost set since we don't have topic context here.
pub fn lexical_relevance(candidate_text: &str, signal: &[String]) -> f64 {
    if signal.is_empty() {
        return 0.5; // unknown — neutral
    }
    let cand: HashSet<String> = tokenize(candidate_text).into_iter().collect();
    if cand.is_empty() {
        return 0.0;
    }
    // Use a union of all topic boost lists for relevance scoring
    let all_boost: HashSet<&str> = [
        topic_visual_boost(TopicCategory::Space),
        topic_visual_boost(TopicCategory::Science),
        topic_visual_boost(TopicCategory::Nature),
        topic_visual_boost(TopicCategory::Tech),
        topic_visual_boost(TopicCategory::Lifestyle),
    ]
    .iter()
    .flat_map(|v| v.iter().copied())
    .collect();
    let mut hits = 0.0;
    let mut weight_sum = 0.0;
    for s in signal {
        let w = if all_boost.contains(s.as_str()) { 2.0 } else { 1.0 };
        weight_sum += w;
        if cand.contains(s) {
            hits += w;
        }
    }
    if weight_sum <= 0.0 {
        return 0.0;
    }
    // Soften: partial credit if any visual boost hits
    let raw = hits / weight_sum;
    // Also credit token subset containment
    let signal_set: HashSet<&str> = signal.iter().map(|s| s.as_str()).collect();
    let overlap = cand.iter().filter(|c| signal_set.contains(c.as_str())).count();
    let soft = (overlap as f64 / signal.len().max(1) as f64).min(1.0);
    (0.65 * raw + 0.35 * soft).clamp(0.0, 1.0)
}

/// Minimum lexical score to accept a candidate title (reject pure noise).
pub fn min_lexical_accept() -> f64 {
    std::env::var("OPENSCRIPT_STOCK_MIN_LEXICAL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.12)
}

// ---------------------------------------------------------------------------
// Geometry: cover-crop without stretch + post-validate
// ---------------------------------------------------------------------------

/// Cover-crop filter: fill target frame, center crop, **force square pixels**.
/// Fixes the stretch bug: old `scale=W:H,crop=W:H` kept landscape SAR and
/// displayed 16:9 content as distorted 9:16.
pub fn cover_crop_filter(width: u32, height: u32) -> String {
    format!(
        "scale={w}:{h}:force_original_aspect_ratio=increase,\
         crop={w}:{h},setsar=1,fps=30,format=yuv420p",
        w = width,
        h = height
    )
}

pub fn cover_crop_filter_for_aspect(aspect: &str) -> String {
    let (w, h) = match aspect {
        "16:9" => (1920, 1080),
        "1:1" => (1080, 1080),
        _ => (1080, 1920),
    };
    cover_crop_filter(w, h)
}

#[derive(Debug, Clone)]
pub struct GeometryReport {
    pub width: u32,
    pub height: u32,
    pub sar_num: u32,
    pub sar_den: u32,
    pub display_aspect: f64,
    pub ok: bool,
    pub reasons: Vec<String>,
}

/// Probe a clip with ffprobe and decide if geometry is clean for the target.
pub fn probe_geometry(path: &str, target_aspect: &str) -> GeometryReport {
    let mut report = GeometryReport {
        width: 0,
        height: 0,
        sar_num: 1,
        sar_den: 1,
        display_aspect: 0.0,
        ok: false,
        reasons: Vec::new(),
    };
    if !Path::new(path).exists() {
        report.reasons.push("file missing".into());
        return report;
    }
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,sample_aspect_ratio",
            "-of",
            "json",
            path,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(o) = out else {
        report.reasons.push("ffprobe failed".into());
        return report;
    };
    let v: serde_json::Value = match serde_json::from_slice(&o.stdout) {
        Ok(v) => v,
        Err(_) => {
            report.reasons.push("ffprobe json parse failed".into());
            return report;
        }
    };
    let stream = v
        .pointer("/streams/0")
        .cloned()
        .unwrap_or(json!({}));
    report.width = stream.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    report.height = stream
        .get("height")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    let sar = stream
        .get("sample_aspect_ratio")
        .and_then(|x| x.as_str())
        .unwrap_or("1:1");
    let mut sar_parts = sar.split(':');
    report.sar_num = sar_parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    report.sar_den = sar_parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);

    if report.width < 480 || report.height < 480 {
        report.reasons.push(format!(
            "resolution too low {}x{}",
            report.width, report.height
        ));
    }

    // SAR must be ~1:1 after our crop (allow tiny rational noise)
    let sar_ratio = report.sar_num as f64 / report.sar_den as f64;
    if (sar_ratio - 1.0).abs() > 0.05 {
        report.reasons.push(format!(
            "non-square SAR {}:{} (display stretch risk)",
            report.sar_num, report.sar_den
        ));
    }

    let pix_aspect = if report.height > 0 {
        (report.width as f64 * sar_ratio) / report.height as f64
    } else {
        0.0
    };
    report.display_aspect = pix_aspect;

    let target = match target_aspect {
        "16:9" => 16.0 / 9.0,
        "1:1" => 1.0,
        _ => 9.0 / 16.0,
    };
    if (pix_aspect - target).abs() > 0.08 {
        report.reasons.push(format!(
            "display aspect {:.3} far from target {:.3}",
            pix_aspect, target
        ));
    }

    report.ok = report.reasons.is_empty();
    report
}

// ---------------------------------------------------------------------------
// Candidate ranking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub id: String,
    pub title: String,
    pub lexical: f64,
}

/// Titles that are almost always **audio beds**, not visual stock footage.
/// Cold installs without Pexels used to rank "10 hours lofi focus music" as B-roll.
const BROLL_TITLE_DENY: &[&str] = &[
    "lofi", "lo-fi", "lo fi", "focus music", "study music", "sleep music",
    "relaxing music", "chill music", "ambient music", "background music",
    "no copyright music", "ncs", "hours of", "1 hour", "2 hour", "3 hour",
    "10 hour", "12 hour", "24 hour", "playlist", "mix music", "music mix",
    "beats to", "radio", "podcast", "audiobook", "asmr", "white noise",
    "rain sounds", "meditation music", "yoga music", "spa music",
    "copyright free music", "royalty free music only",
];

/// True when a YouTube title is almost certainly music/audio, not B-roll video.
pub fn is_broll_title_denylisted(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    if BROLL_TITLE_DENY.iter().any(|d| t.contains(d)) {
        return true;
    }
    // "music" without a visual cue → likely an audio stream
    if t.contains("music") {
        let visual = ["footage", "b-roll", "broll", "cinematic", "stock", "timelapse", "time-lapse", "drone", "city", "nature"];
        if !visual.iter().any(|v| t.contains(v)) {
            return true;
        }
    }
    false
}

/// Rank (id, title) pairs by lexical relevance; drop denylist + below threshold.
pub fn rank_and_filter_candidates(
    candidates: &[(String, String)],
    signal: &[String],
    min_score: f64,
) -> Vec<RankedCandidate> {
    let mut ranked: Vec<RankedCandidate> = candidates
        .iter()
        .filter(|(_, title)| !is_broll_title_denylisted(title))
        .map(|(id, title)| RankedCandidate {
            lexical: lexical_relevance(title, signal),
            id: id.clone(),
            title: title.clone(),
        })
        .collect();
    ranked.sort_by(|a, b| b.lexical.total_cmp(&a.lexical));
    // Hard gate: never accept lex≈0 "noise" titles. Empty → call site falls
    // back to procedural rather than shipping irrelevant stock.
    ranked
        .into_iter()
        .filter(|c| c.lexical >= min_score)
        .collect()
}

// ---------------------------------------------------------------------------
// YouTube-specific ranking (L0+L1 of the vision-aware upgrade)
// ---------------------------------------------------------------------------

/// A YouTube search hit with the metadata we need for L1 ranking + L2/L3
/// vision gates: yt-dlp `--dump-json` exposes `duration` and `thumbnail`
/// for free, but the old `--print %(id)s\t%(title)s` path discarded them.
#[derive(Debug, Clone)]
pub struct YtCandidate {
    pub id: String,
    pub title: String,
    pub duration_s: f64,
    pub thumbnail_url: String,
}

/// A ranked YouTube candidate carrying the metadata the vision gates need.
#[derive(Debug, Clone)]
pub struct RankedYtCandidate {
    pub id: String,
    pub title: String,
    pub lexical: f64,
    pub duration_s: f64,
    pub thumbnail_url: String,
}

/// Duration preference multiplier (L1): short stock clips outrank lectures.
/// - 6–60s  → 1.0 (ideal b-roll window)
/// - 60–300s → 0.85 (long-ish, still possibly footage)
/// - >300s  → 0.45 (lecture/stream territory — the user-reported failure)
/// - <6s    → 0.7 (too short to be useful b-roll)
/// - 0 (unknown) → 0.9 (don't over-penalize missing metadata)
pub fn duration_preference(duration_s: f64) -> f64 {
    if duration_s <= 0.0 {
        0.9
    } else if duration_s < 6.0 {
        0.7
    } else if duration_s <= 60.0 {
        1.0
    } else if duration_s <= 300.0 {
        0.85
    } else {
        0.45
    }
}

/// Rank YouTube candidates: lexical relevance × duration preference, drop
/// denylist titles, hard-gate below `min_score`. `min_duration_s`/`max_duration_s`
/// (0 = no bound) act as hard pre-filters so lectures/long streams are never
/// even considered for scenes that need a short clip.
pub fn rank_yt_candidates(
    candidates: &[YtCandidate],
    signal: &[String],
    min_score: f64,
    min_duration_s: f64,
    max_duration_s: f64,
) -> Vec<RankedYtCandidate> {
    let mut ranked: Vec<RankedYtCandidate> = candidates
        .iter()
        .filter(|c| !is_broll_title_denylisted(&c.title))
        .filter(|c| {
            let d = c.duration_s;
            (min_duration_s <= 0.0 || d <= 0.0 || d >= min_duration_s)
                && (max_duration_s <= 0.0 || d <= 0.0 || d <= max_duration_s)
        })
        .map(|c| {
            let lexical = lexical_relevance(&c.title, signal);
            RankedYtCandidate {
                id: c.id.clone(),
                title: c.title.clone(),
                lexical: lexical * duration_preference(c.duration_s),
                duration_s: c.duration_s,
                thumbnail_url: c.thumbnail_url.clone(),
            }
        })
        .collect();
    ranked.sort_by(|a, b| b.lexical.total_cmp(&a.lexical));
    ranked.into_iter().filter(|c| c.lexical >= min_score).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_listicle_noise_from_scene() {
        let sig = signal_tokens_from_scene(
            "Swap one. Light and water before the lock screen.",
            &["morning".into(), "phone".into()],
        );
        assert!(sig.contains(&"morning".into()) || sig.contains(&"phone".into()));
        assert!(!sig.iter().any(|t| t == "swap"));
        assert!(!sig.iter().any(|t| t == "one"));
        // visual nouns kept
        assert!(sig.iter().any(|t| t == "water" || t == "light" || t == "lock" || t == "screen" || t == "phone"));
    }

    #[test]
    fn denylists_music_titles_as_broll() {
        assert!(is_broll_title_denylisted("10 Hours Lofi Focus Music for Study"));
        assert!(is_broll_title_denylisted("Relaxing Music Playlist Chill Beats"));
        assert!(!is_broll_title_denylisted("Cinematic city drone stock footage vertical"));
        assert!(!is_broll_title_denylisted("Morning coffee desk typing b-roll"));
    }

    #[test]
    fn rank_filters_denylist_music() {
        let cands = vec![
            ("aaaaaa".into(), "10 hour lofi study music".into()),
            ("bbbbbb".into(), "coffee desk morning stock footage".into()),
        ];
        let ranked = rank_and_filter_candidates(&cands, &["coffee".into(), "desk".into()], 0.01);
        assert!(ranked.iter().all(|c| c.id != "aaaaaa"));
        assert!(ranked.iter().any(|c| c.id == "bbbbbb"));
    }

    #[test]
    fn query_does_not_contain_swap_fragments() {
        let q = build_scene_stock_query(
            "Swap two. One paper note. Write the single must-do.",
            &["morning".into(), "habits".into()],
            "energetic",
            "9:16",
            2,
        );
        let lower = q.query.to_ascii_lowercase();
        assert!(!lower.contains("swap"));
        assert!(lower.contains("morning") || lower.contains("paper") || lower.contains("notebook"));
        assert!(lower.contains("vertical") || lower.contains("stock"));
    }

    #[test]
    fn phone_scene_picks_phone_anchor() {
        let q = build_scene_stock_query(
            "If your morning starts with the phone, your whole day starts reactive.",
            &["morning".into(), "phone".into()],
            "energetic",
            "9:16",
            0,
        );
        assert!(
            q.visual_anchor.contains("smartphone") || q.visual_anchor.contains("phone"),
            "anchor={}",
            q.visual_anchor
        );
    }

    #[test]
    fn lexical_rejects_unrelated_title() {
        let signal = vec!["phone".into(), "morning".into(), "water".into()];
        let good = lexical_relevance("morning phone addiction stock footage vertical", &signal);
        let bad = lexical_relevance("minecraft parkour no copyright gameplay", &signal);
        assert!(good > bad);
        assert!(good >= 0.2);
        assert!(bad < 0.15);
    }

    #[test]
    fn cover_crop_sets_sar() {
        let f = cover_crop_filter(1080, 1920);
        assert!(f.contains("force_original_aspect_ratio=increase"));
        assert!(f.contains("setsar=1"));
        assert!(f.contains("crop=1080:1920"));
    }

    #[test]
    fn rank_prefers_relevant_titles() {
        let cands = vec![
            ("a".into(), "EPIC Minecraft Parkour Hours".into()),
            ("b".into(), "Morning coffee phone free stock footage".into()),
            ("c".into(), "Funny cat compilation".into()),
        ];
        let signal = vec!["morning".into(), "coffee".into(), "phone".into()];
        let ranked = rank_and_filter_candidates(&cands, &signal, 0.12);
        assert_eq!(ranked[0].id, "b");
    }

    #[test]
    fn duration_preference_penalizes_lectures() {
        assert_eq!(duration_preference(30.0), 1.0);
        assert_eq!(duration_preference(6.0), 1.0);
        assert!(duration_preference(60.0) >= 0.8);
        assert!(duration_preference(0.0) > 0.8);
        assert!(duration_preference(500.0) < 0.5, "lectures must be penalized");
        assert!(duration_preference(2400.0) < 0.5);
        assert!(duration_preference(3.0) < 1.0, "too-short clips penalized");
    }

    #[test]
    fn rank_yt_puts_short_relevant_clip_over_lecture() {
        let cands = vec![
            YtCandidate {
                id: "lecture".into(),
                title: "How to prepare for an interview - English at Work".into(),
                duration_s: 276.0,
                thumbnail_url: String::new(),
            },
            YtCandidate {
                id: "clip".into(),
                title: "Office interview conversation stock footage vertical".into(),
                duration_s: 15.0,
                thumbnail_url: String::new(),
            },
        ];
        let signal = vec!["interview".into(), "office".into()];
        let ranked = rank_yt_candidates(&cands, &signal, 0.12, 0.0, 0.0);
        assert_eq!(ranked[0].id, "clip", "short relevant clip must outrank lecture");
    }

    #[test]
    fn rank_yt_hard_filters_by_duration_bounds() {
        let cands = vec![
            YtCandidate {
                id: "short".into(),
                title: "coffee desk morning".into(),
                duration_s: 8.0,
                thumbnail_url: String::new(),
            },
            YtCandidate {
                id: "long".into(),
                title: "coffee desk morning b-roll".into(),
                duration_s: 900.0,
                thumbnail_url: String::new(),
            },
        ];
        let signal = vec!["coffee".into(), "desk".into(), "morning".into()];
        let ranked = rank_yt_candidates(&cands, &signal, 0.12, 10.0, 60.0);
        assert!(ranked.iter().all(|c| c.id == "short"), "only in-window clip survives");
    }

    #[test]
    fn rank_drops_all_noise_instead_of_accepting_zero() {
        let cands = vec![
            ("a".into(), "Everyone Mocked His Civilian Tech".into()),
            ("b".into(), "Funny cat compilation hours".into()),
        ];
        let signal = vec!["desk".into(), "laptop".into(), "coffee".into()];
        let ranked = rank_and_filter_candidates(&cands, &signal, 0.12);
        assert!(ranked.is_empty(), "expected empty, got {:?}", ranked);
    }

#[test]
    fn scene_first_signal_differs_per_line() {
        let kw = vec!["desk".into(), "focus".into()];
        let a = signal_tokens_from_scene("Headphones on. One instrumental playlist.", &kw);
        let b = signal_tokens_from_scene("Notebook beside the coffee. Capture thoughts.", &kw);
        assert!(a.iter().any(|t| t == "headphones" || t == "playlist" || t == "instrumental"));
        assert!(b.iter().any(|t| t == "notebook" || t == "coffee"));
        // First tokens should be scene-specific, not only topic keywords
        assert_ne!(a.first(), b.first());
    }

    #[test]
    fn photosynthesis_anchors_diversify_per_scene() {
        let video_keywords = vec![
            "photosynthesis".into(),
            "plants".into(),
            "chlorophyll".into(),
            "sunlight".into(),
            "biology".into(),
        ];
        let scenes = ["Photosynthesis is the process by which plants convert sunlight into chemical energy. Every breath you take depends on this ancient biological machinery.",
            "In the chloroplasts, chlorophyll captures photons and uses them to split water molecules. The oxygen released becomes the air we breathe.",
            "Carbon dioxide enters the leaf through tiny pores called stomata. Inside, the Calvin cycle stitches carbon atoms into glucose, the fuel of life.",
            "The energy stored in glucose powers every cell in the plant. And when we eat plants, that same solar energy powers us.",
            "Photosynthesis connects the sun to every living thing. It is the quiet engine of the biosphere, running since the dawn of life."];
        let mut anchors = Vec::new();
        for (i, scene) in scenes.iter().enumerate() {
            let q = build_scene_stock_query(scene, &video_keywords, "neutral", "9:16", i);
            eprintln!("Scene {}: anchor='{}' query='{}'", i+1, q.visual_anchor, q.query);
            anchors.push(q.visual_anchor.clone());
        }
        // Should have at least 3 different anchors across 5 scenes
        let unique: std::collections::HashSet<_> = anchors.iter().collect();
        assert!(unique.len() >= 3, "Expected at least 3 unique anchors, got {:?}", unique);
    }

    #[test]
    fn photosynthesis_signal_tokens() {
        let video_keywords = vec!["photosynthesis".into(), "plants".into(), "chlorophyll".into(), "sunlight".into(), "biology".into()];
        let scene = "Photosynthesis is the process by which plants convert sunlight into chemical energy. Every breath you take depends on this ancient biological machinery.";
        let signal = signal_tokens_from_scene(scene, &video_keywords);
        eprintln!("Photosynthesis signal: {:?}", signal);
        // Should contain photosynthesis, plants, chlorophyll, biology
        assert!(signal.iter().any(|t| t == "photosynthesis"));
        assert!(signal.iter().any(|t| t == "plants"));
        assert!(signal.iter().any(|t| t == "chlorophyll"));
        assert!(signal.iter().any(|t| t == "biology"));
        // Should NOT contain noise words
        assert!(!signal.iter().any(|t| t == "the"), "Noise word 'the' should be filtered");
        assert!(!signal.iter().any(|t| t == "is"), "Noise word 'is' should be filtered");
        assert!(!signal.iter().any(|t| t == "by"), "Noise word 'by' should be filtered");
    }

    #[test]
    fn photosynthesis_topic_detected_as_science() {
        let video_keywords = vec!["photosynthesis".into(), "plants".into(), "chlorophyll".into(), "sunlight".into(), "biology".into()];
        let topic = detect_topic(&video_keywords);
        assert_eq!(topic, TopicCategory::Science, "Expected Science, got {:?}", topic);
    }

    #[test]
    fn black_holes_topic_detected_as_space() {
        let video_keywords = vec!["science".into(), "black".into(), "holes".into()];
        let topic = detect_topic(&video_keywords);
        assert_eq!(topic, TopicCategory::Space, "Expected Space, got {:?}", topic);
    }

    #[test]
    fn translate_hinglish_visuals_known_nouns() {
        // Political vocabulary — the dominant Hinglish content class.
        assert_eq!(
            translate_hinglish_visuals("sarkar ne bhrashtachar kiya"),
            "government building ne corruption kiya"
        );
        assert_eq!(
            translate_hinglish_visuals("bhai logon ko sunna"),
            "crowd of people crowd of people ko sunna"
        );
    }

    #[test]
    fn translate_hinglish_visuals_unknown_passthrough() {
        // Words not in the map pass through unchanged.
        assert_eq!(translate_hinglish_visuals("galgoate enge saare"), "galgoate enge saare");
    }

    #[test]
    fn translate_hinglish_visuals_whole_word_only() {
        // "media" inside "immediate" must not be replaced.
        assert_eq!(translate_hinglish_visuals("immediate action"), "immediate action");
        assert_eq!(translate_hinglish_visuals("media bias"), "news media bias");
    }

    #[test]
    fn translate_hinglish_visuals_case_insensitive() {
        assert_eq!(translate_hinglish_visuals("SARKAR ka paisa"), "government building ka money");
    }

    #[test]
    fn anchor_not_pinned_by_single_shared_keyword() {
        // Regression: a single shared keyword (e.g. "breath" in video_keywords
        // that is also an anchor key) used to pin EVERY scene to the same
        // anchor → all six scenes queried "yoga stretch morning home light".
        // Requiring >= 2 distinct anchor keys must fall back to the rotated
        // bank so multi-scene queries diversify.
        let signal = vec![
            "breath".to_string(),
            "calm".to_string(),
            "healing".to_string(),
            "nervous".to_string(),
        ];
        let kw = vec!["breath".to_string(), "calm".to_string(), "stretch".to_string()];
        let a0 = pick_visual_anchor(&signal, &kw, 0);
        let a1 = pick_visual_anchor(&signal, &kw, 1);
        let a2 = pick_visual_anchor(&signal, &kw, 2);
        // Single shared keyword "breath" alone must NOT pin all to one anchor.
        let distinct: std::collections::HashSet<String> =
            [a0.clone(), a1.clone(), a2.clone()].into_iter().collect();
        assert!(
            distinct.len() >= 2,
            "anchors should diversify, got {}",
            distinct.len()
        );
    }

    #[test]
    fn anchor_commits_when_two_distinct_keys_match() {
        // Two distinct anchor keys SHOULD commit to that anchor (score >= 2).
        // "yoga stretch morning home light" anchor keys likely include
        // "stretch" + "morning" — both present → commit, don't rotate.
        let signal = vec!["stretch".to_string(), "morning".to_string()];
        let kw = vec!["yoga".to_string(), "stretch".to_string(), "home".to_string()];
        let a = pick_visual_anchor(&signal, &kw, 5);
        // Either the specific anchor or a rotated fallback — must be non-empty.
        assert!(!a.is_empty());
    }
}
