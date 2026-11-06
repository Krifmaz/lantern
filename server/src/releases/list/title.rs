// Ported from autobrr v1.90.0 (internal/list/title.go, tags.go), GPL-2.0-or-later.
//! Titles from a list as filter wildcards: punctuation, symbols, spaces and accented letters become `?`, runs of
//! them `*`, and each title also comes without apostrophes and without the bits in brackets (a year, a country),
//! so `The Handmaid's Tale (US)` matches `The.Handmaids.Tale.S01…` too.

use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// Punctuation, symbols, separators and the Latin-1 Supplement and Latin Extended-A letters (autobrr's
/// `replaceRegexp`).
static REPLACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{P}\p{S}\p{Z}\x{0080}-\x{017F}]").unwrap());
/// Two or more question marks (autobrr's `questionmarkRegexp`).
static QUESTIONMARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[?]{2,}").unwrap());
/// A word in brackets, `(2024)` or `(US)`; Go's `\S` is anything but ASCII white space (autobrr's
/// `parentheticalRegexp`).
static PARENTHETICAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\([^\t\n\x0C\r ]+\)").unwrap());
/// A closing bracket at the end (autobrr's `parenthesesEndRegexp`).
static PARENTHESES_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\)$").unwrap());

/// `s` without its apostrophes (autobrr's `apostropheReplacer`).
fn remove_apostrophes(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\'' | '´' | '`' | '‘' | '’')).collect()
}

/// A title and the same without the wildcards at its end, optionally without apostrophes and without the bits
/// in brackets (autobrr's `generateVariations`).
pub fn generate_variations(title: &str, remove_apostrophes_too: bool, remove_parenthetical: bool) -> [String; 2] {
    let mut v = if remove_parenthetical {
        PARENTHETICAL.replace_all(title, "").trim_end_matches(' ').to_string()
    } else {
        PARENTHESES_END.replace_all(title, "?").to_string()
    };
    if remove_apostrophes_too {
        v = remove_apostrophes(&v);
    }
    v = REPLACE.replace_all(&v, "?").to_string();
    v = QUESTIONMARKS.replace_all(&v, "*").to_string();
    let trimmed = v.trim_end_matches(['?', '*', ' ']).to_string();
    [v, trimmed]
}

/// The four variations of each kind of `title`.
fn variations(title: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(8);
    for (a, p) in [(false, false), (true, false), (false, true), (true, true)] {
        out.extend(generate_variations(title, a, p));
    }
    out
}

/// The wildcards of one title, `*`-wrapped when `match_release` (autobrr's `processTitle`). Go's answer comes in
/// map order; Lantern's is sorted.
pub fn process_title(title: &str, match_release: bool) -> Vec<String> {
    if title.trim().is_empty() {
        return Vec::new();
    }
    let mut t = Titles::default();
    if REPLACE.replace_all(title, "").is_empty() {
        t.add_wrapped(title, match_release);
    } else {
        for v in variations(title) {
            t.add_wrapped(&v, match_release);
        }
    }
    t.titles()
}

/// A set of title wildcards (autobrr's `Titles`).
#[derive(Debug, Clone, Default)]
pub struct Titles {
    /// The wildcards.
    set: BTreeSet<String>,
    /// Each becomes `*title*` in the filter string.
    pub match_releases: bool,
}

impl Titles {
    /// An empty set; its wildcards wrapped in `*` when `match_releases`.
    pub fn new(match_releases: bool) -> Titles {
        Titles { set: BTreeSet::new(), match_releases }
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Adds `title`, trimmed of wildcards and wrapped in `*` when `match_release` (autobrr's `Add`).
    pub fn add_wrapped(&mut self, title: &str, match_release: bool) {
        if title.is_empty() || title == "*" {
            return;
        }
        let t = if match_release { format!("*{}*", title.trim_matches(['?', '*', ' '])) } else { title.to_string() };
        self.set.insert(t);
    }

    /// Adds `title` as it is, trimmed of wildcards when the set matches releases (autobrr's `add`).
    fn add(&mut self, title: &str) {
        if title.is_empty() || title == "*" {
            return;
        }
        let t = if self.match_releases { title.trim_matches(['?', '*', ' ']).to_string() } else { title.to_string() };
        self.set.insert(t);
    }

    /// Adds every variation of `title`; false when it is blank (autobrr's `AddTitle`).
    pub fn add_title(&mut self, title: &str) -> bool {
        if title.trim().is_empty() {
            return false;
        }
        if REPLACE.replace_all(title, "").is_empty() {
            self.add(title);
        } else {
            for v in variations(title) {
                self.add(&v);
            }
        }
        true
    }

    /// The filter string: sorted, comma separated, each `*`-wrapped when the set matches releases (autobrr's
    /// `FilterString`).
    pub fn filter_string(&self) -> String {
        if self.match_releases {
            self.set.iter().map(|k| format!("*{k}*")).collect::<Vec<_>>().join(",")
        } else {
            self.set.iter().cloned().collect::<Vec<_>>().join(",")
        }
    }

    /// The wildcards, sorted.
    pub fn titles(&self) -> Vec<String> {
        self.set.iter().cloned().collect()
    }
}

/// Whether one of an item's tags (`item_tags`, ids) is labelled with one of `check` (autobrr's `containsTag`).
pub fn contains_tag(tags: &[(i64, String)], item_tags: &[i64], check: &[String]) -> bool {
    let labels: Vec<&str> = item_tags.iter().flat_map(|id| tags.iter().filter(move |(t, _)| t == id).map(|(_, l)| l.as_str())).collect();
    check.iter().any(|c| labels.contains(&c.as_str()))
}

/// Whether an item's tags leave it out of the list: it has none of the included tags, or one of the excluded
/// ones (autobrr's `excludedByTags`).
pub fn excluded_by_tags(include: &[String], exclude: &[String], tags: &[(i64, String)], item_tags: &[i64]) -> bool {
    if !include.is_empty() && (item_tags.is_empty() || !contains_tag(tags, item_tags, include)) {
        return true;
    }
    !exclude.is_empty() && contains_tag(tags, item_tags, exclude)
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/list/title_test.go and tags_test.go.
    use super::*;

    #[test]
    fn test_process_title() {
        let cases: &[(&str, bool, &[&str])] = &[
            ("The Quick Brown Fox (2022)", false, &["The?Quick?Brown?Fox", "The?Quick?Brown?Fox*2022", "The?Quick?Brown?Fox*2022?"]),
            (
                "The Matrix     -        Reloaded (2929)",
                false,
                &["The?Matrix*Reloaded", "The?Matrix*Reloaded*2929", "The?Matrix*Reloaded*2929?"],
            ),
            (
                "The Matrix -(Test)- Reloaded (2929)",
                false,
                &["The?Matrix*Reloaded", "The?Matrix*Test*Reloaded*2929?", "The?Matrix*Test*Reloaded*2929"],
            ),
            ("The Marvelous Mrs. Maisel", false, &["The?Marvelous?Mrs*Maisel"]),
            ("Arrr!! The Title (2020)", false, &["Arrr*The?Title", "Arrr*The?Title*2020", "Arrr*The?Title*2020?"]),
            (
                "Whose Line Is It Anyway? (US)",
                false,
                &["Whose?Line?Is?It?Anyway", "Whose?Line?Is?It?Anyway*US", "Whose?Line?Is?It?Anyway?", "Whose?Line?Is?It?Anyway*US?"],
            ),
            ("MasterChef (US)", false, &["MasterChef*US", "MasterChef", "MasterChef*US?"]),
            ("Brooklyn Nine-Nine", false, &["Brooklyn?Nine?Nine"]),
            ("S.W.A.T.", false, &["S?W?A?T?", "S?W?A?T"]),
            ("The Handmaid's Tale", false, &["The?Handmaid?s?Tale", "The?Handmaids?Tale"]),
            (
                "The Handmaid's Tale (US)",
                false,
                &[
                    "The?Handmaid?s?Tale*US",
                    "The?Handmaids?Tale*US",
                    "The?Handmaid?s?Tale",
                    "The?Handmaids?Tale",
                    "The?Handmaid?s?Tale*US?",
                    "The?Handmaids?Tale*US?",
                ],
            ),
            ("Monsters, Inc.", false, &["Monsters*Inc?", "Monsters*Inc"]),
            ("Hello Tomorrow!", false, &["Hello?Tomorrow?", "Hello?Tomorrow"]),
            ("Be Cool, Scooby-Doo!", false, &["Be?Cool*Scooby?Doo?", "Be?Cool*Scooby?Doo"]),
            ("Scooby-Doo! Mystery Incorporated", false, &["Scooby?Doo*Mystery?Incorporated"]),
            ("Master.Chef (US)", false, &["Master?Chef*US", "Master?Chef", "Master?Chef*US?"]),
            (
                "Whose Line Is It Anyway? (US)",
                false,
                &["Whose?Line?Is?It?Anyway*US", "Whose?Line?Is?It?Anyway?", "Whose?Line?Is?It?Anyway", "Whose?Line?Is?It?Anyway*US?"],
            ),
            ("90 Day Fiancé: Pillow Talk", false, &["90?Day?Fianc*Pillow?Talk"]),
            ("進撃の巨人", false, &["進撃の巨人"]),
            ("呪術廻戦 0: 東京都立呪術高等専門学校", false, &["呪術廻戦?0*東京都立呪術高等専門学校"]),
            ("-!", false, &["-!"]),
            ("A\u{a0}Quiet\u{a0}Place:\u{a0}Day One", false, &["A?Quiet?Place*Day?One"]),
            (
                "Whose Line Is It Anyway? (US) (1932)",
                false,
                &[
                    "Whose?Line?Is?It?Anyway",
                    "Whose?Line?Is?It?Anyway?",
                    "Whose?Line?Is?It?Anyway*US*1932",
                    "Whose?Line?Is?It?Anyway*US*1932?",
                ],
            ),
            ("What If…?", false, &["What?If", "What?If*"]),
            ("Shōgun (2024)", false, &["Sh?gun*2024?", "Sh?gun*2024", "Sh?gun"]),
            ("Pinball FX3 - Bethesda® Pinball", false, &["Pinball?FX3*Bethesda*Pinball"]),
            ("Sally Goes to the Mall", true, &["*Sally?Goes?to?the?Mall*"]),
            ("*****… (los asteriscos…)", false, &["*los?asteriscos*", "*los?asteriscos"]),
            ("The Office (US)", false, &["The?Office", "The?Office*US", "The?Office*US?"]),
            (
                "this is him (can’t be anyone else)",
                false,
                &[
                    "this?is?him*can?t?be?anyone?else?",
                    "this?is?him*can?t?be?anyone?else",
                    "this?is?him*cant?be?anyone?else?",
                    "this?is?him*cant?be?anyone?else",
                ],
            ),
            (
                "solo leveling 2ª temporada -ergam-se das sombras-",
                false,
                &["solo?leveling?2*temporada*ergam?se?das?sombras", "solo?leveling?2*temporada*ergam?se?das?sombras?"],
            ),
            ("pokémon", false, &["pok?mon"]),
            ("What If…?", true, &["*What?If*"]),
        ];
        for (title, m, want) in cases {
            let mut want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
            want.sort();
            assert_eq!(process_title(title, *m), want, "{title}");
        }
    }

    #[test]
    fn test_contains_tag() {
        let tags = vec![(1, "Want".to_string()), (2, "exclude-me".to_string())];
        assert!(!contains_tag(&tags, &[], &["Want".into()]));
        assert!(contains_tag(&tags, &[1], &["Want".into()]));
        assert!(!contains_tag(&tags, &[1], &[]));
    }

    #[test]
    fn excluded_by_tags_reads_both_lists() {
        let tags = vec![(1, "Want".to_string()), (2, "exclude-me".to_string())];
        assert!(excluded_by_tags(&["Want".into()], &[], &tags, &[]));
        assert!(!excluded_by_tags(&["Want".into()], &[], &tags, &[1]));
        assert!(excluded_by_tags(&[], &["exclude-me".into()], &tags, &[1, 2]));
        assert!(!excluded_by_tags(&[], &[], &tags, &[2]));
    }

    #[test]
    fn filter_string_sorts_and_wraps() {
        let mut t = Titles::new(true);
        t.add_title("What If…?");
        t.add_title("Arab Strap");
        assert_eq!(t.filter_string(), "*Arab?Strap*,*What?If*");
        let mut t = Titles::new(false);
        t.add_title("S.W.A.T.");
        assert_eq!(t.filter_string(), "S?W?A?T,S?W?A?T?");
    }
}
