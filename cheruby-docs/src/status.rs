//! The index runs as the config panel shows them: whether one is going, how
//! the last one ended, what the index holds and when the next comes, from
//! the status file the indexer writes and from systemd.

use serde_json::{Map, Value};

/// "3 indexed, 1 waiting for OCR" from {status: count}.
fn said(counts: &Value) -> String {
    counts
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, n)| {
            format!(
                "{n} {}",
                if k == "needs_ocr" {
                    "waiting for OCR"
                } else {
                    k
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn is_empty(v: &Value) -> bool {
    v.is_null() || v.as_object().is_some_and(Map::is_empty)
}

/// The panel's lines, one "- **...**" each. state is systemd's for the
/// index unit (activating while a run goes); next, when given, the timer's
/// next run as systemd says it.
pub fn render(status: &Map<String, Value>, state: &str, next: Option<&str>, unit: &str) -> String {
    let get = |k: &str| status.get(k).cloned().unwrap_or(Value::Null);
    let result = if status.is_empty() {
        "none".to_string()
    } else {
        text(&get("result"))
    };
    let (started, ended) = (text(&get("started")), text(&get("ended")));
    let mut lines = vec![if state == "activating" {
        let since = if result == "running" {
            format!(", since {started}")
        } else {
            String::new()
        };
        format!("- **Running now**{since}.")
    } else {
        match result.as_str() {
            "none" => "- **No run yet.**".to_string(),
            "running" => format!(
                "- **Last run**: started {started}, stopped before the end: journalctl -u {unit}."
            ),
            "done" => format!("- **Last run**: {started} to {ended}, done."),
            "paused" => {
                format!(
                    "- **Last run**: {started} to {ended}, paused for the quiet hours; the next run carries on."
                )
            }
            _ => {
                let error = match get("error") {
                    Value::Null | Value::Bool(false) => format!("journalctl -u {unit}"),
                    e => text(&e),
                };
                format!("- **Last run**: {started} to {ended}, failed: {error}")
            }
        }
    }];
    if state != "activating" && !is_empty(&get("run")) {
        lines.push(format!("- **It did**: {}.", said(&get("run"))));
    }
    let on: Vec<String> = match get("embedded_on") {
        Value::Array(a) => a.iter().map(text).collect(),
        _ => vec!["pi".into()],
    };
    let note = match get("remote_note") {
        Value::Null | Value::Bool(false) => None,
        n => Some(text(&n)),
    };
    if on != ["pi"] || note.is_some() {
        let places: Vec<&str> = on
            .iter()
            .map(|p| match p.as_str() {
                "desktop" => "the desktop",
                "pi" => "this server",
                _ => "null",
            })
            .collect();
        let note = note.map(|n| format!(". {n}")).unwrap_or_default();
        lines.push(format!(
            "- **Embedded on**: {}{note}",
            places.join(", then on ")
        ));
    }
    let files = get("files");
    let holds = if is_empty(&files) {
        "no file yet".to_string()
    } else {
        said(&files)
    };
    lines.push(format!("- **The index holds**: {holds}."));
    if let Some(next) = next {
        lines.push(format!("- **Next run**: {next}"));
    }
    lines.join("\n")
}

/// systemd's value of a property of a unit, or "" when it cannot say.
pub fn systemd(unit: &str, property: &str) -> String {
    std::process::Command::new("systemctl")
        .args(["show", unit, "-p", property, "--value"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_done_run() {
        let status = json!({"started": "2026-11-21 10:20", "ended": "2026-11-21 10:21", "result": "done",
                            "run": {"unchanged": 3}, "files": {"indexed": 2, "needs_ocr": 1},
                            "embedded_on": ["pi"], "remote_note": null});
        assert_eq!(
            render(
                status.as_object().unwrap(),
                "inactive",
                Some("Sat 2026-11-21 11:20:00 CET"),
                "x-index"
            ),
            "- **Last run**: 2026-11-21 10:20 to 2026-11-21 10:21, done.\n- **It did**: 3 unchanged.\n\
             - **The index holds**: 2 indexed, 1 waiting for OCR.\n- **Next run**: Sat 2026-11-21 11:20:00 CET"
        );
    }

    #[test]
    fn no_run_yet() {
        assert_eq!(
            render(&Map::new(), "inactive", None, "x"),
            "- **No run yet.**\n- **The index holds**: no file yet."
        );
    }
}
