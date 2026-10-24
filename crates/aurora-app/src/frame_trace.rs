//! An env-gated log of slow event-loop phases (task 152).
//!
//! `AURORA_FRAME_TRACE=<path>` appends one line per `about_to_wait`,
//! `RedrawRequested` or other window event whose whole duration is at
//! least `AURORA_FRAME_TRACE_MS` milliseconds (default 12), naming the
//! time spent between each [`mark`]. Unset (the default) every call is one
//! relaxed load of a cached flag and nothing is recorded or written.
//!
//! A diagnostic, like `bench_launch`'s hooks: it changes no behaviour.

use std::cell::RefCell;
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Sink {
    file: Mutex<std::fs::File>,
    threshold_ms: f64,
    epoch: Instant,
}

fn sink() -> Option<&'static Sink> {
    static SINK: OnceLock<Option<Sink>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path = std::env::var_os("AURORA_FRAME_TRACE")?;
        if path.is_empty() {
            return None;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        let threshold_ms = std::env::var("AURORA_FRAME_TRACE_MS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(12.0);
        Some(Sink {
            file: Mutex::new(file),
            threshold_ms,
            epoch: Instant::now(),
        })
    })
    .as_ref()
}

struct Open {
    label: &'static str,
    start: Instant,
    last: Instant,
    marks: Vec<(&'static str, f64)>,
    notes: Vec<(&'static str, f64)>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

/// Starts a traced phase named `label` (a nested `begin` is ignored).
pub(crate) fn begin(label: &'static str) {
    if sink().is_none() {
        return;
    }
    OPEN.with(|open| {
        let mut open = open.borrow_mut();
        if open.is_none() {
            let now = Instant::now();
            *open = Some(Open {
                label,
                start: now,
                last: now,
                marks: Vec::new(),
                notes: Vec::new(),
            });
        }
    });
}

/// Records the time since the previous mark (or `begin`) under `name`.
pub(crate) fn mark(name: &'static str) {
    if sink().is_none() {
        return;
    }
    OPEN.with(|open| {
        if let Some(open) = open.borrow_mut().as_mut() {
            let now = Instant::now();
            open.marks
                .push((name, now.duration_since(open.last).as_secs_f64() * 1e3));
            open.last = now;
        }
    });
}

/// Records a value (a count, a branch) under `name` in the open phase.
pub(crate) fn note(name: &'static str, value: f64) {
    if sink().is_none() {
        return;
    }
    OPEN.with(|open| {
        if let Some(open) = open.borrow_mut().as_mut() {
            open.notes.push((name, value));
        }
    });
}

/// Ends the phase `label` began, writing it when it was slow.
pub(crate) fn end(label: &'static str) {
    let Some(sink) = sink() else {
        return;
    };
    let Some(open) = OPEN.with(|open| {
        let mut open = open.borrow_mut();
        if open.as_ref().is_some_and(|o| o.label == label) {
            open.take()
        } else {
            None
        }
    }) else {
        return;
    };
    let now = Instant::now();
    let total = now.duration_since(open.start).as_secs_f64() * 1e3;
    if total < sink.threshold_ms {
        return;
    }
    let mut line = format!(
        "t={:.3} {} total={total:.2}",
        open.start.duration_since(sink.epoch).as_secs_f64(),
        open.label
    );
    for (name, ms) in &open.marks {
        if *ms >= 0.5 {
            line.push_str(&format!(" {name}={ms:.2}"));
        }
    }
    for (name, value) in &open.notes {
        line.push_str(&format!(" #{name}={value}"));
    }
    let rest = now.duration_since(open.last).as_secs_f64() * 1e3;
    if rest >= 0.5 {
        line.push_str(&format!(" rest={rest:.2}"));
    }
    line.push('\n');
    if let Ok(mut file) = sink.file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}
