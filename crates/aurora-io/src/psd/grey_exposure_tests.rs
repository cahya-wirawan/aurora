//! Exposure in a Grayscale file through its grey profile (task 112): the
//! profile resource (1039) reaches the reader, an Exposure layer of a
//! Grayscale file becomes `Adjustment::ExposureTrc` only with a 256-entry
//! tone curve, every other profile is left out with its own report line,
//! a RGB file ignores the profile, hostile profiles are safe, and the
//! corpus fixture (which prints `SKIPPED` when absent) carries Dot Gain
//! 20%.

#![allow(clippy::indexing_slicing, clippy::too_many_lines)]

use aurora_doc::Adjustment;

use super::PsdDocument;
use super::read;
use super::read_adjust_tests::{EXPA_FILL, adjustments_of, corpus, unhex};
use super::test_writer::{TestLayer, TestPsd};
use super::write_tests::ok;

/// Resource 1039 holding `body`.
fn resource_1039(body: &[u8]) -> Vec<u8> {
    let mut r = b"8BIM".to_vec();
    r.extend_from_slice(&1039_u16.to_be_bytes());
    r.extend_from_slice(&[0, 0]);
    r.extend_from_slice(&(body.len() as u32).to_be_bytes());
    r.extend_from_slice(body);
    if body.len() % 2 == 1 {
        r.push(0);
    }
    r
}

/// A `GRAY` profile whose only tag is a `kTRC` of the given type bytes.
fn gray_profile(tag: &[u8]) -> Vec<u8> {
    let mut p = vec![0_u8; 128];
    p[16..20].copy_from_slice(b"GRAY");
    p.extend_from_slice(&1_u32.to_be_bytes());
    p.extend_from_slice(b"kTRC");
    p.extend_from_slice(&144_u32.to_be_bytes());
    p.extend_from_slice(&(tag.len() as u32).to_be_bytes());
    p.extend_from_slice(tag);
    p
}

fn curv(entries: &[u16]) -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for e in entries {
        t.extend_from_slice(&e.to_be_bytes());
    }
    t
}

/// A smooth dot-gain-like table, 0 to 65535.
fn table(n: usize) -> Vec<u16> {
    (0..n)
        .map(|i| {
            let x = i as f64 / (n - 1) as f64;
            ((x.powf(1.6) * 0.9 + x * 0.1) * 65535.0).round() as u16
        })
        .collect()
}

/// A 2 x 2 file (Grayscale or RGB) with one Exposure layer and `profile`
/// as resource 1039 (none when `None`).
fn file(gray: bool, profile: Option<&[u8]>) -> TestPsd {
    let layer = TestLayer::empty("Exposure 1").with(|l| {
        l.channels.retain(|(id, _)| *id < 1);
        l.blocks.push((*b"expA", unhex(EXPA_FILL)));
    });
    let base = if gray {
        TestLayer::gray(
            "p",
            0,
            0,
            2,
            2,
            8,
            &[[10, 255], [20, 255], [30, 255], [40, 255]],
        )
    } else {
        TestLayer::pixels("p", 0, 0, 2, 2, 8, &[[10, 20, 30, 255]; 4])
    };
    TestPsd::new(1, 2, 2, 8).with(|p| {
        if gray {
            p.color_mode = 1;
            p.channels = 1;
        }
        if let Some(profile) = profile {
            p.resources = resource_1039(profile);
        }
        p.layers = vec![
            TestLayer::divider(),
            base,
            layer,
            TestLayer::group("G", *b"pass"),
        ];
    })
}

fn open(f: &TestPsd) -> PsdDocument {
    ok(read(&f.write()))
}

fn items(d: &PsdDocument) -> String {
    d.report.items.join("\n")
}

fn only(d: &PsdDocument) -> Option<Adjustment> {
    let all = adjustments_of(d);
    (all.len() == 1).then(|| all[0].1.clone())
}

#[test]
fn a_256_entry_table_makes_a_grey_profile_exposure() {
    let entries = table(256);
    let d = open(&file(true, Some(&gray_profile(&curv(&entries)))));
    let Some(Adjustment::ExposureTrc(p)) = only(&d) else {
        unreachable!("imported: {}", items(&d))
    };
    assert_eq!(p.trc().entries(), entries.as_slice());
    // EXPA_FILL is the fixture's own block; the sliders are what it holds.
    assert!(
        items(&d).contains(
            "1 Exposure layer in this Grayscale file is drawn through the file's grey profile"
        ),
        "{}",
        items(&d)
    );
    assert!(!items(&d).contains("left out"), "{}", items(&d));
}

#[test]
fn other_profiles_leave_the_layer_out_with_their_own_line() {
    let mut down = table(256);
    down.swap(100, 101);
    let mut short = table(256);
    short[255] = 60000;
    for (profile, clause) in [
        (gray_profile(&curv(&[])), "plain gamma curve"),
        (gray_profile(&curv(&[0x0233])), "plain gamma curve"),
        (gray_profile(b"para\0\0\0\0\0\0\0\0"), "parametric curve"),
        (
            gray_profile(&curv(&table(1024))),
            "(it has checked 256 entries)",
        ),
        (gray_profile(&curv(&down)), "decreases or does not run"),
        (gray_profile(&curv(&short)), "decreases or does not run"),
        (gray_profile(b"mft2\0\0\0\0\0\0\0\0"), "could not be read"),
        (
            {
                let mut p = gray_profile(&curv(&table(256)));
                p[16..20].copy_from_slice(b"RGB ");
                p
            },
            "is not a grey profile",
        ),
    ] {
        let d = open(&file(true, Some(&profile)));
        assert!(adjustments_of(&d).is_empty(), "{clause}");
        let text = items(&d);
        assert!(
            text.contains("1 Exposure layer in this Grayscale file was left out")
                && text.contains(clause),
            "{clause}: {text}"
        );
        assert!(
            !text.contains("hasn't yet verified how Photoshop applies Exposure"),
            "{text}"
        );
    }
}

#[test]
fn no_profile_keeps_the_old_left_out_line() {
    let d = open(&file(true, None));
    assert!(adjustments_of(&d).is_empty());
    assert!(
        items(&d)
            .contains("hasn't yet verified how Photoshop applies Exposure to a Grayscale image"),
        "{}",
        items(&d)
    );
}

#[test]
fn an_rgb_file_ignores_a_grey_profile_and_keeps_the_plain_exposure() {
    let d = open(&file(false, Some(&gray_profile(&curv(&table(256))))));
    assert!(
        matches!(only(&d), Some(Adjustment::Exposure(_))),
        "{}",
        items(&d)
    );
    assert!(!items(&d).contains("grey profile"), "{}", items(&d));
}

#[test]
fn the_first_profile_resource_wins() {
    let mut f = file(true, None);
    let mut two = resource_1039(&gray_profile(&curv(&table(256))));
    two.extend(resource_1039(&gray_profile(&curv(&[]))));
    f.resources = two;
    let d = open(&f);
    assert!(matches!(only(&d), Some(Adjustment::ExposureTrc(_))));
}

#[test]
fn every_truncation_of_a_real_shaped_profile_is_safe() {
    let good = gray_profile(&curv(&table(256)));
    for n in (0..good.len()).step_by(7).chain([good.len() - 1]) {
        let d = open(&file(true, Some(&good[..n])));
        assert!(
            !matches!(only(&d), Some(Adjustment::ExposureTrc(_))),
            "{n} bytes cannot give a table"
        );
        assert!(adjustments_of(&d).is_empty(), "{n}");
    }
    // The whole thing does.
    assert!(matches!(
        only(&open(&file(true, Some(&good)))),
        Some(Adjustment::ExposureTrc(_))
    ));
}

#[test]
fn a_hostile_curve_count_is_refused_without_allocating() {
    let mut huge = b"curv\0\0\0\0".to_vec();
    huge.extend_from_slice(&u32::MAX.to_be_bytes());
    let d = open(&file(true, Some(&gray_profile(&huge))));
    assert!(adjustments_of(&d).is_empty());
    assert!(items(&d).contains("could not be read") || items(&d).contains("checked 256"));
}

#[test]
fn an_out_of_range_exposure_is_still_left_out_with_a_valid_profile() {
    // Gamma 0.05 is below Aurora's 0.10.
    let mut block = unhex(EXPA_FILL);
    block[10..14].copy_from_slice(&0.05_f32.to_bits().to_be_bytes());
    let mut f = file(true, Some(&gray_profile(&curv(&table(256)))));
    for layer in &mut f.layers {
        for b in &mut layer.blocks {
            if &b.0 == b"expA" {
                b.1 = block.clone();
            }
        }
    }
    let d = open(&f);
    assert!(adjustments_of(&d).is_empty());
    assert!(items(&d).contains("Exposure layer"), "{}", items(&d));
}

#[test]
fn the_corpus_fixture_carries_dot_gain_20_on_all_four_layers() {
    let Some(d) = corpus("adjustments/exposure_grayscale.psd") else {
        return;
    };
    let all = adjustments_of(&d);
    assert_eq!(all.len(), 4, "{}", items(&d));
    let mut first = None;
    for (name, a) in &all {
        let Adjustment::ExposureTrc(p) = a else {
            unreachable!("{name}: {a:?}")
        };
        assert_eq!(p.trc().entries().len(), 256, "{name}");
        // Dot Gain 20%: the 128th entry is the measured 0.30205.
        let mid = f64::from(p.trc().entries()[128]) / 65535.0;
        assert!((mid - 0.302_05).abs() < 1e-4, "{name}: {mid}");
        first.get_or_insert_with(|| p.trc().clone());
        assert_eq!(Some(p.trc()), first.as_ref(), "{name}: one profile");
    }
    assert!(
        items(&d).contains("4 Exposure layers in this Grayscale file are drawn through"),
        "{}",
        items(&d)
    );
    assert!(!items(&d).contains("left out"), "{}", items(&d));
}

#[test]
fn the_corpus_rgb_control_stays_a_plain_exposure() {
    let Some(d) = corpus("adjustments/exposure_rgb.psd") else {
        return;
    };
    let all = adjustments_of(&d);
    assert_eq!(all.len(), 4);
    assert!(
        all.iter()
            .all(|(_, a)| matches!(a, Adjustment::Exposure(_)))
    );
}
