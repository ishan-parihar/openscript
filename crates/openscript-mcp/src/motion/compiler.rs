//! Compiler — TypeScript compilation check without rendering.
//!
//! Runs `tsc --noEmit` on the remotion project to catch type errors,
//! missing imports, and wrong prop types before attempting a render.
//! 2-5 second feedback loop with structured error messages.

use std::path::PathBuf;
use tokio::fs;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct CompileError {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    pub code: String,
}

#[derive(Debug, Clone)]
pub struct DurationOverflowIssue {
    pub max_sequence_end: u32,
    pub composition_limit: u32,
    pub overflow_frames: u32,
}

#[derive(Debug)]
pub struct CompileCheckResult {
    pub valid: bool,
    pub errors: Vec<CompileError>,
    pub raw_output: String,
    pub duration_overflow: Option<DurationOverflowIssue>,
}

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

pub async fn compile_check_tsx(tsx_code: &str) -> Result<CompileCheckResult, String> {
    let remotion_root = find_remotion_root()
        .ok_or_else(|| "Could not find remotion/ directory.".to_string())?;

    let composition_path = remotion_root.join("src/compositions/hot-composition.tsx");
    if let Some(parent) = composition_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create composition directory: {e}"))?;
    }
    fs::write(&composition_path, tsx_code)
        .await
        .map_err(|e| format!("Failed to write TSX composition: {e}"))?;

    let tsc_output = Command::new("npx")
        .args(["tsc", "--noEmit", "--pretty", "false"])
        .current_dir(&remotion_root)
        .output()
        .await
        .map_err(|e| format!("Failed to execute tsc: {e}"))?;

    let stdout = String::from_utf8_lossy(&tsc_output.stdout);
    let stderr = String::from_utf8_lossy(&tsc_output.stderr);
    let combined = if !stdout.is_empty() {
        stdout.to_string()
    } else {
        stderr.to_string()
    };

    if tsc_output.status.success() {
        // tsc passed — check for duration overflow as a warning-level issue
        let composition_limit = read_composition_duration(
            &remotion_root.join("src/RemotionRoot.tsx"),
            "HotMotion",
        );
        let duration_overflow = check_duration_overflow(tsx_code, composition_limit);

        let mut raw_output = combined.trim().to_string();
        if let Some(ref overflow) = duration_overflow {
            raw_output.push_str(&format!(
                "\n⚠ Duration overflow: Sequences extend to frame {} but composition is limited to {} frames (overflow: {} frames).",
                overflow.max_sequence_end, overflow.composition_limit, overflow.overflow_frames
            ));
        }

        return Ok(CompileCheckResult {
            valid: true,
            errors: Vec::new(),
            raw_output,
            duration_overflow,
        });
    }

    let errors = parse_tsc_output(&combined);

    // Even on tsc failure, still run overflow check for completeness
    let composition_limit = read_composition_duration(
        &remotion_root.join("src/RemotionRoot.tsx"),
        "HotMotion",
    );
    let duration_overflow = check_duration_overflow(tsx_code, composition_limit);

    Ok(CompileCheckResult {
        valid: errors.is_empty(),
        errors,
        raw_output: combined.trim().to_string(),
        duration_overflow,
    })
}

/// Parse TSX code to find all `<Sequence` tags and check if any extend
/// beyond the composition's duration limit.
///
/// For each Sequence, calculates `end_frame = from + durationInFrames`.
/// Returns `Some(DurationOverflowIssue)` if the maximum end frame exceeds
/// the composition limit.
fn check_duration_overflow(tsx_code: &str, composition_limit: u32) -> Option<DurationOverflowIssue> {
    let mut max_end_frame: u32 = 0;

    let mut pos = 0;
    while let Some(seq_start) = tsx_code[pos..].find("<Sequence") {
        let attr_start = pos + seq_start;
        // Search within the next ~2000 chars for the Sequence attributes
        let seq_block_end = (attr_start + 2000).min(tsx_code.len());
        let seq_block = &tsx_code[attr_start..seq_block_end];

        // Find the closing of the opening tag
        if let Some(tag_end) = seq_block.find('>') {
            let tag_content = &seq_block[..tag_end];

            // Extract durationInFrames
            let duration = extract_sequence_duration(tag_content);
            if duration.is_none() {
                pos = attr_start + 1;
                continue;
            }
            let duration = duration.unwrap();

            // Extract from (defaults to 0 if missing)
            let from = extract_sequence_from(tag_content);

            let end_frame = from + duration;
            if end_frame > max_end_frame {
                max_end_frame = end_frame;
            }
        }

        pos = attr_start + 1;
    }

    if max_end_frame > composition_limit {
        Some(DurationOverflowIssue {
            max_sequence_end: max_end_frame,
            composition_limit,
            overflow_frames: max_end_frame - composition_limit,
        })
    } else {
        None
    }
}

/// Extract `durationInFrames={NUMBER}` from a Sequence opening tag.
/// Handles both braced `{NUMBER}` and bare `NUMBER` forms.
fn extract_sequence_duration(tag_content: &str) -> Option<u32> {
    // Look for durationInFrames=
    let attr = "durationInFrames";
    let idx = tag_content.find(attr)?;
    let after_attr = &tag_content[idx + attr.len()..];
    let after_eq = after_attr.strip_prefix('=')?.trim_start();

    if let Some(first) = after_eq.chars().next() {
        if first == '{' {
            let close = after_eq.find('}')?;
            let num_str = after_eq[1..close].trim();
            return num_str.parse::<u32>().ok();
        } else {
            let end = after_eq
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_eq.len());
            if end > 0 {
                return after_eq[..end].parse::<u32>().ok();
            }
        }
    }
    None
}

/// Extract `from={NUMBER}` from a Sequence opening tag.
/// Defaults to 0 if the attribute is missing.
fn extract_sequence_from(tag_content: &str) -> u32 {
    let attr = "from";
    // Make sure we match the exact attribute (not e.g. "fromFrame")
    if let Some(idx) = tag_content.find(attr) {
        // Verify it's not part of a longer identifier
        let before = &tag_content[..idx];
        let is_standalone = before.ends_with(' ')
            || before.ends_with('\t')
            || before.ends_with('\n')
            || before.is_empty();

        if is_standalone {
            let after_attr = &tag_content[idx + attr.len()..];
            if let Some(after_eq) = after_attr.strip_prefix('=') {
                let trimmed = after_eq.trim_start();
                if let Some(first) = trimmed.chars().next() {
                    if first == '{' {
                        if let Some(close) = trimmed.find('}') {
                            let num_str = trimmed[1..close].trim();
                            if let Ok(val) = num_str.parse::<u32>() {
                                return val;
                            }
                        }
                    } else {
                        let end = trimmed
                            .find(|c: char| !c.is_ascii_digit())
                            .unwrap_or(trimmed.len());
                        if end > 0 {
                            if let Ok(val) = trimmed[..end].parse::<u32>() {
                                return val;
                            }
                        }
                    }
                }
            }
        }
    }
    0
}

/// Read the composition duration from RemotionRoot.tsx.
///
/// Finds the `<Composition` block with the given composition_id and
/// extracts its `durationInFrames` value. Returns 900 as default if
/// not found.
fn read_composition_duration(remotion_root_path: &PathBuf, composition_id: &str) -> u32 {
    let content = match std::fs::read_to_string(remotion_root_path) {
        Ok(c) => c,
        Err(_) => return 900,
    };

    // Find all <Composition blocks and look for the matching id
    let mut pos = 0;
    while let Some(comp_start) = content[pos..].find("<Composition") {
        let block_start = pos + comp_start;
        // Search within ~2000 chars for the full opening tag
        let block_end = (block_start + 2000).min(content.len());
        let block = &content[block_start..block_end];

        // Check if this Composition has the matching id
        let id_attr = format!("id=\"{}\"", composition_id);
        if block.contains(&id_attr) || block.contains(&format!("id='{}'", composition_id)) {
            // Extract durationInFrames from this block
            if let Some(dur) = extract_sequence_duration(block) {
                return dur;
            }
        }

        pos = block_start + 1;
    }

    900
}

fn parse_tsc_output(output: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();

    // TypeScript errors follow pattern:
    // file.ts(line,col): error TSXXXX: message
    // or multi-line:
    // file.ts(line,col): error TSXXXX: message
    //   code snippet
    //   ~~~~~~~~~~~~~~
    for line in output.lines() {
        if let Some(error) = parse_tsc_line(line) {
            errors.push(error);
        }
    }

    errors
}

fn parse_tsc_line(line: &str) -> Option<CompileError> {
    // Match: path/file.ts(line,col): error TS1234: message
    let line = line.trim();

    if !line.contains("error TS") {
        return None;
    }

    // Find the file:line:col part
    let paren_open = line.find('(')?;
    let paren_close = line.find(')')?;
    let file = line[..paren_open].to_string();

    let coords = &line[paren_open + 1..paren_close];
    let parts: Vec<&str> = coords.split(',').collect();
    if parts.len() < 2 {
        return None;
    }

    let line_num: u32 = parts[0].trim().parse().ok()?;
    let col_num: u32 = parts[1].trim().parse().ok()?;

    // Extract error code and message after ")": "error TSXXXX: message"
    let after_paren = line[paren_close + 1..].trim();
    if !after_paren.starts_with("error TS") {
        return None;
    }

    let after_error = after_paren.strip_prefix("error ")?;
    let colon_pos = after_error.find(':')?;
    let code = after_error[..colon_pos].trim().to_string();
    let message = after_error[colon_pos + 1..].trim().to_string();

    Some(CompileError {
        file,
        line: line_num,
        column: col_num,
        message,
        code,
    })
}
