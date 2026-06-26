//! Per-session HEAT: a relative frecency score driven by user-prompt events.
//!
//! Each UserPromptSubmit hook bumps a per-session accumulator (one pre-decayed
//! float `S` plus the timestamp `t_last` of the last bump). On read, `S` is
//! decayed forward to a shared `now`, then the working set is normalized to a
//! continuous ratio-to-max in `[0, 1]` that drives a hot->cold color ramp.
//!
//! Why pre-decayed-float + timestamp rather than an event log: it is O(1) to
//! update and read, survives a daemon restart (it is a tiny sidecar), and the
//! uniform exponential decay cancels in the ratio so the picture freezes (does
//! not equalize) when the user does nothing, e.g. over a weekend.

/// Half-life of the heat accumulator: after this long with no prompts, a
/// session's contribution halves. 8h keeps "today" hot and yesterday cool.
const HALF_LIFE_SECS: f64 = 28_800.0;

/// All-cold mute window: if no session in the set was prompted within this
/// many seconds, every row renders Neutral (the deliberate absolute-time
/// input, so a long-idle board does not paint a misleading gradient). 18h.
const MUTE_AFTER_SECS: i64 = 64_800;

/// The continuous scale only "engages" once the hottest score exceeds roughly
/// this many effective recent prompts. Below it, the lone fresh session reads
/// warm (mid ramp) rather than full hot, matching "a single Monday prompt =
/// mildly warm".
const ENGAGE_S_MAX: f64 = 2.0;

/// Pre-engage ratio for every contributing row while `s_max < ENGAGE_S_MAX`:
/// mid-ramp warm, not full hot.
const PREENGAGE_RATIO: f32 = 0.5;

/// Display-only floor for an active (Running/Waiting) session's ratio so it
/// never reads cold. Does NOT mutate the stored `S`. A Running session is
/// never the coldest, but active sessions are not all forced to one hot color.
const ACTIVE_RATIO_FLOOR: f32 = 0.45;

/// One prompt's worth of heat is clamped to this range by burst saturation:
/// a fast burst of prompts in one task contributes near the floor, a session
/// returned-to across the day contributes the full amount.
const BURST_INC_MIN: f64 = 0.2;
const BURST_INC_MAX: f64 = 1.0;

/// Seconds-since-last-prompt that maps to the full burst increment. Prompts
/// closer together than this contribute proportionally less (down to the
/// floor) so a burst cannot out-rank a repeatedly-returned-to session.
const BURST_FULL_GAP_SECS: f64 = 60.0;

/// The pre-decayed heat accumulator persisted per session.
///
/// `s` is the decayed-to-`t_last` score; `t_last` is the epoch-seconds
/// timestamp of the last bump (0 means "never prompted").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeatAccumulator {
    pub s: f64,
    pub t_last: i64,
}

impl Default for HeatAccumulator {
    fn default() -> Self {
        HeatAccumulator { s: 0.0, t_last: 0 }
    }
}

/// The per-poll cached heat state on a row. `Neutral` means "paint
/// `theme.dimmed`" (today's look): never prompted, heat disabled, or the
/// all-cold mute is engaged. `Ramp(ratio)` carries a ratio in `[0, 1]` for the
/// continuous hot->cold color lookup.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum HeatLevel {
    #[default]
    Neutral,
    Ramp(f32),
}

/// Burst-saturated increment for a single prompt given the gap since the last
/// prompt. A long gap (a session returned-to) earns the full increment; a
/// rapid burst earns near the floor.
fn burst_inc(gap_secs: f64) -> f64 {
    (gap_secs / BURST_FULL_GAP_SECS).clamp(BURST_INC_MIN, BURST_INC_MAX)
}

/// Decay factor for elapsing `dt` seconds. Negative `dt` (clock skew between
/// the writer's `t_last` and the reader's `now`) is clamped to 0 so the score
/// can never grow on read.
fn decay_factor(dt_secs: f64) -> f64 {
    let dt = dt_secs.max(0.0);
    0.5f64.powf(dt / HALF_LIFE_SECS)
}

impl HeatAccumulator {
    /// Apply one prompt at `now`: decay the stored score forward to `now`,
    /// then add the burst-saturated increment. The first prompt ever
    /// (`t_last == 0`) adds the full increment rather than a 1970-epoch gap.
    pub fn bump(&mut self, now: i64) {
        let inc = if self.t_last == 0 {
            BURST_INC_MAX
        } else {
            let gap = (now - self.t_last) as f64;
            burst_inc(gap)
        };
        let decayed = self.s * decay_factor((now - self.t_last) as f64);
        self.s = decayed + inc;
        self.t_last = now;
    }

    /// The decayed score at `now` without mutating the accumulator. Returns 0
    /// for a never-prompted or empty accumulator.
    pub fn s_at(&self, now: i64) -> f64 {
        if self.s <= 0.0 || self.t_last == 0 {
            return 0.0;
        }
        self.s * decay_factor((now - self.t_last) as f64)
    }
}

/// Normalize a working set of `(decayed_score, is_active)` rows to per-row
/// [`HeatLevel`]s.
///
/// `freshest_t_last` is the maximum `t_last` across the set (0 if none ever
/// prompted); `now` is the shared decay timestamp. The all-cold mute keys off
/// `freshest_t_last` (the one absolute-time input). Pure: no clock reads, no
/// I/O, so the math is fully testable.
pub fn normalize(rows: &[(f64, bool)], freshest_t_last: i64, now: i64) -> Vec<HeatLevel> {
    // All-cold mute: nothing prompted recently enough, or never prompted.
    if freshest_t_last == 0 || now - freshest_t_last > MUTE_AFTER_SECS {
        return vec![HeatLevel::Neutral; rows.len()];
    }

    let s_max = rows.iter().fold(0.0f64, |acc, (s, _)| acc.max(*s));
    if s_max <= 0.0 {
        return vec![HeatLevel::Neutral; rows.len()];
    }

    // Pre-engage: a single fresh session (s_max below the engage threshold)
    // reads warm, not full hot. Every contributing row shares the warm ratio,
    // still floored for active rows below.
    let engaged = s_max >= ENGAGE_S_MAX;

    rows.iter()
        .map(|(s, active)| {
            if *s <= 0.0 {
                // A row with no heat reads Neutral so it matches the
                // never-prompted look, even when it is currently active: the
                // active floor must not manufacture heat for a session that
                // never fired the prompt hook (zero-prompt => Neutral). The
                // floor below only lifts sessions that already have heat.
                return HeatLevel::Neutral;
            }
            let base = if engaged {
                (*s / s_max) as f32
            } else {
                PREENGAGE_RATIO
            };
            let floored = if *active {
                base.max(ACTIVE_RATIO_FLOOR)
            } else {
                base
            };
            HeatLevel::Ramp(floored.clamp(0.0, 1.0))
        })
        .collect()
}

/// Parse a persisted accumulator from its sidecar bytes (`"<s> <t_last>"`).
/// Tolerant: returns `None` on any malformed content so the caller can fall
/// back to the default and re-bump.
pub fn parse_accumulator(bytes: &[u8]) -> Option<HeatAccumulator> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut parts = text.split_whitespace();
    let s: f64 = parts.next()?.parse().ok()?;
    let t_last: i64 = parts.next()?.parse().ok()?;
    if !s.is_finite() {
        return None;
    }
    Some(HeatAccumulator { s, t_last })
}

/// Serialize an accumulator for its sidecar (`"<s> <t_last>"`).
pub fn format_accumulator(acc: &HeatAccumulator) -> String {
    format!("{} {}", acc.s, acc.t_last)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HL: i64 = HALF_LIFE_SECS as i64;

    #[test]
    fn decay_factor_at_key_points() {
        assert!((decay_factor(0.0) - 1.0).abs() < 1e-9);
        assert!((decay_factor(HALF_LIFE_SECS) - 0.5).abs() < 1e-9);
        assert!((decay_factor(2.0 * HALF_LIFE_SECS) - 0.25).abs() < 1e-9);
        // Huge dt decays toward zero.
        assert!(decay_factor(100.0 * HALF_LIFE_SECS) < 1e-9);
        // Negative dt (clock skew) is clamped: no growth.
        assert!((decay_factor(-1000.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn burst_inc_clamps() {
        assert!((burst_inc(10.0) - 0.2).abs() < 1e-9, "10s -> floor 0.2");
        assert!((burst_inc(30.0) - 0.5).abs() < 1e-9, "30s -> 0.5");
        assert!((burst_inc(60.0) - 1.0).abs() < 1e-9, "60s -> full 1.0");
        assert!((burst_inc(600.0) - 1.0).abs() < 1e-9, "long gap -> cap 1.0");
    }

    #[test]
    fn first_prompt_adds_full_increment() {
        let mut acc = HeatAccumulator::default();
        acc.bump(1_000_000);
        assert!((acc.s - 1.0).abs() < 1e-9, "first prompt = full inc");
        assert_eq!(acc.t_last, 1_000_000);
    }

    #[test]
    fn s_at_is_zero_for_never_prompted() {
        let acc = HeatAccumulator::default();
        assert_eq!(acc.s_at(1_000_000), 0.0);
    }

    #[test]
    fn s_at_decays_without_mutating() {
        // Use a nonzero base: t_last == 0 is the "never prompted" sentinel, so
        // a real bump never lands there (epoch 0 is 1970).
        let base = 1_000_000i64;
        let mut acc = HeatAccumulator::default();
        acc.bump(base);
        let half = acc.s_at(base + HL);
        assert!((half - 0.5).abs() < 1e-9, "one half-life halves the score");
        // s_at did not mutate the stored value.
        assert!((acc.s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bump_decays_prior_score() {
        let base = 1_000_000i64;
        let mut acc = HeatAccumulator::default();
        acc.bump(base);
        // A second prompt one half-life later: prior 1.0 decays to 0.5, plus a
        // full increment (gap == HALF_LIFE_SECS >> 60s burst window).
        acc.bump(base + HL);
        assert!((acc.s - 1.5).abs() < 1e-9, "0.5 decayed + 1.0 inc = 1.5");
    }

    #[test]
    fn ratio_to_max() {
        // Three sessions, distinct scores, all active=false, fresh enough.
        let now = 1_000_000;
        let rows = vec![(4.0, false), (2.0, false), (1.0, false)];
        let levels = normalize(&rows, now, now);
        match (levels[0], levels[1], levels[2]) {
            (HeatLevel::Ramp(a), HeatLevel::Ramp(b), HeatLevel::Ramp(c)) => {
                assert!((a - 1.0).abs() < 1e-6);
                assert!((b - 0.5).abs() < 1e-6);
                assert!((c - 0.25).abs() < 1e-6);
            }
            other => panic!("expected all Ramp, got {other:?}"),
        }
    }

    #[test]
    fn weekend_proof_ratios_freeze() {
        // Two snapshots N hours apart with no bumps: identical ratios, because
        // uniform decay cancels in the ratio. The all-cold mute keys off
        // freshest_t_last, so keep both snapshots inside the mute window.
        let base = 10_000_000i64;
        let rows_now: Vec<(f64, bool)> = {
            let mut a = HeatAccumulator::default();
            a.bump(base);
            let mut b = HeatAccumulator::default();
            b.bump(base);
            b.bump(base + 30);
            vec![(a.s_at(base + 100), false), (b.s_at(base + 100), false)]
        };
        let later = base + 4 * HL; // 32h later, but freshest still within mute
                                   // Recompute with the same accumulators decayed further.
        let mut a = HeatAccumulator::default();
        a.bump(base);
        let mut b = HeatAccumulator::default();
        b.bump(base);
        b.bump(base + 30);
        // freshest_t_last is base+30; later - that must stay <= MUTE_AFTER_SECS
        // for this test, so cap the "later" delta.
        let later = (base + 30 + MUTE_AFTER_SECS).min(later);
        let rows_later = vec![(a.s_at(later), false), (b.s_at(later), false)];

        let l_now = normalize(&rows_now, base + 30, base + 100);
        let l_later = normalize(&rows_later, base + 30, later);
        let ratio = |lv: HeatLevel| match lv {
            HeatLevel::Ramp(r) => r,
            HeatLevel::Neutral => -1.0,
        };
        // The relative ordering and the ratio of the cold row to the hot row
        // are preserved across the snapshots.
        assert!((ratio(l_now[1]) - ratio(l_later[1])).abs() < 1e-4);
    }

    #[test]
    fn active_clamp_floors_but_keeps_stored_score() {
        // A cold-scored but active session must not read coldest.
        let now = 1_000_000;
        let rows = vec![(10.0, false), (0.01, true), (5.0, false)];
        let levels = normalize(&rows, now, now);
        let r = |lv: HeatLevel| match lv {
            HeatLevel::Ramp(x) => x,
            HeatLevel::Neutral => -1.0,
        };
        assert!(r(levels[1]) >= ACTIVE_RATIO_FLOOR, "active floored");
        // It is distinct from (above) what its raw ratio would have been.
        assert!(r(levels[1]) > (0.01f32 / 10.0));
        // The other rows are unaffected and the hottest is still 1.0.
        assert!((r(levels[0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn active_session_never_the_minimum() {
        let now = 1_000_000;
        let rows = vec![(10.0, false), (0.0001, true), (0.5, false)];
        let levels = normalize(&rows, now, now);
        let r = |lv: HeatLevel| match lv {
            HeatLevel::Ramp(x) => x,
            HeatLevel::Neutral => 0.0,
        };
        // The cold inactive row (0.5/10 = 0.05) must be below the floored
        // active row.
        assert!(r(levels[1]) > r(levels[2]));
    }

    #[test]
    fn all_cold_mute_engaged_just_inside_window() {
        let freshest = 1_000_000i64;
        let now = freshest + MUTE_AFTER_SECS - 60; // 17h59m
        let rows = vec![(3.0, false), (1.0, false)];
        let levels = normalize(&rows, freshest, now);
        assert!(
            matches!(levels[0], HeatLevel::Ramp(_)),
            "engaged inside window"
        );
    }

    #[test]
    fn all_cold_mute_engages_just_outside_window() {
        let freshest = 1_000_000i64;
        let now = freshest + MUTE_AFTER_SECS + 60; // 18h01m
        let rows = vec![(3.0, false), (1.0, false)];
        let levels = normalize(&rows, freshest, now);
        assert!(
            levels.iter().all(|l| *l == HeatLevel::Neutral),
            "muted outside window"
        );
    }

    #[test]
    fn all_cold_mute_when_never_prompted() {
        let rows = vec![(0.0, false), (0.0, false)];
        let levels = normalize(&rows, 0, 1_000_000);
        assert!(levels.iter().all(|l| *l == HeatLevel::Neutral));
    }

    #[test]
    fn single_fresh_prompt_reads_warm_not_hot() {
        // One session, one prompt: s_max = 1.0 < ENGAGE_S_MAX, so it reads the
        // pre-engage warm ratio, not full hot.
        let now = 1_000_000;
        let mut acc = HeatAccumulator::default();
        acc.bump(now);
        let rows = vec![(acc.s_at(now), false)];
        let levels = normalize(&rows, now, now);
        match levels[0] {
            HeatLevel::Ramp(r) => assert!((r - PREENGAGE_RATIO).abs() < 1e-6, "warm, got {r}"),
            HeatLevel::Neutral => panic!("a fresh prompt must not be neutral"),
        }
    }

    #[test]
    fn zero_prompt_row_is_neutral() {
        // A row with no heat among hotter rows reads Neutral (matches the
        // never-prompted look) while the others ramp.
        let now = 1_000_000;
        let rows = vec![(5.0, false), (0.0, false)];
        let levels = normalize(&rows, now, now);
        assert!(matches!(levels[0], HeatLevel::Ramp(_)));
        assert_eq!(levels[1], HeatLevel::Neutral);
    }

    #[test]
    fn zero_prompt_active_row_is_neutral() {
        // A never-prompted session that happens to be Running/Waiting (a tool
        // call, an adopted session before its first prompt, or a pre-feature
        // session with no sidecar) must read Neutral: the active floor only
        // lifts sessions that already have heat, it never manufactures it.
        let now = 1_000_000;
        let rows = vec![(5.0, false), (0.0, true)];
        let levels = normalize(&rows, now, now);
        assert!(matches!(levels[0], HeatLevel::Ramp(_)));
        assert_eq!(
            levels[1],
            HeatLevel::Neutral,
            "active but zero-prompt stays Neutral"
        );
    }

    #[test]
    fn parse_format_round_trip() {
        let acc = HeatAccumulator {
            s: 1.234_5,
            t_last: 1_700_000_000,
        };
        let text = format_accumulator(&acc);
        let back = parse_accumulator(text.as_bytes()).unwrap();
        assert_eq!(back, acc);
    }

    #[test]
    fn parse_tolerates_trailing_whitespace() {
        let back = parse_accumulator(b"2.5 1700000000\n").unwrap();
        assert_eq!(
            back,
            HeatAccumulator {
                s: 2.5,
                t_last: 1_700_000_000
            }
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_accumulator(b"").is_none());
        assert!(parse_accumulator(b"not a number").is_none());
        assert!(parse_accumulator(b"1.0").is_none(), "missing t_last");
        assert!(parse_accumulator(b"nan 5").is_none(), "non-finite rejected");
    }
}
