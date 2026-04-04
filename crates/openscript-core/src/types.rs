use serde::{Deserialize, Serialize};

/// Editorial roles for narrative-driven SFX/music placement.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EditorialRole {
    Hook,
    Setup,
    Proof,
    Contrast,
    Payoff,
    Cta,
    Intro,
    Transition,
    Highlight,
    Outro,
}

/// All 6 track types in the timeline.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TrackType {
    Dialogue,
    Voiceover,
    Captions,
    Broll,
    Music,
    Sfx,
}

impl std::fmt::Display for TrackType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrackType::Dialogue => write!(f, "dialogue"),
            TrackType::Voiceover => write!(f, "voiceover"),
            TrackType::Captions => write!(f, "captions"),
            TrackType::Broll => write!(f, "broll"),
            TrackType::Music => write!(f, "music"),
            TrackType::Sfx => write!(f, "sfx"),
        }
    }
}

impl std::str::FromStr for TrackType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dialogue" => Ok(TrackType::Dialogue),
            "voiceover" => Ok(TrackType::Voiceover),
            "captions" => Ok(TrackType::Captions),
            "broll" => Ok(TrackType::Broll),
            "music" => Ok(TrackType::Music),
            "sfx" => Ok(TrackType::Sfx),
            _ => Err(format!("Unknown track type: {}", s)),
        }
    }
}

impl std::fmt::Display for EditorialRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorialRole::Hook => write!(f, "hook"),
            EditorialRole::Setup => write!(f, "setup"),
            EditorialRole::Proof => write!(f, "proof"),
            EditorialRole::Contrast => write!(f, "contrast"),
            EditorialRole::Payoff => write!(f, "payoff"),
            EditorialRole::Cta => write!(f, "cta"),
            EditorialRole::Intro => write!(f, "intro"),
            EditorialRole::Transition => write!(f, "transition"),
            EditorialRole::Highlight => write!(f, "highlight"),
            EditorialRole::Outro => write!(f, "outro"),
        }
    }
}
