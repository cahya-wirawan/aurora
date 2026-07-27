//! PSD write spike — PLAN 0.6, ADR 0004.
//!
//! Aurora commits to full layered PSD *write*: a file edited in Aurora must
//! reopen in Photoshop with its layers intact. Phase 3 is ten months long and
//! rests on that being feasible. This writes a real layered PSD from scratch so
//! the assumption can be checked against independent readers.
//!
//!   cargo run                writes out/spike.psd
//!   cargo run -- --tysh-demo parses+patches a real Photoshop text layer (see descriptor.rs)
//!
//! Verification is external, in `verify.sh`: Apple's system decoder (`sips`)
//! and `psd-tools`. Neither is Photoshop — that check remains outstanding — but
//! both are independent of this writer, which is the property that matters.

mod descriptor;
mod engine_data;
mod psd;

use psd::{Document, Group, Item, Layer};

const W: u32 = 320;
const H: u32 = 240;

/// A filled rectangle with a soft alpha edge, so alpha handling is actually
/// exercised rather than being all-or-nothing.
fn swatch(w: u32, h: u32, rgb: [u8; 3], feather: f32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h) as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) as usize) * 4;
            let edge = (x as f32)
                .min(y as f32)
                .min((w - 1 - x) as f32)
                .min((h - 1 - y) as f32);
            let a = if feather > 0.0 {
                (edge / feather).clamp(0.0, 1.0)
            } else {
                1.0
            };
            px[i] = rgb[0];
            px[i + 1] = rgb[1];
            px[i + 2] = rgb[2];
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    px
}

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--tysh-demo") {
        return tysh_demo();
    }

    // Written bottom-up throughout: PSD stores the layer list from the bottom
    // of the stack upwards, the opposite of how a layers panel reads. Groups
    // follow the same convention for their own children.
    let doc = Document {
        width: W,
        height: H,
        items: vec![
            Item::Layer(Layer {
                name: "Background".into(),
                rect: (0, 0, W as i32, H as i32),
                pixels: swatch(W, H, [32, 40, 56], 0.0),
                opacity: 255,
                visible: true,
                blend: *b"norm",
            }),
            // A group containing two layers, one of which is itself a
            // one-layer closed sub-group — exercises multi-level nesting, not
            // just a single flat folder.
            Item::Group(Group {
                name: "Warm cluster".into(),
                open: true,
                opacity: 255,
                visible: true,
                blend: *b"pass", // "pass through" — the usual default for groups
                children: vec![
                    Item::Group(Group {
                        name: "Nested (closed)".into(),
                        open: false,
                        opacity: 255,
                        visible: true,
                        blend: *b"pass",
                        children: vec![Item::Layer(Layer {
                            name: "Multiply 60%".into(),
                            rect: (120, 90, 300, 220),
                            pixels: swatch(180, 130, [80, 190, 220], 8.0),
                            opacity: 153, // 60 %
                            visible: true,
                            blend: *b"mul ",
                        })],
                    }),
                    Item::Layer(Layer {
                        name: "Warm shape".into(),
                        rect: (40, 30, 200, 150),
                        pixels: swatch(160, 120, [242, 158, 64], 12.0),
                        opacity: 255,
                        visible: true,
                        blend: *b"norm",
                    }),
                ],
            }),
            Item::Layer(Layer {
                name: "Hidden layer".into(),
                rect: (10, 180, 120, 235),
                pixels: swatch(110, 55, [230, 60, 90], 4.0),
                opacity: 255,
                visible: false,
                blend: *b"norm",
            }),
            // A non-ASCII name, because layer names are a Pascal string with
            // padding and this is where an off-by-one shows up.
            Item::Layer(Layer {
                name: "レイヤー 5".into(),
                rect: (200, 20, 310, 90),
                pixels: swatch(110, 70, [140, 220, 140], 6.0),
                opacity: 200,
                visible: true,
                blend: *b"scrn",
            }),
        ],
    };

    let bytes = doc.write();
    std::fs::create_dir_all("out")?;
    std::fs::write("out/spike.psd", &bytes)?;

    println!("wrote out/spike.psd — {} bytes", bytes.len());
    print_tree(&doc.items, 0);
    println!("\nNow run ./verify.sh — this program cannot mark its own homework.");
    Ok(())
}

/// Parses a real Photoshop-authored `TySh` block (extracted from psd-tools'
/// own `text.psd` test fixture — see `reference/README.md` and
/// `descriptor.rs`), patches its text content, and reports what changed.
///
/// This does NOT write a full PSD file — that full-file, independently
/// verified (psd-tools + sips + visual inspection) round trip was done as a
/// Python proof-of-concept, documented in FINDINGS.md, precisely because a
/// full Rust-side file splice wasn't warranted for a spike once the format
/// understanding itself was validated here. See FINDINGS.md for the scope
/// boundary and what's left for a real implementation.
fn tysh_demo() -> std::io::Result<()> {
    let real_tysh = include_bytes!("../reference/tysh.bin");
    let mut tysh = descriptor::parse_fixture(real_tysh)?;

    println!(
        "Parsed a real Photoshop TySh block ({} bytes)",
        real_tysh.len()
    );
    println!(
        "  version={} text_version={}",
        tysh.version, tysh.text_version
    );
    println!("  text: {:?}", tysh.text());
    println!("  bbox: {:?}  transform: {:?}", tysh.bbox, tysh.transform);

    let before = tysh.to_bytes();
    let reparsed_before = descriptor::parse_fixture(&before)?;
    assert_eq!(
        reparsed_before, tysh,
        "unmodified re-serialize must round-trip"
    );
    println!(
        "\nUnmodified round-trip: {} bytes (same length as source: {}), \
         semantically identical on re-parse.",
        before.len(),
        before.len() == real_tysh.len()
    );

    tysh.set_text("Aurora spike\u{0}");

    // Also patch EngineData.Editor.Text -- the nested text-engine content,
    // via engine_data.rs, which descriptor.rs treats as an opaque blob (it's
    // a completely different text-based format, not the Descriptor binary
    // format). Patching both fields is what the Python proof-of-concept in
    // FINDINGS.md finding 7 did at the full-file level; this is the same
    // operation natively in Rust, closing the gap the original version of
    // this demo left open.
    if let descriptor::Value::Raw(engine_bytes) = tysh
        .text_data
        .get(b"EngineData")
        .expect("EngineData field must exist")
        .clone()
    {
        let mut engine = engine_data::parse(&engine_bytes)
            .expect("a real Photoshop EngineData payload must parse");
        let new_text = "Aurora spike\r";
        if let Some(slot) = engine.get_path_mut("EngineDict.Editor.Text") {
            *slot = engine_data::Value::Str(new_text.into());
        }
        // Closes the gap finding 10 named: without this, ParagraphRun/
        // StyleRun's RunLengthArrays would still sum to the OLD text's
        // length (30), which is exactly the internal inconsistency a real
        // writer must not produce. See engine_data.rs's module docs on
        // recompute_run_lengths for what this does and doesn't cover.
        engine_data::recompute_run_lengths(&mut engine, new_text.encode_utf16().count())
            .expect("a real fixture must have well-formed ParagraphRun/StyleRun");
        let new_engine_bytes = engine_data::write(&engine);
        tysh.text_data
            .set(b"EngineData", descriptor::Value::Raw(new_engine_bytes));
    }

    let after = tysh.to_bytes();
    let reparsed_after = descriptor::parse_fixture(&after)?;
    println!("\nPatched `Txt ` to: {:?}", reparsed_after.text());
    if let Some(descriptor::Value::Raw(engine_bytes)) = reparsed_after.text_data.get(b"EngineData")
    {
        let engine = engine_data::parse(engine_bytes).expect("re-parse patched EngineData");
        println!(
            "Patched EngineDict.Editor.Text to: {:?}",
            engine
                .get_path("EngineDict.Editor.Text")
                .and_then(engine_data::Value::as_str)
        );
        println!(
            "Recomputed ParagraphRun.RunLengthArray: {:?}",
            engine.get_path("EngineDict.ParagraphRun.RunLengthArray")
        );
        println!(
            "Recomputed StyleRun.RunLengthArray: {:?}",
            engine.get_path("EngineDict.StyleRun.RunLengthArray")
        );
    }
    println!(
        "New size: {} bytes (was {}) — the top-level `Txt ` field, the nested \
         EngineData.Editor.Text, and both RunLengthArrays now agree; `warp` \
         bytes are untouched",
        after.len(),
        before.len()
    );

    println!(
        "\nOne thing this demo still deliberately leaves inconsistent, a real \
         gap for a production writer: the layer's rasterized pixel preview is \
         untouched. A real implementation must re-render glyphs into the pixel \
         channels too, or the file is visually inconsistent \
         (FINDINGS.md finding 8) — unaffected by the run-length fix above \
         (finding 12), which only closes the descriptor-level bookkeeping gap \
         (finding 10)."
    );
    Ok(())
}

fn print_tree(items: &[Item], depth: usize) {
    let indent = "  ".repeat(depth);
    // Printed top-down (reversed) to match how a layers panel reads; the file
    // itself stores these bottom-up, per the module docs in psd.rs.
    for item in items.iter().rev() {
        match item {
            Item::Layer(l) => println!(
                "{indent}{:<14} {:>3}×{:<3} at ({},{})  opacity {:>3}  blend {}  {}",
                l.name,
                l.width(),
                l.height(),
                l.rect.0,
                l.rect.1,
                l.opacity,
                String::from_utf8_lossy(&l.blend),
                if l.visible { "visible" } else { "hidden" }
            ),
            Item::Group(g) => {
                println!(
                    "{indent}[{}] {}  {}",
                    if g.open { "open" } else { "closed" },
                    g.name,
                    if g.visible { "visible" } else { "hidden" }
                );
                print_tree(&g.children, depth + 1);
            }
        }
    }
}
