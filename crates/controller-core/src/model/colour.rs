//! A colour setting's value in a profile: `#` and six hex digits, red then green then
//! blue, such as `#7e3200`.

/// The red, green and blue bits of a colour code, `0x00RRGGBB`.
const RGB_MASK: u32 = 0x00FF_FFFF;

/// The colour `text` names, as `0x00RRGGBB`, if it is `#` and six hex digits. Either
/// case of digit is accepted.
#[must_use]
pub fn parse(text: &str) -> Option<u32> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// `code` as `#rrggbb` in lower case. Bits above the low 24 are dropped.
#[must_use]
pub fn format(code: u32) -> String {
    format!("#{:06x}", code & RGB_MASK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_reads_back_as_written() {
        assert_eq!(parse("#7E3200"), Some(0x007E_3200));
        assert_eq!(format(0x2000_7E32), "#007e32", "the top byte is no colour");
        assert_eq!(parse(&format(0x00CC_6200)), Some(0x00CC_6200));
    }

    #[test]
    fn anything_but_hash_and_six_hex_digits_is_no_colour() {
        for text in ["7e3200", "#7e320", "#7e32000", "#7e320g", "", "#", "#+7e320"] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
