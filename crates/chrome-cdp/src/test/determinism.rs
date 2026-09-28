//! What makes each test's browser context behave the same in every run
//! (plan 11, milestone 11.6), from `[e2e.determinism]` with its defaults
//! ([`e2e::config::Fixed`]): a frozen clock (`time freeze`, which mocks'
//! `{{now}}` follows too), seeded random values and no animations (an
//! init script, [`script`]), `prefers-reduced-motion`, and a fixed locale
//! and time zone. The runner sends [`commands`] to a fresh context
//! before the test's steps; the viewport and pixel ratio are set with the
//! test's `@viewport` or `@device`.

use e2e::config::Fixed;

/// The init script of `fixed`'s random seed and animations, if either is
/// on.
pub(super) fn script(fixed: &Fixed) -> Option<String> {
    if fixed.random_seed.is_none() && !fixed.disable_animations {
        return None;
    }
    let (low, high) = fixed.random_seed.map_or_else(
        || ("null".to_owned(), "null".to_owned()),
        |seed| {
            // The halves of the seed, as JS numbers hold 53 bits only.
            let [low, high] = [seed & 0xffff_ffff, seed >> 32];
            (low.to_string(), high.to_string())
        },
    );
    Some(format!(
        "{}({low}, {high}, {});",
        include_str!("determinism.js").trim_end(),
        fixed.disable_animations
    ))
}

/// The commands that give a context `fixed`'s settings, in order.
pub(super) fn commands(fixed: &Fixed) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let words = |words: &[&str]| words.iter().map(|&w| w.to_owned()).collect::<Vec<_>>();
    if let Some(time) = &fixed.freeze_time {
        commands.push(words(&["time", "freeze", time]));
    }
    if let Some(code) = script(fixed) {
        commands.push(vec![
            "init-script".into(),
            "add".into(),
            "--code".into(),
            code,
        ]);
    }
    if fixed.disable_animations {
        commands.push(words(&["media", "--reduced-motion", "reduce"]));
    }
    if let Some(locale) = &fixed.locale {
        commands.push(words(&["locale", locale]));
    }
    if let Some(timezone) = &fixed.timezone {
        commands.push(words(&["tz", timezone]));
    }
    commands
}

#[cfg(test)]
mod tests {
    use e2e::config::Fixed;

    use super::{commands, script};

    fn fixed() -> Fixed {
        e2e::config::Determinism::default().resolve()
    }

    #[test]
    fn every_setting_is_a_command() {
        let all = commands(&fixed());
        let heads: Vec<String> = all.iter().map(|c| c[..2].join(" ")).collect();
        assert_eq!(
            heads,
            [
                "time freeze",
                "init-script add",
                "media --reduced-motion",
                "locale en-US",
                "tz UTC"
            ]
        );
        assert_eq!(all[0][2], "2026-01-01T10:00:00Z");
        assert!(all[1][3].ends_with("(0, 0, true);"), "{}", all[1][3]);
        let off = Fixed {
            freeze_time: None,
            random_seed: None,
            disable_animations: false,
            locale: None,
            timezone: None,
        };
        assert!(commands(&off).is_empty());
    }

    #[test]
    fn a_large_seed_is_split_in_halves() {
        let seeded = Fixed {
            random_seed: Some(0x1_0000_0002),
            disable_animations: false,
            ..fixed()
        };
        let code = script(&seeded).unwrap();
        assert!(code.ends_with("(2, 1, false);"), "{code}");
        assert!(code.starts_with("// chrome-cdp test: determinism"));
    }
}
