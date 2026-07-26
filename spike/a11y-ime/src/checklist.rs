//! An on-screen checklist that walks a tester through the verification and
//! records the answers.
//!
//! The point is not convenience for us — it is that the Windows and Linux legs
//! will be run by strangers following a document, and a document is a poor
//! instrument. This turns "remember what to listen for" into "press one key",
//! and writes a result file that can be pasted into an issue.
//!
//! Anything the program can observe for itself (IME events, whether an
//! assistive technology actually connected) is captured automatically rather
//! than asked, because a tester's recollection is weaker evidence than a log.

use std::fmt::Write as _;

pub struct Item {
    pub id: &'static str,
    pub prompt: &'static str,
    /// What to do, and what a pass looks like.
    pub hint: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Skipped,
}

impl Verdict {
    fn mark(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Skipped => "skipped",
        }
    }
}

pub const ITEMS: &[Item] = &[
    Item {
        id: "sr-window",
        prompt: "Screen reader announces the window at all",
        hint: "Turn on VoiceOver (Cmd+F5) / Narrator (Win+Ctrl+Enter) / Orca. \
               Click the window. A total silence here is the serious failure.",
    },
    Item {
        id: "sr-role",
        prompt: "Field is announced as an editable text field",
        hint: "Listen for \"edit text\" or \"text field\". Tests Role::TextInput.",
    },
    Item {
        id: "sr-label",
        prompt: "Label \"Layer name\" is announced with the field",
        hint: "Tests the separate label node and the labelled_by relationship.",
    },
    Item {
        id: "sr-value",
        prompt: "Current field value is announced",
        hint: "Should read back whatever the field contains.",
    },
    Item {
        id: "sr-live",
        prompt: "Typing a character is announced as the value changes",
        hint: "Type a letter. Silence means our tree updates are not reaching \
               the platform.",
    },
    Item {
        id: "sr-nav",
        prompt: "Field is reachable by screen-reader navigation",
        hint: "VO+Right / Narrator arrows / Orca Tab. Tests focus and tree structure.",
    },
    Item {
        id: "ime-preedit",
        prompt: "Preedit appears inline while composing CJK",
        hint: "Switch to Pinyin/kana/jamo and type. The field should show the \
               composition in brackets before you commit.",
    },
    Item {
        id: "ime-candidates",
        prompt: "Candidate window appears AT THE FIELD, not in a window corner",
        hint: "Tests set_ime_cursor_area. A corner-stranded candidate window is \
               a real usability failure, not a cosmetic one.",
    },
    Item {
        id: "ime-commit",
        prompt: "Committing inserts the composed characters correctly",
        hint: "Pick a candidate. The characters should land in the field intact.",
    },
    Item {
        id: "ime-deadkey",
        prompt: "Dead keys compose (e.g. option+e then e gives e-acute)",
        hint: "Optional but cheap. Skip if you have no such layout.",
    },
];

pub struct Checklist {
    pub index: usize,
    pub verdicts: Vec<Option<Verdict>>,
}

impl Default for Checklist {
    fn default() -> Self {
        Self {
            index: 0,
            verdicts: vec![None; ITEMS.len()],
        }
    }
}

impl Checklist {
    pub fn current(&self) -> Option<&'static Item> {
        ITEMS.get(self.index)
    }

    pub fn record(&mut self, v: Verdict) {
        if self.index < self.verdicts.len() {
            self.verdicts[self.index] = Some(v);
            self.index = (self.index + 1).min(ITEMS.len());
        }
    }

    pub fn back(&mut self) {
        self.index = self.index.saturating_sub(1);
    }

    pub fn done(&self) -> bool {
        self.index >= ITEMS.len()
    }

    pub fn answered(&self) -> usize {
        self.verdicts.iter().filter(|v| v.is_some()).count()
    }

    /// What is drawn on screen. Deliberately shows the hint for the current
    /// item only — a wall of ten prompts is not a checklist, it is a document.
    pub fn screen_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "CHECKLIST  {}/{} answered",
            self.answered(),
            ITEMS.len()
        );
        let _ = writeln!(s, "ctrl+Y pass   ctrl+N fail   ctrl+K skip   ctrl+B back   Esc finish\n");

        if let Some(item) = self.current() {
            let _ = writeln!(s, "  >> {}", item.prompt);
            let _ = writeln!(s, "     {}\n", item.hint);
        } else {
            let _ = writeln!(s, "  All items answered. Press Esc to write results.\n");
        }

        for (i, item) in ITEMS.iter().enumerate() {
            let mark = match self.verdicts[i] {
                Some(v) => v.mark(),
                None if i == self.index => "<-- now",
                None => "-",
            };
            let _ = writeln!(s, "   [{:>7}] {}", mark, item.id);
        }
        s
    }

    /// Markdown, so it can be pasted straight into an issue or the FINDINGS file.
    pub fn report(&self, ctx: &Context) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# a11y + IME verification result\n");
        let _ = writeln!(s, "- Platform: **{}**", ctx.platform);
        let _ = writeln!(s, "- GPU backend: {}", ctx.backend);
        let _ = writeln!(
            s,
            "- Assistive technology connected (observed by the program, not self-reported): **{}**",
            if ctx.a11y_activated { "YES" } else { "no" }
        );
        let _ = writeln!(s, "- IME events observed: **{}**\n", ctx.ime_events.len());

        let (mut pass, mut fail, mut skip) = (0, 0, 0);
        let _ = writeln!(s, "| Item | Result |");
        let _ = writeln!(s, "|---|---|");
        for (i, item) in ITEMS.iter().enumerate() {
            let v = match self.verdicts[i] {
                Some(Verdict::Pass) => {
                    pass += 1;
                    "PASS"
                }
                Some(Verdict::Fail) => {
                    fail += 1;
                    "**FAIL**"
                }
                Some(Verdict::Skipped) => {
                    skip += 1;
                    "skipped"
                }
                None => "not answered",
            };
            let _ = writeln!(s, "| {} — {} | {} |", item.id, item.prompt, v);
        }
        let _ = writeln!(s, "\n**{pass} passed, {fail} failed, {skip} skipped.**\n");

        if fail > 0 {
            let _ = writeln!(
                s,
                "> A failure here is not automatically ADR 0001's escape-hatch trigger. \
                 The trigger is a *structural* failure — AccessKit or winit cannot express \
                 the thing on this platform — as opposed to our code not doing it yet. \
                 Note which it looked like.\n"
            );
        }
        if !ctx.a11y_activated {
            let _ = writeln!(
                s,
                "> No assistive technology ever connected during this run. If a screen \
                 reader was running, that is itself the finding: the adapter is not \
                 reaching the platform API.\n"
            );
        }

        let _ = writeln!(s, "## IME event log\n");
        if ctx.ime_events.is_empty() {
            let _ = writeln!(
                s,
                "None. Either no IME was used, or winit is not delivering \
                 `Ime::*` events on this platform — the latter would be a finding."
            );
        } else {
            let _ = writeln!(s, "```");
            for e in &ctx.ime_events {
                let _ = writeln!(s, "{e}");
            }
            let _ = writeln!(s, "```");
        }

        let _ = writeln!(s, "\n## Final field contents\n\n```\n{}\n```", ctx.field_text);
        let _ = writeln!(
            s,
            "\n---\nGenerated by `spike/a11y-ime`. Paste into an issue or \
             `spike/a11y-ime/FINDINGS.md`."
        );
        s
    }
}

/// Print a filled-in report from sample answers. Exists so reviewers can see
/// the artifact a tester will produce without needing to run the GUI.
pub fn demo_report() {
    let mut c = Checklist::default();
    c.record(Verdict::Pass);
    c.record(Verdict::Pass);
    c.record(Verdict::Fail);
    c.record(Verdict::Pass);
    c.record(Verdict::Pass);
    c.record(Verdict::Skipped);
    c.record(Verdict::Pass);
    c.record(Verdict::Fail);
    c.record(Verdict::Pass);
    c.record(Verdict::Skipped);
    assert!(c.done(), "sample answers should fill the checklist");

    let ctx = Context {
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        backend: "Metal".into(),
        a11y_activated: true,
        ime_events: vec![
            "Enabled".into(),
            "Preedit(\"nihao\", cursor=Some((5, 5)))".into(),
            "Commit(\"\u{4f60}\u{597d}\")".into(),
        ],
        field_text: "\u{4f60}\u{597d}".into(),
    };
    println!("{}", c.report(&ctx));
}

pub struct Context {
    pub platform: String,
    pub backend: String,
    pub a11y_activated: bool,
    pub ime_events: Vec<String>,
    pub field_text: String,
}
