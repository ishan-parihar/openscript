//! V2V visual-layer alternation planner (docs/V2V_ALTERNATION_ARCHITECTURE.md).
//!
//! The renderer (`render_from_timeline`) already composites `[broll → video →
//! broll]`: the ORIGINAL source video is the base input and b-roll events are
//! full-frame cutaway overlays — a segment with no b-roll event shows the
//! original footage. This module decides WHICH segments get b-roll ("broll"
//! role) vs. which show the original video ("source" role), segregated by the
//! transcript segmentation.
//!
//! Pure logic — no I/O — so every pattern/phase/ratio is unit-testable and
//! deterministic.

use std::collections::HashMap;

use crate::timeline::Segment;

/// Role constants stored in `Directives::presentation.visual_roles`.
pub const ROLE_BROLL: &str = "broll";
pub const ROLE_SOURCE: &str = "source";

/// Supported alternation patterns.
pub const PATTERN_EVERY_OTHER: &str = "every_other";
pub const PATTERN_BROLL_LEAD: &str = "broll_lead";
pub const PATTERN_SOURCE_LEAD: &str = "source_lead";
pub const PATTERN_EVERY_N: &str = "every_n";

/// Assign every segment a visual role ("broll" | "source") per the pattern.
///
/// Rules:
/// - `every_other` (alias of `broll_lead` with `every_n = 2`): segment 0 →
///   broll, segment 1 → source, alternating — `[broll → source → broll → …]`.
/// - `broll_lead`: same as every_other (first segment is b-roll).
/// - `source_lead`: phase-shifted — segment 0 → source, then alternating.
/// - `every_n`: `n` consecutive broll segments, then 1 source, repeating.
/// - `broll_ratio` (0.0–1.0) overrides the cadence: it is the share of
///   segments assigned "broll", spread evenly across the sequence. 0.0 =
///   all-source (pure captioned original footage), 1.0 = all-broll (today's
///   full-coverage behaviour). When `broll_ratio` is `None`, the cadence
///   pattern determines roles.
///
/// Non-redundancy: roles alternate structurally, so the fetcher's existing
/// distinct-clip cycling (`fresh_candidates`) prevents adjacent broll segments
/// from sharing footage — this planner never places two broll roles adjacent
/// in `every_other` cadences.
pub fn plan_alternation(
    segments: &[Segment],
    pattern: &str,
    every_n: u32,
    broll_ratio: Option<f64>,
) -> HashMap<String, String> {
    let mut roles = HashMap::with_capacity(segments.len());
    if segments.is_empty() {
        return roles;
    }

    // Explicit ratio overrides the cadence pattern entirely.
    if let Some(r) = broll_ratio {
        let ratio = r.clamp(0.0, 1.0);
        let total = segments.len();
        let n_broll = (total as f64 * ratio).round() as usize;
        // Spread broll roles evenly: a segment is broll when its index is a
        // multiple of the spacing (guarantees no long source runs at any ratio).
        let spacing = if n_broll == 0 {
            usize::MAX
        } else {
            (total as f64 / n_broll as f64).ceil().max(1.0) as usize
        };
        for (i, seg) in segments.iter().enumerate() {
            let role = if n_broll > 0 && i % spacing < 1 {
                ROLE_BROLL
            } else {
                ROLE_SOURCE
            };
            roles.insert(seg.id.clone(), role.to_string());
        }
        return roles;
    }

    // Cadence: for the fixed alternation patterns (every_other / broll_lead /
    // source_lead) the visual cycle is exactly [broll, source] — 1 broll then
    // 1 source — regardless of any every_n value passed. For `every_n` the
    // cycle is [n × broll, 1 × source]. `source_lead` phase-shifts the cycle
    // so the FIRST segment shows the original video.
    let (n_broll, cycle_len) = if pattern == PATTERN_EVERY_N {
        let n = every_n.max(1) as usize;
        (n, n + 1)
    } else {
        (1, 2)
    };
    let phase_shift = if pattern == PATTERN_SOURCE_LEAD { 1 } else { 0 };
    for (i, seg) in segments.iter().enumerate() {
        let pos_in_cycle = (i + phase_shift) % cycle_len;
        let role = if pos_in_cycle < n_broll { ROLE_BROLL } else { ROLE_SOURCE };
        roles.insert(seg.id.clone(), role.to_string());
    }
    roles
}

/// Convenience: role for a single segment id via the plan output.
pub fn role_of(roles: &HashMap<String, String>, segment_id: &str) -> String {
    roles
        .get(segment_id)
        .cloned()
        .unwrap_or_else(|| ROLE_BROLL.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(n: usize) -> Vec<Segment> {
        (0..n)
            .map(|i| Segment {
                id: format!("seg_{:03}", i + 1),
                start: i as f64,
                end: i as f64 + 1.0,
                caption: format!("caption {}", i + 1),
                crossfade_ms: 80,
                semantic_role: None,
            })
            .collect()
    }

    fn role_seq(roles: &HashMap<String, String>, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| roles.get(&format!("seg_{:03}", i + 1)).cloned().unwrap_or_default())
            .collect()
    }

    #[test]
    fn every_other_alternates_broll_source() {
        let roles = plan_alternation(&segs(6), PATTERN_EVERY_OTHER, 2, None);
        assert_eq!(
            role_seq(&roles, 6),
            vec![
                ROLE_BROLL, ROLE_SOURCE, ROLE_BROLL, ROLE_SOURCE, ROLE_BROLL, ROLE_SOURCE
            ]
        );
    }

    #[test]
    fn broll_lead_matches_every_other() {
        let a = plan_alternation(&segs(5), PATTERN_BROLL_LEAD, 2, None);
        let b = plan_alternation(&segs(5), PATTERN_EVERY_OTHER, 2, None);
        assert_eq!(a, b);
    }

    #[test]
    fn source_lead_phase_shifted() {
        let roles = plan_alternation(&segs(4), PATTERN_SOURCE_LEAD, 2, None);
        assert_eq!(
            role_seq(&roles, 4),
            vec![ROLE_SOURCE, ROLE_BROLL, ROLE_SOURCE, ROLE_BROLL]
        );
    }

    #[test]
    fn every_n_three_broll_one_source() {
        let roles = plan_alternation(&segs(8), PATTERN_EVERY_N, 3, None);
        assert_eq!(
            role_seq(&roles, 8),
            vec![
                ROLE_BROLL, ROLE_BROLL, ROLE_BROLL, ROLE_SOURCE,
                ROLE_BROLL, ROLE_BROLL, ROLE_BROLL, ROLE_SOURCE,
            ]
        );
    }

    #[test]
    fn ratio_zero_is_all_source() {
        let roles = plan_alternation(&segs(5), PATTERN_EVERY_OTHER, 2, Some(0.0));
        assert_eq!(role_seq(&roles, 5), vec![ROLE_SOURCE; 5]);
    }

    #[test]
    fn ratio_one_is_all_broll() {
        let roles = plan_alternation(&segs(5), PATTERN_EVERY_OTHER, 2, Some(1.0));
        assert_eq!(role_seq(&roles, 5), vec![ROLE_BROLL; 5]);
    }

    #[test]
    fn ratio_half_spreads_evenly() {
        let roles = plan_alternation(&segs(6), PATTERN_EVERY_OTHER, 2, Some(0.5));
        // 3 broll roles spread with spacing ceil(6/3)=2 → indices 0,2,4.
        assert_eq!(
            role_seq(&roles, 6),
            vec![ROLE_BROLL, ROLE_SOURCE, ROLE_BROLL, ROLE_SOURCE, ROLE_BROLL, ROLE_SOURCE]
        );
    }

    #[test]
    fn ratio_quarter_spreads() {
        let roles = plan_alternation(&segs(8), PATTERN_EVERY_OTHER, 2, Some(0.25));
        // 2 broll roles with spacing ceil(8/2)=4 → indices 0,4.
        let seq = role_seq(&roles, 8);
        assert_eq!(seq[0], ROLE_BROLL);
        assert_eq!(seq[4], ROLE_BROLL);
        assert_eq!(seq.iter().filter(|r| *r == ROLE_BROLL).count(), 2);
    }

    #[test]
    fn empty_segments_no_roles() {
        let roles = plan_alternation(&[], PATTERN_EVERY_OTHER, 2, None);
        assert!(roles.is_empty());
    }

    #[test]
    fn single_segment_is_broll_by_default() {
        let roles = plan_alternation(&segs(1), PATTERN_EVERY_OTHER, 2, None);
        assert_eq!(role_seq(&roles, 1), vec![ROLE_BROLL]);
    }

    #[test]
    fn role_of_defaults_to_broll() {
        let roles = HashMap::new();
        assert_eq!(role_of(&roles, "missing"), ROLE_BROLL);
    }

    #[test]
    fn ratio_out_of_bounds_clamped() {
        let roles = plan_alternation(&segs(4), PATTERN_EVERY_OTHER, 2, Some(7.0));
        assert_eq!(role_seq(&roles, 4), vec![ROLE_BROLL; 4]);
        let roles2 = plan_alternation(&segs(4), PATTERN_EVERY_OTHER, 2, Some(-2.0));
        assert_eq!(role_seq(&roles2, 4), vec![ROLE_SOURCE; 4]);
    }
}
