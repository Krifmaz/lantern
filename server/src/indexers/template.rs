//! The small template language of the indexer definitions: a subset of Go's `text/template`, as the Cardigann
//! definitions use it.
//!
//! ```text
//! {{ .Keywords }}  {{ .Config.sort }}  {{ .Query.IMDBID }}  {{ .Result.title }}  {{ .Today.Year }}
//! {{ if .Keywords }}…{{ else if … }}…{{ else }}…{{ end }}
//! {{ range .Categories }}&cat={{ . }}{{ end }}   {{ range $i, $c := .Categories }}…{{ end }}
//! {{ and a b }} {{ or a b }} {{ not a }} {{ eq a b }} {{ ne a b }} {{ join .Categories "," }}
//! {{ re_replace .Keywords "\s+" "-" }} {{ len .Categories }} {{ index .Categories 0 }}
//! ```
//!
//! Values are text, lists of text or nothing ([`Val`]). A checked checkbox is `"True"` (`.True`), an unchecked one
//! is nothing (`.False`), so `eq .Config.x .False` asks "is it off?".

use std::collections::HashMap;

/// A value a template works with.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// Nothing: a missing value, an unchecked checkbox, `.False`.
    Null,
    /// Text.
    Str(String),
    /// A list (`.Categories`).
    List(Vec<String>),
}

impl Val {
    /// Whether `if` takes the value as true: non-empty text or list.
    pub fn truthy(&self) -> bool {
        match self {
            Val::Null => false,
            Val::Str(s) => !s.is_empty(),
            Val::List(l) => !l.is_empty(),
        }
    }

    /// The value as the template prints it (a list as Go prints one: `[a b]`).
    pub fn text(&self) -> String {
        match self {
            Val::Null => String::new(),
            Val::Str(s) => s.clone(),
            Val::List(l) => format!("[{}]", l.join(" ")),
        }
    }
}

impl From<&str> for Val {
    fn from(s: &str) -> Val {
        Val::Str(s.to_string())
    }
}

impl From<String> for Val {
    fn from(s: String) -> Val {
        Val::Str(s)
    }
}

/// What a template can read: dotted names (`.Config.sort` is `Config.sort`) to values.
#[derive(Debug, Clone, Default)]
pub struct Vars {
    /// name → value
    pub map: HashMap<String, Val>,
}

impl Vars {
    /// Sets `name` (without the leading dot).
    pub fn set(&mut self, name: impl Into<String>, v: impl Into<Val>) {
        self.map.insert(name.into(), v.into());
    }

    /// Sets `name` to nothing.
    pub fn null(&mut self, name: impl Into<String>) {
        self.map.insert(name.into(), Val::Null);
    }

    /// The value of `name`, nothing when unset.
    pub fn get(&self, name: &str) -> Val {
        self.map.get(name).cloned().unwrap_or(Val::Null)
    }
}

/// Renders `src` with `vars`. A template that can't be read gives an error sentence.
pub fn render(src: &str, vars: &Vars) -> Result<String, String> {
    if !src.contains("{{") {
        return Ok(src.to_string());
    }
    let nodes = parse(src)?;
    let mut out = String::new();
    let mut scope = Scope { vars, dot: None, locals: HashMap::new() };
    exec(&nodes, &mut scope, &mut out)?;
    Ok(out)
}

/// One piece of a parsed template.
#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Print(Expr),
    If(Vec<(Expr, Vec<Node>)>, Vec<Node>),
    Range { key: Option<String>, val: Option<String>, over: Expr, body: Vec<Node>, otherwise: Vec<Node> },
}

/// An expression: a call or a single operand.
#[derive(Debug, Clone)]
enum Expr {
    Field(String),
    Dot,
    Var(String, String),
    Lit(String),
    Call(String, Vec<Expr>),
}

/// The text and `{{ }}` actions of a template, `{{-` and `-}}` trimming the space beside them.
fn lex(src: &str) -> Result<Vec<(bool, String)>, String> {
    let mut parts: Vec<(bool, String)> = Vec::new();
    let mut rest = src;
    while let Some(start) = rest.find("{{") {
        let mut text = rest[..start].to_string();
        let mut inner_start = start + 2;
        if rest[inner_start..].starts_with('-') && rest[inner_start + 1..].starts_with(|c: char| c.is_whitespace()) {
            text = text.trim_end().to_string();
            inner_start += 1;
        }
        parts.push((false, text));
        let after = &rest[inner_start..];
        let end = find_close(after).ok_or_else(|| "A template action is not closed with }}".to_string())?;
        let mut inner = &after[..end];
        let mut trim_after = false;
        if inner.ends_with('-') && inner[..inner.len() - 1].ends_with(|c: char| c.is_whitespace()) {
            inner = &inner[..inner.len() - 1];
            trim_after = true;
        }
        parts.push((true, inner.trim().to_string()));
        rest = &after[end + 2..];
        if trim_after {
            rest = rest.trim_start();
        }
    }
    parts.push((false, rest.to_string()));
    Ok(parts)
}

/// Where `}}` closes an action, skipping quoted strings.
fn find_close(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        match quote {
            Some(q) => {
                if b[i] == b'\\' && q == b'"' {
                    i += 1;
                } else if b[i] == q {
                    quote = None;
                }
            }
            None => {
                if b[i] == b'"' || b[i] == b'`' {
                    quote = Some(b[i]);
                } else if b[i] == b'}' && b.get(i + 1) == Some(&b'}') {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

fn parse(src: &str) -> Result<Vec<Node>, String> {
    let parts = lex(src)?;
    let mut pos = 0;
    let (nodes, end) = parse_list(&parts, &mut pos)?;
    match end {
        None => Ok(nodes),
        Some(e) => Err(format!("A template has {{{{ {e} }}}} without its {{{{ if }}}} or {{{{ range }}}}")),
    }
}

/// Nodes until `end` / `else…` (returned) or the end of the template.
fn parse_list(parts: &[(bool, String)], pos: &mut usize) -> Result<(Vec<Node>, Option<String>), String> {
    let mut out = Vec::new();
    while *pos < parts.len() {
        let (action, text) = &parts[*pos];
        *pos += 1;
        if !action {
            if !text.is_empty() {
                out.push(Node::Text(text.clone()));
            }
            continue;
        }
        let word = text.split_whitespace().next().unwrap_or("");
        match word {
            "" => {}
            _ if text.starts_with("/*") => {}
            "end" => return Ok((out, Some("end".into()))),
            "else" => return Ok((out, Some(text.clone()))),
            "if" => {
                let mut branches = Vec::new();
                let mut cond = parse_expr(text[2..].trim())?;
                let otherwise;
                loop {
                    let (body, end) = parse_list(parts, pos)?;
                    branches.push((cond, body));
                    match end.as_deref() {
                        Some("end") => {
                            otherwise = Vec::new();
                            break;
                        }
                        Some(e) if e.starts_with("else if") => {
                            cond = parse_expr(e["else if".len()..].trim())?;
                        }
                        Some("else") => {
                            let (body, end) = parse_list(parts, pos)?;
                            if end.as_deref() != Some("end") {
                                return Err("An {{ else }} is not closed with {{ end }}".into());
                            }
                            otherwise = body;
                            break;
                        }
                        _ => return Err("An {{ if }} is not closed with {{ end }}".into()),
                    }
                }
                out.push(Node::If(branches, otherwise));
            }
            "range" => {
                let spec = text[5..].trim();
                let (key, val, over) = match spec.split_once(":=") {
                    Some((vars, over)) => {
                        let names: Vec<String> = vars.split(',').map(|v| v.trim().trim_start_matches('$').to_string()).collect();
                        match names.as_slice() {
                            [v] => (None, Some(v.clone()), over.trim()),
                            [k, v] => (Some(k.clone()), Some(v.clone()), over.trim()),
                            _ => return Err(format!("A range can't be read: {spec}")),
                        }
                    }
                    None => (None, None, spec),
                };
                let over = parse_expr(over)?;
                let (body, end) = parse_list(parts, pos)?;
                let otherwise = match end.as_deref() {
                    Some("end") => Vec::new(),
                    Some("else") => {
                        let (b, end) = parse_list(parts, pos)?;
                        if end.as_deref() != Some("end") {
                            return Err("A {{ range }} is not closed with {{ end }}".into());
                        }
                        b
                    }
                    _ => return Err("A {{ range }} is not closed with {{ end }}".into()),
                };
                out.push(Node::Range { key, val, over, body, otherwise });
            }
            _ => out.push(Node::Print(parse_expr(text)?)),
        }
    }
    Ok((out, None))
}

/// The words of an expression: operands, literals and parenthesised groups.
fn words(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '"' {
            let mut lit = String::from("\"");
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    lit.push(b[i]);
                    i += 1;
                }
                lit.push(b[i]);
                i += 1;
            }
            lit.push('"');
            i += 1;
            out.push(lit);
        } else if c == '`' {
            let mut lit = String::from("`");
            i += 1;
            while i < b.len() && b[i] != '`' {
                lit.push(b[i]);
                i += 1;
            }
            lit.push('`');
            i += 1;
            out.push(lit);
        } else if c == '(' {
            let mut depth = 0;
            let start = i;
            let mut q = false;
            while i < b.len() {
                if b[i] == '"' && (i == 0 || b[i - 1] != '\\') {
                    q = !q;
                }
                if !q {
                    if b[i] == '(' {
                        depth += 1;
                    } else if b[i] == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                i += 1;
            }
            if i >= b.len() {
                return Err(format!("A ( is not closed in {s}"));
            }
            out.push(b[start..=i].iter().collect());
            i += 1;
        } else {
            let start = i;
            while i < b.len() && !b[i].is_whitespace() && b[i] != '(' && b[i] != ')' {
                i += 1;
            }
            out.push(b[start..i].iter().collect());
        }
    }
    Ok(out)
}

fn parse_expr(s: &str) -> Result<Expr, String> {
    // a pipeline `a | f` passes a as f's last argument
    if let Some(at) = top_level_pipe(s) {
        let left = parse_expr(&s[..at])?;
        let right = parse_expr(&s[at + 1..])?;
        return Ok(match right {
            Expr::Call(f, mut args) => {
                args.push(left);
                Expr::Call(f, args)
            }
            Expr::Field(f) => Expr::Call(f, vec![left]),
            other => other,
        });
    }
    let ws = words(s)?;
    match ws.len() {
        0 => Err("An empty template action".into()),
        1 => operand(&ws[0]),
        _ => {
            let f = ws[0].clone();
            if f.starts_with('.') || f.starts_with('$') || f.starts_with('"') || f.starts_with('(') {
                // `(.Keywords)` alone, or a field followed by words: Go would fail; read the first
                return operand(&ws[0]);
            }
            let args = ws[1..].iter().map(|w| operand(w)).collect::<Result<Vec<_>, _>>()?;
            Ok(Expr::Call(f, args))
        }
    }
}

fn top_level_pipe(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut q = false;
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'"' if i == 0 || b[i - 1] != b'\\' => q = !q,
            b'(' if !q => depth += 1,
            b')' if !q => depth -= 1,
            b'|' if !q && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

fn operand(w: &str) -> Result<Expr, String> {
    if w == "." {
        Ok(Expr::Dot)
    } else if let Some(inner) = w.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        parse_expr(inner)
    } else if let Some(name) = w.strip_prefix('.') {
        Ok(Expr::Field(name.to_string()))
    } else if let Some(v) = w.strip_prefix('$') {
        let (name, path) = v.split_once('.').unwrap_or((v, ""));
        Ok(Expr::Var(name.to_string(), path.to_string()))
    } else if w.starts_with('"') {
        Ok(Expr::Lit(unquote(&w[1..w.len().saturating_sub(1).max(1)])))
    } else if w.starts_with('`') {
        Ok(Expr::Lit(w[1..w.len().saturating_sub(1).max(1)].to_string()))
    } else if w == "true" {
        Ok(Expr::Lit("True".into()))
    } else if w == "false" || w == "nil" {
        Ok(Expr::Call("__null".into(), vec![]))
    } else if w.parse::<f64>().is_ok() {
        Ok(Expr::Lit(w.to_string()))
    } else {
        Ok(Expr::Call(w.to_string(), vec![]))
    }
}

/// A Go string literal's text: `\\`, `\"`, `\n`, `\t`, `é`.
fn unquote(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('u') => {
                let hex: String = (0..4).filter_map(|_| it.next()).collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

struct Scope<'a> {
    vars: &'a Vars,
    dot: Option<Val>,
    locals: HashMap<String, Val>,
}

fn eval(e: &Expr, sc: &Scope) -> Result<Val, String> {
    Ok(match e {
        Expr::Lit(s) => Val::Str(s.clone()),
        Expr::Dot => sc.dot.clone().unwrap_or(Val::Null),
        Expr::Field(name) => sc.vars.get(name),
        Expr::Var(name, path) => {
            if name.is_empty() {
                sc.vars.get(path)
            } else {
                sc.locals.get(name).cloned().unwrap_or(Val::Null)
            }
        }
        Expr::Call(f, args) => {
            let vals = || args.iter().map(|a| eval(a, sc)).collect::<Result<Vec<_>, _>>();
            match f.as_str() {
                "__null" => Val::Null,
                "and" => {
                    let mut last = Val::Null;
                    for a in args {
                        last = eval(a, sc)?;
                        if !last.truthy() {
                            return Ok(last);
                        }
                    }
                    last
                }
                "or" => {
                    let mut last = Val::Null;
                    for a in args {
                        last = eval(a, sc)?;
                        if last.truthy() {
                            return Ok(last);
                        }
                    }
                    last
                }
                "not" => truth(!vals()?.first().is_some_and(Val::truthy)),
                "eq" => {
                    let v = vals()?;
                    let first = v.first().cloned().unwrap_or(Val::Null);
                    truth(v.iter().skip(1).any(|x| same(&first, x)))
                }
                "ne" => {
                    let v = vals()?;
                    truth(!same(v.first().unwrap_or(&Val::Null), v.get(1).unwrap_or(&Val::Null)))
                }
                "join" => {
                    let v = vals()?;
                    let sep = v.get(1).map(Val::text).unwrap_or_default();
                    match v.first() {
                        Some(Val::List(l)) => Val::Str(l.join(&sep)),
                        Some(other) => Val::Str(other.text()),
                        None => Val::Null,
                    }
                }
                "re_replace" => {
                    let v = vals()?;
                    let input = v.first().map(Val::text).unwrap_or_default();
                    let pat = v.get(1).map(Val::text).unwrap_or_default();
                    let rep = v.get(2).map(Val::text).unwrap_or_default();
                    Val::Str(super::filters::re_replace(&input, &pat, &rep)?)
                }
                "len" => Val::Str(match vals()?.first() {
                    Some(Val::List(l)) => l.len().to_string(),
                    Some(Val::Str(s)) => s.chars().count().to_string(),
                    _ => "0".into(),
                }),
                "index" => {
                    let v = vals()?;
                    let i: usize = v.get(1).map(Val::text).and_then(|t| t.parse().ok()).unwrap_or(0);
                    match v.first() {
                        Some(Val::List(l)) => l.get(i).cloned().map(Val::Str).unwrap_or(Val::Null),
                        _ => Val::Null,
                    }
                }
                "print" => Val::Str(vals()?.iter().map(Val::text).collect::<Vec<_>>().join("")),
                "urlquery" => {
                    Val::Str(url::form_urlencoded::byte_serialize(vals()?.iter().map(Val::text).collect::<String>().as_bytes()).collect())
                }
                "lower" | "tolower" => Val::Str(vals()?.first().map(Val::text).unwrap_or_default().to_lowercase()),
                "upper" | "toupper" => Val::Str(vals()?.first().map(Val::text).unwrap_or_default().to_uppercase()),
                other => return Err(format!("The template function {other} is not known")),
            }
        }
    })
}

fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Null, Val::Null) => true,
        (Val::Null, Val::Str(s)) | (Val::Str(s), Val::Null) => s.is_empty(),
        _ => a == b,
    }
}

fn truth(b: bool) -> Val {
    if b {
        Val::Str("True".into())
    } else {
        Val::Null
    }
}

fn exec(nodes: &[Node], sc: &mut Scope, out: &mut String) -> Result<(), String> {
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Print(e) => out.push_str(&eval(e, sc)?.text()),
            Node::If(branches, otherwise) => {
                let mut done = false;
                for (cond, body) in branches {
                    if eval(cond, sc)?.truthy() {
                        exec(body, sc, out)?;
                        done = true;
                        break;
                    }
                }
                if !done {
                    exec(otherwise, sc, out)?;
                }
            }
            Node::Range { key, val, over, body, otherwise } => {
                let items = match eval(over, sc)? {
                    Val::List(l) => l,
                    Val::Str(s) if !s.is_empty() => vec![s],
                    _ => Vec::new(),
                };
                if items.is_empty() {
                    exec(otherwise, sc, out)?;
                    continue;
                }
                let saved_dot = sc.dot.take();
                for (i, item) in items.into_iter().enumerate() {
                    if let Some(k) = key {
                        sc.locals.insert(k.clone(), Val::Str(i.to_string()));
                    }
                    if let Some(v) = val {
                        sc.locals.insert(v.clone(), Val::Str(item.clone()));
                    }
                    sc.dot = Some(Val::Str(item));
                    exec(body, sc, out)?;
                }
                sc.dot = saved_dot;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        let mut v = Vars::default();
        v.set("Keywords", "the show s01");
        v.set("Config.sort", "time");
        v.set("Config.type", "desc");
        v.null("Config.disablesort");
        v.set("True", "True");
        v.null("False");
        v.map.insert("Categories".into(), Val::List(vec!["5".into(), "41".into()]));
        v
    }

    #[test]
    fn prints_fields_conditions_and_ranges() {
        let v = vars();
        let t = r#"{{ if and (.Keywords) (eq .Config.disablesort .False) }}sort-{{ else }}{{ end }}{{ if .Keywords }}search/{{ .Keywords }}{{ else }}cat/Movies{{ end }}/{{ .Config.sort }}/{{ .Config.type }}/1/"#;
        assert_eq!(render(t, &v).unwrap(), "sort-search/the show s01/time/desc/1/");
        assert_eq!(render("{{ range .Categories }}&c={{.}}{{end}}", &v).unwrap(), "&c=5&c=41");
        assert_eq!(render("{{ range $i, $c := .Categories }}{{$i}}:{{$c}};{{ end }}", &v).unwrap(), "0:5;1:41;");
        assert_eq!(render(r#"{{ join .Categories "," }}"#, &v).unwrap(), "5,41");
        assert_eq!(render(r#"{{ re_replace .Keywords "\\s+" "-" }}"#, &v).unwrap(), "the-show-s01");
        assert_eq!(render("{{ if .Query.IMDBID }}imdb{{ else if .Keywords }}kw{{ else }}none{{ end }}", &v).unwrap(), "kw");
        assert_eq!(render("{{ or .Result.a .Result.b .Keywords }}", &v).unwrap(), "the show s01");
        assert_eq!(render("a {{- \" b \" -}} c", &v).unwrap(), "a b c");
        assert_eq!(render("{{ if not .Config.disablesort }}on{{ end }}", &v).unwrap(), "on");
        assert!(render("{{ if .X }}open", &v).is_err());
    }
}
