//! Date/time operations skill for the Argentor agent framework.
//!
//! Provides a comprehensive set of date and time operations using the `chrono`
//! crate. Inspired by the Semantic Kernel `TimePlugin` and the built-in "time"
//! skill concept.
//!
//! # Supported operations
//!
//! - `now` — Current datetime in ISO 8601 (optional timezone).
//! - `parse` — Parse a datetime string (tries ISO 8601, RFC 2822, common formats).
//! - `format` — Format a datetime with a strftime pattern.
//! - `add` — Add a duration to a datetime.
//! - `subtract` — Subtract a duration from a datetime.
//! - `diff` — Difference between two datetimes in a given unit.
//! - `unix_timestamp` — Convert a datetime to a unix timestamp.
//! - `from_timestamp` — Convert a unix timestamp to ISO 8601.
//! - `day_of_week` — Get the day of the week name.
//! - `is_weekend` — Check if a datetime falls on Saturday or Sunday.
//! - `is_leap_year` — Check if a year is a leap year.
//! - `days_in_month` — Get the number of days in a given month/year.
//! - `start_of` — Get the start of a day/week/month/year.
//! - `end_of` — Get the end of a day/week/month/year.

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::skill::{Skill, SkillDescriptor};
use async_trait::async_trait;
use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike, Utc, Weekday,
};

/// Skill that performs date/time operations.
pub struct DateTimeSkill {
    descriptor: SkillDescriptor,
}

impl DateTimeSkill {
    /// Create a new `DateTimeSkill`.
    pub fn new() -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: "datetime".to_string(),
                description: "Perform date/time operations: now, parse, format, add, subtract, \
                              diff, unix_timestamp, from_timestamp, day_of_week, is_weekend, \
                              is_leap_year, days_in_month, start_of, end_of."
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": [
                                "now", "parse", "format", "add", "subtract", "diff",
                                "unix_timestamp", "from_timestamp", "day_of_week",
                                "is_weekend", "is_leap_year", "days_in_month",
                                "start_of", "end_of"
                            ],
                            "description": "The datetime operation to perform"
                        },
                        "datetime": {
                            "type": "string",
                            "description": "ISO 8601 datetime string"
                        },
                        "input": {
                            "type": "string",
                            "description": "Input string to parse (for parse operation)"
                        },
                        "format": {
                            "type": "string",
                            "description": "strftime format pattern (for format/parse)"
                        },
                        "amount": {
                            "type": "integer",
                            "description": "Amount for add/subtract"
                        },
                        "unit": {
                            "type": "string",
                            "enum": [
                                "seconds", "minutes", "hours", "days", "weeks",
                                "months", "years"
                            ],
                            "description": "Time unit for add/subtract/diff/start_of/end_of"
                        },
                        "from": {
                            "type": "string",
                            "description": "Start datetime for diff operation"
                        },
                        "to": {
                            "type": "string",
                            "description": "End datetime for diff operation"
                        },
                        "timestamp": {
                            "type": "number",
                            "description": "Unix timestamp (seconds)"
                        },
                        "year": {
                            "type": "integer",
                            "description": "Year (for is_leap_year/days_in_month)"
                        },
                        "month": {
                            "type": "integer",
                            "description": "Month 1-12 (for days_in_month)"
                        },
                        "timezone": {
                            "type": "string",
                            "description": "Timezone offset like +05:30 or -08:00 or Z"
                        }
                    },
                    "required": ["operation"]
                }),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

impl Default for DateTimeSkill {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a timezone string like "+05:30", "-08:00", "Z", "UTC" into a FixedOffset.
fn parse_timezone(tz: &str) -> Result<FixedOffset, String> {
    let tz = tz.trim();
    if tz.eq_ignore_ascii_case("Z") || tz.eq_ignore_ascii_case("UTC") {
        return FixedOffset::east_opt(0).ok_or_else(|| "UTC offset 0 is invalid".to_string());
    }

    // Parse +HH:MM or -HH:MM
    if (tz.starts_with('+') || tz.starts_with('-')) && tz.len() >= 5 {
        let sign = if tz.starts_with('+') { 1 } else { -1 };
        let parts: Vec<&str> = tz[1..].split(':').collect();
        if parts.len() == 2 {
            let hours: i32 = parts[0]
                .parse()
                .map_err(|_| format!("Invalid timezone hours: {}", parts[0]))?;
            let minutes: i32 = parts[1]
                .parse()
                .map_err(|_| format!("Invalid timezone minutes: {}", parts[1]))?;
            let total_seconds = sign * (hours * 3600 + minutes * 60);
            return FixedOffset::east_opt(total_seconds)
                .ok_or_else(|| format!("Timezone offset out of range: {tz}"));
        }
    }

    Err(format!(
        "Invalid timezone format '{tz}'. Use +HH:MM, -HH:MM, Z, or UTC"
    ))
}

/// Try to parse a datetime string using multiple common formats.
/// Returns a `DateTime<FixedOffset>` on success.
fn try_parse_datetime(input: &str) -> Result<DateTime<FixedOffset>, String> {
    // Try ISO 8601 / RFC 3339 with timezone
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt);
    }

    // Try RFC 2822 (e.g. "Tue, 01 Jan 2024 12:00:00 +0000")
    if let Ok(dt) = DateTime::parse_from_rfc2822(input) {
        return Ok(dt);
    }

    // Try ISO 8601 with space instead of T
    if let Ok(dt) = DateTime::parse_from_str(input, "%Y-%m-%d %H:%M:%S%z") {
        return Ok(dt);
    }
    if let Ok(dt) = DateTime::parse_from_str(input, "%Y-%m-%d %H:%M:%S%.f%z") {
        return Ok(dt);
    }

    // Try without timezone (assume UTC)
    let formats = &[
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y",
    ];

    for fmt in formats {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(input, fmt) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc).fixed_offset());
        }
        // Try as date-only
        if let Ok(nd) = NaiveDate::parse_from_str(input, fmt) {
            let ndt = nd.and_hms_opt(0, 0, 0).ok_or("Invalid date")?;
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc).fixed_offset());
        }
    }

    Err(format!(
        "Could not parse datetime '{input}'. Supported formats: ISO 8601, RFC 2822, \
         YYYY-MM-DD, YYYY-MM-DD HH:MM:SS, DD/MM/YYYY, MM/DD/YYYY"
    ))
}

/// Parse the `datetime` argument from a tool call, returning a parsed value.
fn get_datetime(args: &serde_json::Value, key: &str) -> Result<DateTime<FixedOffset>, String> {
    let s = args[key]
        .as_str()
        .ok_or_else(|| format!("Missing or non-string '{key}' parameter"))?;
    try_parse_datetime(s)
}

/// Add months to a date, clamping the day to the max day of the target month.
fn add_months(dt: DateTime<FixedOffset>, months: i32) -> Result<DateTime<FixedOffset>, String> {
    let total_months = dt.year() * 12 + (dt.month() as i32 - 1) + months;
    let new_year = total_months.div_euclid(12);
    let new_month = (total_months.rem_euclid(12) + 1) as u32;

    let max_day = days_in_month_helper(new_year, new_month);
    let new_day = dt.day().min(max_day);

    let new_date = NaiveDate::from_ymd_opt(new_year, new_month, new_day).ok_or_else(|| {
        format!("Invalid date after adding months: {new_year}-{new_month}-{new_day}")
    })?;
    let new_ndt = new_date
        .and_hms_nano_opt(dt.hour(), dt.minute(), dt.second(), dt.nanosecond())
        .ok_or("Invalid time after month arithmetic")?;

    dt.offset()
        .from_local_datetime(&new_ndt)
        .single()
        .ok_or_else(|| "Ambiguous or invalid datetime after month arithmetic".to_string())
}

/// Get the number of days in a given month/year.
fn days_in_month_helper(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Check if a year is a leap year.
fn is_leap_year_helper(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ---------------------------------------------------------------------------
// Operation implementations
// ---------------------------------------------------------------------------

fn op_now(args: &serde_json::Value) -> serde_json::Value {
    let now_utc = Utc::now();

    if let Some(tz_str) = args["timezone"].as_str() {
        match parse_timezone(tz_str) {
            Ok(offset) => {
                let now_tz = now_utc.with_timezone(&offset);
                serde_json::json!({
                    "datetime": now_tz.to_rfc3339(),
                    "timezone": tz_str,
                })
            }
            Err(e) => serde_json::json!({"error": e}),
        }
    } else {
        serde_json::json!({
            "datetime": now_utc.to_rfc3339(),
            "timezone": "UTC",
        })
    }
}

fn op_parse(args: &serde_json::Value) -> serde_json::Value {
    let input = match args["input"].as_str() {
        Some(s) => s,
        None => return serde_json::json!({"error": "Missing 'input' parameter"}),
    };

    // If a custom format is provided, use it
    if let Some(fmt) = args["format"].as_str() {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(input, fmt) {
            let dt = DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc);
            return serde_json::json!({
                "datetime": dt.to_rfc3339(),
                "parsed_with_format": fmt,
            });
        }
        if let Ok(nd) = NaiveDate::parse_from_str(input, fmt) {
            if let Some(ndt) = nd.and_hms_opt(0, 0, 0) {
                let dt = DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc);
                return serde_json::json!({
                    "datetime": dt.to_rfc3339(),
                    "parsed_with_format": fmt,
                });
            }
        }
        return serde_json::json!({
            "error": format!("Could not parse '{input}' with format '{fmt}'"),
        });
    }

    // Auto-detect format
    match try_parse_datetime(input) {
        Ok(dt) => serde_json::json!({
            "datetime": dt.to_rfc3339(),
            "parsed_with_format": "auto-detected",
        }),
        Err(e) => serde_json::json!({"error": e}),
    }
}

fn op_format(args: &serde_json::Value) -> serde_json::Value {
    let dt = match get_datetime(args, "datetime") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let fmt = match args["format"].as_str() {
        Some(f) => f,
        None => return serde_json::json!({"error": "Missing 'format' parameter"}),
    };

    let formatted = dt.format(fmt).to_string();
    serde_json::json!({
        "result": formatted,
        "format": fmt,
    })
}

fn op_add(args: &serde_json::Value) -> serde_json::Value {
    let dt = match get_datetime(args, "datetime") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let amount = args["amount"].as_i64().unwrap_or(0);
    let unit = args["unit"].as_str().unwrap_or("days");

    let result = match unit {
        "seconds" => Some(dt + Duration::seconds(amount)),
        "minutes" => Some(dt + Duration::minutes(amount)),
        "hours" => Some(dt + Duration::hours(amount)),
        "days" => Some(dt + Duration::days(amount)),
        "weeks" => Some(dt + Duration::weeks(amount)),
        "months" => match add_months(dt, amount as i32) {
            Ok(r) => Some(r),
            Err(e) => return serde_json::json!({"error": e}),
        },
        "years" => match add_months(dt, (amount * 12) as i32) {
            Ok(r) => Some(r),
            Err(e) => return serde_json::json!({"error": e}),
        },
        _ => {
            return serde_json::json!({
                "error": format!("Unknown unit '{unit}'. Use: seconds, minutes, hours, days, weeks, months, years"),
            });
        }
    };

    match result {
        Some(r) => serde_json::json!({
            "result": r.to_rfc3339(),
            "original": dt.to_rfc3339(),
            "added": format!("{amount} {unit}"),
        }),
        None => serde_json::json!({"error": "Date arithmetic overflow"}),
    }
}

fn op_subtract(args: &serde_json::Value) -> serde_json::Value {
    let dt = match get_datetime(args, "datetime") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let amount = args["amount"].as_i64().unwrap_or(0);
    let unit = args["unit"].as_str().unwrap_or("days");

    let result = match unit {
        "seconds" => Some(dt - Duration::seconds(amount)),
        "minutes" => Some(dt - Duration::minutes(amount)),
        "hours" => Some(dt - Duration::hours(amount)),
        "days" => Some(dt - Duration::days(amount)),
        "weeks" => Some(dt - Duration::weeks(amount)),
        "months" => match add_months(dt, -(amount as i32)) {
            Ok(r) => Some(r),
            Err(e) => return serde_json::json!({"error": e}),
        },
        "years" => match add_months(dt, -(amount as i32) * 12) {
            Ok(r) => Some(r),
            Err(e) => return serde_json::json!({"error": e}),
        },
        _ => {
            return serde_json::json!({
                "error": format!("Unknown unit '{unit}'. Use: seconds, minutes, hours, days, weeks, months, years"),
            });
        }
    };

    match result {
        Some(r) => serde_json::json!({
            "result": r.to_rfc3339(),
            "original": dt.to_rfc3339(),
            "subtracted": format!("{amount} {unit}"),
        }),
        None => serde_json::json!({"error": "Date arithmetic overflow"}),
    }
}

fn op_diff(args: &serde_json::Value) -> serde_json::Value {
    let from = match get_datetime(args, "from") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };
    let to = match get_datetime(args, "to") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let unit = args["unit"].as_str().unwrap_or("seconds");

    let duration = to.signed_duration_since(from);

    let value: f64 = match unit {
        "seconds" => duration.num_seconds() as f64,
        "minutes" => duration.num_minutes() as f64,
        "hours" => duration.num_hours() as f64,
        "days" => duration.num_days() as f64,
        "weeks" => duration.num_weeks() as f64,
        _ => {
            return serde_json::json!({
                "error": format!("Unknown unit '{unit}'. Use: seconds, minutes, hours, days, weeks"),
            });
        }
    };

    serde_json::json!({
        "difference": value,
        "unit": unit,
        "from": from.to_rfc3339(),
        "to": to.to_rfc3339(),
    })
}

fn op_unix_timestamp(args: &serde_json::Value) -> serde_json::Value {
    if let Some(dt_str) = args["datetime"].as_str() {
        match try_parse_datetime(dt_str) {
            Ok(dt) => serde_json::json!({
                "timestamp": dt.timestamp(),
                "timestamp_millis": dt.timestamp_millis(),
                "datetime": dt.to_rfc3339(),
            }),
            Err(e) => serde_json::json!({"error": e}),
        }
    } else {
        // No datetime provided, return current timestamp
        let now = Utc::now();
        serde_json::json!({
            "timestamp": now.timestamp(),
            "timestamp_millis": now.timestamp_millis(),
            "datetime": now.to_rfc3339(),
        })
    }
}

fn op_from_timestamp(args: &serde_json::Value) -> serde_json::Value {
    let ts = match args["timestamp"].as_f64() {
        Some(t) => t,
        None => {
            return serde_json::json!({
                "error": "Missing or non-numeric 'timestamp' parameter"
            });
        }
    };

    let seconds = ts as i64;
    let nanos = ((ts - seconds as f64) * 1_000_000_000.0) as u32;

    match DateTime::from_timestamp(seconds, nanos) {
        Some(dt) => {
            let utc_dt: DateTime<Utc> = dt;
            serde_json::json!({
                "datetime": utc_dt.to_rfc3339(),
                "timestamp": ts,
            })
        }
        None => serde_json::json!({
            "error": format!("Invalid timestamp: {ts}")
        }),
    }
}

fn op_day_of_week(args: &serde_json::Value) -> serde_json::Value {
    let dt = match get_datetime(args, "datetime") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let day = dt.weekday();
    let day_name = match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    };

    serde_json::json!({
        "day_of_week": day_name,
        "day_number": day.num_days_from_monday() + 1,
        "iso_day_number": day.num_days_from_monday() + 1,
        "datetime": dt.to_rfc3339(),
    })
}

fn op_is_weekend(args: &serde_json::Value) -> serde_json::Value {
    let dt = match get_datetime(args, "datetime") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let is_weekend = matches!(dt.weekday(), Weekday::Sat | Weekday::Sun);

    serde_json::json!({
        "is_weekend": is_weekend,
        "day_of_week": format!("{:?}", dt.weekday()),
        "datetime": dt.to_rfc3339(),
    })
}

fn op_is_leap_year(args: &serde_json::Value) -> serde_json::Value {
    let year = match args["year"].as_i64() {
        Some(y) => y as i32,
        None => {
            return serde_json::json!({
                "error": "Missing or non-integer 'year' parameter"
            });
        }
    };

    serde_json::json!({
        "is_leap_year": is_leap_year_helper(year),
        "year": year,
    })
}

fn op_days_in_month(args: &serde_json::Value) -> serde_json::Value {
    let year = match args["year"].as_i64() {
        Some(y) => y as i32,
        None => {
            return serde_json::json!({
                "error": "Missing or non-integer 'year' parameter"
            });
        }
    };

    let month = match args["month"].as_i64() {
        Some(m) if (1..=12).contains(&m) => m as u32,
        Some(m) => {
            return serde_json::json!({
                "error": format!("Invalid month: {m} (must be 1-12)")
            });
        }
        None => {
            return serde_json::json!({
                "error": "Missing or non-integer 'month' parameter"
            });
        }
    };

    serde_json::json!({
        "days": days_in_month_helper(year, month),
        "year": year,
        "month": month,
    })
}

fn op_start_of(args: &serde_json::Value) -> serde_json::Value {
    let dt = match get_datetime(args, "datetime") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let unit = args["unit"].as_str().unwrap_or("day");
    let offset = *dt.offset();

    let result = match unit {
        "day" => {
            let naive = dt.date_naive().and_hms_opt(0, 0, 0);
            naive.and_then(|n| offset.from_local_datetime(&n).single())
        }
        "week" => {
            // ISO week starts on Monday
            let days_from_monday = dt.weekday().num_days_from_monday();
            let monday = dt.date_naive() - Duration::days(days_from_monday as i64);
            monday
                .and_hms_opt(0, 0, 0)
                .and_then(|n| offset.from_local_datetime(&n).single())
        }
        "month" => NaiveDate::from_ymd_opt(dt.year(), dt.month(), 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|n| offset.from_local_datetime(&n).single()),
        "year" => NaiveDate::from_ymd_opt(dt.year(), 1, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|n| offset.from_local_datetime(&n).single()),
        _ => {
            return serde_json::json!({
                "error": format!("Unknown unit '{unit}'. Use: day, week, month, year"),
            });
        }
    };

    match result {
        Some(r) => serde_json::json!({
            "result": r.to_rfc3339(),
            "original": dt.to_rfc3339(),
            "unit": unit,
        }),
        None => serde_json::json!({"error": "Failed to compute start_of"}),
    }
}

fn op_end_of(args: &serde_json::Value) -> serde_json::Value {
    let dt = match get_datetime(args, "datetime") {
        Ok(dt) => dt,
        Err(e) => return serde_json::json!({"error": e}),
    };

    let unit = args["unit"].as_str().unwrap_or("day");
    let offset = *dt.offset();

    let result = match unit {
        "day" => {
            let naive = dt.date_naive().and_time(
                NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999)
                    .unwrap_or(NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(NaiveTime::MIN)),
            );
            offset.from_local_datetime(&naive).single()
        }
        "week" => {
            // ISO week ends on Sunday
            let days_to_sunday = 6 - dt.weekday().num_days_from_monday();
            let sunday = dt.date_naive() + Duration::days(days_to_sunday as i64);
            let naive = sunday.and_time(
                NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999)
                    .unwrap_or(NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(NaiveTime::MIN)),
            );
            offset.from_local_datetime(&naive).single()
        }
        "month" => {
            let last_day = days_in_month_helper(dt.year(), dt.month());
            NaiveDate::from_ymd_opt(dt.year(), dt.month(), last_day)
                .map(|d| {
                    d.and_time(
                        NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap_or(
                            NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(NaiveTime::MIN),
                        ),
                    )
                })
                .and_then(|n| offset.from_local_datetime(&n).single())
        }
        "year" => NaiveDate::from_ymd_opt(dt.year(), 12, 31)
            .map(|d| {
                d.and_time(
                    NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999)
                        .unwrap_or(NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(NaiveTime::MIN)),
                )
            })
            .and_then(|n| offset.from_local_datetime(&n).single()),
        _ => {
            return serde_json::json!({
                "error": format!("Unknown unit '{unit}'. Use: day, week, month, year"),
            });
        }
    };

    match result {
        Some(r) => serde_json::json!({
            "result": r.to_rfc3339(),
            "original": dt.to_rfc3339(),
            "unit": unit,
        }),
        None => serde_json::json!({"error": "Failed to compute end_of"}),
    }
}

// ---------------------------------------------------------------------------
// Skill implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Skill for DateTimeSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let operation = call.arguments["operation"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if operation.is_empty() {
            return Ok(ToolResult::error(
                &call.id,
                "Operation parameter is required",
            ));
        }

        let result = match operation.as_str() {
            "now" => op_now(&call.arguments),
            "parse" => op_parse(&call.arguments),
            "format" => op_format(&call.arguments),
            "add" => op_add(&call.arguments),
            "subtract" => op_subtract(&call.arguments),
            "diff" => op_diff(&call.arguments),
            "unix_timestamp" => op_unix_timestamp(&call.arguments),
            "from_timestamp" => op_from_timestamp(&call.arguments),
            "day_of_week" => op_day_of_week(&call.arguments),
            "is_weekend" => op_is_weekend(&call.arguments),
            "is_leap_year" => op_is_leap_year(&call.arguments),
            "days_in_month" => op_days_in_month(&call.arguments),
            "start_of" => op_start_of(&call.arguments),
            "end_of" => op_end_of(&call.arguments),
            _ => {
                return Ok(ToolResult::error(
                    &call.id,
                    format!(
                        "Unknown operation '{operation}'. Supported: now, parse, format, add, \
                         subtract, diff, unix_timestamp, from_timestamp, day_of_week, \
                         is_weekend, is_leap_year, days_in_month, start_of, end_of"
                    ),
                ));
            }
        };

        // If the operation returned an error key, report as tool error
        if result.get("error").is_some() {
            return Ok(ToolResult::error(&call.id, result.to_string()));
        }

        Ok(ToolResult::success(&call.id, result.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn skill() -> DateTimeSkill {
        DateTimeSkill::new()
    }

    fn call(op: &str, args: serde_json::Value) -> ToolCall {
        let mut merged = args.clone();
        merged["operation"] = serde_json::json!(op);
        ToolCall {
            id: "t1".to_string(),
            name: "datetime".to_string(),
            arguments: merged,
        }
    }

    // -- Descriptor -----------------------------------------------------------

    #[test]
    fn test_descriptor() {
        let s = skill();
        assert_eq!(s.descriptor().name, "datetime");
        assert!(s.descriptor().required_capabilities.is_empty());
    }

    // -- now ------------------------------------------------------------------

    #[tokio::test]
    async fn test_now_utc() {
        let s = skill();
        let c = call("now", serde_json::json!({}));
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["datetime"].as_str().unwrap().contains("T"));
        assert_eq!(v["timezone"], "UTC");
    }

    #[tokio::test]
    async fn test_now_with_timezone() {
        let s = skill();
        let c = call("now", serde_json::json!({"timezone": "+05:30"}));
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["datetime"].as_str().unwrap().contains("+05:30"));
    }

    #[tokio::test]
    async fn test_now_invalid_timezone() {
        let s = skill();
        let c = call("now", serde_json::json!({"timezone": "invalid"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
    }

    // -- parse ----------------------------------------------------------------

    #[tokio::test]
    async fn test_parse_iso8601() {
        let s = skill();
        let c = call(
            "parse",
            serde_json::json!({"input": "2024-03-15T10:30:00Z"}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["datetime"].as_str().unwrap().contains("2024-03-15"));
    }

    #[tokio::test]
    async fn test_parse_date_only() {
        let s = skill();
        let c = call("parse", serde_json::json!({"input": "2024-03-15"}));
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["datetime"].as_str().unwrap().contains("2024-03-15"));
    }

    #[tokio::test]
    async fn test_parse_with_custom_format() {
        let s = skill();
        let c = call(
            "parse",
            serde_json::json!({"input": "15/03/2024", "format": "%d/%m/%Y"}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["datetime"].as_str().unwrap().contains("2024-03-15"));
    }

    #[tokio::test]
    async fn test_parse_invalid() {
        let s = skill();
        let c = call("parse", serde_json::json!({"input": "not a date"}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
    }

    // -- format ---------------------------------------------------------------

    #[tokio::test]
    async fn test_format() {
        let s = skill();
        let c = call(
            "format",
            serde_json::json!({
                "datetime": "2024-03-15T10:30:00Z",
                "format": "%B %d, %Y"
            }),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["result"], "March 15, 2024");
    }

    #[tokio::test]
    async fn test_format_time_only() {
        let s = skill();
        let c = call(
            "format",
            serde_json::json!({
                "datetime": "2024-03-15T14:30:00Z",
                "format": "%H:%M:%S"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["result"], "14:30:00");
    }

    // -- add ------------------------------------------------------------------

    #[tokio::test]
    async fn test_add_days() {
        let s = skill();
        let c = call(
            "add",
            serde_json::json!({
                "datetime": "2024-03-15T10:00:00Z",
                "amount": 5,
                "unit": "days"
            }),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"].as_str().unwrap().contains("2024-03-20"));
    }

    #[tokio::test]
    async fn test_add_hours() {
        let s = skill();
        let c = call(
            "add",
            serde_json::json!({
                "datetime": "2024-03-15T10:00:00Z",
                "amount": 3,
                "unit": "hours"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"].as_str().unwrap().contains("13:00:00"));
    }

    #[tokio::test]
    async fn test_add_months() {
        let s = skill();
        let c = call(
            "add",
            serde_json::json!({
                "datetime": "2024-01-31T10:00:00Z",
                "amount": 1,
                "unit": "months"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        // Jan 31 + 1 month should clamp to Feb 29 (2024 is a leap year)
        assert!(v["result"].as_str().unwrap().contains("2024-02-29"));
    }

    #[tokio::test]
    async fn test_add_years() {
        let s = skill();
        let c = call(
            "add",
            serde_json::json!({
                "datetime": "2024-02-29T00:00:00Z",
                "amount": 1,
                "unit": "years"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        // Feb 29 + 1 year should clamp to Feb 28 (2025 is not a leap year)
        assert!(v["result"].as_str().unwrap().contains("2025-02-28"));
    }

    // -- subtract -------------------------------------------------------------

    #[tokio::test]
    async fn test_subtract_days() {
        let s = skill();
        let c = call(
            "subtract",
            serde_json::json!({
                "datetime": "2024-03-15T10:00:00Z",
                "amount": 20,
                "unit": "days"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"].as_str().unwrap().contains("2024-02-24"));
    }

    #[tokio::test]
    async fn test_subtract_months() {
        let s = skill();
        let c = call(
            "subtract",
            serde_json::json!({
                "datetime": "2024-03-31T10:00:00Z",
                "amount": 1,
                "unit": "months"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        // Mar 31 - 1 month should clamp to Feb 29
        assert!(v["result"].as_str().unwrap().contains("2024-02-29"));
    }

    // -- diff -----------------------------------------------------------------

    #[tokio::test]
    async fn test_diff_days() {
        let s = skill();
        let c = call(
            "diff",
            serde_json::json!({
                "from": "2024-01-01T00:00:00Z",
                "to": "2024-01-11T00:00:00Z",
                "unit": "days"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["difference"], 10.0);
    }

    #[tokio::test]
    async fn test_diff_hours() {
        let s = skill();
        let c = call(
            "diff",
            serde_json::json!({
                "from": "2024-01-01T00:00:00Z",
                "to": "2024-01-01T06:00:00Z",
                "unit": "hours"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["difference"], 6.0);
    }

    #[tokio::test]
    async fn test_diff_negative() {
        let s = skill();
        let c = call(
            "diff",
            serde_json::json!({
                "from": "2024-01-11T00:00:00Z",
                "to": "2024-01-01T00:00:00Z",
                "unit": "days"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["difference"], -10.0);
    }

    // -- unix_timestamp -------------------------------------------------------

    #[tokio::test]
    async fn test_unix_timestamp() {
        let s = skill();
        let c = call(
            "unix_timestamp",
            serde_json::json!({"datetime": "2024-01-01T00:00:00Z"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["timestamp"], 1704067200);
    }

    #[tokio::test]
    async fn test_unix_timestamp_no_datetime() {
        let s = skill();
        let c = call("unix_timestamp", serde_json::json!({}));
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["timestamp"].as_i64().is_some());
    }

    // -- from_timestamp -------------------------------------------------------

    #[tokio::test]
    async fn test_from_timestamp() {
        let s = skill();
        let c = call(
            "from_timestamp",
            serde_json::json!({"timestamp": 1704067200}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["datetime"].as_str().unwrap().contains("2024-01-01"));
    }

    #[tokio::test]
    async fn test_from_timestamp_float() {
        let s = skill();
        let c = call(
            "from_timestamp",
            serde_json::json!({"timestamp": 1704067200.5}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(!r.is_error);
    }

    #[tokio::test]
    async fn test_from_timestamp_missing() {
        let s = skill();
        let c = call("from_timestamp", serde_json::json!({}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
    }

    // -- day_of_week ----------------------------------------------------------

    #[tokio::test]
    async fn test_day_of_week() {
        let s = skill();
        // 2024-03-15 is a Friday
        let c = call(
            "day_of_week",
            serde_json::json!({"datetime": "2024-03-15T10:00:00Z"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["day_of_week"], "Friday");
        assert_eq!(v["day_number"], 5);
    }

    #[tokio::test]
    async fn test_day_of_week_monday() {
        let s = skill();
        // 2024-03-11 is a Monday
        let c = call(
            "day_of_week",
            serde_json::json!({"datetime": "2024-03-11T00:00:00Z"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["day_of_week"], "Monday");
        assert_eq!(v["day_number"], 1);
    }

    // -- is_weekend -----------------------------------------------------------

    #[tokio::test]
    async fn test_is_weekend_saturday() {
        let s = skill();
        // 2024-03-16 is a Saturday
        let c = call(
            "is_weekend",
            serde_json::json!({"datetime": "2024-03-16T10:00:00Z"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["is_weekend"], true);
    }

    #[tokio::test]
    async fn test_is_weekend_weekday() {
        let s = skill();
        // 2024-03-15 is a Friday
        let c = call(
            "is_weekend",
            serde_json::json!({"datetime": "2024-03-15T10:00:00Z"}),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["is_weekend"], false);
    }

    // -- is_leap_year ---------------------------------------------------------

    #[tokio::test]
    async fn test_is_leap_year() {
        let s = skill();
        for (year, expected) in &[(2024, true), (2023, false), (2000, true), (1900, false)] {
            let c = call("is_leap_year", serde_json::json!({"year": year}));
            let r = s.execute(c).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
            assert_eq!(
                v["is_leap_year"], *expected,
                "Year {year} should be leap={expected}"
            );
        }
    }

    #[tokio::test]
    async fn test_is_leap_year_missing() {
        let s = skill();
        let c = call("is_leap_year", serde_json::json!({}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
    }

    // -- days_in_month --------------------------------------------------------

    #[tokio::test]
    async fn test_days_in_month() {
        let s = skill();
        let cases = vec![
            (2024, 1, 31),
            (2024, 2, 29), // leap year
            (2023, 2, 28), // non-leap
            (2024, 4, 30),
            (2024, 12, 31),
        ];
        for (year, month, expected) in cases {
            let c = call(
                "days_in_month",
                serde_json::json!({"year": year, "month": month}),
            );
            let r = s.execute(c).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
            assert_eq!(
                v["days"], expected,
                "Expected {expected} days for {year}-{month:02}"
            );
        }
    }

    #[tokio::test]
    async fn test_days_in_month_invalid() {
        let s = skill();
        let c = call(
            "days_in_month",
            serde_json::json!({"year": 2024, "month": 13}),
        );
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
    }

    // -- start_of -------------------------------------------------------------

    #[tokio::test]
    async fn test_start_of_day() {
        let s = skill();
        let c = call(
            "start_of",
            serde_json::json!({
                "datetime": "2024-03-15T14:30:45Z",
                "unit": "day"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-03-15T00:00:00"));
    }

    #[tokio::test]
    async fn test_start_of_month() {
        let s = skill();
        let c = call(
            "start_of",
            serde_json::json!({
                "datetime": "2024-03-15T14:30:45Z",
                "unit": "month"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-03-01T00:00:00"));
    }

    #[tokio::test]
    async fn test_start_of_year() {
        let s = skill();
        let c = call(
            "start_of",
            serde_json::json!({
                "datetime": "2024-07-20T14:30:45Z",
                "unit": "year"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-01-01T00:00:00"));
    }

    #[tokio::test]
    async fn test_start_of_week() {
        let s = skill();
        // 2024-03-15 is Friday; Monday of that week is 2024-03-11
        let c = call(
            "start_of",
            serde_json::json!({
                "datetime": "2024-03-15T14:30:45Z",
                "unit": "week"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-03-11T00:00:00"));
    }

    // -- end_of ---------------------------------------------------------------

    #[tokio::test]
    async fn test_end_of_day() {
        let s = skill();
        let c = call(
            "end_of",
            serde_json::json!({
                "datetime": "2024-03-15T10:00:00Z",
                "unit": "day"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-03-15T23:59:59"));
    }

    #[tokio::test]
    async fn test_end_of_month() {
        let s = skill();
        let c = call(
            "end_of",
            serde_json::json!({
                "datetime": "2024-02-15T10:00:00Z",
                "unit": "month"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        // February 2024 (leap year) ends on the 29th
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-02-29T23:59:59"));
    }

    #[tokio::test]
    async fn test_end_of_year() {
        let s = skill();
        let c = call(
            "end_of",
            serde_json::json!({
                "datetime": "2024-06-15T10:00:00Z",
                "unit": "year"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-12-31T23:59:59"));
    }

    #[tokio::test]
    async fn test_end_of_week() {
        let s = skill();
        // 2024-03-15 is Friday; Sunday of that week is 2024-03-17
        let c = call(
            "end_of",
            serde_json::json!({
                "datetime": "2024-03-15T10:00:00Z",
                "unit": "week"
            }),
        );
        let r = s.execute(c).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
        assert!(v["result"]
            .as_str()
            .unwrap()
            .contains("2024-03-17T23:59:59"));
    }

    // -- Error handling -------------------------------------------------------

    #[tokio::test]
    async fn test_unknown_operation() {
        let s = skill();
        let c = call("bogus", serde_json::json!({}));
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    #[tokio::test]
    async fn test_empty_operation() {
        let s = skill();
        let c = ToolCall {
            id: "t1".to_string(),
            name: "datetime".to_string(),
            arguments: serde_json::json!({"operation": ""}),
        };
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn test_add_unknown_unit() {
        let s = skill();
        let c = call(
            "add",
            serde_json::json!({
                "datetime": "2024-01-01T00:00:00Z",
                "amount": 1,
                "unit": "fortnights"
            }),
        );
        let r = s.execute(c).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("Unknown unit"));
    }

    // -- Default trait --------------------------------------------------------

    #[test]
    fn test_default() {
        let s = DateTimeSkill::default();
        assert_eq!(s.descriptor().name, "datetime");
    }
}
