//! Time formatting for the wire.
//!
//! Timestamps are RFC 3339 in JSON, because the web client displays them, and Unix
//! milliseconds in protobuf, because protobuf clients are programs. Both are
//! produced here so the two facades cannot disagree about an instant.
//!
//! No date library is involved: the conversion needs days-since-epoch to civil
//! date, which is a closed-form formula, and pulling in a dependency for that
//! would be the larger cost.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current time in Unix milliseconds.
pub fn now_unix_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(delta) => delta.as_millis() as i64,
        Err(error) => -(error.duration().as_millis() as i64),
    }
}

/// Formats Unix milliseconds as an RFC 3339 UTC timestamp.
///
/// Sub-second precision is dropped: the API reports task and library entry times,
/// and milliseconds past the second are noise at that granularity.
pub fn unix_ms_to_rfc3339(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let (days, second_of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since 1970-01-01 to a civil date.
///
/// Howard Hinnant's `civil_from_days`: the era-based form keeps every division
/// exact, so it holds for the whole range of `i64` days without special cases.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants_round_trip() {
        assert_eq!(unix_ms_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_ms_to_rfc3339(1_000), "1970-01-01T00:00:01Z");
        // 2024-02-29T12:34:56Z, a leap day, which is where naive day arithmetic
        // goes wrong.
        assert_eq!(
            unix_ms_to_rfc3339(1_709_210_096_000),
            "2024-02-29T12:34:56Z"
        );
        // A negative instant, which happens when a clock is set before 1970.
        assert_eq!(unix_ms_to_rfc3339(-1_000), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn the_current_time_is_formatted_and_sane() {
        let formatted = unix_ms_to_rfc3339(now_unix_ms());
        assert!(
            formatted.starts_with("20"),
            "unexpected timestamp {formatted}"
        );
        assert!(formatted.ends_with('Z'));
        assert_eq!(formatted.len(), 20);
    }
}
