//! Unified keyword infrastructure — the single owner of "given a segment,
//! produce the right search keywords."
//!
//! Consolidates the fragmented keyword-drafting paths that previously lived in
//! `tools.rs` (`llm_draft_keywords`, `extract_broll_concept`, `safe_search_query`),
//! `tools_broll.rs` (`handle_broll_keywords` fallback), `tools_sticker.rs`
//! (`handle_sticker_keywords` fallback), and `stock_signal.rs` (heuristic query
//! builder + ASCII-only tokenizer) into ONE module:
//!
//! - [`draft_scene_keywords`] — one batched LLM call emitting BOTH `visual`
//!   (stock-footage) and `reactions` (GIPHY sticker) keywords per segment,
//!   with strict id echo-back, a missing-id redraft pass, and a draft-quality
//!   gate. This is the primary path for every workflow (script→video, A2V, V2V).
//! - [`extract_salient_keywords`] / [`heuristic_scene_query`] — the single
//!   universal LLM-down fallback: salience-scored (never "first three words"),
//!   Unicode-aware (Devanagari/Arabic/Cyrillic/CJK survive), topic-registry
//!   free (content-derived, no position-rotation bias).
//! - [`sanitize_query`] / [`keywords_to_query`] — shared post-processors applied
//!   by every workflow (content-safety rewrite + theme enrichment + shaping).
//!
//! Design rules (see docs/KEYWORD_PIPELINE_AUDIT.md):
//! 1. One fallback for ALL entry points — same "LLM down" → same quality.
//! 2. No hardcoded-language bias: [`auto_detect_language`] replaces the
//!    hardcoded `"hinglish"` default in prompts.
//! 3. The heuristic never rotates content-blind anchors — it derives signal
//!    from the scene's own text.
//! 4. B-roll and stickers share ONE draft pass but consume DIFFERENT outputs
//!    (`visual` vs `reactions`) — never visual nouns for sticker search.

use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;

/// Maximum segments per LLM draft call.
pub const MAX_DRAFT_BATCH: usize = 15;
/// Minimum keyword length (chars) to be searchable.
pub const MIN_KEYWORD_LEN: usize = 3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What a segment is, for the drafter. Everything the LLM needs to tailor
/// keywords to THIS segment (not just the caption text).
#[derive(Debug, Clone)]
pub struct SegmentInput {
    pub segment_id: String,
    pub caption: String,
    /// Explicit source-language hint (e.g. "hinglish", "hindi", "english").
    /// `None` → auto-detected from the caption.
    pub language_hint: Option<String>,
    /// Segment window in seconds (0.0 when unknown). The drafter uses this to
    /// pick clip-appropriate specificity.
    pub duration_s: f64,
    /// Position in the video (0-based). Used so the drafter knows whether this
    /// is the hook, the body, or the close.
    pub scene_idx: usize,
    pub total_scenes: usize,
    /// Whole-video context (title + topic keywords) — biases drafts to the
    /// video's subject while keeping per-scene specificity.
    pub video_title: String,
    pub video_keywords: Vec<String>,
    /// Visual concepts already covered elsewhere in the timeline — the drafter
    /// must AVOID re-suggesting them (non-redundant single-shot pass).
    pub covered_concepts: Vec<String>,
}

/// The drafted keywords for one segment.
#[derive(Debug, Clone)]
pub struct SceneKeywords {
    pub segment_id: String,
    /// Stock-footage search keywords (Pexels / Pixabay / YouTube).
    pub visual: Vec<String>,
    /// GIPHY sticker search keywords (reaction / emotion / meme).
    pub reactions: Vec<String>,
    /// Emotional intent label (sticker gate).
    pub intent: Option<String>,
    /// True when the segment carries real emotional weight (sticker gate).
    pub emphatic: bool,
    /// Where the keywords came from (LLM / heuristic / hybrid merge).
    pub source: KeywordSource,
    /// Draft-quality confidence 0..1 — used by the quality gate.
    pub confidence: f64,
    /// LLM backend that produced the draft ("heuristic-v1" when LLM down).
    pub backend: String,
}

impl SceneKeywords {
    fn fallback(input: &SegmentInput) -> SceneKeywords {
        let mut visual = extract_salient_keywords(&input.caption, 4);
        // Hinglish fallback must also be English-only (same residue gate as the
        // LLM output — the translated caption still leaks function words).
        let lang = input.language_hint.as_deref().unwrap_or("english");
        if is_hinglish_lang(lang) {
            visual.retain(|k| is_searchable_english_keyword(k));
        }
        SceneKeywords {
            segment_id: input.segment_id.clone(),
            visual,
            reactions: Vec::new(), // LLM-down ⇒ no sticker auto-approval
            intent: None,
            emphatic: false,
            source: KeywordSource::Heuristic,
            confidence: 0.35,
            backend: "heuristic-v1".into(),
        }
    }
}

/// Origin of a keyword set (for observability + the quality gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordSource {
    Llm,
    Heuristic,
    Hybrid,
}

impl KeywordSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeywordSource::Llm => "llm",
            KeywordSource::Heuristic => "heuristic",
            KeywordSource::Hybrid => "hybrid",
        }
    }
}

// ---------------------------------------------------------------------------
// Unicode-aware tokenizer + consolidated stopwords
// ---------------------------------------------------------------------------

/// Consolidated stopwords: articles/prepositions/conjunctions/pronouns/
/// auxiliaries + listicle noise + generic non-visual verbs + Hinglish function
/// words. This REPLACES the duplicated `NOISE_TOKENS` (stock_signal) and the
/// `extract_broll_concept` stopword array (tools.rs) for all drafting paths.
const STOPWORDS: &[&str] = &[
    // articles / prepositions / conjunctions / pronouns
    "the", "a", "an", "in", "on", "at", "to", "of", "by", "with", "from", "into",
    "during", "including", "until", "against", "among", "throughout", "despite",
    "towards", "upon", "within", "without", "and", "or", "but", "nor", "yet", "so",
    "for", "is", "are", "was", "were", "be", "been", "being", "am",
    "have", "has", "had", "do", "does", "did", "will", "would", "could", "should",
    "may", "might", "must", "shall", "can", "cant", "dont", "doesn", "isn", "aren",
    "wasn", "won", "that", "this", "these", "those", "it", "its", "as", "if", "then",
    "than", "when", "where", "why", "how", "which", "who", "whom", "whose", "what",
    "your", "you", "yourself", "they", "them", "their", "theirs", "he", "she", "him",
    "her", "his", "hers", "we", "us", "our", "ours", "my", "me", "mine", "i", "im",
    "there", "here", "all", "any", "some", "more", "most", "other", "such", "only",
    "own", "same", "than", "too", "very", "just", "also", "even", "still", "really",
    "exactly", "thing", "things", "whole", "single", "every", "once", "again", "about",
    "because", "tell", "told", "says", "work", "works", "working", "start", "started",
    "starts", "going", "come", "comes", "came", "look", "looks", "looking",
    "make", "makes", "made", "get", "gets", "got", "want", "wants", "need",
    "needs", "think", "thinks", "thought", "know", "knows", "mean", "means",
    "today", "yesterday", "tomorrow", "actually", "basically", "literally",
    "guys", "people", "something", "someone", "somebody", "anything", "anyone",
    "everything", "everyone", "nothing", "nobody",
    // listicle / structure noise
    "swap", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    "ten", "first", "second", "third", "firstly", "secondly", "thirdly", "later",
    "before", "after", "then", "next", "step", "tip", "habit", "habits",
    // generic verbs with no visual
    "start", "starts", "started", "make", "makes", "made", "try", "tries", "watch",
    "come", "comes", "back", "get", "gets", "got", "keep", "keeps", "open", "opens",
    "touch", "touches", "write", "writes", "check", "checking", "see", "look", "looks",
    "say", "says", "said", "go", "goes", "went", "put", "puts", "take", "takes", "took",
    "give", "gives", "gave", "want", "wants", "need", "needs", "think", "thinks",
    "know", "knows", "mean", "means", "remember", "forget", "feels", "feel",
    // negations / non-visual chatter
    "not", "never", "ever", "daily", "out", "name", "slowly", "gently", "shift",
    "fix", "broken", "stuck", "survival", "mode", "safety", "signals", "practices",
    "present", "signal", "practice", "fixing", "fixes", "small", "wiggle", "turn",
    "orienting", "orient", "safe", "nerve", "nerves",
    // meta
    "stock", "footage", "cinematic", "video", "clip", "free", "royalty",
    // Hinglish function words (Latin-script) — no visual content
    "hai", "ho", "ka", "ki", "ke", "ko", "se", "mein", "par", "aur", "yeh", "woh",
    "jo", "kya", "kaise", "kyun", "nahi", "haan", "bhi", "ab", "phir", "toh", "yaar",
    "dekho", "suno", "bolo", "kar", "karne", "kare", "hoga", "thi", "tha", "raha",
    "rah", "baat", "chahiye", "hoon", "wala", "wale", "wali", "koi", "bahut", "saare",
    "log", "logon", "bhai", "bhaiyo", "aap", "tum", "tera", "mera", "apna", "kuchh",
    "kuch", "sab", "jab", "tab", "agar", "lekin", "jis", "jinki", "hain", "hue", "hua",
];

/// Unicode-aware tokenization: split on any non-alphanumeric char, keep any
/// script (Devanagari, Arabic, Cyrillic, CJK, Latin) + digits, lowercase via
/// Unicode. Tokens need ≥2 chars and at least one alphabetic char.
pub fn unicode_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|s| s.to_lowercase())
        .filter(|s| {
            let n = s.chars().count();
            n >= 2 && (s.chars().any(|c| c.is_alphabetic()) || s.chars().all(|c| c.is_ascii_digit()))
        })
        .collect()
}

fn is_stopword(tok: &str) -> bool {
    STOPWORDS.contains(&tok)
}

// ---------------------------------------------------------------------------
// Salience-based heuristic extraction (replaces "first three words")
// ---------------------------------------------------------------------------

/// Extract the most salient search keywords from a caption WITHOUT the LLM.
///
/// Scores tokens by: length (longer = more specific), proper-noun casing,
/// digit-content ("rule 72", "10x"), and within-caption frequency. Returns up
/// to `max_n` keywords ordered by salience — never positional (the old
/// `extract_broll_concept` took the first three non-stopwords, which for
/// "I want to tell you about the stock market" yielded "want tell stock").
pub fn extract_salient_keywords(caption: &str, max_n: usize) -> Vec<String> {
    let mut counts: HashMap<String, (usize, usize, bool, bool)> = HashMap::new();
    // first occurrence index, occurrence count, was capitalized, contains digit
    for (i, raw) in caption.split_whitespace().enumerate() {
        let was_cap = raw
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        for tok in unicode_tokens(raw) {
            if is_stopword(&tok) {
                continue;
            }
            let has_digit = tok.chars().any(|c| c.is_ascii_digit());
            let e = counts.entry(tok).or_insert((i, 0, was_cap, has_digit));
            e.1 += 1;
            e.2 |= was_cap;
            e.3 |= has_digit;
        }
    }
    let mut scored: Vec<(f64, usize, String)> = counts
        .into_iter()
        .map(|(tok, (first, count, cap, digit))| {
            let len_bonus = if tok.chars().count() >= 6 { 1.5 } else { 0.0 };
            let cap_bonus = if cap { 1.0 } else { 0.0 };
            let digit_bonus = if digit { 0.8 } else { 0.0 };
            let freq = 1.0 + ((count - 1) as f64) * 0.6;
            let score = (1.0 + len_bonus + cap_bonus + digit_bonus) * freq;
            (score, first, tok)
        })
        .collect();
    // Salience desc, then original position asc (ties)
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0).then(a.1.cmp(&b.1))
    });
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for (_, _, tok) in scored {
        // Alpha tokens need >= MIN_KEYWORD_LEN; pure-digit tokens (e.g. "72"
        // in "Rule of 72", "10x") are meaningful at length 2.
        let is_pure_digit = tok.chars().all(|c| c.is_ascii_digit());
        if tok.chars().count() < MIN_KEYWORD_LEN && !is_pure_digit {
            continue;
        }
        if seen.insert(tok.clone()) {
            out.push(tok);
            if out.len() >= max_n {
                break;
            }
        }
    }
    out
}

/// Lightweight source-language auto-detection. Returns one of
/// "hinglish" / "hindi" / "arabic" / "russian" / "cjk" / "english".
/// Replaces the hardcoded `"hinglish"` default in draft prompts.
pub fn auto_detect_language(text: &str) -> String {
    if text.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) {
        return "hindi".into();
    }
    if text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)) {
        return "arabic".into();
    }
    if text
        .chars()
        .any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
    {
        return "russian".into();
    }
    if text.chars().any(|c| c.is_ascii_digit() == false && c.is_alphanumeric() && !c.is_ascii()) {
        // non-Latin, non-Devanagari/Arabic/Cyrillic (CJK etc.)
        return "cjk".into();
    }
    // Whole-token Hinglish function-word markers (Latin-script Hinglish uses
    // the Roman alphabet, so it needs content-based detection rather than a
    // script-range check). Exact-token matching avoids false positives like
    // "kar" inside "karma" or English "log".
    let lower = text.to_lowercase();
    let toks: std::collections::HashSet<&str> = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|t| !t.is_empty())
        .collect();
    const HINGLISH_MARKERS: &[&str] = &[
        "hai", "ka", "ki", "ke", "ko", "se", "mein", "par", "aur", "yeh", "woh",
        "bhai", "nahi", "kya", "baat", "chahiye", "log", "logon", "saare", "hoga",
        "kar", "ne", "raha", "tha", "thi", "hua", "hue", "kiya", "kare", "karne",
        "dekho", "suno", "bolo", "hain", "hoon", "wala", "wale", "aap", "tum",
        "apna", "tera", "mera", "kuch", "sab", "jab", "tab", "agar", "lekin",
    ];
    if HINGLISH_MARKERS.iter().any(|m| toks.contains(m)) {
        return "hinglish".into();
    }
    "english".into()
}

/// Implements the documented-but-missing `video_keywords` auto-extraction from
/// the script title. Up to 5 topic keywords, salience-ordered.
pub fn auto_extract_video_keywords(title: &str) -> Vec<String> {
    let mut kws = extract_salient_keywords(title, 5);
    if kws.is_empty() {
        // Title all-stopwords (rare) — fall back to raw tokens
        kws = unicode_tokens(title)
            .into_iter()
            .filter(|t| t.chars().count() >= MIN_KEYWORD_LEN)
            .take(5)
            .collect();
    }
    kws
}

/// True when the source language needs Hinglish→English visual translation
/// (hi / hindi / hinglish). English and other scripts pass through untouched.
pub fn is_hinglish_lang(lang: &str) -> bool {
    let l = lang.to_ascii_lowercase();
    l.starts_with("hi") || l.contains("hinglish") || l.contains("hindi")
}

/// Map known Hinglish/Hindi visual nouns to English (sarkaar → government) so
/// BOTH the LLM draft prompt and the salience fallback receive readable
/// English. Non-Hinglish input is returned unchanged (no-op).
pub fn translate_caption_if_needed(caption: &str, language: &str) -> String {
    if is_hinglish_lang(language) {
        crate::stock_signal::translate_hinglish_visuals(caption)
    } else {
        caption.to_string()
    }
}

/// Hinglish function/residue tokens the drafter tends to echo verbatim instead
/// of translating (pronouns, particles, verbs, filler — never stock-searchable).
const HINGLISH_RESIDUE: &[&str] = &[
    // Pronouns / determiners
    "hisaab", "saahab", "sahab", "mere", "meri", "mera", "tere", "teri", "teri",
    "tumhara", "humara", "apna", "apni", "apne", "unka", "unke", "unki", "iska",
    "uska", "inke", "inhen", "inhe", "inko", "unki", "kisi", "kisiko", "koi",
    "kuchh", "kuch", "sab", "sabhi", "saare", "kaafi", "bahut", "itna", "kitna",
    // Verbs / auxiliaries
    "hai", "hain", "ho", "tha", "thi", "the", "hoga", "hua", "hui", "hue",
    "raha", "rahi", "rahe", "rakha", "rakhi", "rakhe", "kiya", "kiye", "karta",
    "karte", "karna", "karne", "kar", "karke", "dete", "deta", "diya", "diye",
    "lena", "lete", "jata", "jaate", "gaya", "gaye", "bana", "bane", "banti",
    "mila", "mili", "mil", "nikal", "nikala", "khila", "khilaya", "pahunch",
    "pahunche", "samajh", "samjha", "chahiye", "chahie", "dekh", "dekha", "suno",
    "suna", "bola", "bole", "aaya", "aaye", "liya", "liye", "karo", "karo",
    // Particles / adverbs / conjunctions
    "bhi", "hi", "na", "nahin", "nahi", "mat", "ab", "abhi", "yah", "yeh",
    "woh", "wo", "aur", "par", "per", "se", "ko", "ka", "ke", "ki", "mein",
    "maine", "jis", "jin", "jinhonne", "inhonne", "usne", "unhone", "yahan",
    "wahan", "wahin", "keval", "sirf", "baad", "pehle", "phir", "tab", "jab",
    "kyun", "kyon", "kyonki", "isliye", "lekin", "magar", "agar", "toh", "to",
    "idhar", "udhar", "andar", "bahar", "upar", "neeche", "aaj", "kal", "uss",
    // Nouns / filler the drafter echoes instead of translating (drop: the
    // translated-caption heuristic covers the meaningful ones)
    "zindagi", "aavaaz", "avaz", "garibi", "kaanoon", "kanoon", "qanoon", "farq",
    "farak", "fark", "thodi", "thoda", "shuruaat", "badhiya", "chaaloo", "chalu",
    "zaroori", "tarike", "tarika", "chaahe", "chayan", "phati", "patti", "patta",
    "uchh", "ash", "aash", "bachana", "bacha", "bach", "desh", "log", "logon",
    "baat", "baatein", "rasta", "raasta", "ghar", "kaam", "kam", "waqt", "din",
    "raat", "saal", "paisa", "paise", "gaadi", "gaadiyaan", "gadi", "sadak",
    "enge", "bajaaya", "galgoate", "aise", "aisa", "usi", "unhi", "inhi",
    "jaisa", "jaise", "waise", "bhai", "dost", "yaar", "bhaai", "kya", "kyu",
];

/// Compact English content-word whitelist used by the Hinglish English-only
/// gate. ASR garbage ("phishiega", "galgoate", "enge") is unbounded and can
/// never be blacklisted — requiring at least one recognized English content
/// token per keyword rejects it deterministically while keeping real English
/// terms ("government", "crowd", "smartphone").
const COMMON_ENGLISH_WORDS: &[&str] = &[
    "people", "person", "man", "woman", "men", "women", "child", "children", "family",
    "crowd", "group", "city", "town", "street", "road", "building", "house", "home",
    "government", "office", "school", "hospital", "court", "police", "army", "soldier",
    "protest", "rally", "march", "flag", "india", "indian", "country", "nation", "world",
    "money", "cash", "bank", "loan", "tax", "business", "market", "shop", "factory",
    "corruption", "crime", "criminal", "murder", "rape", "prison", "jail", "handcuff",
    "law", "laws", "rule", "rules", "justice", "media", "news", "press", "journalist",
    "tv", "television", "camera", "phone", "mobile", "smartphone", "social", "media",
    "internet", "online", "account", "app", "computer", "laptop", "screen", "vote",
    "election", "politics", "politician", "leader", "minister", "modi", "parliament",
    "speech", "speaker", "public", "audience", "speaker", "voice", "sound", "microphone",
    "gas", "fuel", "oil", "petrol", "diesel", "cooking", "kitchen", "stove", "food",
    "water", "paani", "river", "sea", "ocean", "rain", "air", "pollution", "smoke",
    "fire", "car", "cars", "vehicle", "bus", "train", "truck", "bike", "traffic",
    "road", "highway", "station", "airport", "travel", "journey", "time", "clock",
    "night", "morning", "evening", "day", "sun", "moon", "sky", "cloud", "mountain",
    "forest", "tree", "nature", "field", "farm", "agriculture", "farmer", "crop", "village",
    "india", "delhi", "mumbai", "city", "life", "quality", "living", "standard",
    "work", "job", "worker", "labour", "employee", "salary", "unemployment", "jobless",
    "health", "doctor", "medicine", "hospital", "disease", "pandemic", "vaccine", "mask",
    "education", "teacher", "student", "university", "college", "exam", "book", "pen",
    "paper", "notebook", "handwriting", "write", "writing", "read", "reading", "document",
    "form", "signature", "file", "filing", "papers", "news", "newspaper", "article",
    "report", "story", "channel", "broadcast", "live", "video", "film", "movie", "song",
    "music", "dance", "celebration", "party", "festival", "happy", "sad", "angry", "fear",
    "shocked", "surprise", "surprised", "excited", "excitement", "love", "hate", "anger",
    "hope", "freedom", "right", "rights", "security", "safety", "danger", "war", "peace",
    "revolution", "change", "development", "progress", "growth", "economy", "financial",
    "inflation", "price", "cost", "ration", "subsidy", "scheme", "welfare", "pension",
    "house", "housing", "slum", "construction", "bridge", "dam", "power", "electricity",
    "light", "streetlight", "powercut", "loadshedding", "water", "supply", "pipeline",
    "drain", "sewage", "garbage", "waste", "clean", "cleanliness", "toilet", "sanitation",
    "road", "pothole", "infrastructure", "railway", "rail", "metro", "transport", "commute",
    "traffic", "jam", "accident", "crash", "injury", "death", "funeral", "candle", "tribute",
    "justice", "case", "cases", "hearing", "judge", "lawyer", "witness", "evidence", "guilty",
    "innocent", "arrest", "suspect", "gang", "mafia", "scam", "fraud", "fake", "hoax",
    "truth", "lie", "lies", "propaganda", "censorship", "ban", "banned", "block", "blocked",
    "protest", "slogan", "demonstration", "strike", "violence", "riot", "conflict", "tension",
    "youth", "student", "job", "aspiration", "dream", "goal", "future", "generation", "youth",
    "vote", "voter", "ballot", "mandate", "opposition", "party", "coalition", "bill", "act",
    "constitution", "democracy", "dictatorship", "authoritarian", "suppression", "oppression",
    "human", "rights", "protest", "movement", "campaign", "awareness", "message", "speech",
    "interview", "debate", "discussion", "talk", "question", "answer", "opinion", "view",
    "analysis", "commentary", "expert", "panel", "host", "guest", "anchor", "reporter",
    "camera", "studio", "set", "stage", "lighting", "gallery", "audience", "applause",
    "clap", "clapping", "laugh", "laughing", "smile", "smiling", "tears", "crying", "cry",
    "scream", "shout", "whisper", "silence", "quiet", "noise", "music", "drum", "guitar",
    "dance", "party", "wedding", "marriage", "ceremony", "ritual", "temple", "mosque",
    "church", "gurudwara", "festival", "diwali", "holi", "eid", "christmas", "new year",
    "food", "rice", "wheat", "grain", "ration", "cooking", "meal", "hunger", "starvation",
    "poverty", "poor", "rich", "wealth", "luxury", "corruption", "black money", "smuggling",
    "border", "war", "army", "defence", "missile", "tank", "terrorism", "terrorist", "attack",
    "bomb", "blast", "explosion", "injury", "wounded", "hospital", "ambulance", "rescue",
    "relief", "aid", "donation", "charity", "volunteer", "help", "support", "solidarity",
    // Common adjectives / adverbs / misc English content words
    "new", "old", "big", "small", "high", "low", "long", "short", "young", "first",
    "last", "good", "bad", "great", "best", "worst", "real", "fake", "true", "false",
    "open", "closed", "public", "private", "local", "national", "international", "global",
    "york", "yogi", "world", "city", "state", "center", "central", "union", "federal",
    "total", "full", "empty", "strong", "weak", "fast", "slow", "hard", "soft", "hot",
    "cold", "dark", "bright", "clear", "clean", "dirty", "safe", "free", "equal", "major",
    "minor", "special", "general", "common", "social", "digital", "physical", "mental",
    "national", "regional", "rural", "urban", "tribal", "daily", "weekly", "monthly",
    "annual", "present", "future", "past", "current", "recent", "early", "late", "main",
    "top", "bottom", "right", "left", "central", "direct", "indirect", "official", "illegal",
    // Indian geography + prominent names (proper nouns appear in Hinglish drafts)
    "delhi", "mumbai", "kolkata", "chennai", "bangalore", "bengaluru", "hyderabad", "jaipur",
    "lucknow", "patna", "kanpur", "varanasi", "agra", "goa", "chandigarh", "pune", "ahmedabad",
    "surat", "indore", "bhopal", "nagpur", "thiruvananthapuram", "guwahati", "amritsar", "jammu",
    "srinagar", "shimla", "dehradun", "ranchi", "raipur", "bhubaneswar", "gangtok", "itanagar",
    "dispur", "imphal", "aizawl", "kohima", "agartala", "port blair", "puducherry", "daman", "diu",
    "bihar", "up", "uttar", "pradesh", "punjab", "haryana", "rajasthan", "gujarat", "maharashtra",
    "kerala", "tamil", "nadu", "karnataka", "andhra", "telangana", "odisha", "jharkhand",
    "chhattisgarh", "himachal", "uttarakhand", "assam", "bengal", "west bengal", "madhya", "kashmir",
    "ladakh", "sikkim", "nagaland", "manipur", "meghalaya", "tripura", "mizoram", "arunachal",
    "modi", "narendra", "shah", "amit", "rahul", "gandhi", "priyanka", "sonia", "manmohan",
    "singh", "atal", "vajpayee", "indira", "jawaharlal", "nehru", "ambedkar", "bhagat", "singh",
    "subhash", "chandra", "bose", "tilak", "savarkar", "shivaji", "mahatma", "gandhi", "kejriwal",
    "arvind", "mayawati", "mamata", "banerjee", "owaisi", "asaduddin", "nitish", "kumar", "soren",
    "hemant", "yogi", "adityanath", "sushma", "swaraj", "smriti", "irani", "rajnath", "singh",
    "jaishankar", "nirmala", "sitharaman", "piyush", "goyal", "amit", "nadda", "hemant", "biren",
    "singh", "manik", "saha", "conrad", "sangma", "pema", "khandu", "premier", "naveen", "patnaik",
    "mohan", "yadav", "akhilesh", "mulayam", "lalu", "prasad", "tejashwi", "rabri", "devi", "upendra",
    "kushwaha", "tejasvi", "surya", "mallikarjun", "kharge", "jagan", "mohan", "reddy", "kcr",
    "chandrababu", "naidu", "pawan", "kalyan", "ram", "gopal", "yadav", "owaisi", "asaduddin",
    "sanjay", "raut", "uddhav", "thackeray", "devendra", "fadnavis", "eknath", "shinde", "ajit", "pawar",
    "sharad", "pawar", "supriya", "sule", "mamata", "abhishek", "banerjee", "derek", "o'brien",
    "arindam", "bagchi", "shatrughan", "sinha", "shekhar", "gupta", "ravish", "kumar", "rajdeep",
    "sardesai", "arnab", "goswami", "vikram", "chandra", "swati", "maliwal", "kiran", "bedi",
    "anna", "hazare", "army", "bipin", "rawat", "gen", "narrative", "narratives", "fake news",
];

/// True when at least one whitespace token of `kw` is a recognized English
/// content word (or the whole phrase contains a known English word).
pub fn has_english_content_word(kw: &str) -> bool {
    kw.split_whitespace().any(|tok| {
        let lower = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_lowercase();
        COMMON_ENGLISH_WORDS.contains(&lower.as_str())
    })
}

/// Deterministic English-only gate for DRAFTED keywords on Hinglish/Hindi
/// sources. The LLM is non-deterministic — on a bad draw it echoes raw Hinglish
/// tokens ("chaaloo", "farq", "thodi") that Pexels cannot search. A keyword is
/// searchable when it has no Devanagari characters, no Hinglish residue, and
/// contains at least one recognized English content word (catches unbounded
/// ASR garbage like "phishiega").
pub fn is_searchable_english_keyword(kw: &str) -> bool {
    if kw.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) {
        return false;
    }
    let lower = kw.trim().to_lowercase();
    if HINGLISH_RESIDUE.contains(&lower.as_str()) {
        return false;
    }
    has_english_content_word(kw)
}

/// Derive the whole-video context (title + topic keywords) from a transcript —
/// the V2V/A2V-from-audio equivalent of a script's `title` + `video_keywords`.
/// One LLM call extracts 3-6 topical keywords that anchor every per-segment
/// draft; LLM-down falls back to the deterministic salience heuristic over the
/// translated transcript + title tokens. Without this anchor the per-segment
/// drafter hallucinates from noisy ASR fragments (the "cooking turkey oven"
/// for an India-politics video bug). Returns (title, topic_keywords).
pub async fn derive_video_context(
    captions: &[String],
    title_hint: &str,
    language: &str,
) -> (String, Vec<String>) {
    let title = clean_video_title(title_hint, captions);
    let joined: String = captions
        .iter()
        .filter(|c| !c.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let transcript = translate_caption_if_needed(&joined, language);
    if transcript.trim().is_empty() {
        let kws = derive_video_context_heuristic("", &title);
        return (title, kws);
    }
    // LLM-first: one call for the video's topic keywords.
    let system = "You are a video-topic keyword extractor for a stock-footage pipeline. \
        Given a video transcript (possibly noisy ASR with Hinglish/Hindi), return 3-6 short English \
        topic keywords that describe what the WHOLE video is about — concrete nouns/topics a stock \
        camera could search for (e.g. 'india politics protest', 'corruption media censorship'). \
        Output ONLY a JSON object: {\"keywords\": [\"...\"]}.";
    let user = format!(
        "Video title: \"{}\"\n\nTranscript:\n{}\n\nOutput ONLY the JSON object.",
        title, transcript
    );
    if let Ok(r) = crate::llm::chat_complete(system, &user, None).await {
        if let Some(kws) = extract_json_obj(&r.text)
            .get("keywords")
            .and_then(|v| v.as_array())
        {
            let kws: Vec<String> = kws
                .iter()
                .filter_map(|k| k.as_str().map(|s| s.trim().to_string()))
                .filter(|k| k.chars().count() >= MIN_KEYWORD_LEN)
                .take(6)
                .collect();
            if !kws.is_empty() {
                return (title, kws);
            }
        }
    }
    let kws = derive_video_context_heuristic(&transcript, &title);
    (title, kws)
}

/// Deterministic LLM-down topic fallback: salience over the translated
/// transcript, anchored with title tokens. Never empty.
pub fn derive_video_context_heuristic(transcript: &str, title: &str) -> Vec<String> {
    let mut signal = extract_salient_keywords(transcript, 5);
    let mut seen: HashSet<String> = signal.iter().cloned().collect();
    for kw in auto_extract_video_keywords(title) {
        for tok in unicode_tokens(&kw) {
            if !is_stopword(&tok) && seen.insert(tok.clone()) {
                signal.push(tok);
            }
        }
    }
    if signal.is_empty() {
        signal = vec!["video".to_string()];
    }
    signal
}

fn clean_video_title(hint: &str, captions: &[String]) -> String {
    let hint = hint
        .replace('_', " ")
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !hint.is_empty() {
        return hint;
    }
    auto_extract_video_keywords(
        &captions
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    )
    .first()
    .cloned()
    .unwrap_or_else(|| "Untitled Video".to_string())
}

/// The UNIFIED per-segment keyword set that drives BOTH b-roll and stickers:
/// reaction/intent terms first (what GIPHY actually indexes), then concrete
/// visual subject nouns (keeps the sticker context-relevant to the segment's
/// subject, not just its mood). Deduped, capped at 4, never empty.
pub fn blend_sticker_keywords(visual: &[String], reactions: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for kw in reactions.iter().chain(visual.iter()) {
        let k = kw.trim().to_string();
        if k.chars().count() >= 3 && seen.insert(k.clone()) {
            out.push(k);
        }
        if out.len() >= 4 {
            break;
        }
    }
    if out.is_empty() {
        out.push("funny".to_string());
    }
    out
}

// ---------------------------------------------------------------------------
// Topic registry (data-driven, expanded) — used for heuristic context only.
// The LLM draft is primary; this registry only matters when the cascade is down.
// ---------------------------------------------------------------------------

const TOPIC_SEEDS: &[(&str, &[&str])] = &[
    (
        "psychology",
        &[
            "psychology", "brain", "mind", "behavior", "behavioral", "influence",
            "persuasion", "habit", "emotion", "feelings", "cognition", "perception",
            "motivation", "rapport", "hypnosis", "confidence", "anxiety", "focus",
            "attention", "memory", "consciousness", "subconscious",
        ],
    ),
    (
        "society",
        &[
            "society", "politics", "political", "government", "election", "protest",
            "culture", "social", "media", "news", "law", "justice", "rights",
            "community", "class", "inequality", "propaganda", "ideology",
        ],
    ),
    (
        "finance",
        &[
            "finance", "money", "invest", "investment", "stock", "market", "trading",
            "crypto", "bitcoin", "economy", "business", "startup", "entrepreneur",
            "wealth", "income", "budget", "saving", "debt", "bank", "recession",
        ],
    ),
    (
        "sports",
        &[
            "sport", "sports", "fitness", "gym", "workout", "training", "exercise",
            "running", "football", "cricket", "basketball", "tennis", "yoga", "athlete",
            "competition", "match", "stadium", "strength",
        ],
    ),
    (
        "food",
        &[
            "food", "cooking", "recipe", "kitchen", "chef", "restaurant", "cuisine",
            "ingredients", "baking", "meal", "coffee", "tea", "breakfast", "dinner",
        ],
    ),
    (
        "fashion",
        &[
            "fashion", "style", "clothing", "outfit", "designer", "model", "runway",
            "accessories", "shopping", "brand", "beauty", "makeup",
        ],
    ),
    (
        "gaming",
        &[
            "game", "gaming", "video game", "esports", "console", "playstation",
            "xbox", "pc gaming", "minecraft", "fortnite", "gamer", "streaming",
        ],
    ),
    (
        "music",
        &[
            "music", "song", "singing", "guitar", "piano", "drums", "band", "concert",
            "artist", "album", "rapper", "beat", "melody", "dj",
        ],
    ),
    (
        "education",
        &[
            "education", "school", "college", "university", "student", "teacher",
            "study", "learning", "exam", "course", "degree", "classroom", "tutorial",
        ],
    ),
    (
        "travel",
        &[
            "travel", "tourism", "trip", "adventure", "destination", "flight",
            "hotel", "backpacking", "wander", "explore", "vacation", "road trip",
        ],
    ),
    (
        "health",
        &[
            "health", "healthcare", "medical", "doctor", "hospital", "medicine",
            "disease", "treatment", "wellness", "therapy", "mental health", "sleep",
            "nutrition", "diet",
        ],
    ),
    (
        "space",
        &[
            "space", "galaxy", "nebula", "universe", "cosmos", "star", "planet",
            "black hole", "gravity", "astronomy", "telescope", "aurora", "solar",
            "lunar", "comet", "asteroid", "constellation", "orbit", "satellite",
        ],
    ),
    (
        "science",
        &[
            "science", "experiment", "laboratory", "chemistry", "physics", "biology",
            "atom", "molecule", "research", "microscope", "genetics", "dna", "quantum",
            "photosynthesis", "chlorophyll", "stomata", "innovation",
        ],
    ),
    (
        "nature",
        &[
            "nature", "forest", "mountain", "river", "wildlife", "animal", "bird",
            "tree", "flower", "landscape", "waterfall", "beach", "desert", "jungle",
            "canyon", "glacier", "volcano", "ecosystem", "sunrise", "sunset",
        ],
    ),
    (
        "marine",
        &[
            "ocean", "underwater", "sea", "marine", "octopus", "squid", "jellyfish",
            "shark", "whale", "dolphin", "turtle", "fish", "coral", "reef", "diving",
        ],
    ),
    (
        "tech",
        &[
            "technology", "computer", "software", "hardware", "ai", "robot",
            "circuit", "data", "code", "programming", "digital", "cyber", "network",
            "server", "chip", "processor", "machine learning", "algorithm", "gadget",
        ],
    ),
];

/// Detect the dominant topic category from whole-video keywords. Returns the
/// category label ("psychology", "tech", …, default "lifestyle").
pub fn detect_topic_label(video_keywords: &[String]) -> &'static str {
    let lower: Vec<String> = video_keywords
        .iter()
        .map(|k| k.to_ascii_lowercase())
        .collect();
    let mut best = "lifestyle";
    let mut best_count = 0usize;
    for (label, seeds) in TOPIC_SEEDS {
        let count = lower
            .iter()
            .filter(|kw| seeds.iter().any(|w| kw.contains(w)))
            .count();
        if count > best_count {
            best_count = count;
            best = label;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Content-safety + theme enrichment (consolidated from tools.rs)
// ---------------------------------------------------------------------------

/// Unsafe keyword → safe visual replacement (content-safety rewrite).
/// Previously `UNSAFE_KEYWORD_MAP` in tools.rs, applied ONLY by script.to_video.
const UNSAFE_KEYWORD_MAP: &[(&str, &str)] = &[
    ("inhale", "breathing meditation"),
    ("exhale", "breathing relaxation"),
    ("breathe", "breathing calm"),
    ("breathing", "breathing meditation"),
    ("drink", "drinking water wellness"),
    ("smoke", "calm nature"),
    ("drug", "calm nature"),
    ("kill", "calm nature"),
    ("blood", "calm nature"),
    ("pain", "healing wellness"),
    ("stress", "stress relief meditation"),
    ("anxiety", "anxiety relief calm"),
    ("fear", "courage calm nature"),
    ("death", "calm nature peaceful"),
    ("weapon", "calm nature"),
];

/// Enrich a query with mood-aware context to bias results toward
/// calming/energetic content (theme:calm → peaceful footage, not literal
/// "inhale → cigarette" tone-misses).
fn enrich_query_for_theme(query: &str, theme: &str) -> String {
    let lower = query.to_lowercase();
    let already_calm =
        lower.contains("calm") || lower.contains("peaceful") || lower.contains("meditation");
    let already_energetic = lower.contains("energy") || lower.contains("action") || lower.contains("intense");
    match theme {
        "calm" if !already_calm => format!("calm {}", query),
        "energetic" if !already_energetic => format!("energetic {}", query),
        _ => query.to_string(),
    }
}

/// Shared content-safety + theme post-processor for search queries.
/// Replaces `safe_search_query` (tools.rs) — now applied by EVERY workflow,
/// not just script.to_video.
pub fn sanitize_query(raw_keywords: &str, theme: &str) -> String {
    let mut safe_words: Vec<String> = Vec::new();
    for word in raw_keywords.split_whitespace() {
        let lower = word.to_lowercase();
        let replaced = UNSAFE_KEYWORD_MAP
            .iter()
            .find(|(unsafe_word, _)| *unsafe_word == lower.as_str())
            .map(|(_, safe)| safe.to_string())
            .unwrap_or_else(|| word.to_string());
        safe_words.push(replaced);
    }
    enrich_query_for_theme(&safe_words.join(" "), theme)
}

// ---------------------------------------------------------------------------
// Query shaping (one rule, everywhere)
// ---------------------------------------------------------------------------

/// The single keyword→query shaper: cap terms, join, then sanitize + theme
/// enrich. `orientation` ("9:16" | "16:9" | "1:1" | "") appends a vertical
/// qualifier for the 9:16/unknown cases (ytsearch benefits; API-oriented
/// engines handle orientation server-side).
pub fn keywords_to_query(keywords: &[String], max_terms: usize, orientation: &str, theme: &str) -> String {
    let core: Vec<&str> = keywords
        .iter()
        .filter(|k| k.chars().count() >= MIN_KEYWORD_LEN)
        .take(max_terms.max(1))
        .map(|s| s.as_str())
        .collect();
    let joined = core.join(" ");
    let mut q = sanitize_query(&joined, theme);
    if (orientation == "9:16" || orientation.is_empty()) && !q.contains("vertical") {
        q = format!("{} vertical video", q.trim_end());
    }
    q
}

// ---------------------------------------------------------------------------
// Heuristic scene query (de-biased fallback)
// ---------------------------------------------------------------------------

/// Content-derived fallback query for a scene: salience keywords from the
/// scene text + whole-video topic keywords, shaped via [`keywords_to_query`].
/// No position-based anchor rotation, no Lifestyle collapse — works for any
/// script in any script system.
pub fn heuristic_scene_query(
    scene_text: &str,
    video_keywords: &[String],
    theme: &str,
    aspect: &str,
    _scene_idx: usize,
) -> (String, Vec<String>) {
    // Translate known Hinglish visual nouns first (reuses the stock_signal
    // dictionary so the LLM-down path still avoids raw "sarkar"/"bhai" queries).
    let translated = crate::stock_signal::translate_hinglish_visuals(scene_text);
    let mut signal = extract_salient_keywords(&translated, 6);
    let mut seen: HashSet<String> = signal.iter().cloned().collect();
    for kw in video_keywords {
        for tok in unicode_tokens(kw) {
            if !is_stopword(&tok) && seen.insert(tok.clone()) {
                signal.push(tok);
            }
        }
    }
    if signal.is_empty() {
        // Truly empty scene — keep a neutral, non-lifestyle fallback
        signal = vec!["b-roll".to_string()];
    }
    let query = keywords_to_query(&signal, 6, aspect, theme);
    (query, signal)
}

// ---------------------------------------------------------------------------
// LLM drafting — the primary path
// ---------------------------------------------------------------------------

fn extract_json_obj(s: &str) -> serde_json::Value {
    let trimmed = s.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return v;
    }
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]) {
                    return v;
                }
            }
        }
    }
    json!({})
}

fn batch_language(inputs: &[SegmentInput]) -> String {
    for i in inputs {
        if let Some(lang) = &i.language_hint {
            if !lang.is_empty() {
                return lang.clone();
            }
        }
    }
    inputs
        .first()
        .map(|i| auto_detect_language(&i.caption))
        .unwrap_or_else(|| "english".into())
}

fn build_prompt(inputs: &[SegmentInput], language: &str) -> (String, String) {
    let title = inputs
        .iter()
        .find(|i| !i.video_title.is_empty())
        .map(|i| i.video_title.clone())
        .unwrap_or_default();
    let mut title_ctx = String::new();
    if !title.is_empty() {
        title_ctx = format!("\nVideo title/context: \"{}\"\n", title);
    }
    let mut topic_ctx = String::new();
    let topic: Vec<String> = inputs
        .iter()
        .flat_map(|i| i.video_keywords.iter().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .take(6)
        .collect();
    if !topic.is_empty() {
        topic_ctx = format!("\nVideo topic keywords: [{}]\n", topic.join(", "));
    }
    let covered: Vec<String> = inputs
        .iter()
        .flat_map(|i| i.covered_concepts.iter().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let covered_ctx = if covered.is_empty() {
        String::new()
    } else {
        format!(
            "\nVisual concepts ALREADY COVERED elsewhere in this video (AVOID repeating them — each segment needs DISTINCT relevant footage): {}\n",
            covered.join(", ")
        )
    };

    let system = format!(
        "You are a stock-footage AND GIPHY-sticker keyword drafter for a short-form video production pipeline. \
        Your job: for each spoken segment, output TWO keyword sets.\n\
        - \"visual\": 2-3 English VISUAL search keywords for stock video sites (Pexels, Pixabay) — concrete things a camera can film \
        (objects, people, places, actions). Keywords must MATCH THIS SEGMENT's spoken content, not the video topic in general.\n\
        - \"reactions\": 0-3 short GIPHY sticker search keywords describing the REACTION/EMOTION/MEME that fits the spoken content \
        (e.g. 'mind blown', 'facepalm', 'celebration', 'sad', 'thumbs up', 'shocked'). \
        Use EMPTY [] for calm/filler segments — plain statements, connectors, mundane narration — no sticker is better than an irrelevant one.\n\
        - \"intent\": one of anger|surprise|hype|celebration|sarcasm|sad|question|emphasis|none\n\
        - \"emphatic\": true ONLY when the segment carries real emotional weight (shock, anger, hype, punchline, big claim, strong opinion).\n\
        Rules:\n\
        1. Output ONLY valid JSON — no markdown, no explanation\n\
        2. Each keyword 1-3 words; concrete and searchable; no abstractions\n\
        3. Translate Hinglish/Hindi (or any language) by MEANING, not word-for-word. \
        NEVER output raw Hinglish/Hindi words (e.g. 'hisaab', 'saahab', 'chaaloo', 'farq', \
        'thodi') — every visual and reaction keyword MUST be English (or the target \
        stock site's language). If a segment is in Hinglish, write the English concept.\n\
        4. Use the segment's duration (long window → wider shot term; short window → single-subject term) and position (hook/body/close) to pick specificity\n\
        5. Echo the EXACT segment id for every segment — one result per segment, no renumbering\n\
        6. Vary phrasing ACROSS segments: never reuse the same query template for consecutive scenes \
        (e.g. avoid 'person X person Y' chains) — give each scene distinct, concrete visual words \
        (objects, settings, actions, close-ups, lighting) that match ITS OWN content.\n\
        Source language detected: {}{}{}{}\n\
        Output format: {{\"results\": [{{\"id\": \"seg_X\", \"visual\": [\"v1\",\"v2\"], \"reactions\": [\"r1\"], \"intent\": \"emphasis\", \"emphatic\": true}}]}}",
        language, title_ctx, topic_ctx, covered_ctx
    );

    let mut lines = Vec::new();
    for i in inputs.iter() {
        let dur = if i.duration_s > 0.0 {
            format!(" ({}s window)", (i.duration_s * 10.0).round() / 10.0)
        } else {
            String::new()
        };
        lines.push(format!(
            "[{}] scene {}/{}: \"{}\"{}",
            i.segment_id,
            i.scene_idx + 1,
            i.total_scenes.max(1),
            i.caption,
            dur
        ));
    }
    let user = format!(
        "Draft visual + reaction keywords for each segment. Output ONLY the JSON object.\n\n{}",
        lines.join("\n")
    );
    (system, user)
}

fn id_to_index(inputs: &[SegmentInput]) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    for (i, input) in inputs.iter().enumerate() {
        map.insert(input.segment_id.clone(), i);
        // Also accept index-based aliases (LLMs sometimes renumber)
        map.insert(format!("seg_{}", i), i);
        map.insert(format!("seg_{:03}", i), i);
    }
    map
}

fn apply_llm_result(
    inputs: &[SegmentInput],
    results: &[serde_json::Value],
    id_map: &HashMap<String, usize>,
    out: &mut [SceneKeywords],
    backend: &str,
) -> usize {
    let mut applied = 0usize;
    for r in results {
        let Some(id) = r.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(&idx) = id_map.get(id) else {
            continue;
        };
        let lang = inputs[idx].language_hint.as_deref().unwrap_or("english");
        let mut visual: Vec<String> = r
            .get("visual")
            .or_else(|| r.get("keywords"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|k| k.as_str().map(String::from))
                    .filter(|k| k.chars().count() >= MIN_KEYWORD_LEN)
                    .collect()
            })
            .unwrap_or_default();
        // Deterministic English-only gate for Hinglish sources: a bad LLM draw
        // that echoes raw Hinglish is never searchable — the <2-keyword hybrid
        // merge below then fills from the (translated) heuristic.
        if is_hinglish_lang(lang) {
            visual.retain(|k| is_searchable_english_keyword(k));
        }
        let reactions: Vec<String> = r
            .get("reactions")
            .or_else(|| r.get("sticker_keywords"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|k| k.as_str().map(String::from))
                    .filter(|k| k.chars().count() >= 2)
                    .filter(|k| !is_hinglish_lang(lang) || is_searchable_english_keyword(k))
                    .collect()
            })
            .unwrap_or_default();
        if visual.is_empty() && reactions.is_empty() {
            continue; // no usable draft — leave heuristic
        }
        let intent = r
            .get("intent")
            .and_then(|v| v.as_str())
            .map(String::from);
        let emphatic = r
            .get("emphatic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Quality gate: <2 visual keywords → hybrid-merge with the heuristic
        // so a weak/hallucinated draft never reaches search on its own.
        let (visual, source) = if visual.len() < 2 {
            let fallback = SceneKeywords::fallback(&inputs[idx]);
            let mut merged = visual.clone();
            for t in fallback.visual {
                if !merged.contains(&t) && merged.len() < 3 {
                    merged.push(t);
                }
            }
            (merged, KeywordSource::Hybrid)
        } else {
            (visual, KeywordSource::Llm)
        };
        let confidence = ((visual.len().min(3) as f64 / 3.0) * 0.55
            + (reactions.len().min(3) as f64 / 3.0) * 0.45)
            .clamp(0.0, 1.0);
        out[idx] = SceneKeywords {
            segment_id: inputs[idx].segment_id.clone(),
            visual,
            reactions,
            intent: Some(intent.unwrap_or_else(|| "emphasis".into())),
            emphatic,
            source,
            confidence,
            backend: backend.to_string(),
        };
        applied += 1;
    }
    applied
}

/// The unified draft entry point. One batched LLM call per ≤[`MAX_DRAFT_BATCH`]
/// segments emitting both `visual` and `reactions` per segment; missing ids are
/// re-drafted once; LLM-down or weak drafts fall back to the universal
/// salience heuristic (never positional garbage). Returns one [`SceneKeywords`]
/// per input, in input order.
pub async fn draft_scene_keywords(inputs: &[SegmentInput]) -> Vec<SceneKeywords> {
    let n = inputs.len();
    if n == 0 {
        return Vec::new();
    }
    let language = batch_language(inputs);
    // Hinglish/Hindi pre-translation: map known visual nouns to English so
    // BOTH the LLM prompt and the salience fallback receive readable English.
    // The raw ASR word-salad previously made the drafter hallucinate
    // ("cooking turkey oven" for a politics video). Unified for every caller
    // (broll.auto, broll.keywords, sticker.keywords, script.to_video, repair).
    let effective: Vec<SegmentInput> = if is_hinglish_lang(&language) {
        inputs
            .iter()
            .map(|i| {
                let mut c = i.clone();
                c.caption = translate_caption_if_needed(&i.caption, &language);
                c
            })
            .collect()
    } else {
        inputs.to_vec()
    };
    let inputs = &effective;
    let mut out: Vec<SceneKeywords> = inputs.iter().map(SceneKeywords::fallback).collect();
    let id_map = id_to_index(inputs);

    // First pass: batched draft over all inputs
    let mut attempted: Vec<bool> = vec![false; n];
    for chunk in inputs.chunks(MAX_DRAFT_BATCH) {
        let (system, user) = build_prompt(chunk, &language);
        match crate::llm::chat_complete(&system, &user, None).await {
            Ok(r) => {
                let parsed = extract_json_obj(&r.text);
                if let Some(results) = parsed.get("results").and_then(|v| v.as_array()) {
                    // Mark every chunk member as attempted (we made the call) —
                    // keyed by ABSOLUTE index, never the chunk-local position:
                    // chunks beyond the first would otherwise never be marked
                    // and would be skipped by the redraft pass below.
                    for c in chunk {
                        if let Some(&ai) = id_map.get(&c.segment_id) {
                            attempted[ai] = true;
                        }
                    }
                    // Find the absolute indices of chunk members
                    let abs_idxs: Vec<usize> = chunk
                        .iter()
                        .filter_map(|c| id_map.get(&c.segment_id).copied())
                        .collect();
                    let mut abs_out: Vec<SceneKeywords> = vec![SceneKeywords::fallback(&inputs[0]); abs_idxs.len()];
                    // Build a temporary out slice restricted to chunk members
                    for (k, &ai) in abs_idxs.iter().enumerate() {
                        abs_out[k] = out[ai].clone();
                    }
                    let mut sub_out: Vec<SceneKeywords> = abs_out;
                    // CRITICAL: resolve result ids against the chunk-LOCAL map,
                    // never the global id_map. apply_llm_result writes
                    // out[idx] where idx comes from the map, and sub_out is
                    // sized to THIS chunk (≤ MAX_DRAFT_BATCH members) — a
                    // global index (15+ for the second chunk) indexes out of
                    // bounds and panics. This was the >15-segment V2V bug:
                    // "index out of bounds: the len is 15 but the index is 15".
                    let chunk_map = id_to_index(chunk);
                    let applied = apply_llm_result(
                        chunk,
                        results,
                        &chunk_map,
                        &mut sub_out,
                        &format!("{}/{}", r.backend, r.model),
                    );
                    if applied > 0 {
                        for (k, &ai) in abs_idxs.iter().enumerate() {
                            out[ai] = sub_out[k].clone();
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[keywords] draft batch LLM failed: {} — using salience heuristic fallback",
                    e
                );
            }
        }
    }

    // Redraft pass: inputs that were attempted but got no LLM keywords
    // (id-echo mismatch). One extra call for exactly those — never silent swap.
    let missing: Vec<SegmentInput> = inputs
        .iter()
        .enumerate()
        .filter(|(i, _)| attempted[*i] && out[*i].source == KeywordSource::Heuristic)
        .map(|(_, i)| i.clone())
        .collect();
    if !missing.is_empty() {
        let (system, user) = build_prompt(&missing, &language);
        if let Ok(r) = crate::llm::chat_complete(&system, &user, None).await {
            let parsed = extract_json_obj(&r.text);
            if let Some(results) = parsed.get("results").and_then(|v| v.as_array()) {
                let miss_map = id_to_index(&missing);
                let mut sub_out: Vec<SceneKeywords> =
                    missing.iter().map(SceneKeywords::fallback).collect();
                apply_llm_result(
                    &missing,
                    results,
                    &miss_map,
                    &mut sub_out,
                    &format!("{}/{}", r.backend, r.model),
                );
                for (k, m) in missing.iter().enumerate() {
                    if let Some(&ai) = id_map.get(&m.segment_id) {
                        if sub_out[k].source != KeywordSource::Heuristic {
                            out[ai] = sub_out[k].clone();
                        }
                    }
                }
            }
        }
    }

    // Backend labeling: LLM/hybrid items carry "<backend>/<model>"; heuristic
    // items keep the "heuristic-v1" sentinel (handlers report it directly).
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_tokens_keep_non_latin_scripts() {
        let toks = unicode_tokens("सरकार ने भ्रष्टाचार किया");
        assert!(
            !toks.is_empty(),
            "Devanagari must tokenize (ASCII-only tokenizer dropped it entirely)"
        );
        assert!(toks.iter().any(|t| t.contains("सरकार")));
        let ar = unicode_tokens("الحكومة والفساد");
        assert!(!ar.is_empty(), "Arabic must tokenize");
    }

    #[test]
    fn unicode_tokens_keep_digits() {
        let toks = unicode_tokens("5 habits rule of 72");
        assert!(toks.iter().any(|t| t == "72"), "digits survive: {:?}", toks);
    }

    #[test]
    fn salient_keywords_not_first_three_words() {
        // Regression for G1: the old extract_broll_concept returned
        // "want tell stock" for this caption.
        let kws = extract_salient_keywords(
            "I want to tell you about the stock market today",
            3,
        );
        assert!(kws.iter().any(|k| k.contains("stock") || k.contains("market")), "got {:?}", kws);
        assert!(!kws.iter().any(|k| k == "want" || k == "tell"));
    }

    #[test]
    fn salient_keeps_proper_nouns_and_numbers() {
        let kws = extract_salient_keywords(
            "The Rule of 72 works because Warren Buffett compounds",
            5,
        );
        assert!(kws.iter().any(|k| k.contains("72")), "{:?}", kws);
        assert!(kws.iter().any(|k| k.contains("buffett") || k.contains("warren")), "{:?}", kws);
    }

    #[test]
    fn stopwords_consolidated_single_list() {
        for w in ["hai", "ka", "ki", "the", "and", "swap", "want"] {
            assert!(is_stopword(w), "{} must be stopword", w);
        }
    }

    #[test]
    fn auto_detect_language_variants() {
        assert_eq!(auto_detect_language("sarkar ne bhrashtachar kiya"), "hinglish");
        assert_eq!(auto_detect_language("सरकार और भ्रष्टाचार"), "hindi");
        assert_eq!(auto_detect_language("الحكومة والفساد"), "arabic");
        assert_eq!(auto_detect_language("The government and corruption"), "english");
    }

    #[test]
    fn auto_extract_video_keywords_from_title() {
        let kws = auto_extract_video_keywords("The Psychology of Influence and Persuasion");
        assert!(kws.iter().any(|k| k.contains("psychology")), "{:?}", kws);
        assert!(kws.iter().any(|k| k.contains("influence") || k.contains("persuasion")), "{:?}", kws);
        assert!(kws.len() <= 5);
    }

    #[test]
    fn detect_topic_no_lifestyle_collapse_for_psychology() {
        let kw = vec![
            "psychology".to_string(),
            "influence".to_string(),
            "persuasion".to_string(),
        ];
        assert_eq!(detect_topic_label(&kw), "psychology");
    }

    #[test]
    fn sanitize_query_rewrites_unsafe_and_enriches_theme() {
        assert_eq!(sanitize_query("blood pressure", "neutral"), "calm nature pressure");
        assert_eq!(sanitize_query("breathing exercise", "calm"), "breathing meditation exercise");
        assert_eq!(sanitize_query("focus routine", "calm"), "calm focus routine");
        // already-calm must not double-enrich
        assert_eq!(sanitize_query("calm focus routine", "calm"), "calm focus routine");
    }

    #[test]
    fn keywords_to_query_shapes_and_orients() {
        let kws = vec!["brain".to_string(), "focus".to_string(), "morning".to_string()];
        let q = keywords_to_query(&kws, 2, "9:16", "neutral");
        assert!(q.contains("brain") && q.contains("focus"));
        assert!(!q.contains("morning"), "max_terms caps the query");
        assert!(q.contains("vertical"));
    }

    #[test]
    fn heuristic_scene_query_derives_content_not_position() {
        let (q1, s1) = heuristic_scene_query(
            "The prefrontal cortex drives decision making under stress",
            &["psychology".into(), "brain".into()],
            "neutral",
            "9:16",
            0,
        );
        let (q2, s2) = heuristic_scene_query(
            "Stock market crashes follow panic selling waves",
            &["finance".into(), "market".into()],
            "neutral",
            "9:16",
            0,
        );
        assert!(s1.iter().any(|t| t.contains("cortex") || t.contains("decision")), "{:?}", s1);
        assert!(s2.iter().any(|t| t.contains("market") || t.contains("crash")), "{:?}", s2);
        // Content-derived: the two scenes must NOT share the same fallback query
        assert_ne!(q1, q2);
        assert!(!q1.contains("coffee"), "no lifestyle collapse: {}", q1);
    }

    #[test]
    fn heuristic_scene_query_handles_hinglish() {
        let (q, s) = heuristic_scene_query(
            "sarkar ne bhrashtachar aur mehngai badhaya",
            &[],
            "neutral",
            "9:16",
            0,
        );
        assert!(
            s.iter().any(|t| t.contains("government") || t.contains("corruption") || t.contains("rising") || t.contains("market")),
            "Hinglish must translate to English visuals: {:?}",
            s
        );
        assert!(!q.is_empty());
    }

    #[test]
    fn heuristic_scene_query_empty_scene_neutral_fallback() {
        let (q, s) = heuristic_scene_query("", &[], "neutral", "9:16", 3);
        assert!(!q.is_empty());
        assert!(!s.is_empty());
    }

    #[test]
    fn test_apply_llm_result_chunk_local_map_prevents_oob_panic() {
        // 16 inputs → two chunks of 15+1. The second chunk's only member has
        // GLOBAL index 15. apply_llm_result must resolve ids against the
        // chunk-LOCAL map: writing out[15] on a 1-element slice is the OOB
        // panic the >15-segment V2V run hit ("len is 15, index is 15").
        let inputs: Vec<SegmentInput> = (0..16)
            .map(|i| SegmentInput {
                segment_id: format!("seg_{:03}", i),
                caption: format!("caption {}", i),
                language_hint: None,
                duration_s: 3.0,
                scene_idx: i,
                total_scenes: 16,
                video_title: "test".into(),
                video_keywords: Vec::new(),
                covered_concepts: Vec::new(),
            })
            .collect();
        let chunk = &inputs[15..]; // the second chunk (1 member, global idx 15)
        let local_map = id_to_index(chunk);
        assert_eq!(local_map.get("seg_015"), Some(&0));
        let results_val =
            serde_json::json!([{"id": "seg_015", "visual": ["city", "night"], "reactions": []}]);
        let results = results_val.as_array().unwrap();
        let mut out = vec![SceneKeywords::fallback(&inputs[0]); chunk.len()];
        let applied = apply_llm_result(chunk, results, &local_map, &mut out, "test/test");
        assert_eq!(applied, 1);
        assert_eq!(out[0].segment_id, "seg_015");
        assert_eq!(
            out[0].visual,
            vec!["city".to_string(), "night".to_string()]
        );
        // The old global-map call would have written out[15] on this 1-len slice.
    }

    #[test]
    fn is_hinglish_lang_detects_hindi_variants() {
        assert!(is_hinglish_lang("hinglish"));
        assert!(is_hinglish_lang("hi"));
        assert!(is_hinglish_lang("hindi"));
        assert!(!is_hinglish_lang("english"));
        assert!(!is_hinglish_lang("es"));
    }

    #[test]
    fn translate_caption_if_needed_maps_hinglish_nouns() {
        // Known visual noun in the map must be translated; unknown words pass.
        let out = translate_caption_if_needed("sarkar corruption bhai log", "hinglish");
        assert!(
            !out.to_lowercase().contains("sarkar"),
            "'sarkar' should map to government: {}",
            out
        );
        assert!(
            out.to_lowercase().contains("corruption"),
            "known English nouns survive: {}",
            out
        );
        // English input is a no-op.
        assert_eq!(
            translate_caption_if_needed("the quick brown fox", "english"),
            "the quick brown fox"
        );
    }

    #[test]
    fn derive_video_context_heuristic_never_empty_and_anchors_title() {
        let kws = derive_video_context_heuristic("sarkar corruption media aavaaz", "india-politics");
        assert!(!kws.is_empty());
        // Title tokens should anchor the set.
        let joined = kws.join(" ").to_lowercase();
        assert!(
            joined.contains("politic") || joined.contains("india"),
            "title tokens must anchor topics: {}",
            joined
        );
        let empty = derive_video_context_heuristic("", "");
        assert_eq!(empty, vec!["video".to_string()]);
    }

    #[test]
    fn blend_sticker_keywords_unifies_reactions_and_visual() {
        // Reactions first (GIPHY-friendly), visual nouns after — one shared set.
        let blended = blend_sticker_keywords(
            &["corruption".into(), "handcuffs".into(), "court".into()],
            &["shocked".into(), "facepalm".into()],
        );
        assert_eq!(blended, vec!["shocked", "facepalm", "corruption", "handcuffs"]);
        // Dedup + cap at 4; 1-char terms filtered.
        let big = blend_sticker_keywords(
            &["aaa".into(), "bbb".into(), "ccc".into(), "ddd".into(), "eee".into()],
            &["bbb".into()],
        );
        assert_eq!(big, vec!["bbb", "aaa", "ccc", "ddd"]);
        assert!(!big.contains(&"eee".to_string()), "capped at 4");
        // Never empty.
        assert_eq!(blend_sticker_keywords(&[], &[]), vec!["funny".to_string()]);
    }

    #[test]
    fn searchable_english_keyword_rejects_hinglish_residue_and_devanagari() {
        // Devanagari — never searchable.
        assert!(!is_searchable_english_keyword("सरकार"));
        // Hinglish residue the LLM echoes — never searchable.
        for bad in [
            "chaaloo", "hisaab", "farq", "thodi", "saahab", "inhonne", "rakha",
            "mere", "kisi", "kaanoon", "uchh", "ash", "zindagi", "garibi",
        ] {
            assert!(
                !is_searchable_english_keyword(bad),
                "'{}' must be filtered as Hinglish residue",
                bad
            );
        }
        // English keywords pass.
        for good in [
            "government", "building", "protest", "corruption", "crowd", "new york", "yogi modi",
            "time gas", "murder criminal",
        ] {
            assert!(is_searchable_english_keyword(good), "'{}' must pass", good);
        }
        // Unbounded ASR garbage must be rejected by the whitelist.
        for bad in ["phishiega", "galgoate", "bajaaya", "enge", "uchh"] {
            assert!(!is_searchable_english_keyword(bad), "'{}' must be rejected", bad);
        }
        // Case-insensitive.
        assert!(!is_searchable_english_keyword("Hisaab"));
    }
}
