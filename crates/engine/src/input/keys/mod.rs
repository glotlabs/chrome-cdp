//! Keys (plan 05, milestone 5.3): the US keyboard layout, key names as
//! `press` takes them (`Enter`, `Control+A`, `Shift+Tab`, `Mod+A`), and
//! the key events a chord of them sends.
//!
//! The layout and the macOS editing commands ([`table`]) are generated
//! from Playwright's data by `bench/playwright/gen-keys.cjs`. A key is
//! named by its `code` (`KeyA`, `Digit1`, `Numpad0`), by the `key` it
//! types (`a`, `A`, `!`), or by a name (`Enter`, `ArrowDown`, `F5`,
//! `Shift`); multi-letter names also match in any case, with a few
//! synonyms (`Esc`, `Ctrl`, `Cmd`, `Up`, …).

#[rustfmt::skip]
mod table;

use table::{LAYOUT, MAC_EDITING_COMMANDS};

/// One key of the layout, as the table has it.
#[derive(Debug)]
pub(super) struct KeyDef {
    pub(super) code: &'static str,
    pub(super) key: &'static str,
    /// `keyCode` in the page (events send the one without location).
    #[allow(dead_code)]
    pub(super) key_code: u32,
    /// `windowsVirtualKeyCode` for keys on both sides (Shift, Control,
    /// Alt), which differ in `key_code`.
    pub(super) key_code_without_location: u32,
    /// What the key types with Shift held.
    pub(super) shift_key: Option<&'static str>,
    pub(super) shift_key_code: Option<u32>,
    /// What it types, when that is not `key` (Enter types `\r`).
    pub(super) text: &'static str,
    /// 1 left, 2 right, 3 numpad.
    pub(super) location: u8,
}

/// `KeyboardEvent.location` of the numpad.
const NUMPAD: u8 = 3;

/// What a key event says: from the layout, with Shift applied.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Key {
    pub(crate) code: &'static str,
    pub(crate) key: &'static str,
    /// `windowsVirtualKeyCode`.
    pub(crate) key_code: u32,
    /// What it types (empty for keys that type nothing).
    pub(crate) text: &'static str,
    pub(crate) location: u8,
}

impl Key {
    pub(crate) fn is_keypad(&self) -> bool {
        self.location == NUMPAD
    }

    /// The modifier this key is, if any.
    pub(crate) fn modifier(&self) -> Option<Mod> {
        Some(match self.key {
            "Alt" => Mod::Alt,
            "Control" => Mod::Control,
            "Meta" => Mod::Meta,
            "Shift" => Mod::Shift,
            _ => return None,
        })
    }
}

/// A modifier key, as a bit of `Input` events' `modifiers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mod {
    Alt = 1,
    Control = 2,
    Meta = 4,
    Shift = 8,
}

/// A key as a name finds it: itself, and what it is with Shift held.
#[derive(Debug, Clone, Copy)]
struct Found {
    key: Key,
    shifted: Option<Key>,
}

fn unshifted(def: &KeyDef) -> Key {
    Key {
        code: def.code,
        key: def.key,
        key_code: def.key_code_without_location,
        text: if !def.text.is_empty() {
            def.text
        } else if def.key.chars().count() == 1 {
            def.key
        } else {
            ""
        },
        location: def.location,
    }
}

fn shifted(def: &KeyDef) -> Option<Key> {
    def.shift_key.map(|key| Key {
        key,
        text: key,
        key_code: def.shift_key_code.unwrap_or(def.key_code_without_location),
        ..unshifted(def)
    })
}

fn by_code(code: &str) -> Option<&'static KeyDef> {
    LAYOUT.iter().find(|d| d.code == code)
}

/// Names that stand for another (`Shift` is the left one; `\n` is Enter).
const ALIASES: &[(&str, &str)] = &[
    ("Shift", "ShiftLeft"),
    ("Control", "ControlLeft"),
    ("Alt", "AltLeft"),
    ("Meta", "MetaLeft"),
    ("\n", "Enter"),
    ("\r", "Enter"),
];

/// Synonyms matched in any case, for names agents often type.
const SYNONYMS: &[(&str, &str)] = &[
    ("esc", "Escape"),
    ("return", "Enter"),
    ("ctrl", "ControlLeft"),
    ("control", "ControlLeft"),
    ("shift", "ShiftLeft"),
    ("alt", "AltLeft"),
    ("option", "AltLeft"),
    ("meta", "MetaLeft"),
    ("cmd", "MetaLeft"),
    ("command", "MetaLeft"),
    ("del", "Delete"),
    ("ins", "Insert"),
    ("up", "ArrowUp"),
    ("down", "ArrowDown"),
    ("left", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    ("spacebar", "Space"),
];

/// The key `name` names (as `press` takes it; `Mod` resolved already).
fn find(name: &str) -> Option<Found> {
    if let Some(def) = by_code(name) {
        return Some(Found {
            key: unshifted(def),
            shifted: shifted(def),
        });
    }
    if let Some((_, code)) = ALIASES.iter().find(|(alias, _)| *alias == name) {
        let def = by_code(code)?;
        return Some(Found {
            key: unshifted(def),
            shifted: None,
        });
    }
    // What a key off the numpad types.
    for def in LAYOUT.iter().filter(|d| d.location == 0) {
        if def.key.chars().count() == 1 && def.key == name {
            return Some(Found {
                key: unshifted(def),
                shifted: shifted(def),
            });
        }
        if def.shift_key == Some(name) {
            return shifted(def).map(|key| Found { key, shifted: None });
        }
    }
    if name.chars().count() < 2 {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let code = SYNONYMS
        .iter()
        .find(|(synonym, _)| *synonym == lower)
        .map(|(_, code)| *code)
        .or_else(|| {
            LAYOUT
                .iter()
                .find(|d| {
                    d.code.eq_ignore_ascii_case(name)
                        || (d.key.chars().count() > 1 && d.key.eq_ignore_ascii_case(name))
                })
                .map(|d| d.code)
        })?;
    let def = by_code(code)?;
    Some(Found {
        key: unshifted(def),
        shifted: shifted(def),
    })
}

/// Whether `name` is `Mod`: Meta on macOS browsers, Control elsewhere.
pub(crate) fn is_mod(name: &str) -> bool {
    name.eq_ignore_ascii_case("mod")
}

/// A chord: keys held together (`Control+Shift+Z`), in order.
pub(crate) type Chord = Vec<String>;

/// Splits `press`'s argument into chords: whitespace separates chords,
/// `+` the keys of a chord (a `+` that ends a chord or stands alone is
/// the key). Checks that every key exists.
pub(crate) fn parse(keys: &str) -> Result<Vec<Chord>, String> {
    let chords: Vec<Chord> = keys.split_whitespace().map(split_chord).collect();
    if chords.is_empty() {
        return Err("no keys to press".to_owned());
    }
    for name in chords.iter().flatten() {
        if !is_mod(name) && find(name).is_none() {
            return Err(format!("unknown key `{name}`"));
        }
    }
    Ok(chords)
}

/// `Control+A` → `[Control, A]`; `Shift++` → `[Shift, +]`.
fn split_chord(chord: &str) -> Chord {
    let mut keys = Vec::new();
    let mut building = String::new();
    for c in chord.chars() {
        if c == '+' && !building.is_empty() {
            keys.push(std::mem::take(&mut building));
        } else {
            building.push(c);
        }
    }
    keys.push(building);
    keys
}

/// One key event of a chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyEvent {
    pub(crate) down: bool,
    pub(crate) key: Key,
    /// The modifiers held (after a modifier's own down, before its up).
    pub(crate) modifiers: i64,
    /// Editing commands for a macOS browser (`selectAll`, …).
    pub(crate) commands: Vec<&'static str>,
}

/// The events of `chord`: each key down in order, then up in reverse.
/// With a modifier other than Shift held, keys type nothing; with Shift
/// held, keys are their shifted selves. `mac` is the browser's platform:
/// `Mod` is Meta there (Control elsewhere), and key downs carry the
/// editing commands macOS would run for them.
pub(crate) fn chord_events(chord: &[String], mac: bool) -> Result<Vec<KeyEvent>, String> {
    let mut held: Vec<Mod> = Vec::new();
    let bits = |held: &[Mod]| held.iter().fold(0, |bits, m| bits | *m as i64);
    let mut keys = Vec::new();
    let mut events = Vec::new();
    for name in chord {
        let name = if is_mod(name) {
            if mac { "Meta" } else { "Control" }
        } else {
            name.as_str()
        };
        let found = find(name).ok_or_else(|| format!("unknown key `{name}`"))?;
        let mut key = match found.shifted {
            Some(shifted) if held.contains(&Mod::Shift) => shifted,
            _ => found.key,
        };
        if held.len() > 1 || (held.len() == 1 && held[0] != Mod::Shift) {
            key.text = "";
        }
        if let Some(m) = key.modifier()
            && !held.contains(&m)
        {
            held.push(m);
        }
        let commands = if mac {
            mac_commands(&held, key.code)
        } else {
            Vec::new()
        };
        events.push(KeyEvent {
            down: true,
            key,
            modifiers: bits(&held),
            commands,
        });
        keys.push(key);
    }
    for key in keys.into_iter().rev() {
        if let Some(m) = key.modifier() {
            held.retain(|h| *h != m);
        }
        events.push(KeyEvent {
            down: false,
            key,
            modifiers: bits(&held),
            commands: Vec::new(),
        });
    }
    Ok(events)
}

/// The editing commands macOS runs for `code` with `held`.
fn mac_commands(held: &[Mod], code: &str) -> Vec<&'static str> {
    let mut shortcut = String::new();
    for (m, name) in [
        (Mod::Shift, "Shift"),
        (Mod::Control, "Control"),
        (Mod::Alt, "Alt"),
        (Mod::Meta, "Meta"),
    ] {
        if held.contains(&m) {
            shortcut.push_str(name);
            shortcut.push('+');
        }
    }
    shortcut.push_str(code);
    MAC_EDITING_COMMANDS
        .iter()
        .find(|(s, _)| *s == shortcut)
        .map(|(_, commands)| commands.to_vec())
        .unwrap_or_default()
}

/// The key that types `c` on the layout, without modifiers, if any (for
/// `type`; others are inserted as text).
pub(crate) fn typed(c: char) -> Option<Key> {
    let mut buf = [0; 4];
    let name: &str = c.encode_utf8(&mut buf);
    if c.is_whitespace() && !matches!(c, ' ' | '\n' | '\r') {
        return None;
    }
    // Only what the layout types, not names (a lone letter always is one).
    let found = find(name)?;
    (found.key.text == name || (name == "\n" && found.key.code == "Enter")).then_some(found.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(events: &[KeyEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                format!(
                    "{} {} {} {:?} m{}{}",
                    if e.down { "down" } else { "up" },
                    e.key.key,
                    e.key.code,
                    e.key.text,
                    e.modifiers,
                    if e.commands.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", e.commands.join(","))
                    }
                )
            })
            .collect()
    }

    fn chord(text: &str) -> Chord {
        split_chord(text)
    }

    #[test]
    fn chords_split_on_spaces_and_plus() {
        assert_eq!(
            parse("Control+A Backspace").unwrap(),
            vec![chord("Control+A"), vec!["Backspace".to_owned()]]
        );
        assert_eq!(split_chord("Shift++"), ["Shift", "+"]);
        assert_eq!(split_chord("+"), ["+"]);
        assert_eq!(parse("Mod+a").unwrap(), vec![chord("Mod+a")]);
        assert_eq!(parse("  ").unwrap_err(), "no keys to press");
        assert_eq!(parse("Control+Nope").unwrap_err(), "unknown key `Nope`");
    }

    #[test]
    fn names_codes_characters_and_synonyms_find_keys() {
        let key = |name: &str| find(name).map(|f| f.key.code);
        assert_eq!(key("Enter"), Some("Enter"));
        assert_eq!(key("enter"), Some("Enter"));
        assert_eq!(key("KeyA"), Some("KeyA"));
        assert_eq!(key("a"), Some("KeyA"));
        assert_eq!(key("A"), Some("KeyA"));
        assert_eq!(key("!"), Some("Digit1"));
        assert_eq!(key("Esc"), Some("Escape"));
        assert_eq!(key("ctrl"), Some("ControlLeft"));
        assert_eq!(key("Shift"), Some("ShiftLeft"));
        assert_eq!(key("up"), Some("ArrowUp"));
        assert_eq!(key("F12"), Some("F12"));
        assert_eq!(key("f5"), Some("F5"));
        assert_eq!(key("Numpad5"), Some("Numpad5"));
        assert_eq!(key("Space"), Some("Space"));
        assert_eq!(key(" "), Some("Space"));
        assert_eq!(key("\n"), Some("Enter"));
        assert_eq!(key("é"), None);
        assert_eq!(key("Hyper"), None);
        let upper = find("A").unwrap().key;
        assert_eq!((upper.key, upper.text, upper.key_code), ("A", "A", 65));
        let enter = find("Enter").unwrap().key;
        assert_eq!((enter.text, enter.key_code), ("\r", 13));
        let shift = find("Shift").unwrap().key;
        assert_eq!((shift.key_code, shift.location), (16, 1));
        assert!(find("NumpadEnter").unwrap().key.is_keypad());
    }

    #[test]
    fn a_chord_holds_modifiers_around_its_key() {
        let events = chord_events(&chord("Control+a"), false).unwrap();
        assert_eq!(
            names(&events),
            [
                "down Control ControlLeft \"\" m2",
                "down a KeyA \"\" m2",
                "up a KeyA \"\" m2",
                "up Control ControlLeft \"\" m0",
            ]
        );
        // Shift makes keys their shifted selves, and they still type.
        let events = chord_events(&chord("Shift+1"), false).unwrap();
        assert_eq!(names(&events)[1], "down ! Digit1 \"!\" m8");
        let events = chord_events(&chord("Shift+Tab"), false).unwrap();
        assert_eq!(names(&events)[1], "down Tab Tab \"\" m8");
        let events = chord_events(&chord("Enter"), false).unwrap();
        assert_eq!(
            names(&events),
            ["down Enter Enter \"\\r\" m0", "up Enter Enter \"\\r\" m0"]
        );
        // Shift with another modifier types nothing.
        let events = chord_events(&chord("Control+Shift+z"), false).unwrap();
        assert_eq!(names(&events)[2], "down Z KeyZ \"\" m10");
    }

    #[test]
    fn mod_and_editing_commands_follow_the_browser_platform() {
        let linux = chord_events(&chord("Mod+a"), false).unwrap();
        assert_eq!(linux[0].key.key, "Control");
        assert!(linux.iter().all(|e| e.commands.is_empty()));
        let mac = chord_events(&chord("Mod+a"), true).unwrap();
        assert_eq!(
            names(&mac),
            [
                "down Meta MetaLeft \"\" m4",
                "down a KeyA \"\" m4 selectAll",
                "up a KeyA \"\" m4",
                "up Meta MetaLeft \"\" m0",
            ]
        );
        let redo = chord_events(&chord("Shift+Meta+z"), true).unwrap();
        assert_eq!(redo[2].commands, ["redo"]);
        let back = chord_events(&chord("Backspace"), true).unwrap();
        assert_eq!(back[0].commands, ["deleteBackward"]);
        // Commands that insert text are left to the key's own text.
        let enter = chord_events(&chord("Enter"), true).unwrap();
        assert!(enter[0].commands.is_empty());
    }

    #[test]
    fn typing_uses_the_layout_for_what_it_types() {
        let code = |c| typed(c).map(|k| (k.code, k.text));
        assert_eq!(code('a'), Some(("KeyA", "a")));
        assert_eq!(code('A'), Some(("KeyA", "A")));
        assert_eq!(code('?'), Some(("Slash", "?")));
        assert_eq!(code(' '), Some(("Space", " ")));
        assert_eq!(code('\n'), Some(("Enter", "\r")));
        assert_eq!(code('é'), None);
        assert_eq!(code('😀'), None);
        assert_eq!(code('\t'), None);
    }
}
