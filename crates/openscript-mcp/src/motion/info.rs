//! Project capability introspection.
//!
//! Queries the Remotion project to discover installed fonts,
//! available codecs, and configuration details so agents
//! never guess about project capabilities.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tokio::process::Command;

fn find_remotion_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let candidate = PathBuf::from(&root).join("remotion");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let mut current = std::env::current_dir().ok()?;
    loop {
        let candidate = current.join("remotion");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionInfo {
    pub id: String,
    pub duration_in_frames: u32,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

pub struct MotionInfo {
    pub remotion_root: String,
    pub compositions: Vec<CompositionInfo>,
    pub installed_fonts: Vec<String>,
    pub node_version: String,
    pub remotion_version: String,
}

pub async fn get_motion_info() -> Result<MotionInfo, String> {
    let remotion_root = find_remotion_root()
        .ok_or_else(|| "Could not find remotion/ directory.".to_string())?;

    let remotion_root_str = remotion_root.to_string_lossy().to_string();

    let compositions = discover_compositions(&remotion_root).await;
    let installed_fonts = discover_fonts(&remotion_root).await;
    let node_version = get_node_version().await;
    let remotion_version = get_remotion_version(&remotion_root).await;

    Ok(MotionInfo {
        remotion_root: remotion_root_str,
        compositions,
        installed_fonts,
        node_version,
        remotion_version,
    })
}

/// Extract a u32 value from a pattern like `attrName={123}` or `attrName={someVar}`.
/// Returns 0 if the value is a variable (non-numeric) or not found.
fn extract_jsx_numeric_attr(block: &str, attr_name: &str) -> u32 {
    let search = format!("{}={{", attr_name);
    if let Some(start) = block.find(&search) {
        let after_brace = &block[start + search.len()..];
        if let Some(end) = after_brace.find('}') {
            let value = after_brace[..end].trim();
            if let Ok(n) = value.parse::<u32>() {
                return n;
            }
            // variable reference like `DURATION` — return 0
        }
    }
    0
}

/// Extract a string value from a pattern like `attrName="value"`.
/// Returns empty string if not found.
fn extract_jsx_string_attr(block: &str, attr_name: &str) -> String {
    let search = format!("{}=\"", attr_name);
    if let Some(start) = block.find(&search) {
        let after_quote = &block[start + search.len()..];
        if let Some(end) = after_quote.find('"') {
            return after_quote[..end].to_string();
        }
    }
    String::new()
}

async fn discover_compositions(remotion_root: &PathBuf) -> Vec<CompositionInfo> {
    let root_path = remotion_root.join("src/RemotionRoot.tsx");
    let content = match fs::read_to_string(&root_path).await {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut compositions = Vec::new();
    let mut search_from = 0;

    while let Some(comp_start) = content[search_from..].find("<Composition") {
        let absolute_start = search_from + comp_start;

        // Find the closing of this Composition element (either /> or </Composition>)
        let block_end = if let Some(idx) = content[absolute_start..].find("/>") {
            absolute_start + idx + 2
        } else if let Some(idx) = content[absolute_start..].find("</Composition>") {
            absolute_start + idx + "</Composition>".len()
        } else {
            // Malformed — skip this occurrence
            search_from = absolute_start + "<Composition".len();
            continue;
        };

        let block = &content[absolute_start..block_end];

        let id = extract_jsx_string_attr(block, "id");
        if id.is_empty() {
            search_from = block_end;
            continue;
        }

        let duration_in_frames = extract_jsx_numeric_attr(block, "durationInFrames");
        let fps = extract_jsx_numeric_attr(block, "fps");
        let width = extract_jsx_numeric_attr(block, "width");
        let height = extract_jsx_numeric_attr(block, "height");

        compositions.push(CompositionInfo {
            id,
            duration_in_frames,
            fps,
            width,
            height,
        });

        search_from = block_end;
    }

    compositions
}

async fn discover_fonts(remotion_root: &PathBuf) -> Vec<String> {
    let mut fonts = Vec::new();

    // Check for font files in remotion/fonts/ or remotion/public/fonts/
    let font_dirs = [
        remotion_root.join("fonts"),
        remotion_root.join("public/fonts"),
        remotion_root.join("assets/fonts"),
    ];

    for dir in &font_dirs {
        if let Ok(mut entries) = fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(name) = entry.file_name().into_string() {
                    let lower = name.to_lowercase();
                    if lower.ends_with(".ttf")
                        || lower.ends_with(".otf")
                        || lower.ends_with(".woff")
                        || lower.ends_with(".woff2")
                    {
                        let stem = name
                            .trim_end_matches(".ttf")
                            .trim_end_matches(".otf")
                            .trim_end_matches(".woff")
                            .trim_end_matches(".woff2");
                        if !fonts.contains(&stem.to_string()) {
                            fonts.push(stem.to_string());
                        }
                    }
                }
            }
        }
    }

    // Check package.json for @fontsource or font dependencies
    let pkg_path = remotion_root.join("package.json");
    if let Ok(content) = fs::read_to_string(&pkg_path).await {
        if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
            for dep_key in &["dependencies", "devDependencies"] {
                if let Some(deps) = pkg.get(*dep_key).and_then(|v| v.as_object()) {
                    for dep_name in deps.keys() {
                        if dep_name.contains("font") || dep_name.contains("@fontsource") {
                            fonts.push(dep_name.clone());
                        }
                    }
                }
            }
        }
    }

    fonts.sort();
    fonts.dedup();
    fonts
}

async fn get_node_version() -> String {
    match Command::new("node")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) => {
            let v = String::from_utf8_lossy(&output.stdout);
            v.trim().to_string()
        }
        Err(_) => "unknown".to_string(),
    }
}

async fn get_remotion_version(remotion_root: &PathBuf) -> String {
    let pkg_path = remotion_root.join("package.json");
    if let Ok(content) = fs::read_to_string(&pkg_path).await {
        if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_object()) {
                if let Some(ver) = deps.get("remotion").and_then(|v| v.as_str()) {
                    return ver.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}
