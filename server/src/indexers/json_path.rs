//! The JSON selectors of the definitions: dotted paths (`meta.poster`, `files[0].name`) with the pseudo-classes
//! the definitions use on JSON rows (`data:not(free:contains(1))`, `:has(imdb)`).

use serde_json::Value;

/// The value at `path` in `v` (`a.b[0].c`), `None` when any step is missing.
pub fn get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.trim().trim_start_matches("$.").trim_start_matches('$');
    if path.is_empty() {
        return Some(v);
    }
    let mut cur = v;
    for part in path.split('.') {
        let (name, indexes) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        if !name.is_empty() {
            cur = cur.get(name)?;
        }
        for idx in indexes.split('[').filter(|s| !s.is_empty()) {
            let n: usize = idx.trim_end_matches(']').parse().ok()?;
            cur = cur.get(n)?;
        }
    }
    Some(cur)
}

/// A JSON value as the text a field reads: strings as they are, numbers and booleans as written, arrays joined
/// with `,`, null as nothing.
pub fn text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(if *b { "True".into() } else { "False".into() }),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(a) => Some(a.iter().filter_map(text).collect::<Vec<_>>().join(",")),
        Value::Object(_) => Some(v.to_string()),
    }
}

/// A selector split into its path and its pseudo-class tests.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    /// The path.
    pub path: String,
    /// The tests: (negated, kind, argument).
    pub tests: Vec<(bool, String, String)>,
}

/// Splits `sel` (`data.rows:not(free:contains(1)):has(imdb)`).
pub fn parse(sel: &str) -> Selector {
    let s = sel.trim();
    let mut path_end = s.len();
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ':' if depth == 0 => {
                path_end = i;
                break;
            }
            _ => {}
        }
    }
    let path = s[..path_end].to_string();
    let mut tests = Vec::new();
    let mut rest = &s[path_end..];
    while let Some(r) = rest.strip_prefix(':') {
        let open = match r.find('(') {
            Some(o) => o,
            None => break,
        };
        let name = &r[..open];
        let mut depth = 0;
        let mut close = None;
        for (i, c) in r[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { break };
        let arg = r[open + 1..close].trim().trim_matches('"').trim_matches('\'').to_string();
        tests.push((false, name.to_string(), arg));
        rest = &r[close + 1..];
    }
    // `:not(x:contains(y))` → a negated contains on x
    let tests = tests
        .into_iter()
        .map(|(_, kind, arg)| {
            if kind == "not" {
                let inner = parse(&arg);
                match inner.tests.first() {
                    Some((_, k, a)) => (true, format!("{}@{}", k.trim_end_matches('@'), inner.path), a.clone()),
                    None => (true, "has@".to_string() + &inner.path, String::new()),
                }
            } else {
                (false, format!("{kind}@"), arg)
            }
        })
        .collect();
    Selector { path, tests }
}

/// Whether the object `row` passes the tests of `sel` (the path of each test is inside `row`).
pub fn passes(row: &Value, sel: &Selector) -> bool {
    sel.tests.iter().all(|(negated, kind, arg)| {
        let (k, sub) = kind.split_once('@').unwrap_or((kind.as_str(), ""));
        let ok = match k {
            "contains" => get(row, sub).and_then(text).is_some_and(|t| t.contains(arg.as_str())),
            "has" => get(row, if sub.is_empty() { arg } else { sub }).is_some_and(|v| !v.is_null()),
            _ => true,
        };
        ok != *negated
    })
}

/// The rows a row selector names in `doc`: the array at its path (or the one object), less those its tests drop.
pub fn rows(doc: &Value, sel: &str) -> Vec<Value> {
    let s = parse(sel);
    let found = match get(doc, &s.path) {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => {
            // an object of objects (`{"123": {...}, "124": {...}}`) is a list of rows
            if !o.is_empty() && o.values().all(Value::is_object) {
                o.values().cloned().collect()
            } else {
                vec![Value::Object(o.clone())]
            }
        }
        _ => Vec::new(),
    };
    found.into_iter().filter(|r| passes(r, &s)).collect()
}

/// A field selector's text in `row`: the path's value, when the tests pass.
pub fn field(row: &Value, sel: &str) -> Option<String> {
    let s = parse(sel);
    let v = get(row, &s.path)?;
    if !s.tests.is_empty() && !passes(row, &Selector { path: String::new(), tests: s.tests.clone() }) {
        return None;
    }
    text(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_rows_and_tests() {
        let doc = json!({"data": {"data": [
            {"name": "A", "promo": {"down": 0}, "files": [{"name": "a.mkv"}], "genres": ["x", "y"]},
            {"name": "B", "promo": {"down": 1}}
        ]}});
        assert_eq!(rows(&doc, "data.data").len(), 2);
        let free = rows(&doc, "data.data:not(promo.down:contains(1))");
        assert_eq!(free.len(), 1);
        assert_eq!(field(&free[0], "files[0].name").as_deref(), Some("a.mkv"));
        assert_eq!(field(&free[0], "genres").as_deref(), Some("x,y"));
        assert_eq!(rows(&doc, "data.data:has(files)").len(), 1);
        assert_eq!(field(&doc, "missing.x"), None);
    }
}
