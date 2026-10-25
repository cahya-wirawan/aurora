//! The 256-bit bearer token and its comparison.

use std::sync::atomic::{AtomicU64, Ordering};

/// Length of a token in hexadecimal characters (256 bits).
pub const TOKEN_HEX_LEN: usize = 64;

/// A fresh token from the operating system's random number generator.
///
/// # Errors
/// The generator failed (it never does on a working system); the caller
/// must not fall back to anything weaker.
pub fn generate() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| format!("the OS random number generator failed: {e}"))?;
    Ok(hex(&bytes))
}

/// Lower-case hexadecimal.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(
            DIGITS.get(usize::from(byte >> 4)).copied().unwrap_or(b'0'),
        ));
        out.push(char::from(
            DIGITS.get(usize::from(byte & 15)).copied().unwrap_or(b'0'),
        ));
    }
    out
}

/// Whether `text` has the shape of a token (64 hexadecimal characters).
#[must_use]
pub fn well_formed(text: &str) -> bool {
    text.len() == TOKEN_HEX_LEN && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Compares two byte strings without stopping at the first difference: the
/// time taken depends on the longer length only, never on where (or
/// whether) the inputs differ. Returns the verdict and how many positions
/// were examined (the probe the tests use to pin the no-early-exit rule).
#[must_use]
pub fn ct_eq_probe(a: &[u8], b: &[u8]) -> (bool, usize) {
    let length = a.len().max(b.len());
    let mut diff = u64::try_from(a.len() ^ b.len()).unwrap_or(u64::MAX);
    let mut examined = 0_usize;
    for index in 0..length {
        let x = a.get(index).copied().unwrap_or(0);
        let y = b.get(index).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
        examined += 1;
    }
    (std::hint::black_box(diff) == 0, examined)
}

/// [`ct_eq_probe`]'s verdict.
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    ct_eq_probe(a, b).0
}

static VERIFICATIONS: AtomicU64 = AtomicU64::new(0);

/// Checks a presented token against the real one. The one place a token is
/// compared; counts its calls so a test can prove the listener goes through
/// it.
#[must_use]
pub fn verify(presented: &str, expected: &str) -> bool {
    VERIFICATIONS.fetch_add(1, Ordering::Relaxed);
    ct_eq(presented.as_bytes(), expected.as_bytes())
}

/// How many times [`verify`] has run in this process (tests).
#[must_use]
pub fn verifications() -> u64 {
    VERIFICATIONS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_generated_token_is_64_hex_and_not_repeated() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert!(well_formed(&a), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn comparison_is_right_and_never_stops_early() {
        assert_eq!(ct_eq_probe(b"abcd", b"abcd"), (true, 4));
        // differs at the first byte: still every position is examined
        assert_eq!(ct_eq_probe(b"xbcd", b"abcd"), (false, 4));
        // differs at the last byte
        assert_eq!(ct_eq_probe(b"abcx", b"abcd"), (false, 4));
        // different lengths: false, and the longer length is walked
        assert_eq!(ct_eq_probe(b"abc", b"abcd"), (false, 4));
        assert_eq!(ct_eq_probe(b"", b""), (true, 0));
        // a prefix of zero bytes does not equal a shorter string
        assert!(!ct_eq(b"a\0", b"a"));
    }

    #[test]
    fn hex_is_lower_case_and_padded() {
        assert_eq!(hex(&[0, 1, 0xab, 0xff]), "0001abff");
    }
}
