//! A cron schedule (minute hour day month weekday) and the quiet hours
//! around it: which minutes are due, and when the next one comes. The tool
//! server runs the index on it; the page and the status say the next run.

use std::collections::BTreeSet;

use chrono::{Datelike, Duration, NaiveDateTime, NaiveTime, Timelike};

const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
/// How far next() looks: a 29 February comes within five years.
const HORIZON_DAYS: i64 = 5 * 366;

/// A field with its three-letter names given as numbers.
fn numbered(field: &str) -> String {
    let mut out = field.to_string();
    for (n, d) in WEEKDAYS.iter().enumerate() {
        out = out.replace(d, &n.to_string());
    }
    for (n, m) in MONTHS.iter().enumerate() {
        out = out.replace(m, &(n + 1).to_string());
    }
    out
}

/// The sorted numbers a cron field names; None on what it cannot read.
fn values(field: &str, low: u32, high: u32) -> Option<BTreeSet<u32>> {
    let mut out = BTreeSet::new();
    let field = numbered(field);
    if field.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, Some(s.parse::<u32>().ok()?)),
            None => (part, None),
        };
        let (first, last) = match range.split_once('-') {
            Some((a, b)) => (a, Some(b.parse::<u32>().ok()?)),
            None => (range, None),
        };
        if first == "*" && last.is_some() {
            return None;
        }
        let start = if first == "*" {
            low
        } else {
            first.parse::<u32>().ok()?
        };
        let end = last.unwrap_or(if first == "*" || step.is_some() {
            high
        } else {
            start
        });
        if !(low <= start && start <= end && end <= high) || step == Some(0) {
            return None;
        }
        out.extend((start..=end).step_by(step.unwrap_or(1) as usize));
    }
    Some(out)
}

/// A cron line, read once.
#[derive(Clone, Debug)]
pub struct Cron {
    minutes: BTreeSet<u32>,
    hours: BTreeSet<u32>,
    days: Option<BTreeSet<u32>>,
    months: BTreeSet<u32>,
    weekdays: Option<BTreeSet<u32>>,
}

impl Cron {
    /// None on what cron would not read, and on a day of the month with a
    /// weekday: cron joins them with OR, which the page cannot show plainly.
    pub fn parse(line: &str) -> Option<Cron> {
        let lower = line.to_lowercase();
        let fields: Vec<&str> = lower.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields[..] else {
            return None;
        };
        if day != "*" && weekday != "*" {
            return None;
        }
        Some(Cron {
            minutes: values(minute, 0, 59)?,
            hours: values(hour, 0, 23)?,
            days: if day == "*" {
                None
            } else {
                Some(values(day, 1, 31)?)
            },
            months: values(month, 1, 12)?,
            weekdays: if weekday == "*" {
                None
            } else {
                // 0 and 7 are both Sunday.
                Some(values(weekday, 0, 7)?.into_iter().map(|d| d % 7).collect())
            },
        })
    }

    fn on_day(&self, t: NaiveDateTime) -> bool {
        self.months.contains(&t.month())
            && self.days.as_ref().is_none_or(|d| d.contains(&t.day()))
            && self
                .weekdays
                .as_ref()
                .is_none_or(|d| d.contains(&t.weekday().num_days_from_sunday()))
    }

    /// Whether the minute of t is one cron runs on, outside the quiet hours.
    pub fn due(&self, quiet: &str, t: NaiveDateTime) -> bool {
        self.on_day(t)
            && self.hours.contains(&t.hour())
            && self.minutes.contains(&t.minute())
            && !quiet_at(quiet, t.hour())
    }

    /// The first due minute after t; None when none comes, as when the
    /// quiet hours take every hour or the date never exists.
    pub fn next(&self, quiet: &str, t: NaiveDateTime) -> Option<NaiveDateTime> {
        let first = t.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        for offset in 0..HORIZON_DAYS {
            let day = first.date() + Duration::days(offset);
            if !self.on_day(day.and_time(NaiveTime::MIN)) {
                continue;
            }
            for &hour in &self.hours {
                if quiet_at(quiet, hour) {
                    continue;
                }
                for &minute in &self.minutes {
                    let at = day.and_hms_opt(hour, minute, 0)?;
                    if at >= first {
                        return Some(at);
                    }
                }
            }
        }
        None
    }
}

/// Whether quiet hours "<from>-<to>", to excluded, are well formed: two
/// different hours from 0 to 24 (24 is 0), or empty for none.
pub fn quiet_ok(quiet: &str) -> bool {
    if quiet.is_empty() {
        return true;
    }
    let Some((a, b)) = quiet.split_once('-') else {
        return false;
    };
    let hour = |s: &str| {
        (!s.is_empty() && s.len() <= 2 && s.bytes().all(|c| c.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
            .filter(|h| *h <= 24)
    };
    matches!((hour(a), hour(b)), (Some(a), Some(b)) if a % 24 != b % 24)
}

/// Whether the hour is within quiet hours "<from>-<to>", to excluded.
pub fn quiet_at(quiet: &str, hour: u32) -> bool {
    let Some((a, b)) = quiet.split_once('-') else {
        return false;
    };
    let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) else {
        return false;
    };
    let (a, b) = (a % 24, b % 24);
    if a < b {
        a <= hour && hour < b
    } else {
        hour >= a || hour < b
    }
}

/// Whether this hour is within the quiet hours.
pub fn quiet_now(quiet: &str) -> bool {
    quiet_at(quiet, chrono::Local::now().hour())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn next(cron: &str, quiet: &str, from: &str) -> Option<String> {
        Cron::parse(cron)
            .unwrap()
            .next(quiet, at(from))
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
    }

    #[test]
    fn the_next_run_skips_the_quiet_hours() {
        assert_eq!(
            next("20 * * * *", "01-06", "2026-11-25 00:30").as_deref(),
            Some("2026-11-25 06:20")
        );
        assert_eq!(
            next("20 * * * *", "", "2026-11-25 00:30").as_deref(),
            Some("2026-11-25 01:20")
        );
        assert_eq!(
            next("20 * * * *", "", "2026-11-25 00:20").as_deref(),
            Some("2026-11-25 01:20")
        );
        assert_eq!(
            next("20 */6 * * *", "01-07", "2026-11-25 00:30").as_deref(),
            Some("2026-11-25 12:20")
        );
        assert_eq!(next("20 23 * * *", "22-02", "2026-11-25 00:30"), None);
        assert_eq!(
            next("0 12 * * mon-fri", "", "2026-11-27 13:00").as_deref(),
            Some("2026-11-30 12:00")
        );
        assert_eq!(
            next("0 3 29 2 *", "", "2026-03-01 00:00").as_deref(),
            Some("2028-02-29 03:00")
        );
        assert_eq!(next("0 3 31 2 *", "", "2026-03-01 00:00"), None);
        assert_eq!(
            next("0 3 * * 7", "", "2026-11-25 00:00").as_deref(),
            Some("2026-11-29 03:00")
        );
    }

    #[test]
    fn due_minutes() {
        let cron = Cron::parse("*/30 8-20 * * *").unwrap();
        assert!(cron.due("", at("2026-11-25 08:30")));
        assert!(!cron.due("", at("2026-11-25 08:31")));
        assert!(!cron.due("", at("2026-11-25 21:00")));
        assert!(!cron.due("08-09", at("2026-11-25 08:30")));
    }

    #[test]
    fn what_cron_cannot_mean() {
        for bad in [
            "0 3 1 * 1",
            "0 3 * *",
            "60 * * * *",
            "34 6-24 * * *",
            "*/0 * * * *",
            "a * * * *",
        ] {
            assert!(Cron::parse(bad).is_none(), "{bad}");
        }
        assert!(Cron::parse("0 3 * jan-mar sun").is_some());
    }

    #[test]
    fn quiet_hours() {
        assert!(quiet_at("01-06", 1) && quiet_at("01-06", 5) && !quiet_at("01-06", 6));
        assert!(quiet_at("22-02", 23) && quiet_at("22-02", 1) && !quiet_at("22-02", 2));
        assert!(!quiet_at("", 3));
        for good in ["", "01-06", "1-6", "22-07", "0-23"] {
            assert!(quiet_ok(good), "{good}");
        }
        for bad in ["01", "06-06", "0-24-1", "25-01", "a-b", "-1-3", "24-0"] {
            assert!(!quiet_ok(bad), "{bad}");
        }
    }
}
