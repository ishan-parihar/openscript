#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub severity: String,
    pub detail: String,
    pub suggestion: String,
}

#[derive(Debug)]
pub struct ValidationResult {
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
    pub estimated_duration_ms: u64,
}

pub fn validate_motion_tsx(tsx_code: &str) -> ValidationResult {
    let mut issues: Vec<ValidationIssue> = Vec::new();

    check_export(tsx_code, &mut issues);
    check_remotion_imports(tsx_code, &mut issues);
    check_component_definition(tsx_code, &mut issues);
    check_jsx_balance(tsx_code, &mut issues);
    check_asset_paths(tsx_code, &mut issues);
    check_sequence_presence(tsx_code, &mut issues);
    check_interpolate_clamping(tsx_code, &mut issues);
    let estimated_duration_ms = estimate_duration(tsx_code);

    let valid = !issues.iter().any(|i| i.severity == "error");

    ValidationResult {
        valid,
        issues,
        estimated_duration_ms,
    }
}

fn check_export(tsx_code: &str, issues: &mut Vec<ValidationIssue>) {
    let has_default_export = tsx_code.contains("export default");
    let has_named_component =
        tsx_code.contains("export function") || tsx_code.contains("export const");

    if !has_default_export && !has_named_component {
        issues.push(ValidationIssue {
            severity: "error".to_string(),
            detail: "No component export found. Remotion requires an exported React component.".to_string(),
            suggestion: "Add `export default function HotMotion(props)` or `export const HotMotion = (props) => ...`".to_string(),
        });
    }
}

fn check_remotion_imports(tsx_code: &str, issues: &mut Vec<ValidationIssue>) {
    let imports_from_remotion =
        tsx_code.contains("from 'remotion'") || tsx_code.contains("from \"remotion\"");

    if !imports_from_remotion {
        issues.push(ValidationIssue {
            severity: "error".to_string(),
            detail: "No imports from 'remotion' found.".to_string(),
            suggestion: "Import at minimum `AbsoluteFill` and `useCurrentFrame` from 'remotion'."
                .to_string(),
        });
        return;
    }

    let has_sequence = tsx_code.contains("Sequence");
    let has_absolute_fill = tsx_code.contains("AbsoluteFill");

    if !has_sequence {
        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            detail: "Sequence not imported from remotion.".to_string(),
            suggestion: "Import Sequence to time-scoped your animations.".to_string(),
        });
    }

    if !has_absolute_fill {
        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            detail: "AbsoluteFill not imported from remotion.".to_string(),
            suggestion: "Import AbsoluteFill as the root canvas container.".to_string(),
        });
    }
}

fn check_component_definition(tsx_code: &str, issues: &mut Vec<ValidationIssue>) {
    let has_function = tsx_code.contains("function ")
        && (tsx_code.contains("HotMotion") || tsx_code.contains("Motion"));
    let has_const_component = tsx_code.contains("const ")
        && tsx_code.contains("=>")
        && (tsx_code.contains("HotMotion") || tsx_code.contains("Motion"));

    if !has_function && !has_const_component {
        issues.push(ValidationIssue {
            severity: "error".to_string(),
            detail: "No React component definition found.".to_string(),
            suggestion: "Define a component: `export default function HotMotion(props) {{ ... }}`"
                .to_string(),
        });
    }
}

fn check_jsx_balance(tsx_code: &str, issues: &mut Vec<ValidationIssue>) {
    let open_angle = tsx_code.matches('<').count();
    let close_angle = tsx_code.matches('>').count();

    let self_closing = tsx_code.matches("/>").count();
    let opening_tags = open_angle.saturating_sub(self_closing);

    if opening_tags.abs_diff(close_angle) > 3 {
        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            detail: format!(
                "Possible JSX imbalance: {} opening brackets vs {} closing brackets.",
                open_angle, close_angle
            ),
            suggestion: "Check for unclosed JSX tags or missing closing tags.".to_string(),
        });
    }
}

fn check_asset_paths(tsx_code: &str, issues: &mut Vec<ValidationIssue>) {
    for cap in find_asset_srcs(tsx_code) {
        if !cap.starts_with('/') && !cap.starts_with("./") && !cap.starts_with("../") {
            issues.push(ValidationIssue {
                severity: "warning".to_string(),
                detail: format!("Asset path \"{cap}\" does not start with /, ./, or ../."),
                suggestion: "Use absolute paths (/path/to/asset) or relative paths (./asset.png) for Remotion.".to_string(),
            });
        }
    }
}

fn find_asset_srcs(tsx_code: &str) -> Vec<String> {
    let mut results = Vec::new();
    let tags = ["Img", "Video", "OffthreadVideo", "Audio"];
    let quotes = ['"', '\''];

    for tag in &tags {
        for quote in &quotes {
            let open = format!("<{tag} src={quote}");
            let mut pos = 0;
            while let Some(start) = tsx_code[pos..].find(&open) {
                let content_start = pos + start + open.len();
                if let Some(end) = tsx_code[content_start..].find(*quote) {
                    let value = tsx_code[content_start..content_start + end].to_string();
                    if !value.is_empty() && !value.starts_with('{') {
                        results.push(value);
                    }
                    pos = content_start + end + 1;
                } else {
                    break;
                }
            }
        }
    }

    results
}

fn check_sequence_presence(tsx_code: &str, issues: &mut Vec<ValidationIssue>) {
    let has_sequence_tag = tsx_code.contains("<Sequence");

    if !has_sequence_tag {
        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            detail: "No <Sequence> components found. The render will be completely static.".to_string(),
            suggestion: "Wrap animated elements in <Sequence from={{frame}} durationInFrames={{frames}}> to add timing.".to_string(),
        });
    }
}

fn check_interpolate_clamping(tsx_code: &str, issues: &mut Vec<ValidationIssue>) {
    if !tsx_code.contains("interpolate(") {
        return;
    }

    let interpolate_calls = tsx_code.matches("interpolate(").count();
    let clamped_calls = tsx_code
        .matches("extrapolateRight")
        .filter(|_| true)
        .count();

    if interpolate_calls > 0 && clamped_calls == 0 {
        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            detail: format!(
                "{interpolate_calls} interpolate() call(s) found but none use extrapolateRight: 'clamp'."
            ),
            suggestion: "Add {{ extrapolateRight: 'clamp' }} to prevent values from going out of range when the frame extends beyond the input range.".to_string(),
        });
    }
}

fn estimate_duration(tsx_code: &str) -> u64 {
    let mut max_end_frame: u32 = 0;

    let mut pos = 0;
    while let Some(seq_start) = tsx_code[pos..].find("durationInFrames") {
        let attr_pos = pos + seq_start;
        let after_attr = &tsx_code[attr_pos..];

        if let Some(eq_pos) = after_attr.find('=') {
            let after_eq = after_attr[eq_pos + 1..].trim_start();

            if let Some(first_char) = after_eq.chars().next() {
                let is_braced = first_char == '{';

                if is_braced {
                    if let Some(close_brace) = after_eq.find('}') {
                        let num_str = after_eq[1..close_brace].trim();
                        if let Ok(val) = num_str.parse::<u32>() {
                            let from_frame = extract_from_frame(tsx_code, attr_pos);
                            let end = from_frame + val;
                            if end > max_end_frame {
                                max_end_frame = end;
                            }
                        }
                    }
                } else {
                    let end = after_eq
                        .find(|c: char| !c.is_ascii_digit())
                        .unwrap_or(after_eq.len());
                    if end > 0 {
                        if let Ok(val) = after_eq[..end].parse::<u32>() {
                            let from_frame = extract_from_frame(tsx_code, attr_pos);
                            let end_frame = from_frame + val;
                            if end_frame > max_end_frame {
                                max_end_frame = end_frame;
                            }
                        }
                    }
                }
            }
        }

        pos = attr_pos + 1;
    }

    if max_end_frame == 0 {
        return 0;
    }

    ((max_end_frame as f64) / 30.0 * 1000.0) as u64
}

fn extract_from_frame(tsx_code: &str, attr_pos: usize) -> u32 {
    let search_start = attr_pos.saturating_sub(500);
    let context = &tsx_code[search_start..attr_pos];

    for from_match in context.rmatch_indices("from=") {
        let match_start = from_match.0;
        let after_from = &context[match_start + 5..];
        let trimmed = after_from.trim_start();

        if trimmed.starts_with('{') {
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

    0
}
