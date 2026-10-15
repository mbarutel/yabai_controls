//! Hotkey representation and chord parsing.
//!
//! Chord syntax follows skhd: `modifiers - key`, where modifiers are joined
//! with `+`. Examples: `alt - 1`, `cmd + shift - return`, `escape` (no
//! modifier). Key names are listed in `keycodes.rs`.

pub mod keycodes;
pub mod tap;

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use core_graphics::event::CGEventFlags;

/// Modifier mask using CGEventFlags bits so a tapped event can be compared
/// directly. Only the four "real" modifiers plus `fn` are tracked; caps lock,
/// numeric pad and the left/right distinction are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u64);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(CGEventFlags::CGEventFlagShift.bits());
    pub const CTRL: Modifiers = Modifiers(CGEventFlags::CGEventFlagControl.bits());
    pub const ALT: Modifiers = Modifiers(CGEventFlags::CGEventFlagAlternate.bits());
    pub const CMD: Modifiers = Modifiers(CGEventFlags::CGEventFlagCommand.bits());
    pub const FN: Modifiers = Modifiers(CGEventFlags::CGEventFlagSecondaryFn.bits());

    const MASK: u64 = Self::SHIFT.0 | Self::CTRL.0 | Self::ALT.0 | Self::CMD.0 | Self::FN.0;

    /// Keeps only the modifier bits we care about from a raw event flag set.
    pub fn from_flags(flags: CGEventFlags) -> Self {
        Modifiers(flags.bits() & Self::MASK)
    }

    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn parse_name(name: &str) -> Option<Modifiers> {
        Some(match name {
            "shift" => Self::SHIFT,
            "ctrl" | "control" => Self::CTRL,
            "alt" | "opt" | "option" => Self::ALT,
            "cmd" | "command" => Self::CMD,
            "fn" => Self::FN,
            _ => return None,
        })
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

impl fmt::Display for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = [
            (Self::CMD, "cmd"),
            (Self::CTRL, "ctrl"),
            (Self::ALT, "alt"),
            (Self::SHIFT, "shift"),
            (Self::FN, "fn"),
        ];
        let mut first = true;
        for (m, name) in names {
            if self.contains(m) {
                if !first {
                    f.write_str(" + ")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

/// A modifier set plus a hardware keycode (`kVK_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub keycode: u16,
}

impl Hotkey {
    pub fn new(modifiers: Modifiers, keycode: u16) -> Self {
        Self { modifiers, keycode }
    }
}

impl FromStr for Hotkey {
    type Err = anyhow::Error;

    /// Parses `mod + mod - key` or a bare `key`. The separator is the last
    /// ` - ` (with surrounding whitespace) so the `-` character itself can be
    /// written as `minus`.
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            bail!("empty hotkey");
        }

        let (mods_part, key_part) = match s.rfind(" - ").or_else(|| s.rfind('-').filter(|&i| i > 0))
        {
            Some(idx) => {
                let sep_len = if s[idx..].starts_with(" - ") { 3 } else { 1 };
                (s[..idx].trim(), s[idx + sep_len..].trim())
            }
            None => ("", s),
        };

        let mut modifiers = Modifiers::NONE;
        if !mods_part.is_empty() {
            for name in mods_part.split('+') {
                let name = name.trim().to_ascii_lowercase();
                let m = Modifiers::parse_name(&name)
                    .ok_or_else(|| anyhow!("unknown modifier '{name}' in hotkey '{s}'"))?;
                modifiers = modifiers | m;
            }
        }

        let keycode = keycodes::keycode_for_name(&key_part.to_ascii_lowercase())
            .ok_or_else(|| anyhow!("unknown key '{key_part}' in hotkey '{s}'"))?;

        Ok(Hotkey::new(modifiers, keycode))
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = keycodes::name_for_keycode(self.keycode).unwrap_or("?");
        if self.modifiers.is_empty() {
            f.write_str(key)
        } else {
            write!(f, "{} - {key}", self.modifiers)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_modifier_chord() {
        let hk: Hotkey = "alt - 1".parse().unwrap();
        assert_eq!(
            hk,
            Hotkey::new(Modifiers::ALT, keycodes::keycode_for_name("1").unwrap())
        );
    }

    #[test]
    fn parses_multiple_modifiers_and_named_keys() {
        let hk: Hotkey = "cmd + shift - return".parse().unwrap();
        assert_eq!(hk.modifiers, Modifiers::CMD | Modifiers::SHIFT);
        assert_eq!(hk.keycode, 0x24);
    }

    #[test]
    fn parses_bare_key_and_compact_form() {
        assert_eq!(
            "escape".parse::<Hotkey>().unwrap().modifiers,
            Modifiers::NONE
        );
        assert_eq!("alt-h".parse::<Hotkey>().unwrap().modifiers, Modifiers::ALT);
    }

    #[test]
    fn parses_minus_key_by_name() {
        let hk: Hotkey = "alt - minus".parse().unwrap();
        assert_eq!(hk.keycode, 0x1B);
    }

    #[test]
    fn rejects_unknown_modifier_and_key() {
        assert!("hyper - a".parse::<Hotkey>().is_err());
        assert!("alt - nosuchkey".parse::<Hotkey>().is_err());
    }

    #[test]
    fn display_round_trips() {
        let hk: Hotkey = "ctrl + alt - left".parse().unwrap();
        assert_eq!(hk.to_string().parse::<Hotkey>().unwrap(), hk);
    }
}
