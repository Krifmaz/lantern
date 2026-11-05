//! The standard (Newznab) categories every app searches by, and the site categories mapped onto them.
//!
//! Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Indexers/NewznabStandardCategory.cs), GPL-3.0.

/// The standard categories: (id, name), in Prowlarr's `AllCats` order (so a name lookup finds the same one). A
/// site category of its own is shown as 100000 + its number.
pub const STANDARD: &[(u32, &str)] = &[
    (1000, "Console"),
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
    (2000, "Movies"),
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
    (3000, "Audio"),
    (3010, "Audio/MP3"),
    (3020, "Audio/Video"),
    (3030, "Audio/Audiobook"),
    (3040, "Audio/Lossless"),
    (3050, "Audio/Other"),
    (3060, "Audio/Foreign"),
    (4000, "PC"),
    (4010, "PC/0day"),
    (4020, "PC/ISO"),
    (4030, "PC/Mac"),
    (4040, "PC/Mobile-Other"),
    (4050, "PC/Games"),
    (4060, "PC/Mobile-iOS"),
    (4070, "PC/Mobile-Android"),
    (5000, "TV"),
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
    (6000, "XXX"),
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
    (7000, "Books"),
    (7010, "Books/Mags"),
    (7020, "Books/EBook"),
    (7030, "Books/Comics"),
    (7040, "Books/Technical"),
    (7050, "Books/Other"),
    (7060, "Books/Foreign"),
    (8000, "Other"),
    (8010, "Other/Misc"),
    (8020, "Other/Hashed"),
    (0, "Other"),
    (20, "Other/Hashed"),
    (10, "Other/Misc"),
];

/// The parent categories (Prowlarr's `ParentCats`), in its order.
pub const PARENTS: [u32; 9] = [1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 0];

/// The standard category called exactly `name` (Prowlarr's `GetCatByName`: the first in [`STANDARD`]).
pub fn by_name(name: &str) -> Option<(u32, &'static str)> {
    STANDARD.iter().find(|(_, n)| *n == name).copied()
}

/// The standard parent category that has `id` as one of its own sub-categories (or is `id`).
pub fn standard_parent_of(id: u32) -> Option<u32> {
    if PARENTS.contains(&id) {
        return Some(id);
    }
    STANDARD.iter().any(|(i, _)| *i == id).then(|| id / 1000 * 1000)
}

/// The id of a standard category name (`Movies/HD` → 2040), ignoring case; a few spellings definitions use
/// are taken too.
pub fn id_of(name: &str) -> Option<u32> {
    let n = name.trim();
    STANDARD.iter().find(|(_, s)| s.eq_ignore_ascii_case(n)).map(|(id, _)| *id).or_else(|| match n.to_lowercase().as_str() {
        "tv/hd" => Some(5040),
        "movies/webdl" => Some(2080),
        "tv/webdl" => Some(5010),
        "books/ebook" => Some(7020),
        "audio/audiobook" => Some(3030),
        _ => None,
    })
}

/// The name of standard category `id`, if it is one.
pub fn name_of(id: u32) -> Option<&'static str> {
    STANDARD.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
}

/// The parent of a category (5040 → 5000).
pub fn parent(id: u32) -> u32 {
    if id >= 100000 {
        id
    } else {
        id / 1000 * 1000
    }
}

/// Whether a result in `cat` answers a search for `wanted` (the same category, or `wanted` is its parent).
pub fn matches(cat: u32, wanted: u32) -> bool {
    cat == wanted || (wanted.is_multiple_of(1000) && wanted < 100000 && parent(cat) == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_parents() {
        assert_eq!(id_of("Movies/HD"), Some(2040));
        assert_eq!(id_of("tv/anime"), Some(5070));
        assert_eq!(name_of(5000), Some("TV"));
        assert!(matches(5040, 5000) && matches(5040, 5040) && !matches(5040, 2000));
    }
}
