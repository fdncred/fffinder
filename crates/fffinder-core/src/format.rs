//! Finder-style human formatting for sizes and dates.

use chrono::{DateTime, Local};
use std::time::SystemTime;

/// Decimal units, as macOS Finder shows them: "41 bytes", "1.2 KB", "3.4 GB".
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes == 1 {
        return "1 byte".into();
    }
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// "Today at 09:53", "Yesterday at 18:02", or "9 Oct 2026 at 09:53".
pub fn human_date(time: SystemTime) -> String {
    let dt: DateTime<Local> = time.into();
    let today = Local::now().date_naive();
    let date = dt.date_naive();
    let clock = dt.format("%H:%M");
    if date == today {
        format!("Today at {clock}")
    } else if today.pred_opt() == Some(date) {
        format!("Yesterday at {clock}")
    } else {
        format!("{} at {clock}", dt.format("%-d %b %Y"))
    }
}

/// "Tuesday, 27 October 2026 at 09:53", for Get Info.
pub fn long_date(time: SystemTime) -> String {
    let dt: DateTime<Local> = time.into();
    dt.format("%A, %-d %B %Y at %H:%M").to_string()
}

/// RFC 3339 timestamp for machine-readable output.
pub fn iso_date(time: SystemTime) -> String {
    let dt: DateTime<Local> = time.into();
    dt.to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(41), "41 bytes");
        assert_eq!(human_size(1_234), "1.2 KB");
        assert_eq!(human_size(56_000), "56 KB");
        assert_eq!(human_size(999_999), "1.0 MB");
        assert_eq!(human_size(3_400_000_000), "3.4 GB");
    }

    #[test]
    fn today() {
        assert!(human_date(SystemTime::now()).starts_with("Today at "));
    }
}
