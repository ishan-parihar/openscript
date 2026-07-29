//! User configuration for OpenScript.
//!
//! ## Load order (highest priority first)
//!
//! 1. **Environment variables** (`PEXELS_API_KEY`, `OPENROUTER_API_KEY`,
//!    `OPENSCRIPT_LOCAL_MODEL`, …) — for CI / one-off overrides
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

    /// Flat legacy keys from `mcp/assets/.openscript_config.json`
    /// (e.g. `"pexels_api_key": "..."`). Merged into `api_keys` on load.
    #[serde(flatten)]
    pub legacy: std::collections::HashMap<String, Value>,
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
    /// Ollama model name (default: qwen3.5-4b)
    #[serde(default = "default_local_model")]
    pub local_model: String,
    /// OpenAI-compatible base URL for local inference
    #[serde(default = "default_local_base_url")]
    pub local_base_url: String,
    /// Path to local GGUF (expanded ~)
    #[serde(default)]
    pub gguf_path: Option<String>,
    /// Optional multimodal projector GGUF
    #[serde(default)]
    pub mmproj_path: Option<String>,
    /// If true, send images to local Ollama (needs vision-capable model)
    #[serde(default)]
    pub local_vision: bool,
    #[serde(default = "default_openrouter_base_url")]
    pub openrouter_base_url: String,
    /// Ordered OpenRouter model cascade for text + vision fallbacks
    #[serde(default = "default_openrouter_models")]
    pub openrouter_models: Vec<String>,
    /// Prefer openrouter for multimodal when an image is attached (default true)
    #[serde(default = "default_true")]
    pub prefer_openrouter_vision: bool,
    /// OpenCode API base URL (opencode.ai compatible)
    #[serde(default = "default_opencode_base_url")]
    pub opencode_base_url: String,
    /// OpenCode model name
    #[serde(default = "default_opencode_model")]
    pub opencode_model: String,
}

fn default_true() -> bool {
    true
}

fn default_local_model() -> String {
    "qwen3.5-4b".into()
}
fn default_local_base_url() -> String {
    "http://127.0.0.1:11434/v1".into()
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
            local_model: default_local_model(),
            local_base_url: default_local_base_url(),
            gguf_path: None,
            mmproj_path: None,
            local_vision: false,
            openrouter_base_url: default_openrouter_base_url(),
            openrouter_models: default_openrouter_models(),
            prefer_openrouter_vision: true,
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

fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        return home_dir().join(rest).to_string_lossy().into_owned();
    }
    if s == "~" {
        return home_dir().to_string_lossy().into_owned();
    }
    s.to_string()
}

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
        // Expand tildes in llm paths
        if let Some(ref p) = self.llm.gguf_path {
            self.llm.gguf_path = Some(expand_tilde(p));
        }
        if let Some(ref p) = self.llm.mmproj_path {
            self.llm.mmproj_path = Some(expand_tilde(p));
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
        // llm: only fill missing optionals / empty model if somehow empty
        if self.llm.gguf_path.is_none() {
            self.llm.gguf_path = other.llm.gguf_path.clone();
        }
        if self.llm.mmproj_path.is_none() {
            self.llm.mmproj_path = other.llm.mmproj_path.clone();
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
    // Sensible defaults for this machine
    if cfg.llm.gguf_path.is_none() {
        let gguf = home_dir().join("Downloads/Qwen3.5-4B-Q4_K_M.gguf");
        if gguf.exists() {
            cfg.llm.gguf_path = Some(gguf.to_string_lossy().into_owned());
        }
    }
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
            "local_model": cfg.llm.local_model,
            "local_base_url": cfg.llm.local_base_url,
            "gguf_path": cfg.llm.gguf_path,
            "mmproj_path": cfg.llm.mmproj_path,
            "local_vision": cfg.llm.local_vision,
            "openrouter_base_url": cfg.llm.openrouter_base_url,
            "openrouter_models": cfg.llm.openrouter_models,
            "prefer_openrouter_vision": cfg.llm.prefer_openrouter_vision,
            "opencode_base_url": cfg.llm.opencode_base_url,
            "opencode_model": cfg.llm.opencode_model,
        },
        "paths": cfg.paths,
        "render": cfg.render,
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

pub fn resolve_local_model() -> String {
    env_nonempty("OPENSCRIPT_LOCAL_MODEL")
        .unwrap_or_else(|| config().llm.local_model.clone())
}

pub fn resolve_local_base_url() -> String {
    env_nonempty("OPENSCRIPT_LLM_URL")
        .unwrap_or_else(|| config().llm.local_base_url.clone())
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

pub fn resolve_gguf_path() -> Option<PathBuf> {
    if let Some(p) = env_nonempty("OPENSCRIPT_GGUF_PATH") {
        let pb = PathBuf::from(expand_tilde(&p));
        if pb.exists() {
            return Some(pb);
        }
    }
    let cfg = config();
    if let Some(ref p) = cfg.llm.gguf_path {
        let pb = PathBuf::from(expand_tilde(p));
        if pb.exists() {
            return Some(pb);
        }
    }
    // Default scan locations
    let candidates = [
        home_dir().join("Downloads/Qwen3.5-4B-Q4_K_M.gguf"),
        home_dir().join("Downloads/Qwen3.5-4B-Q4_K_S.gguf"),
        config_dir().join("models/Qwen3.5-4B-Q4_K_M.gguf"),
        PathBuf::from("mcp/models/Qwen3.5-4B-Q4_K_M.gguf"),
        PathBuf::from("mcp/assets/models/Qwen3.5-4B-Q4_K_M.gguf"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

pub fn resolve_mmproj_path() -> Option<PathBuf> {
    if let Some(p) = env_nonempty("OPENSCRIPT_MMPROJ_PATH") {
        let pb = PathBuf::from(expand_tilde(&p));
        if pb.exists() {
            return Some(pb);
        }
    }
    if let Some(ref p) = config().llm.mmproj_path {
        let pb = PathBuf::from(expand_tilde(p));
        if pb.exists() {
            return Some(pb);
        }
    }
    let candidates = [
        home_dir().join("Downloads/mmproj-F16.gguf"),
        home_dir().join("Downloads/mmproj-BF16.gguf"),
        config_dir().join("models/mmproj-F16.gguf"),
        PathBuf::from("mcp/models/mmproj-F16.gguf"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

pub fn resolve_local_vision() -> bool {
    if let Some(v) = env_nonempty("OPENSCRIPT_LOCAL_VISION") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    config().llm.local_vision
}

pub fn resolve_prefer_openrouter_vision() -> bool {
    config().llm.prefer_openrouter_vision
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

/// Redacted public view for system.capabilities / system.config.get
pub fn config_public_view() -> Value {
    let cfg = config();
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
            "local_model": resolve_local_model(),
            "local_base_url": resolve_local_base_url(),
            "gguf_path": resolve_gguf_path().map(|p| p.display().to_string()),
            "mmproj_path": resolve_mmproj_path().map(|p| p.display().to_string()),
            "local_vision": resolve_local_vision(),
            "prefer_openrouter_vision": resolve_prefer_openrouter_vision(),
            "openrouter_base_url": resolve_openrouter_base_url(),
            "openrouter_models": resolve_openrouter_models(),
        },
        "paths": cfg.paths,
        "render": cfg.render,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_home() {
        let h = home_dir().to_string_lossy().into_owned();
        assert_eq!(expand_tilde("~/foo"), format!("{}/foo", h));
    }

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
}
