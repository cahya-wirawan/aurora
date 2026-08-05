//! Shared channel-count expansion to this crate's own canonical RGBA
//! layout — real, non-trivial (loop + chunking) logic more than one
//! format module needs (`png`'s grayscale-with-alpha case, `tiff`'s
//! grayscale/grayscale-with-alpha/RGB cases), so it lives here once
//! rather than as near-duplicate copies per module.

use half::f16;

/// Duplicates one gray sample into R/G/B, adding a fully-opaque alpha.
pub(crate) fn gray_to_rgba(samples: &[f16]) -> Vec<f16> {
    let opaque = f16::from_f32(1.0);
    let mut out = Vec::with_capacity(samples.len() * 4);
    for &gray in samples {
        out.push(gray);
        out.push(gray);
        out.push(gray);
        out.push(opaque);
    }
    out
}

/// Duplicates one `Gray, Alpha` pair into `Red, Green, Blue, Alpha`.
pub(crate) fn gray_alpha_to_rgba(samples: &[f16]) -> Vec<f16> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for pair in samples.chunks_exact(2) {
        let Some(&gray) = pair.first() else {
            unreachable!("chunks_exact(2) always yields length-2 slices");
        };
        let Some(&alpha) = pair.get(1) else {
            unreachable!("chunks_exact(2) always yields length-2 slices");
        };
        out.push(gray);
        out.push(gray);
        out.push(gray);
        out.push(alpha);
    }
    out
}

/// Adds a fully-opaque alpha channel to `Red, Green, Blue` triples.
pub(crate) fn rgb_to_rgba(samples: &[f16]) -> Vec<f16> {
    let opaque = f16::from_f32(1.0);
    let mut out = Vec::with_capacity(samples.len() / 3 * 4);
    for triple in samples.chunks_exact(3) {
        let Some(&red) = triple.first() else {
            unreachable!("chunks_exact(3) always yields length-3 slices");
        };
        let Some(&green) = triple.get(1) else {
            unreachable!("chunks_exact(3) always yields length-3 slices");
        };
        let Some(&blue) = triple.get(2) else {
            unreachable!("chunks_exact(3) always yields length-3 slices");
        };
        out.push(red);
        out.push(green);
        out.push(blue);
        out.push(opaque);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{gray_alpha_to_rgba, gray_to_rgba, rgb_to_rgba};
    use half::f16;

    #[test]
    fn gray_to_rgba_duplicates_into_rgb_and_adds_opaque_alpha() {
        let samples = [f16::from_f32(0.5)];
        let expanded = gray_to_rgba(&samples);
        assert_eq!(
            expanded,
            vec![
                f16::from_f32(0.5),
                f16::from_f32(0.5),
                f16::from_f32(0.5),
                f16::from_f32(1.0),
            ]
        );
    }

    #[test]
    fn gray_alpha_to_rgba_duplicates_gray_and_keeps_real_alpha() {
        let samples = [f16::from_f32(0.5), f16::from_f32(0.25)];
        let expanded = gray_alpha_to_rgba(&samples);
        assert_eq!(
            expanded,
            vec![
                f16::from_f32(0.5),
                f16::from_f32(0.5),
                f16::from_f32(0.5),
                f16::from_f32(0.25),
            ]
        );
    }

    #[test]
    fn rgb_to_rgba_adds_opaque_alpha() {
        let samples = [f16::from_f32(0.1), f16::from_f32(0.2), f16::from_f32(0.3)];
        let expanded = rgb_to_rgba(&samples);
        assert_eq!(
            expanded,
            vec![
                f16::from_f32(0.1),
                f16::from_f32(0.2),
                f16::from_f32(0.3),
                f16::from_f32(1.0),
            ]
        );
    }
}
