//! Turning numbers into English.
//!
//! Every count the interface shows used to spell its own plural at the call
//! site — seven near-identical `if n == 1` blocks, which is seven chances to
//! ship "1 files" and seven places to keep in step. It also meant the same
//! count could read as "6 files" in one panel and "6 file(s)" in another, which
//! is the sort of small incoherence that makes an application feel unfinished.

/// `1 file`, `3 files` — the count and its noun.
///
/// The plural is the English default of adding `s`. Nouns that do not follow it
/// take [`plural_with`] instead, rather than this growing a dictionary.
pub fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// As [`plural`], for nouns whose plural is not the singular plus `s`.
pub fn plural_with(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// The noun alone, without the count: `file`, `files`.
///
/// For sentences that have already said the number, or that use a word like
/// "these" in place of one.
pub fn noun(n: usize, singular: &str) -> String {
    if n == 1 {
        singular.to_owned()
    } else {
        format!("{singular}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_singular_and_everything_else_is_not() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(2, "file"), "2 files");
        // Zero is plural in English: "0 files changed", not "0 file changed".
        assert_eq!(plural(0, "file"), "0 files");
    }

    #[test]
    fn irregular_plurals_are_spelled_out() {
        assert_eq!(plural_with(1, "entry", "entries"), "1 entry");
        assert_eq!(plural_with(3, "entry", "entries"), "3 entries");
    }

    #[test]
    fn the_noun_can_come_without_its_number() {
        assert_eq!(noun(1, "change"), "change");
        assert_eq!(noun(4, "change"), "changes");
    }
}
