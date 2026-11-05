//! 5x7 bitmap font lookup. Text is drawn as small rectangles (one per horizontal run of
//! lit pixels), so there is no font texture and no extra bind group.

use crate::glyphs::{COUNT, FIRST, GLYPHS};

/// Seven row bytes, bit 4 = leftmost pixel. Lowercase maps to uppercase; unknown is blank.
pub fn glyph(c: char) -> [u8; 7] {
    let u = c.to_ascii_uppercase() as u32;
    if u >= FIRST && u < FIRST + COUNT as u32 {
        GLYPHS[(u - FIRST) as usize]
    } else {
        [0; 7]
    }
}

pub fn has_art(c: char) -> bool {
    c == ' ' || glyph(c).iter().any(|&r| r != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_punctuation_have_art() {
        for c in ('A'..='Z').chain('0'..='9').chain(".:%+-/()=".chars()) {
            assert!(glyph(c).iter().any(|&r| r != 0), "no art for {c:?}");
        }
        assert!(glyph(' ').iter().all(|&r| r == 0));
    }

    #[test]
    fn lowercase_matches_uppercase_and_rows_fit_five_bits() {
        assert_eq!(glyph('g'), glyph('G'));
        for c in ('A'..='Z').chain('0'..='9') {
            assert!(glyph(c).iter().all(|&r| r < 32), "{c:?} has bits beyond column 5");
        }
    }

    #[test]
    fn non_ascii_is_blank_not_a_panic() {
        assert_eq!(glyph('é'), [0; 7]);
        assert_eq!(glyph('\u{1F600}'), [0; 7]);
    }
}
