//! A cron schedule (minute hour day month weekday) as systemd's OnCalendar,
//! its minutes and hours listed one by one so the quiet hours can be left
//! out. For the package's scripts, which still check the result with
//! systemd-analyze.

use std::collections::BTreeSet;

const DAYS: [&str; 8] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

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

fn listed(found: &BTreeSet<u32>, low: u32, high: u32) -> String {
    if found.len() as u32 == high - low + 1 {
        return "*".into();
    }
    found
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The OnCalendar of a cron schedule, the quiet hours (<from>-<to>, to
/// excluded, empty for none) left out. None when cron's meaning cannot be
/// kept or no hour is left.
pub fn calendar(cron: &str, quiet: &str) -> Option<String> {
    let lower = cron.to_lowercase();
    let fields: Vec<&str> = lower.split_whitespace().collect();
    let [minute, hour, day, month, weekday] = fields[..] else {
        return None;
    };
    let minutes = values(minute, 0, 59)?;
    let mut hours = values(hour, 0, 23)?;
    let days = values(day, 1, 31)?;
    let months = values(month, 1, 12)?;
    let weekdays = values(weekday, 0, 7)?;
    // cron runs when either the day or the weekday matches, systemd when both do.
    if day != "*" && weekday != "*" {
        return None;
    }
    if !quiet.is_empty() {
        let (a, b) = quiet.split_once('-')?;
        let (a, b) = (
            a.trim().parse::<u32>().ok()? % 24,
            b.trim().parse::<u32>().ok()? % 24,
        );
        if a == b {
            return None;
        }
        hours.retain(|&h| {
            !(if a < b {
                a <= h && h < b
            } else {
                h >= a || h < b
            })
        });
    }
    if hours.is_empty() {
        return None;
    }
    let weekday = if weekday == "*" {
        String::new()
    } else {
        let mut names: Vec<&str> = Vec::new();
        for d in &weekdays {
            if !names.contains(&DAYS[*d as usize]) {
                names.push(DAYS[*d as usize]);
            }
        }
        format!("{} ", names.join(","))
    };
    Some(format!(
        "{weekday}*-{}-{} {}:{}:00",
        listed(&months, 1, 12),
        listed(&days, 1, 31),
        listed(&hours, 0, 23),
        listed(&minutes, 0, 59)
    ))
}

#[cfg(test)]
mod tests {
    use super::calendar;

    #[test]
    fn schedules_as_the_scripts_had_them() {
        assert_eq!(
            calendar("20 * * * *", "01-06").as_deref(),
            Some("*-*-* 0,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23:20:00")
        );
        assert_eq!(calendar("20 * * * *", "").as_deref(), Some("*-*-* *:20:00"));
        assert_eq!(
            calendar("0 3 * * mon-fri", "").as_deref(),
            Some("Mon,Tue,Wed,Thu,Fri *-*-* 3:0:00")
        );
        assert_eq!(
            calendar("*/15 8-18/2 1 * *", "").as_deref(),
            Some("*-*-1 8,10,12,14,16,18:0,15,30,45:00")
        );
        assert_eq!(calendar("0 3 * * *", "01-06"), None);
        assert_eq!(calendar("34 6-24 * * *", ""), None);
        assert_eq!(calendar("0 3 1 * 1", ""), None);
        assert_eq!(calendar("0 3 * *", ""), None);
        assert_eq!(
            calendar("0 3 * * 0,7", "").as_deref(),
            Some("Sun *-*-* 3:0:00")
        );
    }
}
