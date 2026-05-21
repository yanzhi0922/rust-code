//! Cron expression parser and scheduler.
//!
//! Parses standard 5-field cron expressions and computes next run times.
//! Supports step (`*/n`), range (`1-5`), and list (`1,3,5`) syntax.

use anyhow::{Result, anyhow};

// ---------------------------------------------------------------------------
// CronExpression
// ---------------------------------------------------------------------------

/// A parsed 5-field cron expression.
///
/// Fields: minute, hour, day-of-month, month, day-of-week.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpression {
    /// Minute field (0-59).
    pub minute: CronField,
    /// Hour field (0-23).
    pub hour: CronField,
    /// Day of month field (1-31).
    pub day_of_month: CronField,
    /// Month field (1-12).
    pub month: CronField,
    /// Day of week field (0-6, where 0 = Sunday).
    pub day_of_week: CronField,
}

impl std::fmt::Display for CronExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {} {} {}",
            self.minute, self.hour, self.day_of_month, self.month, self.day_of_week
        )
    }
}

// ---------------------------------------------------------------------------
// CronField
// ---------------------------------------------------------------------------

/// A single field in a cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronField {
    /// Match any value (`*`).
    Any,
    /// Match a specific value.
    Value(u32),
    /// Match a range of values (start-end).
    Range(u32, u32),
    /// Match with step (*/n or start-end/n).
    Step {
        /// The base field (Any or Range).
        base: Box<CronField>,
        /// The step value.
        step: u32,
    },
    /// Match a list of values.
    List(Vec<CronField>),
}

impl std::fmt::Display for CronField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Any => write!(f, "*"),
            Self::Value(v) => write!(f, "{v}"),
            Self::Range(start, end) => write!(f, "{start}-{end}"),
            Self::Step { base, step } => write!(f, "{base}/{step}"),
            Self::List(items) => {
                let parts: Vec<String> = items.iter().map(|i| i.to_string()).collect();
                write!(f, "{}", parts.join(","))
            }
        }
    }
}

impl CronField {
    /// Check if a given value matches this field.
    ///
    /// # Arguments
    ///
    /// * `value` — The value to check.
    /// * `min` — The minimum valid value.
    /// * `max` — The maximum valid value.
    ///
    /// # Returns
    ///
    /// `true` if the value matches.
    #[must_use]
    pub fn matches(&self, value: u32, min: u32, max: u32) -> bool {
        match self {
            Self::Any => value >= min && value <= max,
            Self::Value(v) => value == *v,
            Self::Range(start, end) => value >= *start && value <= *end,
            Self::Step { base, step } => {
                if *step == 0 {
                    return false;
                }
                let base_matches = base.matches(value, min, max);
                if base_matches {
                    // Check if value is on the step boundary.
                    let base_start = match **base {
                        Self::Any => min,
                        Self::Range(s, _) => s,
                        Self::Value(s) => s,
                        _ => min,
                    };
                    if value >= base_start {
                        return (value - base_start).is_multiple_of(*step);
                    }
                }
                false
            }
            Self::List(items) => items.iter().any(|i| i.matches(value, min, max)),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a 5-field cron expression.
///
/// # Arguments
///
/// * `expression` — The cron expression string (e.g. `"*/5 * * * *"`).
///
/// # Returns
///
/// The parsed `CronExpression`.
pub fn parse_cron(expression: &str) -> Result<CronExpression> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(anyhow!(
            "Expected 5 fields, got {}: '{}'",
            fields.len(),
            expression
        ));
    }

    Ok(CronExpression {
        minute: parse_field(fields[0], 0, 59)?,
        hour: parse_field(fields[1], 0, 23)?,
        day_of_month: parse_field(fields[2], 1, 31)?,
        month: parse_field(fields[3], 1, 12)?,
        day_of_week: parse_field(fields[4], 0, 6)?,
    })
}

/// Parse a single cron field.
fn parse_field(field: &str, min: u32, max: u32) -> Result<CronField> {
    // Check for list syntax (comma-separated).
    if field.contains(',') {
        let items: Result<Vec<CronField>> = field
            .split(',')
            .map(|part| parse_single_field(part.trim(), min, max))
            .collect();
        return Ok(CronField::List(items?));
    }

    parse_single_field(field, min, max)
}

/// Parse a single cron field value (no commas).
fn parse_single_field(field: &str, min: u32, max: u32) -> Result<CronField> {
    // Check for step syntax.
    if let Some(slash_pos) = field.find('/') {
        let base_str = &field[..slash_pos];
        let step_str = &field[slash_pos + 1..];
        let step: u32 = step_str
            .parse()
            .map_err(|_| anyhow!("Invalid step value: '{step_str}'"))?;
        if step == 0 {
            return Err(anyhow!("Step value cannot be zero"));
        }
        let base = parse_single_field(base_str, min, max)?;
        return Ok(CronField::Step {
            base: Box::new(base),
            step,
        });
    }

    // Check for range syntax.
    if let Some(dash_pos) = field.find('-') {
        let start: u32 = field[..dash_pos]
            .parse()
            .map_err(|_| anyhow!("Invalid range start: '{}'", &field[..dash_pos]))?;
        let end: u32 = field[dash_pos + 1..]
            .parse()
            .map_err(|_| anyhow!("Invalid range end: '{}'", &field[dash_pos + 1..]))?;
        validate_range(start, end, min, max)?;
        return Ok(CronField::Range(start, end));
    }

    // Wildcard.
    if field == "*" {
        return Ok(CronField::Any);
    }

    // Single value.
    let value: u32 = field
        .parse()
        .map_err(|_| anyhow!("Invalid field value: '{field}'"))?;
    if value < min || value > max {
        return Err(anyhow!("Value {value} out of range [{min}, {max}]"));
    }
    Ok(CronField::Value(value))
}

/// Validate that a range is within bounds.
fn validate_range(start: u32, end: u32, min: u32, max: u32) -> Result<()> {
    if start > end {
        return Err(anyhow!("Range start {start} > end {end}"));
    }
    if start < min || end > max {
        return Err(anyhow!(
            "Range [{start}, {end}] out of bounds [{min}, {max}]"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Next run time
// ---------------------------------------------------------------------------

/// Calculate the next run time from a given starting point.
///
/// Uses a simple iterative approach to find the next matching time.
///
/// # Arguments
///
/// * `cron` — The parsed cron expression.
/// * `from_minute` — The starting minute (0-59).
/// * `from_hour` — The starting hour (0-23).
/// * `from_day` — The starting day of month (1-31).
/// * `from_month` — The starting month (1-12).
///
/// # Returns
///
/// The next matching time as `(minute, hour, day, month)`, or `None` if
/// no match is found within the search window.
#[must_use]
pub fn next_run(
    cron: &CronExpression,
    from_minute: u32,
    from_hour: u32,
    from_day: u32,
    from_month: u32,
) -> Option<(u32, u32, u32, u32)> {
    // Start searching from the next minute.
    let mut minute = from_minute + 1;
    let mut hour = from_hour;
    let mut day = from_day;
    let mut month = from_month;

    // Search up to 12 months ahead.
    for _ in 0..525_600 {
        // Max minutes in a year.
        if month > 12 {
            return None;
        }

        if cron.month.matches(month, 1, 12)
            && cron.day_of_month.matches(day, 1, 31)
            && cron.hour.matches(hour, 0, 23)
            && cron.minute.matches(minute, 0, 59)
        {
            return Some((minute, hour, day, month));
        }

        // Advance by one minute.
        minute += 1;
        if minute > 59 {
            minute = 0;
            hour += 1;
            if hour > 23 {
                hour = 0;
                day += 1;
                if day > 31 {
                    day = 1;
                    month += 1;
                }
            }
        }

        // Safety limit.
        if month > from_month + 12 {
            return None;
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_cron ---

    #[test]
    fn parse_every_minute() {
        let cron = parse_cron("* * * * *").expect("parse");
        assert_eq!(cron.minute, CronField::Any);
        assert_eq!(cron.hour, CronField::Any);
    }

    #[test]
    fn parse_specific_time() {
        let cron = parse_cron("30 14 * * *").expect("parse");
        assert_eq!(cron.minute, CronField::Value(30));
        assert_eq!(cron.hour, CronField::Value(14));
    }

    #[test]
    fn parse_step() {
        let cron = parse_cron("*/5 * * * *").expect("parse");
        assert!(matches!(cron.minute, CronField::Step { step: 5, .. }));
    }

    #[test]
    fn parse_range() {
        let cron = parse_cron("1-5 * * * *").expect("parse");
        assert_eq!(cron.minute, CronField::Range(1, 5));
    }

    #[test]
    fn parse_list() {
        let cron = parse_cron("1,15,30 * * * *").expect("parse");
        assert!(matches!(cron.minute, CronField::List(_)));
    }

    #[test]
    fn parse_complex() {
        let cron = parse_cron("0 9-17 * * 1-5").expect("parse");
        assert_eq!(cron.minute, CronField::Value(0));
        assert_eq!(cron.hour, CronField::Range(9, 17));
        assert_eq!(cron.day_of_week, CronField::Range(1, 5));
    }

    #[test]
    fn parse_too_few_fields() {
        assert!(parse_cron("* * * *").is_err());
    }

    #[test]
    fn parse_too_many_fields() {
        assert!(parse_cron("* * * * * *").is_err());
    }

    #[test]
    fn parse_invalid_value() {
        assert!(parse_cron("60 * * * *").is_err());
    }

    #[test]
    fn parse_invalid_step() {
        assert!(parse_cron("*/0 * * * *").is_err());
    }

    #[test]
    fn parse_invalid_range() {
        assert!(parse_cron("5-1 * * * *").is_err());
    }

    // --- CronField::matches ---

    #[test]
    fn field_matches_any() {
        let field = CronField::Any;
        assert!(field.matches(5, 0, 59));
        assert!(field.matches(0, 0, 59));
        assert!(field.matches(59, 0, 59));
    }

    #[test]
    fn field_matches_value() {
        let field = CronField::Value(30);
        assert!(field.matches(30, 0, 59));
        assert!(!field.matches(31, 0, 59));
    }

    #[test]
    fn field_matches_range() {
        let field = CronField::Range(1, 5);
        assert!(field.matches(1, 0, 59));
        assert!(field.matches(3, 0, 59));
        assert!(field.matches(5, 0, 59));
        assert!(!field.matches(0, 0, 59));
        assert!(!field.matches(6, 0, 59));
    }

    #[test]
    fn field_matches_step() {
        let field = CronField::Step {
            base: Box::new(CronField::Any),
            step: 5,
        };
        assert!(field.matches(0, 0, 59));
        assert!(field.matches(5, 0, 59));
        assert!(field.matches(10, 0, 59));
        assert!(!field.matches(3, 0, 59));
    }

    #[test]
    fn field_matches_list() {
        let field = CronField::List(vec![
            CronField::Value(1),
            CronField::Value(15),
            CronField::Value(30),
        ]);
        assert!(field.matches(1, 0, 59));
        assert!(field.matches(15, 0, 59));
        assert!(field.matches(30, 0, 59));
        assert!(!field.matches(10, 0, 59));
    }

    // --- CronField::Display ---

    #[test]
    fn field_display_any() {
        assert_eq!(CronField::Any.to_string(), "*");
    }

    #[test]
    fn field_display_value() {
        assert_eq!(CronField::Value(30).to_string(), "30");
    }

    #[test]
    fn field_display_range() {
        assert_eq!(CronField::Range(1, 5).to_string(), "1-5");
    }

    #[test]
    fn field_display_step() {
        let field = CronField::Step {
            base: Box::new(CronField::Any),
            step: 5,
        };
        assert_eq!(field.to_string(), "*/5");
    }

    // --- CronExpression::Display ---

    #[test]
    fn cron_display() {
        let cron = parse_cron("30 14 * * *").expect("parse");
        assert_eq!(cron.to_string(), "30 14 * * *");
    }

    // --- next_run ---

    #[test]
    fn next_run_every_minute() {
        let cron = parse_cron("* * * * *").expect("parse");
        let result = next_run(&cron, 10, 14, 1, 1);
        assert_eq!(result, Some((11, 14, 1, 1)));
    }

    #[test]
    fn next_run_specific_minute() {
        let cron = parse_cron("0 * * * *").expect("parse");
        let result = next_run(&cron, 30, 14, 1, 1);
        assert_eq!(result, Some((0, 15, 1, 1)));
    }

    #[test]
    fn next_run_specific_hour() {
        let cron = parse_cron("0 9 * * *").expect("parse");
        let result = next_run(&cron, 30, 14, 1, 1);
        assert_eq!(result, Some((0, 9, 2, 1)));
    }

    #[test]
    fn next_run_step() {
        let cron = parse_cron("*/15 * * * *").expect("parse");
        let result = next_run(&cron, 10, 14, 1, 1);
        assert_eq!(result, Some((15, 14, 1, 1)));
    }

    #[test]
    fn next_run_at_same_time() {
        let cron = parse_cron("30 14 * * *").expect("parse");
        // At 14:30, next should be 14:30 tomorrow.
        let result = next_run(&cron, 30, 14, 1, 1);
        assert_eq!(result, Some((30, 14, 2, 1)));
    }
}
