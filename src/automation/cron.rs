use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use croner::Cron;

/// Parse a 5-field cron expression. Rejects empty/invalid input.
pub fn parse(expr: &str) -> Result<Cron> {
    Cron::new(expr)
        .parse()
        .with_context(|| format!("invalid cron expression: {expr}"))
}

/// Next occurrence strictly after `after`, computed in the user's local
/// timezone (matching Claude Code's "9am means 9am wherever you are"), then
/// returned as UTC for uniform storage.
pub fn next_fire_after(expr: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let cron = parse(expr)?;
    let local_after: DateTime<Local> = after.with_timezone(&Local);
    let next_local = cron
        .find_next_occurrence(&local_after, false)
        .with_context(|| format!("no future occurrence for cron: {expr}"))?;
    Ok(next_local.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike, Utc};

    #[test]
    fn rejects_garbage_expression() {
        assert!(parse("not a cron").is_err());
    }

    #[test]
    fn every_30_minutes_advances_to_next_half_hour_boundary() {
        // 12:05:00 UTC baseline. With a local tz offset the wall-clock minute
        // still lands on a :00 or :30 boundary, so assert on minute modulo.
        let after = Utc.with_ymd_and_hms(2026, 6, 22, 12, 5, 0).unwrap();
        let next = next_fire_after("*/30 * * * *", after).unwrap();
        assert!(next > after);
        assert!(matches!(next.naive_utc().time().minute() % 30, 0));
    }

    #[test]
    fn next_fire_is_strictly_after_input() {
        let exact = Utc.with_ymd_and_hms(2026, 6, 22, 12, 0, 0).unwrap();
        let next = next_fire_after("0 * * * *", exact).unwrap();
        assert!(next > exact);
    }
}
