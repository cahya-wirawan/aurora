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
mod glyph;
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
    if std::env::args().any(|a| a == "--glyph-demo") {
        return glyph_demo();
    }

    // The first text layer this writer has ever embedded in a real file —
    // every prior TySh patch (--tysh-demo) operated on a standalone
    // extracted block, never inside a written PSD. Shares the exact same
    // patch as --tysh-demo via patch_tysh_text, and the standalone
    // rasterizer proven in glyph.rs (finding 13). Font size and color now
    // come from the patched TySh's own EngineData -- StyleSheetData's
    // FontSize/FillColor, decoded by engine_data::first_run_style -- rather
    // than hardcoded constants (finding 15). Font *resolution* (using the
    // document's actual named font instead of the bundled DejaVu stand-in)
    // remains unstarted; see glyph.rs's module docs.
    let (text_tysh, text_engine) = patch_tysh_text("Aurora spike")?;
    let text_style = engine_data::first_run_style(&text_engine)
        .expect("a real fixture must have a first style run with FontSize/FillColor");
    let text_raster =
        glyph::rasterize("Aurora spike", text_style.font_size, text_style.color, None);

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
                tysh: None,
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
                            tysh: None,
                        })],
                    }),
                    Item::Layer(Layer {
                        name: "Warm shape".into(),
                        rect: (40, 30, 200, 150),
                        pixels: swatch(160, 120, [242, 158, 64], 12.0),
                        opacity: 255,
                        visible: true,
                        blend: *b"norm",
                        tysh: None,
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
                tysh: None,
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
                tysh: None,
            }),
            // The text layer described above.
            Item::Layer(Layer {
                name: "Aurora spike".into(),
                rect: (
                    10,
                    10,
                    10 + text_raster.width as i32,
                    10 + text_raster.height as i32,
                ),
                pixels: text_raster.pixels,
                opacity: 255,
                visible: true,
                blend: *b"norm",
                tysh: Some(text_tysh.to_bytes()),
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

/// Patches a real Photoshop-authored `TySh` block's text content — both the
/// top-level `Txt ` field and the nested `EngineData.Editor.Text` — to
/// `new_text`, recomputing `ParagraphRun`/`StyleRun` `RunLengthArray`s to
/// match (finding 12). Shared by `tysh_demo` and the text layer embedded in
/// `main`'s written PSD, so both exercise the exact same patch rather than
/// two subtly different ones.
///
/// The two fields get different terminators (`\u{0}` vs. `\r`) — an
/// existing, real convention difference between the two representations,
/// not a typo; see the original `--tysh-demo` output this was extracted
/// from.
/// Returns the patched `TySh` together with the final, patched `EngineData`
/// value (post `recompute_run_lengths`) — the latter is what lets a caller
/// read the real `FontSize`/`FillColor` (`engine_data::first_run_style`,
/// finding 15) instead of hardcoding them, without a second parse.
fn patch_tysh_text(
    new_text: &str,
) -> std::io::Result<(descriptor::TypeToolObjectSetting, engine_data::Value)> {
    let real_tysh = include_bytes!("../reference/tysh.bin");
    let mut tysh = descriptor::parse_fixture(real_tysh)?;
    tysh.set_text(format!("{new_text}\u{0}"));

    // EngineData.Editor.Text -- the nested text-engine content, via
    // engine_data.rs, which descriptor.rs treats as an opaque blob (it's a
    // completely different text-based format, not the Descriptor binary
    // format). Patching both fields is what the Python proof-of-concept in
    // FINDINGS.md finding 7 did at the full-file level.
    let descriptor::Value::Raw(engine_bytes) = tysh
        .text_data
        .get(b"EngineData")
        .expect("EngineData field must exist")
        .clone()
    else {
        panic!("EngineData field must be Raw");
    };
    let mut engine =
        engine_data::parse(&engine_bytes).expect("a real Photoshop EngineData payload must parse");
    let engine_text = format!("{new_text}\r");
    if let Some(slot) = engine.get_path_mut("EngineDict.Editor.Text") {
        *slot = engine_data::Value::Str(engine_text.clone());
    }
    // Closes the gap finding 10 named: without this, ParagraphRun/
    // StyleRun's RunLengthArrays would still sum to the OLD text's
    // length, which is exactly the internal inconsistency a real writer
    // must not produce. See engine_data.rs's module docs on
    // recompute_run_lengths for what this does and doesn't cover.
    engine_data::recompute_run_lengths(&mut engine, engine_text.encode_utf16().count())
        .expect("a real fixture must have well-formed ParagraphRun/StyleRun");
    let new_engine_bytes = engine_data::write(&engine);
    tysh.text_data
        .set(b"EngineData", descriptor::Value::Raw(new_engine_bytes));

    Ok((tysh, engine))
}

/// Parses a real Photoshop-authored `TySh` block (extracted from psd-tools'
/// own `text.psd` test fixture — see `reference/README.md` and
/// `descriptor.rs`), patches its text content via `patch_tysh_text`, and
/// reports what changed.
///
/// This does NOT write a full PSD file itself — that now happens in `main`
/// (the first text layer this writer has ever embedded in a real file; see
/// FINDINGS.md finding 14). This demo remains useful on its own for
/// inspecting the patch step in isolation, byte-counted, without the rest of
/// the document around it.
fn tysh_demo() -> std::io::Result<()> {
    let real_tysh = include_bytes!("../reference/tysh.bin");
    let original = descriptor::parse_fixture(real_tysh)?;

    println!(
        "Parsed a real Photoshop TySh block ({} bytes)",
        real_tysh.len()
    );
    println!(
        "  version={} text_version={}",
        original.version, original.text_version
    );
    println!("  text: {:?}", original.text());
    println!(
        "  bbox: {:?}  transform: {:?}",
        original.bbox, original.transform
    );

    let before = original.to_bytes();
    let reparsed_before = descriptor::parse_fixture(&before)?;
    assert_eq!(
        reparsed_before, original,
        "unmodified re-serialize must round-trip"
    );
    println!(
        "\nUnmodified round-trip: {} bytes (same length as source: {}), \
         semantically identical on re-parse.",
        before.len(),
        before.len() == real_tysh.len()
    );

    let (tysh, _) = patch_tysh_text("Aurora spike")?;
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
        "\nThis demo only patches the descriptor, in isolation — no pixels, \
         no written file. `cargo run`'s own \"Aurora spike\" text layer is now \
         the first place this writer embeds a patched TySh block AND \
         matching rendered pixels together in one real file (FINDINGS.md \
         finding 14); this demo remains useful for inspecting the patch step \
         on its own, byte-counted, without the rest of the document around it."
    );
    Ok(())
}

/// Standalone proof that `cosmic-text` can rasterize real text to an RGBA8
/// buffer headlessly, with a bundled font (see `glyph.rs`'s module docs for
/// why this is deliberately *not* wired into a written PSD yet — that is the
/// next step, not this one). Writes `out/glyph-demo.ppm`, viewable in any
/// image viewer that reads binary PPM (GIMP, `feh`, `eog`, ImageMagick's
/// `display`, or `magick out/glyph-demo.ppm out/glyph-demo.png` to convert).
fn glyph_demo() -> std::io::Result<()> {
    let text = "Aurora spike";
    let raster = glyph::rasterize(text, 48.0, (20, 20, 20), None);

    let inked: usize = raster.pixels.chunks_exact(4).filter(|px| px[3] > 0).count();
    println!(
        "Rasterized {text:?}: {}×{} px, {inked} of {} pixels inked ({:.1}%)",
        raster.width,
        raster.height,
        raster.width * raster.height,
        100.0 * inked as f64 / (raster.width * raster.height) as f64
    );

    std::fs::create_dir_all("out")?;
    let path = std::path::Path::new("out/glyph-demo.ppm");
    glyph::write_ppm(&raster, path)?;
    println!(
        "Wrote {} — open it in an image viewer to confirm the text is legible",
        path.display()
    );

    println!(
        "\nThis is rasterization only, standalone. `cargo run`'s own \
         \"Aurora spike\" text layer now wires this exact rasterizer into a \
         written PSD alongside a patched TySh block (FINDINGS.md finding \
         14) — the first genuinely embedded, internally-consistent text \
         layer this writer has produced. Font and color there are still \
         hardcoded, same as here; reading a real FillColor/FontSet is \
         separate, smaller remaining work."
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
