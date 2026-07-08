//! Small formatting preferences shared across commands — currently just the timestamp used to name
//! generated files. Kept apart from the shell/PS1 layer so the one format lives in a single place if
//! more join it later.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current UTC time as `YYYY-mm-DD_HHMM`, for stamping generated filenames (e.g. `gg --save`'s
/// `deep_search_<stamp>`). Almost the PS1 clock (`date -u +%T`), but `-`/`_`-separated and to the
/// minute, since a filename reads better without `:`.
pub(crate) fn datehour_stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_utc(secs)
}

/// `secs` since the Unix epoch → `YYYY-mm-DD_HHMM` in UTC. Split from [`datehour_stamp`] so it's
/// testable against fixed instants (the wall clock isn't). The calendar date comes from Howard
/// Hinnant's days-to-civil algorithm, so no date crate is pulled in.
fn format_utc(secs: u64) -> String {
    let (days, sec_of_day) = ((secs / 86_400) as i64, secs % 86_400);
    let (hour, minute) = (sec_of_day / 3_600, (sec_of_day % 3_600) / 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}_{hour:02}{minute:02}")
}

#[cfg(test)]
mod tests {
    use super::format_utc;

    #[test]
    fn formats_known_utc_instants() {
        assert_eq!(format_utc(0), "1970-01-01_0000"); // the Unix epoch
        assert_eq!(format_utc(1_609_459_200), "2021-01-01_0000"); // 2021-01-01T00:00:00Z
        assert_eq!(format_utc(1_704_067_199), "2023-12-31_2359"); // one second before 2024
        assert_eq!(format_utc(1_709_209_440), "2024-02-29_1224"); // leap day, 12:24 UTC
    }
}
