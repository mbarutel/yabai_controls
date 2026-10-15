//! Hardware keycode table (`kVK_*` from Carbon's `Events.h`).
//!
//! These are physical key positions on a US ANSI layout, not characters.
//! `a` is 0x00 wherever the key labelled A sits on the user's layout.

/// (name, keycode). Names are lowercase; the first entry for a keycode is
/// the canonical name used by `name_for_keycode`.
const TABLE: &[(&str, u16)] = &[
    // Letters
    ("a", 0x00),
    ("s", 0x01),
    ("d", 0x02),
    ("f", 0x03),
    ("h", 0x04),
    ("g", 0x05),
    ("z", 0x06),
    ("x", 0x07),
    ("c", 0x08),
    ("v", 0x09),
    ("b", 0x0B),
    ("q", 0x0C),
    ("w", 0x0D),
    ("e", 0x0E),
    ("r", 0x0F),
    ("y", 0x10),
    ("t", 0x11),
    ("o", 0x1F),
    ("u", 0x20),
    ("i", 0x22),
    ("p", 0x23),
    ("l", 0x25),
    ("j", 0x26),
    ("k", 0x28),
    ("n", 0x2D),
    ("m", 0x2E),
    // Digits
    ("1", 0x12),
    ("2", 0x13),
    ("3", 0x14),
    ("4", 0x15),
    ("6", 0x16),
    ("5", 0x17),
    ("9", 0x19),
    ("7", 0x1A),
    ("8", 0x1C),
    ("0", 0x1D),
    // Punctuation
    ("equal", 0x18),
    ("=", 0x18),
    ("minus", 0x1B),
    ("-", 0x1B),
    ("rightbracket", 0x1E),
    ("]", 0x1E),
    ("leftbracket", 0x21),
    ("[", 0x21),
    ("quote", 0x27),
    ("'", 0x27),
    ("semicolon", 0x29),
    (";", 0x29),
    ("backslash", 0x2A),
    ("\\", 0x2A),
    ("comma", 0x2B),
    (",", 0x2B),
    ("slash", 0x2C),
    ("/", 0x2C),
    ("period", 0x2F),
    (".", 0x2F),
    ("grave", 0x32),
    ("`", 0x32),
    // Whitespace / control
    ("return", 0x24),
    ("enter", 0x24),
    ("tab", 0x30),
    ("space", 0x31),
    ("backspace", 0x33),
    ("delete", 0x33),
    ("escape", 0x35),
    ("esc", 0x35),
    ("forwarddelete", 0x75),
    ("home", 0x73),
    ("end", 0x77),
    ("pageup", 0x74),
    ("pagedown", 0x79),
    ("help", 0x72),
    // Arrows
    ("left", 0x7B),
    ("right", 0x7C),
    ("down", 0x7D),
    ("up", 0x7E),
    // Function keys
    ("f1", 0x7A),
    ("f2", 0x78),
    ("f3", 0x63),
    ("f4", 0x76),
    ("f5", 0x60),
    ("f6", 0x61),
    ("f7", 0x62),
    ("f8", 0x64),
    ("f9", 0x65),
    ("f10", 0x6D),
    ("f11", 0x67),
    ("f12", 0x6F),
    ("f13", 0x69),
    ("f14", 0x6B),
    ("f15", 0x71),
    ("f16", 0x6A),
    ("f17", 0x40),
    ("f18", 0x4F),
    ("f19", 0x50),
    ("f20", 0x5A),
    // Media
    ("volumeup", 0x48),
    ("volumedown", 0x49),
    ("mute", 0x4A),
    // Keypad
    ("keypad0", 0x52),
    ("keypad1", 0x53),
    ("keypad2", 0x54),
    ("keypad3", 0x55),
    ("keypad4", 0x56),
    ("keypad5", 0x57),
    ("keypad6", 0x58),
    ("keypad7", 0x59),
    ("keypad8", 0x5B),
    ("keypad9", 0x5C),
    ("keypaddecimal", 0x41),
    ("keypadmultiply", 0x43),
    ("keypadplus", 0x45),
    ("keypadclear", 0x47),
    ("keypaddivide", 0x4B),
    ("keypadenter", 0x4C),
    ("keypadminus", 0x4E),
    ("keypadequals", 0x51),
];

pub fn keycode_for_name(name: &str) -> Option<u16> {
    TABLE
        .iter()
        .find(|(n, _)| *n == name)
        .map(|&(_, code)| code)
}

pub fn name_for_keycode(code: u16) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(_, c)| *c == code)
        .map(|&(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_not_sequential() {
        // A common mistake is assuming 1..9 map to consecutive codes.
        assert_eq!(keycode_for_name("5"), Some(0x17));
        assert_eq!(keycode_for_name("6"), Some(0x16));
    }

    #[test]
    fn aliases_share_a_code_and_canonical_name_wins() {
        assert_eq!(keycode_for_name("enter"), keycode_for_name("return"));
        assert_eq!(name_for_keycode(0x24), Some("return"));
    }
}
