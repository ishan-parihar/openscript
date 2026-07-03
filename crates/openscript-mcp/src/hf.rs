//! HyperFrames MCP tools — wrappers around the `npx hyperframes` CLI.
//!
//! These tools give AI agents programmatic access to the HyperFrames dev loop:
//! lint, validate, snapshot, render. Each tool shells out to `npx hyperframes`
//! with `--json` where available and returns the parsed JSON envelope.
//!
//! The tools are designed to be agent-friendly:
//! - `--json` output is parsed and returned inline (no log scraping)
//! - Non-TTY mode is auto-detected by the CLI
//! - Errors include the CLI's stderr for self-correction
//! - `project_dir` defaults to `./hyperframes` but can be overridden

use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;

/// Default project directory if none is specified.
const DEFAULT_HF_PROJECT_DIR: &str = "hyperframes";

/// Timeout for CLI operations (lint/validate/snapshot). Render has its own longer timeout.
const CLI_TIMEOUT_SECS: u64 = 120;
/// Render timeout — rendering can take minutes for long videos.
const RENDER_TIMEOUT_SECS: u64 = 600;

/// Run `npx hyperframes <subcommand>` in `project_dir` and return (stdout, stderr, exit_code).
async fn run_hf_cli(
    subcommand: &str,
    project_dir: &str,
    extra_args: &[&str],
    timeout_secs: u64,
) -> Result<HfCliOutput, HfError> {
    let mut cmd = Command::new("npx");
    cmd.arg("hyperframes")
        .arg(subcommand)
        .args(extra_args)
        .current_dir(project_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Wrap in a timeout
    let child = cmd.spawn().map_err(|e| HfError::Spawn(e.to_string()))?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| HfError::Timeout(timeout_secs))?
    .map_err(|e| HfError::Wait(e.to_string()))?;

    Ok(HfCliOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

struct HfCliOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

/// Try to parse the CLI stdout as JSON. If it fails, return the raw stdout in a
/// structured response so the agent can still see what happened.
fn parse_json_output(stdout: &str) -> Value {
    serde_json::from_str::<Value>(stdout).unwrap_or_else(|_| {
        json!({
            "raw_stdout": stdout,
        })
    })
}

// ---------------------------------------------------------------------------
// Tool: hf.lint
// ---------------------------------------------------------------------------

pub async fn handle_hf_lint(args: Value) -> Result<Value, HfError> {
    let project_dir = args
        .get("project_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_HF_PROJECT_DIR);

    let output = run_hf_cli("lint", project_dir, &["--json"], CLI_TIMEOUT_SECS).await?;

    let mut result = parse_json_output(&output.stdout);
    if let Some(obj) = result.as_object_mut() {
        obj.insert("exit_code".into(), json!(output.exit_code));
        obj.insert("project_dir".into(), json!(project_dir));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tool: hf.validate
// ---------------------------------------------------------------------------

pub async fn handle_hf_validate(args: Value) -> Result<Value, HfError> {
    let project_dir = args
        .get("project_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_HF_PROJECT_DIR);

    let output = run_hf_cli("validate", project_dir, &["--json"], CLI_TIMEOUT_SECS).await?;

    let mut result = parse_json_output(&output.stdout);
    if let Some(obj) = result.as_object_mut() {
        obj.insert("exit_code".into(), json!(output.exit_code));
        obj.insert("project_dir".into(), json!(project_dir));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tool: hf.snapshot
// ---------------------------------------------------------------------------

pub async fn handle_hf_snapshot(args: Value) -> Result<Value, HfError> {
    let project_dir = args
        .get("project_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_HF_PROJECT_DIR);

    let mut extra_args = vec!["--json"];

    // Either --frames N or --at t1,t2,t3
    if let Some(frames) = args.get("frames").and_then(|v| v.as_u64()) {
        extra_args.push("--frames");
        extra_args.push(Box::leak(frames.to_string().into_boxed_str()));
    } else if let Some(at) = args.get("at").and_then(|v| v.as_str()) {
        extra_args.push("--at");
        extra_args.push(at);
    } else {
        // Default: 9 evenly-spaced frames
        extra_args.push("--frames");
        extra_args.push("9");
    }

    let output = run_hf_cli("snapshot", project_dir, &extra_args, CLI_TIMEOUT_SECS).await?;

    let mut result = parse_json_output(&output.stdout);
    if let Some(obj) = result.as_object_mut() {
        obj.insert("exit_code".into(), json!(output.exit_code));
        obj.insert("project_dir".into(), json!(project_dir));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tool: hf.render
// ---------------------------------------------------------------------------

pub async fn handle_hf_render(args: Value) -> Result<Value, HfError> {
    let project_dir = args
        .get("project_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_HF_PROJECT_DIR);

    let quality = args
        .get("quality")
        .and_then(|v| v.as_str())
        .unwrap_or("standard");

    let output_path = args
        .get("output_path")
        .and_then(|v| v.as_str())
        .unwrap_or("output.mp4");

    let mut extra_args = vec!["--quality", quality, "--output", output_path];

    // Optional: strict mode for CI gating
    if args
        .get("strict")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        extra_args.push("--strict");
    }

    // Optional: docker for cross-host reproducibility
    if args
        .get("docker")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        extra_args.push("--docker");
    }

    let output =
        run_hf_cli("render", project_dir, &extra_args, RENDER_TIMEOUT_SECS).await?;

    // Render doesn't support --json; return structured result
    let success = output.exit_code == 0;

    // Verify the output file exists and has plausible size
    let (file_exists, file_size) = if success {
        match std::fs::metadata(output_path) {
            Ok(meta) => (true, meta.len()),
            Err(_) => (false, 0),
        }
    } else {
        (false, 0)
    };

    Ok(json!({
        "status": if success && file_exists { "rendered" } else { "failed" },
        "exit_code": output.exit_code,
        "output_path": output_path,
        "file_exists": file_exists,
        "file_size_bytes": file_size,
        "project_dir": project_dir,
        "quality": quality,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum HfError {
    #[error("Failed to spawn npx hyperframes: {0}. Is Node.js >= 22 installed?")]
    Spawn(String),
    #[error("CLI timed out after {0}s")]
    Timeout(u64),
    #[error("Failed to wait for CLI process: {0}")]
    Wait(String),
}

impl serde::Serialize for HfError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tool definitions (for the MCP tool list)
// ---------------------------------------------------------------------------

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "hf.lint",
            "description": "Run HyperFrames static lint checks on a composition project. Catches missing data-composition-id, overlapping tracks, unregistered timelines, and other static issues. Returns JSON with issues array, exit_code, and project_dir. Use BEFORE hf.validate and hf.render to catch cheap errors early.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_dir": {
                        "type": "string",
                        "default": "hyperframes",
                        "description": "Path to the HyperFrames project directory (containing index.html)"
                    }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "hf.validate",
            "description": "Run HyperFrames runtime validation on a composition project. Loads the composition in headless Chrome and reports runtime console errors plus WCAG contrast issues. Returns JSON with issues array, exit_code, and project_dir. Use AFTER hf.lint and BEFORE hf.render.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_dir": {
                        "type": "string",
                        "default": "hyperframes",
                        "description": "Path to the HyperFrames project directory"
                    }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "hf.snapshot",
            "description": "Capture visual snapshots of a HyperFrames composition at specified timestamps. Loads the project like render does but only captures the requested frames — seconds instead of a full render. Use as a visual smoke test before committing to a full render. Outputs PNG files to snapshots/ directory. Returns JSON with snapshot paths, exit_code, and project_dir.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_dir": {
                        "type": "string",
                        "default": "hyperframes",
                        "description": "Path to the HyperFrames project directory"
                    },
                    "frames": {
                        "type": "integer",
                        "default": 9,
                        "description": "Number of evenly-spaced frames to capture"
                    },
                    "at": {
                        "type": "string",
                        "description": "Comma-separated timestamps (seconds) to capture, e.g. '0.5,1.5,2.5'. Overrides 'frames'."
                    }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "hf.render",
            "description": "Render a HyperFrames composition to MP4. This is the final output step. Run hf.lint and hf.validate first. Returns status (rendered/failed), output_path, file_size_bytes, and exit_code. Post-render: verify file_exists=true and file_size_bytes is plausible. For long videos use quality='draft' for iteration, quality='high' for delivery.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_dir": {
                        "type": "string",
                        "default": "hyperframes",
                        "description": "Path to the HyperFrames project directory"
                    },
                    "quality": {
                        "type": "string",
                        "enum": ["draft", "standard", "high"],
                        "default": "standard",
                        "description": "Render quality: draft (fast iteration), standard (preview), high (delivery)"
                    },
                    "output_path": {
                        "type": "string",
                        "default": "output.mp4",
                        "description": "Output MP4 path"
                    },
                    "strict": {
                        "type": "boolean",
                        "default": false,
                        "description": "Fail on lint errors (CI gating)"
                    },
                    "docker": {
                        "type": "boolean",
                        "default": false,
                        "description": "Render in Docker for cross-host reproducibility"
                    }
                },
                "additionalProperties": false
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_output_valid() {
        let result = parse_json_output(r#"{"ok": true, "issues": []}"#);
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn test_parse_json_output_invalid() {
        let result = parse_json_output("not json at all");
        assert_eq!(result["raw_stdout"], "not json at all");
    }

    #[test]
    fn test_tool_definitions_count() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 4);
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"hf.lint"));
        assert!(names.contains(&"hf.validate"));
        assert!(names.contains(&"hf.snapshot"));
        assert!(names.contains(&"hf.render"));
    }
}
