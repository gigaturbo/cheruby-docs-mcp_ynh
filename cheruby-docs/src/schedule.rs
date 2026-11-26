//! The index schedule, run by the tool server in place of a systemd timer,
//! so the page can change it. The server cannot start a unit (no AF_UNIX):
//! at each due minute it leaves a request for each user in requests/, as
//! run_index does, and the index unit's path unit starts the run. A request
//! with no files is a full run.

use std::{sync::Arc, time::Duration};

use chrono::{NaiveDateTime, Timelike};

use crate::{index, tools::Docs};

/// Asks for a full run of each user's index, unless a run is going: a timer
/// firing during its run did nothing either.
fn ask(docs: &Docs, why: &str) {
    if index::running(&docs.index_dir) {
        return;
    }
    let spool = docs.index_dir.join("requests");
    for user in docs.live().users.keys() {
        let name = format!("schedule-{user}");
        // Named .json only once whole: the path unit and the indexer look at .json files.
        let tmp = spool.join(format!(".{name}.tmp"));
        let body = serde_json::json!({"user": user});
        let written = std::fs::write(&tmp, body.to_string())
            .and_then(|_| std::fs::rename(&tmp, spool.join(format!("{name}.json"))));
        match written {
            Ok(()) => eprintln!("cheruby-docs: index run asked for {user}, {why}."),
            Err(e) => eprintln!("cheruby-docs: no index run asked for {user}: {e}"),
        }
    }
}

/// Whether a due minute passed since the last run of the first user's index
/// started, as a timer's Persistent= catches up. Never before a first run:
/// a fresh install waits for its schedule.
fn missed(docs: &Docs, now: NaiveDateTime) -> bool {
    let live = docs.live();
    let Some(user) = live.users.keys().next() else {
        return false;
    };
    let status = index::read_status(&docs.index_dir.join(user));
    let Some(started) = status
        .get("started")
        .and_then(|v| v.as_str())
        .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").ok())
    else {
        return false;
    };
    live.settings.next(started).is_some_and(|due| due <= now)
}

/// The minute t falls in.
fn minute_of(t: NaiveDateTime) -> NaiveDateTime {
    t.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(t)
}

/// Runs until the server stops. Each wake judges every minute since the
/// last, so a busy thread or a clock stepped forward skips no due minute.
/// The settings are those of the wake: a change on the page applies from it.
pub async fn run(docs: Arc<Docs>) {
    let mut last = minute_of(chrono::Local::now().naive_local());
    if missed(&docs, last) {
        ask(&docs, "missed while the server was stopped");
    }
    loop {
        let now = chrono::Local::now().naive_local();
        // A second into the next minute.
        let wait = 61 - u64::from(now.second().min(59));
        tokio::time::sleep(Duration::from_secs(wait)).await;
        let minute = minute_of(chrono::Local::now().naive_local());
        // A clock stepped back judges nothing until it passes the last minute again.
        if minute <= last {
            continue;
        }
        if docs
            .live()
            .settings
            .next(last)
            .is_some_and(|due| due <= minute)
        {
            ask(&docs, "on schedule");
        }
        last = minute;
    }
}
