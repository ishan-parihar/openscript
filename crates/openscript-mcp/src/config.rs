//! User configuration for OpenScript.
//!
//! ## Load order (highest priority first)
//!
//! 1. **Environment variables** (`PEXELS_API_KEY`, `OPENROUTER_API_KEY`,
//!    `OPENCODE_API`, …) — for CI / one-off overrides
//! 2. **`~/.openscript/config.json`** — primary user config (this module)
//! 3. **`mcp/assets/.openscript_config.json`** — legacy / bundled repo config
//! 4. **Built-in defaults**
//!
//! Secrets belong only in `~/.openscript/config.json` (mode 0600) or env vars.
//! Never commit API keys.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// In-memory cache so we don't re-read disk on every tool call.
static CONFIG_CACHE: RwLock<Option<OpenScriptConfig>> = RwLock::new(None);

/// Serializes tests that mutate the process-global `OPENSCRIPT_TTS_*` env
/// vars (config.rs tests + tools_script.rs apply_tts_config_defaults tests
/// run in the same binary and would otherwise race on those vars).
/// Only used under `#[cfg(test)]` — see the test modules.
#[cfg(test)]
pub(crate) static TTS_ENV_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Canonical user config directory: `$HOME/.openscript`
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("OPENSCRIPT_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    home_dir().join(".openscript")
}

/// Canonical config file path: `$HOME/.openscript/config.json`
pub fn config_file_path() -> PathBuf {
    config_dir().join("config.json")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn legacy_repo_config_path() -> PathBuf {
    let p = Path::new("mcp/assets/.openscript_config.json");
    if p.exists() {
        return p.to_path_buf();
    }
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let c = PathBuf::from(root).join("mcp/assets/.openscript_config.json");
        if c.exists() {
            return c;
        }
    }
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let c = Path::new(d).join("../../mcp/assets/.openscript_config.json");
        if c.exists() {
            return c;
        }
    }
    p.to_path_buf()
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenScriptConfig {
    #[serde(default = "default_version")]
    pub version: u32,

    #[serde(default)]
    pub api_keys: ApiKeys,

    #[serde(default)]
    pub llm: LlmConfig,

    #[serde(default)]
    pub paths: PathsConfig,

    #[serde(default)]
    pub render: RenderConfig,

    /// TTS engine defaults for script→video voice generation.
    /// `default_backend` picks the engine family (kokoro | audio8 |
    /// voicedesign | higgs | sidecar); `default_voice` optionally pins a
    /// registered voice profile that wins when a script's speaker uses the
    /// bare voice id "default".
    #[serde(default)]
    pub tts: TtsConfig,

        /// Feature toggles — the active configuration drives what a cold-start
    /// install provisions and what the runtime lets through. Every toggle
    /// defaults to ON (backward compatible); turn off to skip downloads in
    /// setup.sh and to gate tools at runtime. See `FeaturesConfig`.
    #[serde(default)]
    pub features: FeaturesConfig,

    /// Flat legacy keys from `mcp/assets/.openscript_config.json`
    /// (e.g. `"pexels_api_key": "..."`). Merged into `api_keys` on load.
    #[serde(flatten)]
    pub legacy: std::collections::HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Feature toggles
//
// Every toggle defaults to **true** (backward compatible). A feature is
// considered enabled when: env `OPENSCRIPT_FEATURE_<CATEGORY>_<NAME>` is not
// an explicit "off" value → config `features.<category>.<name>` → default true.
//
// setup.sh reads the SAME toggles so a cold-start install only downloads the
// deps for what is active; the runtime gates on them so only active features
// work (clear errors naming the toggle + setup command otherwise).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsFeatures {
    /// Kokoro ONNX presets (default TTS, ~340MB model + deps)
    #[serde(default = "default_true")]
    pub kokoro: bool,
    /// Audio8 zero-shot clone (ONNX INT4, ~1GB model + deps)
    #[serde(default = "default_true")]
    pub audio8: bool,
    /// Qwen3 VoiceDesign character voices (int4 model ~4.3GB + .venv-voicedesign)
    #[serde(default = "default_true")]
    pub voicedesign: bool,
    /// Higgs Audio v3 expressive TTS (4B ONNX GenAI int4, ~3.6GB model + .venv-higgs)
    #[serde(default = "default_true")]
    pub higgs: bool,
    /// Remote voicebox sidecar at OPENSCRIPT_TTS_URL
    #[serde(default = "default_true")]
    pub sidecar: bool,
}

impl Default for TtsFeatures {
    fn default() -> Self {
        Self {
            kokoro: true,
            audio8: true,
            voicedesign: true,
            higgs: true,
            sidecar: true,
        }
    }
}

impl TtsFeatures {
    fn get(&self, name: &str) -> bool {
        match name {
            "kokoro" => self.kokoro,
            "audio8" => self.audio8,
            "voicedesign" => self.voicedesign,
            "higgs" => self.higgs,
            "sidecar" => self.sidecar,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionFeatures {
    /// HinglishGgml transcription (whisper.cpp + GGML model)
    #[serde(default = "default_true")]
    pub hinglish_ggml: bool,
    /// OpenAI-whisper word alignment (Hinglish/Hindi captions)
    #[serde(default = "default_true")]
    pub whisper_align: bool,
    /// Parakeet TDT force-alignment (English captions, ~320MB model)
    #[serde(default = "default_true")]
    pub parakeet_align: bool,
}

impl Default for TranscriptionFeatures {
    fn default() -> Self {
        Self {
            hinglish_ggml: true,
            whisper_align: true,
            parakeet_align: true,
        }
    }
}

impl TranscriptionFeatures {
    fn get(&self, name: &str) -> bool {
        match name {
            "hinglish_ggml" => self.hinglish_ggml,
            "whisper_align" => self.whisper_align,
            "parakeet_align" => self.parakeet_align,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFeatures {
    /// Pexels stock footage (primary multi-broll)
    #[serde(default = "default_true")]
    pub pexels: bool,
    /// GIPHY stickers / reaction GIFs
    #[serde(default = "default_true")]
    pub giphy: bool,
    /// Pixabay stock (music/video alt provider)
    #[serde(default = "default_true")]
    pub pixabay: bool,
    /// YouTube footage search + download (yt-dlp)
    #[serde(default = "default_true")]
    pub youtube: bool,
}

impl Default for MediaFeatures {
    fn default() -> Self {
        Self {
            pexels: true,
            giphy: true,
            pixabay: true,
            youtube: true,
        }
    }
}

impl MediaFeatures {
    fn get(&self, name: &str) -> bool {
        match name {
            "pexels" => self.pexels,
            "giphy" => self.giphy,
            "pixabay" => self.pixabay,
            "youtube" => self.youtube,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFeatureFlags {
    /// OpenCode zen (primary text/vision backend)
    #[serde(default = "default_true")]
    pub opencode: bool,
    /// OpenRouter free models (fallback text/vision)
    #[serde(default = "default_true")]
    pub openrouter: bool,
}

impl Default for LlmFeatureFlags {
    fn default() -> Self {
        Self {
            opencode: true,
            openrouter: true,
        }
    }
}

impl LlmFeatureFlags {
    fn get(&self, name: &str) -> bool {
        match name {
            "opencode" => self.opencode,
            "openrouter" => self.openrouter,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderFeatures {
    /// FFmpeg multilayer/filter-graph render path (default engine)
    #[serde(default = "default_true")]
    pub ffmpeg: bool,
    /// HyperFrames HTML+GSAP render engine
    #[serde(default = "default_true")]
    pub hyperframes: bool,
    /// Remotion escape-hatch render engine
    #[serde(default = "default_true")]
    pub remotion: bool,
    /// NVENC/NVDEC hardware acceleration for ffmpeg (auto-probed; CPU fallback)
    #[serde(default = "default_true")]
    pub nvenc: bool,
}

impl Default for RenderFeatures {
    fn default() -> Self {
        Self {
            ffmpeg: true,
            hyperframes: true,
            remotion: true,
            nvenc: true,
        }
    }
}

impl RenderFeatures {
    fn get(&self, name: &str) -> bool {
        match name {
            "ffmpeg" => self.ffmpeg,
            "hyperframes" => self.hyperframes,
            "remotion" => self.remotion,
            "nvenc" => self.nvenc,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesConfig {
    #[serde(default)]
    pub tts: TtsFeatures,
    #[serde(default)]
    pub transcription: TranscriptionFeatures,
    #[serde(default)]
    pub media: MediaFeatures,
    #[serde(default)]
    pub llm: LlmFeatureFlags,
    #[serde(default)]
    pub render: RenderFeatures,
    /// Tauri/React desktop frontend (npm deps + tsc checks)
    #[serde(default = "default_true")]
    pub frontend: bool,
}

/// All features are ON unless explicitly turned off (backward compatible).
impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            tts: TtsFeatures {
                kokoro: true,
                audio8: true,
                voicedesign: true,
                higgs: true,
                sidecar: true,
            },
            transcription: TranscriptionFeatures {
                hinglish_ggml: true,
                whisper_align: true,
                parakeet_align: true,
            },
            media: MediaFeatures {
                pexels: true,
                giphy: true,
                pixabay: true,
                youtube: true,
            },
            llm: LlmFeatureFlags {
                opencode: true,
                openrouter: true,
            },
            render: RenderFeatures {
                ffmpeg: true,
                hyperframes: true,
                remotion: true,
                nvenc: true,
            },
            frontend: true,
        }
    }
}

fn default_true() -> bool {
    true
}

impl FeaturesConfig {
    fn get(&self, category: &str, name: &str) -> bool {
        match category {
            "tts" => self.tts.get(name),
            "transcription" => self.transcription.get(name),
            "media" => self.media.get(name),
            "llm" => self.llm.get(name),
            "render" => self.render.get(name),
            "frontend" => name == "frontend" && self.frontend,
            _ => true,
        }
    }
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiKeys {
    #[serde(default, alias = "pexels_api_key")]
    pub pexels: String,
    #[serde(default, alias = "giphy_api_key")]
    pub giphy: String,
    #[serde(default, alias = "pixabay_api_key")]
    pub pixabay: String,
    #[serde(default, alias = "openrouter_api_key")]
    pub openrouter: String,
    #[serde(default, alias = "opencode_api_key")]
    pub opencode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_openrouter_base_url")]
    pub openrouter_base_url: String,
    /// Ordered OpenRouter model cascade for text + vision fallbacks
    #[serde(default = "default_openrouter_models")]
    pub openrouter_models: Vec<String>,
    /// OpenCode API base URL (opencode.ai compatible)
    #[serde(default = "default_opencode_base_url")]
    pub opencode_base_url: String,
    /// OpenCode model name
    #[serde(default = "default_opencode_model")]
    pub opencode_model: String,
}

fn default_openrouter_base_url() -> String {
    "https://openrouter.ai/api/v1".into()
}
fn default_openrouter_models() -> Vec<String> {
    vec![
        "google/gemma-4-31b-it:free".into(),
        "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free".into(),
    ]
}
fn default_opencode_base_url() -> String {
    "https://opencode.ai/zen/v1".into()
}
fn default_opencode_model() -> String {
    "mimo-v2.5-free".into()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            openrouter_base_url: default_openrouter_base_url(),
            openrouter_models: default_openrouter_models(),
            opencode_base_url: default_opencode_base_url(),
            opencode_model: default_opencode_model(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathsConfig {
    #[serde(default)]
    pub sfx_path: Option<String>,
    #[serde(default)]
    pub music_path: Option<String>,
    #[serde(default)]
    pub sfx_index: Option<String>,
    #[serde(default)]
    pub music_index: Option<String>,
    #[serde(default)]
    pub tts_url: Option<String>,
    #[serde(default)]
    pub tts_cache: Option<String>,
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub assets_base: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Default TTS backend for script.to_video when the script omits
    /// `tts.backend`. One of: kokoro (default), audio8, voicedesign,
    /// higgs, sidecar.
    #[serde(default = "default_tts_backend")]
    pub default_backend: String,
    /// Default voice profile id (e.g. "ishan"). When set, a speaker
    /// whose voice is the literal string "default" resolves to this profile.
    #[serde(default)]
    pub default_voice: Option<String>,
}

fn default_tts_backend() -> String {
    "kokoro".to_string()
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            // Manual impl: the derive would produce default_backend = ""
            // (String::default()), but the serde default_tts_backend() only
            // fires on deserialization. Callers that construct TtsConfig via
            // Default (e.g. OpenScriptConfig::default when a config file
            // omits the tts section) must still get "kokoro".
            default_backend: default_tts_backend(),
            default_voice: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderConfig {
    #[serde(default = "default_aspect")]
    pub default_aspect: String,
    #[serde(default = "default_lufs")]
    pub normalize_lufs: f64,
}

fn default_aspect() -> String {
    "9:16".into()
}
fn default_lufs() -> f64 {
    -16.0
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            default_aspect: default_aspect(),
            normalize_lufs: default_lufs(),
        }
    }
}

// ---------------------------------------------------------------------------
// Expand paths / merge
// ---------------------------------------------------------------------------

impl OpenScriptConfig {
    /// Apply flat legacy keys into nested `api_keys` if nested fields empty.
    fn absorb_legacy(&mut self) {
        let take = |map: &std::collections::HashMap<String, Value>, key: &str| -> Option<String> {
            map.get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        };
        if self.api_keys.pexels.is_empty() {
            if let Some(v) = take(&self.legacy, "pexels_api_key") {
                self.api_keys.pexels = v;
            }
        }
        if self.api_keys.giphy.is_empty() {
            if let Some(v) = take(&self.legacy, "giphy_api_key") {
                self.api_keys.giphy = v;
            }
        }
        if self.api_keys.pixabay.is_empty() {
            if let Some(v) = take(&self.legacy, "pixabay_api_key") {
                self.api_keys.pixabay = v;
            }
        }
        if self.api_keys.openrouter.is_empty() {
            if let Some(v) = take(&self.legacy, "openrouter_api_key")
                .or_else(|| take(&self.legacy, "openrouter_key"))
            {
                self.api_keys.openrouter = v;
            }
        }
        if self.api_keys.opencode.is_empty() {
            if let Some(v) = take(&self.legacy, "opencode_api_key") {
                self.api_keys.opencode = v;
            }
        }
    }

    /// Merge `other` under self (self wins for non-empty fields).
    fn merge_under(&mut self, other: &OpenScriptConfig) {
        if self.api_keys.pexels.is_empty() {
            self.api_keys.pexels = other.api_keys.pexels.clone();
        }
        if self.api_keys.giphy.is_empty() {
            self.api_keys.giphy = other.api_keys.giphy.clone();
        }
        if self.api_keys.pixabay.is_empty() {
            self.api_keys.pixabay = other.api_keys.pixabay.clone();
        }
        if self.api_keys.openrouter.is_empty() {
            self.api_keys.openrouter = other.api_keys.openrouter.clone();
        }
        if self.api_keys.opencode.is_empty() {
            self.api_keys.opencode = other.api_keys.opencode.clone();
        }
        if self.paths.sfx_path.is_none() {
            self.paths.sfx_path = other.paths.sfx_path.clone();
        }
        if self.paths.music_path.is_none() {
            self.paths.music_path = other.paths.music_path.clone();
        }
        if self.paths.tts_url.is_none() {
            self.paths.tts_url = other.paths.tts_url.clone();
        }
    }
}

fn parse_config_file(path: &Path) -> Option<OpenScriptConfig> {
    let content = fs::read_to_string(path).ok()?;
    let mut cfg: OpenScriptConfig = serde_json::from_str(&content).ok()?;
    cfg.absorb_legacy();
    Some(cfg)
}

/// Load config from disk (user → legacy), apply defaults. Does not apply env overrides.
pub fn load_config_from_disk() -> OpenScriptConfig {
    let mut cfg = OpenScriptConfig::default();
    // User config first
    if let Some(user) = parse_config_file(&config_file_path()) {
        cfg = user;
    }
    // Merge legacy under (fills empty api keys etc.)
    if let Some(legacy) = parse_config_file(&legacy_repo_config_path()) {
        cfg.merge_under(&legacy);
    }
    cfg.absorb_legacy();
    cfg
}

/// Invalidate cache (after write / tests).
pub fn reload_config() {
    let cfg = load_config_from_disk();
    if let Ok(mut guard) = CONFIG_CACHE.write() {
        *guard = Some(cfg);
    }
}

/// Cached config load.
pub fn config() -> OpenScriptConfig {
    {
        if let Ok(guard) = CONFIG_CACHE.read() {
            if let Some(ref c) = *guard {
                return c.clone();
            }
        }
    }
    let cfg = load_config_from_disk();
    if let Ok(mut guard) = CONFIG_CACHE.write() {
        *guard = Some(cfg.clone());
    }
    cfg
}

/// Ensure `~/.openscript/config.json` exists. Returns path + whether created.
pub fn ensure_user_config(seed: Option<&OpenScriptConfig>) -> Result<(PathBuf, bool), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {}", dir.display(), e))?;
    let path = config_file_path();
    if path.exists() {
        return Ok((path, false));
    }
    let mut cfg = seed.cloned().unwrap_or_default();
    cfg.version = 1;
    write_user_config(&cfg)?;
    Ok((path, true))
}

/// Write user config (mode 0600). Reloads cache.
pub fn write_user_config(cfg: &OpenScriptConfig) -> Result<PathBuf, String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {}", dir.display(), e))?;
    let path = config_file_path();
    // Serialize without dumping empty legacy flatten noise
    let out = json!({
        "version": cfg.version,
        "api_keys": {
            "pexels": cfg.api_keys.pexels,
            "giphy": cfg.api_keys.giphy,
            "pixabay": cfg.api_keys.pixabay,
            "openrouter": cfg.api_keys.openrouter,
            "opencode": cfg.api_keys.opencode,
        },
        "llm": {
            "openrouter_base_url": cfg.llm.openrouter_base_url,
            "openrouter_models": cfg.llm.openrouter_models,
            "opencode_base_url": cfg.llm.opencode_base_url,
            "opencode_model": cfg.llm.opencode_model,
        },
        "paths": cfg.paths,
        "render": cfg.render,
        "tts": {
            "default_backend": cfg.tts.default_backend,
            "default_voice": cfg.tts.default_voice,
        },
        "features": cfg.features,
    });
    let text = serde_json::to_string_pretty(&out)
        .map_err(|e| format!("serialize config: {}", e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text.as_bytes()).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, &path).map_err(|e| format!("rename config: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    reload_config();
    Ok(path)
}

// ---------------------------------------------------------------------------
// Resolved accessors (env → config → default)
// ---------------------------------------------------------------------------

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// Resolve an API key: env → user config → legacy flat key names.
pub fn resolve_api_key(kind: &str) -> String {
    let cfg = config();
    match kind {
        "pexels" => env_nonempty("PEXELS_API_KEY")
            .unwrap_or_else(|| cfg.api_keys.pexels.clone()),
        "giphy" => env_nonempty("GIPHY_API_KEY").unwrap_or_else(|| cfg.api_keys.giphy.clone()),
        "pixabay" => env_nonempty("PIXABAY_API_KEY")
            .unwrap_or_else(|| cfg.api_keys.pixabay.clone()),
        "openrouter" => env_nonempty("OPENROUTER_API_KEY")
            .or_else(|| env_nonempty("OPENROUTER_KEY"))
            .unwrap_or_else(|| cfg.api_keys.openrouter.clone()),
        "opencode" => env_nonempty("OPENCODE_API")
            .or_else(|| env_nonempty("OPENCODE_API_KEY"))
            .unwrap_or_else(|| cfg.api_keys.opencode.clone()),
        _ => String::new(),
    }
}

pub fn resolve_openrouter_base_url() -> String {
    env_nonempty("OPENROUTER_BASE_URL")
        .unwrap_or_else(|| config().llm.openrouter_base_url.clone())
}

pub fn resolve_openrouter_models() -> Vec<String> {
    let cfg = config();
    let primary = env_nonempty("OPENSCRIPT_OPENROUTER_VISION_MODEL");
    let fallback = env_nonempty("OPENSCRIPT_OPENROUTER_VISION_FALLBACK");
    if primary.is_some() || fallback.is_some() {
        let mut v = Vec::new();
        if let Some(p) = primary {
            v.push(p);
        } else if let Some(first) = cfg.llm.openrouter_models.first() {
            v.push(first.clone());
        }
        if let Some(f) = fallback {
            v.push(f);
        } else if let Some(second) = cfg.llm.openrouter_models.get(1) {
            v.push(second.clone());
        }
        return v;
    }
    if cfg.llm.openrouter_models.is_empty() {
        default_openrouter_models()
    } else {
        cfg.llm.openrouter_models.clone()
    }
}

pub fn resolve_opencode_api_key() -> String {
    env_nonempty("OPENCODE_API")
        .or_else(|| env_nonempty("OPENCODE_API_KEY"))
        .unwrap_or_else(|| config().api_keys.opencode.clone())
}

pub fn resolve_opencode_base_url() -> String {
    env_nonempty("OPENCODE_BASE_URL")
        .unwrap_or_else(|| config().llm.opencode_base_url.clone())
}

pub fn resolve_opencode_model() -> String {
    env_nonempty("OPENCODE_MODEL")
        .unwrap_or_else(|| config().llm.opencode_model.clone())
}

/// Resolve the default TTS backend: env `OPENSCRIPT_TTS_BACKEND` → config
/// `tts.default_backend` → "kokoro".
pub fn resolve_tts_default_backend() -> String {
    env_nonempty("OPENSCRIPT_TTS_BACKEND")
        .unwrap_or_else(|| config().tts.default_backend.clone())
}

/// Resolve the default TTS voice profile: env `OPENSCRIPT_TTS_VOICE` → config
/// `tts.default_voice`. Returns None when neither is configured — callers then
/// fall back to the backend's built-in voice (e.g. kokoro:af_heart).
pub fn resolve_tts_default_voice() -> Option<String> {
    env_nonempty("OPENSCRIPT_TTS_VOICE").or_else(|| config().tts.default_voice.clone())
}

/// Resolve a feature toggle: env `OPENSCRIPT_FEATURE_<CATEGORY>_<NAME>` (an
/// explicit "0/false/no/off" disables, "1/true/yes/on" enables) → config
/// `features.<category>.<name>` → default true. Unknown toggles are ON.
pub fn feature_enabled(category: &str, name: &str) -> bool {
    // The frontend toggle is a bare feature (no sub-flags), so its env var is
    // OPENSCRIPT_FEATURE_FRONTEND rather than OPENSCRIPT_FEATURE_FRONTEND_FRONTEND.
    let env_name = if category == "frontend" && name == "frontend" {
        "OPENSCRIPT_FEATURE_FRONTEND".to_string()
    } else {
        format!(
            "OPENSCRIPT_FEATURE_{}_{}",
            category.to_uppercase(),
            name.to_uppercase()
        )
    };
    if let Ok(v) = std::env::var(&env_name) {
        let v = v.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
        if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    config().features.get(category, name)
}

/// Typed sugar: TTS engine toggle (kokoro | audio8 | voicedesign | higgs | sidecar).
pub fn feature_tts(engine: &str) -> bool {
    feature_enabled("tts", engine)
}

/// Typed sugar: transcription engine toggle
/// (hinglish_ggml | whisper_align | parakeet_align).
pub fn feature_transcription(engine: &str) -> bool {
    feature_enabled("transcription", engine)
}

/// Typed sugar: media provider toggle (pexels | giphy | pixabay | youtube).
pub fn feature_media(provider: &str) -> bool {
    feature_enabled("media", provider)
}

/// Typed sugar: LLM backend toggle (opencode | openrouter).
pub fn feature_llm(backend: &str) -> bool {
    feature_enabled("llm", backend)
}

/// Typed sugar: render engine toggle (ffmpeg | hyperframes | remotion | nvenc).
pub fn feature_render(engine: &str) -> bool {
    feature_enabled("render", engine)
}

/// Typed sugar: desktop frontend toggle.
pub fn feature_frontend() -> bool {
    feature_enabled("frontend", "frontend")
}

/// Full toggle table for system.capabilities / system.doctor: every feature
/// with its current state, the env override name, and a setup hint for the
/// heavy optional deps (so agents know what a cold-start install will pull).
fn feature_entry(category: &str, name: &str, setup: &str) -> Value {
    let enabled = feature_enabled(category, name);
    let env = if category == "frontend" && name == "frontend" {
        "OPENSCRIPT_FEATURE_FRONTEND".to_string()
    } else {
        format!(
            "OPENSCRIPT_FEATURE_{}_{}",
            category.to_uppercase(),
            name.to_uppercase()
        )
    };
    json!({
        "enabled": enabled,
        "env": env,
        "config_path": format!("features.{}.{}", category, name),
        "setup": setup,
    })
}

pub fn feature_flags_view() -> Value {
    json!({
        "tts": {
            "kokoro": feature_entry("tts", "kokoro", "bash setup.sh (downloads model + installs kokoro-onnx)"),
            "audio8": feature_entry("tts", "audio8", "bash scripts/setup_audio8.sh (downloads int4 model ~1GB + .venv-audio8)"),
            "voicedesign": feature_entry("tts", "voicedesign", "bash scripts/setup_voicedesign.sh (int4 model ~4.3GB + .venv-voicedesign)"),
            "higgs": feature_entry("tts", "higgs", "bash scripts/setup_higgs.sh (downloads cuda_int4 model ~3.6GB + .venv-higgs)"),
            "sidecar": feature_entry("tts", "sidecar", "Run the voicebox sidecar at OPENSCRIPT_TTS_URL"),
        },
        "transcription": {
            "hinglish_ggml": feature_entry("transcription", "hinglish_ggml", "bash setup.sh (builds whisper.cpp + downloads GGML model)"),
            "whisper_align": feature_entry("transcription", "whisper_align", "pip install openai-whisper"),
            "parakeet_align": feature_entry("transcription", "parakeet_align", "bash setup.sh (downloads Parakeet ONNX ~320MB + onnxruntime/librosa)"),
        },
        "media": {
            "pexels": feature_entry("media", "pexels", "Set api_keys.pexels (bash scripts/setup_openscript_config.sh --pexels-key KEY)"),
            "giphy": feature_entry("media", "giphy", "Set api_keys.giphy"),
            "pixabay": feature_entry("media", "pixabay", "Set api_keys.pixabay"),
            "youtube": feature_entry("media", "youtube", "Install yt-dlp"),
        },
        "llm": {
            "opencode": feature_entry("llm", "opencode", "Set api_keys.opencode"),
            "openrouter": feature_entry("llm", "openrouter", "Set api_keys.openrouter"),
        },
        "render": {
            "ffmpeg": feature_entry("render", "ffmpeg", "Install ffmpeg"),
            "hyperframes": feature_entry("render", "hyperframes", "Repo ships hyperframes/ (no download)"),
            "remotion": feature_entry("render", "remotion", "npm install in remotion/ (escape-hatch only)"),
            "nvenc": feature_entry("render", "nvenc", "NVIDIA driver + NVENC-capable ffmpeg; auto-degrades to CPU"),
        },
        "frontend": {
            "enabled": feature_enabled("frontend", "frontend"),
            "env": "OPENSCRIPT_FEATURE_FRONTEND",
            "config_path": "features.frontend",
            "setup": "npm install in crates/openscript-tauri/src/frontend",
        },
    })
}

/// Redacted public view for system.capabilities / system.config.get
pub fn config_public_view() -> Value {    let cfg = config();
    let redact = |s: &str| -> Value {
        if s.is_empty() {
            json!(null)
        } else if s.len() <= 8 {
            json!("***")
        } else {
            json!(format!("{}…{}", &s[..6], &s[s.len().saturating_sub(4)..]))
        }
    };
    json!({
        "config_dir": config_dir().display().to_string(),
        "config_file": config_file_path().display().to_string(),
        "config_exists": config_file_path().exists(),
        "version": cfg.version,
        "api_keys": {
            "pexels_set": !resolve_api_key("pexels").is_empty(),
            "giphy_set": !resolve_api_key("giphy").is_empty(),
            "pixabay_set": !resolve_api_key("pixabay").is_empty(),
            "openrouter_set": !resolve_api_key("openrouter").is_empty(),
            "openrouter_preview": redact(&resolve_api_key("openrouter")),
        },
        "llm": {
            "openrouter_base_url": resolve_openrouter_base_url(),
            "openrouter_models": resolve_openrouter_models(),
            "opencode_base_url": resolve_opencode_base_url(),
            "opencode_model": resolve_opencode_model(),
        },
        "paths": cfg.paths,
        "render": cfg.render,
        "tts": {
            "default_backend": resolve_tts_default_backend(),
            "default_voice": resolve_tts_default_voice(),
        },
        "features": feature_flags_view(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_models_cascade() {
        let m = default_openrouter_models();
        assert!(m[0].contains("gemma"));
        assert!(m[1].contains("nemotron"));
    }

    #[test]
    fn absorb_legacy_keys() {
        let mut cfg = OpenScriptConfig::default();
        cfg.legacy.insert(
            "openrouter_api_key".into(),
            Value::String("sk-test".into()),
        );
        cfg.absorb_legacy();
        assert_eq!(cfg.api_keys.openrouter, "sk-test");
    }

    #[test]
    fn public_view_redacts() {
        // Just ensure it serializes without panic
        let v = config_public_view();
        assert!(v.get("config_file").is_some());
    }

    #[test]
    fn tts_env_resolution_serial() {
        // Env vars are process-global, so both assertion directions run under
        // a shared mutex with the tools_script apply_tts_config_defaults tests
        // (the same pattern as test_find_conda_python_fake_env_falls_back).
        let _guard = TTS_ENV_TEST_MUTEX.lock().unwrap();
        std::env::remove_var("OPENSCRIPT_TTS_BACKEND");
        std::env::remove_var("OPENSCRIPT_TTS_VOICE");
        assert_eq!(resolve_tts_default_backend(), "kokoro");
        assert!(resolve_tts_default_voice().is_none());

        std::env::set_var("OPENSCRIPT_TTS_BACKEND", "audio8");
        std::env::set_var("OPENSCRIPT_TTS_VOICE", "ishan");
        assert_eq!(resolve_tts_default_backend(), "audio8");
        assert_eq!(resolve_tts_default_voice().as_deref(), Some("ishan"));

        std::env::remove_var("OPENSCRIPT_TTS_BACKEND");
        std::env::remove_var("OPENSCRIPT_TTS_VOICE");
    }

    #[test]
    fn tts_config_roundtrips_through_write() {
        // Ensure the tts section survives the persist/load cycle.
        let mut cfg = OpenScriptConfig::default();
        cfg.tts.default_backend = "audio8".into();
        cfg.tts.default_voice = Some("ishan".into());
        let out = json!({
            "tts": {
                "default_backend": cfg.tts.default_backend,
                "default_voice": cfg.tts.default_voice,
            },
        });
        let parsed: OpenScriptConfig = serde_json::from_value(out).unwrap();
        assert_eq!(parsed.tts.default_backend, "audio8");
        assert_eq!(parsed.tts.default_voice.as_deref(), Some("ishan"));
    }

    #[test]
    fn features_default_all_on() {
        // Backward compatibility: a config with no features section enables
        // every subsystem.
        let cfg = OpenScriptConfig::default();
        assert!(cfg.features.tts.voicedesign);
        assert!(cfg.features.tts.kokoro);
        assert!(cfg.features.transcription.parakeet_align);
        assert!(cfg.features.media.youtube);
        assert!(cfg.features.llm.opencode);
        assert!(cfg.features.render.nvenc);
        assert!(cfg.features.frontend);
    }

    #[test]
    fn features_deserialize_unknown_and_partial() {
        // A partial features section must not wipe defaults, and future/unknown
        // toggles are tolerated.
        let v = json!({
            "features": {
                "tts": { "voicedesign": false },
                "media": { "youtube": false },
                "future_category": { "future_toggle": true },
            },
        });
        let cfg: OpenScriptConfig = serde_json::from_value(v).unwrap();
        assert!(!cfg.features.tts.voicedesign);
        assert!(cfg.features.tts.kokoro); // untouched -> default true
        assert!(!cfg.features.media.youtube);
        assert!(cfg.features.media.pexels);
        assert!(cfg.features.llm.openrouter);
    }

    #[test]
    fn features_roundtrip_through_write() {
        // A features section survives the persist/parse cycle so setup.sh and
        // the runtime always agree on what is active.
        let mut cfg = OpenScriptConfig::default();
        cfg.features.tts.voicedesign = false;
        cfg.features.transcription.parakeet_align = false;
        let out = json!({"features": cfg.features});
        let parsed: OpenScriptConfig = serde_json::from_value(out).unwrap();
        assert!(!parsed.features.tts.voicedesign);
        assert!(!parsed.features.transcription.parakeet_align);
        assert!(parsed.features.tts.kokoro);
    }

    #[test]
    fn feature_enabled_env_override_and_default() {
        // Unknown/off values disable; on values enable; no env -> config default.
        let _guard = TTS_ENV_TEST_MUTEX.lock().unwrap();
        std::env::set_var("OPENSCRIPT_FEATURE_TTS_VOICEDESIGN", "0");
        std::env::set_var("OPENSCRIPT_FEATURE_MEDIA_PEXELS", "false");
        std::env::set_var("OPENSCRIPT_FEATURE_LLM_OPENCODE", "on");
        assert!(!feature_enabled("tts", "voicedesign"));
        assert!(!feature_enabled("media", "pexels"));
        assert!(feature_enabled("llm", "opencode"));
        assert!(feature_enabled("tts", "kokoro")); // default true
        assert!(feature_enabled("unknown_category", "x"));
        std::env::remove_var("OPENSCRIPT_FEATURE_TTS_VOICEDESIGN");
        std::env::remove_var("OPENSCRIPT_FEATURE_MEDIA_PEXELS");
        std::env::remove_var("OPENSCRIPT_FEATURE_LLM_OPENCODE");
    }

    #[test]
    fn feature_typed_helpers() {
        assert!(feature_tts("kokoro"));
        assert!(feature_transcription("hinglish_ggml"));
        assert!(feature_media("giphy"));
        assert!(feature_llm("openrouter"));
        assert!(feature_render("ffmpeg"));
        assert!(feature_frontend());
        let _ = feature_flags_view(); // must serialize
    }
}
