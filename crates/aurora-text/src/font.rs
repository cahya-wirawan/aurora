//! The bundled UI font.
//!
//! Inter 4.1 Regular (<https://github.com/rsms/inter>), copyright The Inter
//! Project Authors, licensed under the SIL Open Font License 1.1 — the
//! full licence ships next to the file as `fonts/OFL.txt`. OFL 1.1 permits
//! bundling and redistributing the font with software under any licence,
//! provided the licence travels with it and the font is not sold on its
//! own. `cargo deny` cannot see a font embedded with `include_bytes!`, so
//! this licence is deliberately *not* on `deny.toml`'s allow list (see the
//! comment there).
//!
//! **Provisional.** The design token `type.family` (`design/tokens/
//! scales.toml`) is still `"TBD"`; the UI family is the design owner's
//! decision. Inter is a stand-in so labels can be drawn at all, and is
//! replaced — not argued for — once that token is decided.

/// The raw bytes of the bundled UI font (Inter 4.1 Regular, TrueType).
pub static UI_FONT_REGULAR: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");

#[cfg(test)]
mod tests {
    use super::UI_FONT_REGULAR;
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    #[test]
    fn bundled_font_bytes_are_the_pinned_file() {
        assert_eq!(UI_FONT_REGULAR.len(), 411_640);
        let digest = Sha256::digest(UI_FONT_REGULAR);
        let mut hex = String::new();
        for byte in digest {
            let _ = write!(hex, "{byte:02x}");
        }
        assert_eq!(
            hex,
            "40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82"
        );
    }

    #[test]
    fn the_ofl_licence_ships_next_to_the_font() {
        let licence = include_str!("../fonts/OFL.txt");
        assert!(licence.contains("SIL Open Font License, Version 1.1"));
        assert!(licence.contains("The Inter Project Authors"));
    }
}
