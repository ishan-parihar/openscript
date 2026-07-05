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

/// Default project directory if none is specified. Points at the
/// `main-with-broll` composition which ships with an index.html.
/// Prior versions defaulted to "hyperframes" which has no index.html
/// at root, causing hf.lint/validate/render to fail.
const DEFAULT_HF_PROJECT_DIR: &str = "hyperframes/compositions/main-with-broll";

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
    // Verify project_dir exists before spawning — gives a clearer error than
    // the generic spawn failure message.
    if !std::path::Path::new(project_dir).exists() {
        return Err(HfError::ProjectNotFound(project_dir.to_string()));
    }

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

    // Build owned String args to avoid Box::leak memory leak
    let mut extra_args: Vec<String> = vec!["--json".to_string()];

    // Either --frames N or --at t1,t2,t3
    if let Some(frames) = args.get("frames").and_then(|v| v.as_u64()) {
        extra_args.push("--frames".to_string());
        extra_args.push(frames.to_string());
    } else if let Some(at) = args.get("at").and_then(|v| v.as_str()) {
        extra_args.push("--at".to_string());
        extra_args.push(at.to_string());
    } else {
        // Default: 9 evenly-spaced frames
        extra_args.push("--frames".to_string());
        extra_args.push("9".to_string());
    }

    let extra_args_ref: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
    let output = run_hf_cli("snapshot", project_dir, &extra_args_ref, CLI_TIMEOUT_SECS).await?;

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

    let output = run_hf_cli("render", project_dir, &extra_args, RENDER_TIMEOUT_SECS).await?;

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
    #[error("Invalid argument: {0}")]
    InvalidArg(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Project directory not found: {0}")]
    ProjectNotFound(String),
}

impl serde::Serialize for HfError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tool: hf.classify — Remotion source classifier (PR #214 interop gate)
// ---------------------------------------------------------------------------

/// Blocker patterns from the remotion-to-hyperframes lint rules.
/// If any of these are found, the composition should use the PR #214 interop
/// escape hatch instead of a native HF translation.
const BLOCKER_PATTERNS: &[(&str, &str, &str)] = &[
    (
        "useState",
        "r2hf/use-state",
        "useState drives animation — not deterministic in HF's seek-driven model",
    ),
    (
        "useReducer",
        "r2hf/use-reducer",
        "useReducer drives animation — not deterministic in HF's seek-driven model",
    ),
    (
        "useEffect",
        "r2hf/use-effect-deps",
        "useEffect with deps — side effects break seek-driven determinism",
    ),
    (
        "useLayoutEffect",
        "r2hf/use-effect-deps",
        "useLayoutEffect with deps — side effects break seek-driven determinism",
    ),
    (
        "calculateMetadata",
        "r2hf/async-metadata",
        "calculateMetadata may be async — HF can't resolve at seek time",
    ),
];

/// Third-party React UI libraries that are blockers for HF translation.
const BLOCKER_IMPORTS: &[(&str, &str)] = &[
    ("@mui/material", "r2hf/third-party-react-ui"),
    ("@mui/icons-material", "r2hf/third-party-react-ui"),
    ("@chakra-ui/react", "r2hf/third-party-react-ui"),
    ("@mantine/core", "r2hf/third-party-react-ui"),
    ("antd", "r2hf/third-party-react-ui"),
    ("@shadcn/ui", "r2hf/third-party-react-ui"),
    ("@radix-ui", "r2hf/third-party-react-ui"),
    ("@nextui-org/react", "r2hf/third-party-react-ui"),
];

/// Warning patterns — translate after dropping the construct.
const WARNING_PATTERNS: &[(&str, &str, &str)] = &[
    (
        "@remotion/lambda",
        "r2hf/lambda-import",
        "Lambda config — drop, HF is single-machine",
    ),
    (
        "delayRender",
        "r2hf/delay-render",
        "delayRender — HF handles asset loading differently",
    ),
    (
        "useCallback",
        "r2hf/use-callback",
        "useCallback — decorative, drop and inline",
    ),
    (
        "useMemo",
        "r2hf/use-memo",
        "useMemo — decorative, drop and inline",
    ),
];

/// A lint finding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub severity: String,
    pub rule: String,
    pub message: String,
    pub line: usize,
    pub recommendation: String,
}

/// Lint a Remotion source file for HF translation blockers.
/// Returns a vector of findings (severity, rule, message, line number).
pub fn lint_remotion_source(src: &str) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (line_num, line) in src.lines().enumerate() {
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with("*") {
            continue;
        }

        // Check blocker patterns
        for (pattern, rule, message) in BLOCKER_PATTERNS {
            if line.contains(pattern) {
                // Special case: useEffect/useLayoutEffect — only flag as blocker
                // if we can see a NON-EMPTY deps array on the same line.
                // If the deps array is on a different line (multi-line useEffect),
                // we can't reliably determine if it's empty — skip (conservative).
                if *pattern == "useEffect" || *pattern == "useLayoutEffect" {
                    if let Some(deps_start) = line.find('[') {
                        let after_bracket = &line[deps_start..];
                        // Empty deps [] on same line — not a blocker
                        if after_bracket.starts_with("[]") {
                            continue;
                        }
                        // Non-empty deps [x] on same line — blocker
                        // (the push below will fire)
                    } else {
                        // No bracket on this line — deps are on a different line.
                        // Skip (can't reliably determine if empty).
                        continue;
                    }
                }
                findings.push(Finding {
                    severity: "blocker".to_string(),
                    rule: rule.to_string(),
                    message: message.to_string(),
                    line: line_num + 1,
                    recommendation:
                        "Use the PR #214 interop escape hatch (see hyperframes/interop/)"
                            .to_string(),
                });
            }
        }

        // Check blocker imports
        for (import, rule) in BLOCKER_IMPORTS {
            if line.contains(import) && (line.contains("import") || line.contains("require")) {
                findings.push(Finding {
                    severity: "blocker".to_string(),
                    rule: rule.to_string(),
                    message: format!("Third-party React UI library: {}", import),
                    line: line_num + 1,
                    recommendation: "Use the PR #214 interop escape hatch".to_string(),
                });
            }
        }

        // Check warning patterns
        for (pattern, rule, message) in WARNING_PATTERNS {
            if line.contains(pattern) {
                findings.push(Finding {
                    severity: "warning".to_string(),
                    rule: rule.to_string(),
                    message: message.to_string(),
                    line: line_num + 1,
                    recommendation: "Drop the construct, translate the rest".to_string(),
                });
            }
        }
    }

    findings
}

/// Classify a Remotion source file: should it use HF native or the interop escape hatch?
pub async fn handle_hf_classify(args: Value) -> Result<Value, HfError> {
    let source_path = args
        .get("source_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HfError::InvalidArg("source_path is required".to_string()))?;

    let src = std::fs::read_to_string(source_path)
        .map_err(|e| HfError::Io(format!("Cannot read {}: {}", source_path, e)))?;

    let findings = lint_remotion_source(&src);
    let has_blockers = findings.iter().any(|f| f.severity == "blocker");
    let has_warnings = findings.iter().any(|f| f.severity == "warning");

    let recommendation = if has_blockers { "interop" } else { "hf-native" };

    let recommendation_message = if has_blockers {
        format!(
            "Source has {} blocker(s). Use the PR #214 interop escape hatch: \
             copy hyperframes/interop/index.html and interop/entry-template.tsx, \
             import your composition component, bundle with esbuild, and render via hf.render.",
            findings.iter().filter(|f| f.severity == "blocker").count()
        )
    } else if has_warnings {
        format!(
            "Source is clean (no blockers). Translate to HF native HTML+GSAP. \
             {} warning(s) to address: drop the flagged constructs and inline.",
            findings.iter().filter(|f| f.severity == "warning").count()
        )
    } else {
        "Source is clean. Translate to HF native HTML+GSAP.".to_string()
    };

    Ok(json!({
        "source_path": source_path,
        "recommendation": recommendation,
        "recommendation_message": recommendation_message,
        "has_blockers": has_blockers,
        "has_warnings": has_warnings,
        "blocker_count": findings.iter().filter(|f| f.severity == "blocker").count(),
        "warning_count": findings.iter().filter(|f| f.severity == "warning").count(),
        "findings": findings,
    }))
}

// ---------------------------------------------------------------------------
// Tool: composition.render — unified dispatcher
// ---------------------------------------------------------------------------

/// The unified composition renderer. Takes a composition specification and
/// dispatches to the appropriate engine:
///
/// 1. If `render_hint` is "hf" or "auto" (default) and the source is clean →
///    HF native render via `npx hyperframes render`
/// 2. If `render_hint` is "remotion" or the source has blockers →
///    Remotion interop via the PR #214 escape hatch
/// 3. If `render_hint` is "legacy" →
///    Existing Remotion render via `npx remotion render` (backward compat)
///
/// This is the single entry point agents should use for rendering video.
/// Individual `hf.render` / `timeline.render` tools remain available for
/// advanced/manual control.
pub async fn handle_composition_render(args: Value) -> Result<Value, HfError> {
    let project_dir = args
        .get("project_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_HF_PROJECT_DIR);

    let output_path = args
        .get("output_path")
        .and_then(|v| v.as_str())
        .unwrap_or("output.mp4");

    let quality = args
        .get("quality")
        .and_then(|v| v.as_str())
        .unwrap_or("standard");

    let render_hint = args
        .get("render_hint")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");

    // Optional: classify a Remotion source file to determine routing
    let source_path = args.get("source_path").and_then(|v| v.as_str());

    // Determine the engine
    let (engine, classification) = if render_hint == "legacy" {
        ("legacy-remotion".to_string(), None)
    } else if render_hint == "remotion" {
        ("remotion-interop".to_string(), None)
    } else if render_hint == "hf" {
        ("hf-native".to_string(), None)
    } else {
        // auto — classify if source_path is provided
        if let Some(sp) = source_path {
            let src = std::fs::read_to_string(sp)
                .map_err(|e| HfError::Io(format!("Cannot read {}: {}", sp, e)))?;
            let findings = lint_remotion_source(&src);
            let has_blockers = findings.iter().any(|f| f.severity == "blocker");
            let recommendation = if has_blockers {
                "remotion-interop"
            } else {
                "hf-native"
            };
            (
                recommendation.to_string(),
                Some(json!({
                    "source_path": sp,
                    "findings": findings,
                    "has_blockers": has_blockers,
                    "recommendation": recommendation,
                })),
            )
        } else {
            // auto with no source_path — default to hf-native
            ("hf-native".to_string(), None)
        }
    };

    // Dispatch
    match engine.as_str() {
        "hf-native" => {
            let mut extra_args = vec!["--quality", quality, "--output", output_path];
            if args
                .get("strict")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                extra_args.push("--strict");
            }
            let output =
                run_hf_cli("render", project_dir, &extra_args, RENDER_TIMEOUT_SECS).await?;
            let success = output.exit_code == 0;
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
                "engine": "hf-native",
                "exit_code": output.exit_code,
                "output_path": output_path,
                "file_exists": file_exists,
                "file_size_bytes": file_size,
                "project_dir": project_dir,
                "classification": classification,
                "stdout": output.stdout,
                "stderr": output.stderr,
            }))
        }
        "remotion-interop" => {
            // The interop path: the project_dir should contain an index.html
            // that loads dist/bundle.js (the esbuild-bundled Remotion Player).
            // Check that the bundle exists before rendering.
            let bundle_path = std::path::Path::new(project_dir)
                .join("dist")
                .join("bundle.js");
            if !bundle_path.exists() {
                return Err(HfError::InvalidArg(format!(
                    "Interop bundle not found at {}. Run: npx esbuild <entry>.tsx --bundle --format=iife --outfile=dist/bundle.js --jsx=automatic",
                    bundle_path.display()
                )));
            }
            let extra_args = vec!["--quality", quality, "--output", output_path];
            let output =
                run_hf_cli("render", project_dir, &extra_args, RENDER_TIMEOUT_SECS).await?;
            let success = output.exit_code == 0;
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
                "engine": "remotion-interop",
                "exit_code": output.exit_code,
                "output_path": output_path,
                "file_exists": file_exists,
                "file_size_bytes": file_size,
                "project_dir": project_dir,
                "classification": classification,
                "note": "Rendered via PR #214 interop — Remotion Player mounted in HF, driven frame-by-frame",
                "stdout": output.stdout,
                "stderr": output.stderr,
            }))
        }
        "legacy-remotion" => {
            // Legacy path: shell out to `npx remotion render` directly.
            // This is the backward-compatible path for existing Remotion projects.
            let composition_id = args
                .get("composition_id")
                .and_then(|v| v.as_str())
                .unwrap_or("MainWithBroll");

            let mut cmd = tokio::process::Command::new("npx");
            cmd.arg("remotion")
                .arg("render")
                .arg(composition_id)
                .arg(output_path)
                .current_dir(project_dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);

            let child = cmd.spawn().map_err(|e| HfError::Spawn(e.to_string()))?;
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(RENDER_TIMEOUT_SECS),
                child.wait_with_output(),
            )
            .await
            .map_err(|_| HfError::Timeout(RENDER_TIMEOUT_SECS))?
            .map_err(|e| HfError::Wait(e.to_string()))?;

            let success = output.status.success();
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
                "engine": "legacy-remotion",
                "exit_code": output.status.code().unwrap_or(-1),
                "output_path": output_path,
                "file_exists": file_exists,
                "file_size_bytes": file_size,
                "project_dir": project_dir,
                "composition_id": composition_id,
                "classification": classification,
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }))
        }
        _ => Err(HfError::InvalidArg(format!("Unknown engine: {}", engine))),
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
                        "default": "hyperframes/compositions/main-with-broll",
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
                        "default": "hyperframes/compositions/main-with-broll",
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
                        "default": "hyperframes/compositions/main-with-broll",
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
                        "default": "hyperframes/compositions/main-with-broll",
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
        json!({
            "name": "hf.classify",
            "description": "Classify a Remotion (React) source file to determine whether it should be translated to native HyperFrames HTML+GSAP or use the PR #214 runtime interop escape hatch. Checks for blocker patterns: useState, useReducer, useEffect with deps, async calculateMetadata, third-party React UI libraries (MUI, Chakra, shadcn, Radix, etc.). Returns recommendation ('hf-native' or 'interop'), findings array with severity/rule/line, and actionable guidance. Use BEFORE attempting a Remotion→HF port.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_path": {
                        "type": "string",
                        "description": "Path to the Remotion .tsx/.ts source file to classify"
                    }
                },
                "required": ["source_path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "composition.render",
            "description": "Unified video composition renderer — the single entry point for rendering video. Automatically classifies the composition (if source_path provided) and dispatches to the right engine: (1) hf-native — HyperFrames HTML+GSAP render via 'npx hyperframes render' (DEFAULT for clean sources), (2) remotion-interop — PR #214 escape hatch for stateful Remotion comps (useState/useEffect/3rd-party UI), mounts @remotion/player inside HF and drives frame-by-frame, (3) legacy-remotion — backward-compatible 'npx remotion render' for existing Remotion projects. Use render_hint='auto' (default) to let the classifier decide, or force with 'hf'/'remotion'/'legacy'. Returns: status, engine used, output_path, file_size_bytes, classification details.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_dir": {
                        "type": "string",
                        "default": "hyperframes/compositions/main-with-broll",
                        "description": "Path to the composition project directory"
                    },
                    "output_path": {
                        "type": "string",
                        "default": "output.mp4",
                        "description": "Output MP4 path"
                    },
                    "quality": {
                        "type": "string",
                        "enum": ["draft", "standard", "high"],
                        "default": "standard",
                        "description": "Render quality"
                    },
                    "render_hint": {
                        "type": "string",
                        "enum": ["auto", "hf", "remotion", "legacy"],
                        "default": "auto",
                        "description": "Engine selection: auto (classify), hf (force HyperFrames native), remotion (force PR #214 interop), legacy (force npx remotion render)"
                    },
                    "source_path": {
                        "type": "string",
                        "description": "Path to Remotion .tsx source (used by 'auto' to classify). Optional — omit to default to hf-native."
                    },
                    "composition_id": {
                        "type": "string",
                        "default": "MainWithBroll",
                        "description": "Remotion composition ID (legacy mode only)"
                    },
                    "strict": {
                        "type": "boolean",
                        "default": false,
                        "description": "Fail on lint errors (hf-native mode only)"
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
        assert_eq!(defs.len(), 6);
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"hf.lint"));
        assert!(names.contains(&"hf.validate"));
        assert!(names.contains(&"hf.snapshot"));
        assert!(names.contains(&"hf.render"));
        assert!(names.contains(&"hf.classify"));
        assert!(names.contains(&"composition.render"));
    }

    #[test]
    fn test_lint_clean_source() {
        let src = r#"
            import { useCurrentFrame, interpolate } from "remotion";
            export const Comp = () => {
                const frame = useCurrentFrame();
                const opacity = interpolate(frame, [0, 30], [0, 1]);
                return <div style={{ opacity }}>Hello</div>;
            };
        "#;
        let findings = lint_remotion_source(src);
        assert!(findings.is_empty(), "Expected no findings for clean source");
    }

    #[test]
    fn test_lint_detects_use_state() {
        let src = r#"
            import { useState } from "react";
            const [count, setCount] = useState(0);
        "#;
        let findings = lint_remotion_source(src);
        assert!(findings
            .iter()
            .any(|f| f.rule == "r2hf/use-state" && f.severity == "blocker"));
    }

    #[test]
    fn test_lint_detects_use_effect_with_deps() {
        // Single-line useEffect with non-empty deps — detectable by line-by-line lint
        let src = r#"
            useEffect(() => fetchData(), [id]);
        "#;
        let findings = lint_remotion_source(src);
        assert!(findings
            .iter()
            .any(|f| f.rule == "r2hf/use-effect-deps" && f.severity == "blocker"));
    }

    #[test]
    fn test_lint_allows_use_effect_empty_deps() {
        let src = r#"
            useEffect(() => {
                console.log("mount");
            }, []);
        "#;
        let findings = lint_remotion_source(src);
        assert!(!findings.iter().any(|f| f.rule == "r2hf/use-effect-deps"));
    }

    #[test]
    fn test_lint_detects_third_party_ui() {
        let src = r#"
            import { Button } from "@mui/material";
        "#;
        let findings = lint_remotion_source(src);
        assert!(findings
            .iter()
            .any(|f| f.rule == "r2hf/third-party-react-ui" && f.severity == "blocker"));
    }

    #[test]
    fn test_lint_detects_use_memo_warning() {
        let src = r#"
            const value = useMemo(() => compute(), [deps]);
        "#;
        let findings = lint_remotion_source(src);
        assert!(findings
            .iter()
            .any(|f| f.rule == "r2hf/use-memo" && f.severity == "warning"));
    }
}
