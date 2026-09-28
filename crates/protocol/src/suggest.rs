//! "Did you mean" suggestions (plan 08, milestone 8.3): the known names
//! closest to a mistyped one.
//!
//! Names compare without case and without the punctuation and spaces
//! people drop or add (`iphone-13` is `iPhone 13`, `cputhrottle` is
//! `cpu-throttle`). A name is close when its edit distance (with swapped
//! letters counting once) is at most a third of the typed name's length
//! (rounded down) and at most 3; only the closest names are suggested.

/// The candidates closest to `typed`, all equally close, at most `max`. A
/// candidate equal to `typed` (after [`squash`]) is the only result.
pub fn similar<'a, I>(typed: &str, candidates: I, max: usize) -> Vec<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let wanted = squash(typed);
    if wanted.is_empty() {
        return Vec::new();
    }
    let limit = (wanted.chars().count() / 3).min(3);
    let mut scored: Vec<(usize, &str)> = Vec::new();
    for candidate in candidates {
        let score = strsim::damerau_levenshtein(&wanted, &squash(candidate));
        if score == 0 {
            return vec![candidate];
        }
        if score <= limit && !scored.iter().any(|&(_, c)| c == candidate) {
            scored.push((score, candidate));
        }
    }
    let Some(best) = scored.iter().map(|&(score, _)| score).min() else {
        return Vec::new();
    };
    // Equally close names keep the candidates' order.
    scored
        .into_iter()
        .filter(|&(score, _)| score == best)
        .take(max)
        .map(|(_, c)| c)
        .collect()
}

/// `name` lowercased, with only its letters and digits.
pub fn squash(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{similar, squash};

    const COMMANDS: &[&str] = &["click", "dblclick", "check", "cpu-throttle", "throttle"];

    #[test]
    fn typos_find_their_name() {
        let all = COMMANDS.iter().copied();
        assert_eq!(similar("clik", all.clone(), 3), ["click"]);
        assert_eq!(similar("CLICK", all.clone(), 3), ["click"]);
        assert_eq!(similar("cputhrottle", all.clone(), 3), ["cpu-throttle"]);
        assert_eq!(similar("throtle", all.clone(), 3), ["throttle"]);
        // A swap counts once.
        assert_eq!(similar("cilck", all.clone(), 3), ["click"]);
        assert!(similar("screenshot", all.clone(), 3).is_empty());
        assert!(similar("", all, 3).is_empty());
    }

    #[test]
    fn short_names_need_a_close_match() {
        // One edit in two letters is too many.
        assert!(similar("tx", ["tz"], 3).is_empty());
        assert_eq!(similar("tzz", ["tz"], 3), ["tz"]);
    }

    #[test]
    fn only_the_closest_and_bounded() {
        let names = ["iPhone 13", "iPhone 12", "iPhone 13 Pro", "iPhone 11"];
        assert_eq!(similar("iphone 14", names, 2), ["iPhone 13", "iPhone 12"]);
        assert_eq!(similar("iphone 13 pr", names, 2), ["iPhone 13 Pro"]);
        assert_eq!(similar("iphone-13", names, 2), ["iPhone 13"]);
    }

    #[test]
    fn squash_drops_case_and_punctuation() {
        assert_eq!(squash("iPhone 13 Pro-Max"), "iphone13promax");
    }
}
