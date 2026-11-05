// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Indexers/NewznabStandardCategory.cs,
// src/NzbDrone.Core/Indexers/IndexerCapabilitiesCategories.cs, src/NzbDrone.Core/Indexers/IndexerCategory.cs,
// src/NzbDrone.Core/Indexers/NewznabCategoryFieldConverter.cs), GPL-3.0.

//! Prowlarr's standard categories, the category tree of one indexer, and which of an app's sync categories an
//! indexer supports (`SupportedCategories`), in the order Prowlarr lists them: the tree's top categories first,
//! then every sub category, as Prowlarr builds the tree from the indexer's categories.

use serde_json::{json, Value};

/// A top category's sub categories: number and name.
pub type Subs = &'static [(u32, &'static str)];

/// Prowlarr's top categories with their sub categories, in Prowlarr's order (`NewznabStandardCategory.ParentCats`).
pub const PARENTS: &[(u32, &str, Subs)] = &[
    (
        1000,
        "Console",
        &[
            (1010, "Console/NDS"),
            (1020, "Console/PSP"),
            (1030, "Console/Wii"),
            (1040, "Console/XBox"),
            (1050, "Console/XBox 360"),
            (1060, "Console/Wiiware"),
            (1070, "Console/XBox 360 DLC"),
            (1080, "Console/PS3"),
            (1090, "Console/Other"),
            (1110, "Console/3DS"),
            (1120, "Console/PS Vita"),
            (1130, "Console/WiiU"),
            (1140, "Console/XBox One"),
            (1180, "Console/PS4"),
        ],
    ),
    (
        2000,
        "Movies",
        &[
            (2010, "Movies/Foreign"),
            (2020, "Movies/Other"),
            (2030, "Movies/SD"),
            (2040, "Movies/HD"),
            (2045, "Movies/UHD"),
            (2050, "Movies/BluRay"),
            (2060, "Movies/3D"),
            (2070, "Movies/DVD"),
            (2080, "Movies/WEB-DL"),
            (2090, "Movies/x265"),
        ],
    ),
    (
        3000,
        "Audio",
        &[
            (3010, "Audio/MP3"),
            (3020, "Audio/Video"),
            (3030, "Audio/Audiobook"),
            (3040, "Audio/Lossless"),
            (3050, "Audio/Other"),
            (3060, "Audio/Foreign"),
        ],
    ),
    (
        4000,
        "PC",
        &[
            (4010, "PC/0day"),
            (4020, "PC/ISO"),
            (4030, "PC/Mac"),
            (4040, "PC/Mobile-Other"),
            (4050, "PC/Games"),
            (4060, "PC/Mobile-iOS"),
            (4070, "PC/Mobile-Android"),
        ],
    ),
    (
        5000,
        "TV",
        &[
            (5010, "TV/WEB-DL"),
            (5020, "TV/Foreign"),
            (5030, "TV/SD"),
            (5040, "TV/HD"),
            (5045, "TV/UHD"),
            (5050, "TV/Other"),
            (5060, "TV/Sport"),
            (5070, "TV/Anime"),
            (5080, "TV/Documentary"),
            (5090, "TV/x265"),
        ],
    ),
    (
        6000,
        "XXX",
        &[
            (6010, "XXX/DVD"),
            (6020, "XXX/WMV"),
            (6030, "XXX/XviD"),
            (6040, "XXX/x264"),
            (6045, "XXX/UHD"),
            (6050, "XXX/Pack"),
            (6060, "XXX/ImageSet"),
            (6070, "XXX/Other"),
            (6080, "XXX/SD"),
            (6090, "XXX/WEB-DL"),
        ],
    ),
    (
        7000,
        "Books",
        &[
            (7010, "Books/Mags"),
            (7020, "Books/EBook"),
            (7030, "Books/Comics"),
            (7040, "Books/Technical"),
            (7050, "Books/Other"),
            (7060, "Books/Foreign"),
        ],
    ),
    (8000, "Other", &[(8010, "Other/Misc"), (8020, "Other/Hashed")]),
    (0, "Other", &[(10, "Other/Misc"), (20, "Other/Hashed")]),
];

/// One node of an indexer's category tree: a category and the sub categories under it.
#[derive(Debug, Clone, PartialEq)]
struct Node {
    /// The category's number.
    id: u32,
    /// The sub categories' numbers, in the order they were added.
    subs: Vec<u32>,
}

/// An indexer's categories as Prowlarr's `IndexerCapabilitiesCategories` keeps them: a tree built in the order the
/// categories were added.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    /// The top nodes.
    nodes: Vec<Node>,
}

/// The top category that has `id` as one of its sub categories.
fn parent_of(id: u32) -> Option<u32> {
    PARENTS.iter().find(|(_, _, subs)| subs.iter().any(|(s, _)| *s == id)).map(|(p, _, _)| *p)
}

/// Whether `id` is one of the top categories.
fn is_parent(id: u32) -> bool {
    PARENTS.iter().any(|(p, _, _)| *p == id)
}

impl Tree {
    /// The tree of these categories, added one by one (Prowlarr's `AddTorznabCategoryTree` for each).
    pub fn of(ids: impl IntoIterator<Item = u32>) -> Tree {
        let mut t = Tree::default();
        for id in ids {
            t.add(id);
        }
        t
    }

    /// Adds one category: a top category once, a sub category under its top category (made when missing), a
    /// category of the indexer's own between 1000 and 10000 under the top category of its thousand, any other at
    /// the top.
    pub fn add(&mut self, id: u32) {
        if is_parent(id) {
            if !self.nodes.iter().any(|n| n.id == id) {
                self.nodes.push(Node { id, subs: Vec::new() });
            }
            return;
        }
        let parent = parent_of(id).or_else(|| {
            (id > 1000 && id < 10000).then(|| PARENTS.iter().find(|(p, _, _)| p / 1000 == id / 1000).map(|(p, _, _)| *p)).flatten()
        });
        match parent {
            Some(p) => match self.nodes.iter_mut().find(|n| n.id == p) {
                Some(n) => {
                    if !n.subs.contains(&id) {
                        n.subs.push(id);
                    }
                }
                None => self.nodes.push(Node { id: p, subs: vec![id] }),
            },
            None => self.nodes.push(Node { id, subs: Vec::new() }),
        }
    }

    /// Prowlarr's `SupportedCategories`: the categories of `wanted` this tree has, top categories first, then the
    /// sub categories, each in the tree's order.
    pub fn supported(&self, wanted: &[i64]) -> Vec<i64> {
        if wanted.is_empty() {
            return Vec::new();
        }
        let tops = self.nodes.iter().map(|n| n.id);
        let subs = self.nodes.iter().flat_map(|n| n.subs.iter().copied());
        tops.chain(subs).map(i64::from).filter(|id| wanted.contains(id)).collect()
    }
}

/// Every standard category, as `NewznabStandardCategory.AllCats` lists them (the top ones with their subs).
pub fn all() -> Vec<u32> {
    let mut out = Vec::new();
    for (p, _, subs) in PARENTS {
        out.push(*p);
        out.extend(subs.iter().map(|(s, _)| *s));
    }
    out
}

/// The choices of a category field (`NewznabCategoryFieldConverter`): each top category, then its sub categories by
/// number, with Prowlarr's hint `(id)`.
pub fn select_options() -> Vec<Value> {
    let mut out = Vec::new();
    for (p, name, subs) in PARENTS {
        out.push(json!({ "value": p, "name": name, "order": 0, "hint": format!("({p})") }));
        let mut subs: Vec<_> = subs.to_vec();
        subs.sort_by_key(|(id, _)| *id);
        for (id, name) in subs {
            out.push(json!({ "value": id, "name": name, "order": 0, "hint": format!("({id})"), "parentValue": p }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_categories_list_top_categories_then_subs_in_tree_order() {
        // the categories an indexer like lantern-test maps: Movies/HD, TV/HD, Audio/Lossless, Books/EBook and its own
        let t = Tree::of([2040, 100001, 5040, 100002, 3040, 100003, 7020, 100004]);
        let sonarr = [5000, 5010, 5020, 5030, 5040, 5045, 5050, 5090];
        assert_eq!(t.supported(&sonarr), [5000, 5040]);
        assert_eq!(t.supported(&[5070]), Vec::<i64>::new());
        let radarr = [2000, 2010, 2020, 2030, 2040, 2045, 2050, 2060, 2070, 2080, 2090];
        assert_eq!(t.supported(&radarr), [2000, 2040]);
        assert_eq!(t.supported(&[]), Vec::<i64>::new());
    }

    #[test]
    fn the_tree_follows_prowlarrs_insertion_rules() {
        // a sub category before its top category, then the top category once
        let t = Tree::of([5040, 5000, 5030, 5040, 2000]);
        assert_eq!(t.supported(&[2000, 5000, 5030, 5040]), [5000, 2000, 5040, 5030]);
        // an own category between 1000 and 10000 goes under its thousand
        let t = Tree::of([5999]);
        assert_eq!(t.supported(&[5000, 5999]), [5000, 5999]);
        // the Other/Misc of category 0
        let t = Tree::of([10]);
        assert_eq!(t.supported(&[0, 10]), [0, 10]);
    }

    #[test]
    fn select_options_follow_the_category_converter() {
        let o = select_options();
        assert_eq!(o[0], json!({ "value": 1000, "name": "Console", "order": 0, "hint": "(1000)" }));
        assert_eq!(o[1], json!({ "value": 1010, "name": "Console/NDS", "order": 0, "hint": "(1010)", "parentValue": 1000 }));
        assert_eq!(o.len(), all().len());
        assert_eq!(all().len(), 76);
    }
}
