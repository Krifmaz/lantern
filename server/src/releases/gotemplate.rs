// Ported from Go 1.27 (src/text/template), BSD-3-Clause, and Masterminds/sprig v3.3.0 (functions.go), MIT, as autobrr v1.90.0 runs its macros.
//! The part of Go's `text/template` (with the sprig functions) autobrr's macros use: `{{ .Field }}`,
//! `{{ .RawVars.key }}`, `{{ if … }}{{ else if … }}{{ else }}{{ end }}`, `{{ with … }}`, `{{ range … }}`,
//! pipelines (`{{ .Title | upper }}`), parenthesized calls, `$` variables, `{{-`/`-}}` trimming and
//! `{{/* comments */}}`. A function or field that isn't there is an error, as in Go.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A value a template works with.
#[derive(Debug, Clone, PartialEq)]
pub enum Tv {
    /// Nothing (a missing map key, `nil`).
    Nil,
    /// Yes or no.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A whole number that can't be negative.
    Uint(u64),
    /// A decimal number.
    Float(f64),
    /// Text.
    Str(String),
    /// Bytes (printed as a list of numbers).
    Bytes(Vec<u8>),
    /// A list.
    List(Vec<Tv>),
    /// A map: a missing key is [`Tv::Nil`].
    Map(BTreeMap<String, Tv>),
    /// A struct (Go type name and fields): a missing field is an error.
    Struct(&'static str, BTreeMap<String, Tv>),
}

impl Tv {
    /// Go's `template.IsTrue`: false, 0, nil and empty text, lists and maps are false.
    fn truth(&self) -> bool {
        match self {
            Tv::Nil => false,
            Tv::Bool(b) => *b,
            Tv::Int(n) => *n != 0,
            Tv::Uint(n) => *n != 0,
            Tv::Float(f) => *f != 0.0,
            Tv::Str(s) => !s.is_empty(),
            Tv::Bytes(b) => !b.is_empty(),
            Tv::List(l) => !l.is_empty(),
            Tv::Map(m) => !m.is_empty(),
            Tv::Struct(..) => true,
        }
    }

    /// The value as Go's `%v` prints it.
    pub fn text(&self) -> String {
        match self {
            Tv::Nil => "<nil>".into(),
            Tv::Bool(b) => b.to_string(),
            Tv::Int(n) => n.to_string(),
            Tv::Uint(n) => n.to_string(),
            Tv::Float(f) => go_float(*f),
            Tv::Str(s) => s.clone(),
            Tv::Bytes(b) => format!("[{}]", b.iter().map(u8::to_string).collect::<Vec<_>>().join(" ")),
            Tv::List(l) => format!("[{}]", l.iter().map(Tv::text).collect::<Vec<_>>().join(" ")),
            Tv::Map(m) => format!("map[{}]", m.iter().map(|(k, v)| format!("{k}:{}", v.text())).collect::<Vec<_>>().join(" ")),
            Tv::Struct(_, m) => format!("{{{}}}", m.values().map(Tv::text).collect::<Vec<_>>().join(" ")),
        }
    }

    /// The value as a number, as sprig's `cast.ToFloat64` reads it (text that isn't a number is 0).
    fn float(&self) -> f64 {
        match self {
            Tv::Int(n) => *n as f64,
            Tv::Uint(n) => *n as f64,
            Tv::Float(f) => *f,
            Tv::Bool(b) => f64::from(u8::from(*b)),
            Tv::Str(s) => s.trim().parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// The value as a whole number, as sprig's `cast.ToInt64` reads it.
    fn int(&self) -> i64 {
        match self {
            Tv::Int(n) => *n,
            Tv::Uint(n) => *n as i64,
            Tv::Float(f) => *f as i64,
            Tv::Bool(b) => i64::from(*b),
            Tv::Str(s) => s.trim().parse().unwrap_or(0),
            _ => 0,
        }
    }

    /// The value as text, as sprig's `strval` reads it.
    fn string(&self) -> String {
        match self {
            Tv::Nil => String::new(),
            Tv::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => other.text(),
        }
    }

    /// The value as a list of texts, as sprig's `strslice` reads it.
    fn strings(&self) -> Vec<String> {
        match self {
            Tv::List(l) => l.iter().filter(|v| **v != Tv::Nil).map(Tv::string).collect(),
            Tv::Nil => Vec::new(),
            other => vec![other.string()],
        }
    }
}

/// A float as Go's `%v` prints it (`strconv.FormatFloat(f, 'g', -1, 64)`).
fn go_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let e = format!("{f:e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    // the shortest form switches to an exponent below 1e-4 and from 1e+06 up
    if !(-4..6).contains(&exp) {
        return go_exp(mant, exp);
    }
    format!("{f}")
}

/// A mantissa and exponent as Go writes them (`1.5e+06`).
fn go_exp(mant: &str, exp: i32) -> String {
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mant}e{sign}{:02}", exp.abs())
}

/// One piece of a parsed template.
#[derive(Debug, Clone)]
enum Node {
    /// Text written as it is.
    Text(String),
    /// `{{ pipeline }}`: the value written.
    Action(Pipe),
    /// `{{ if }}`, `{{ with }}` or `{{ range }}`.
    Branch {
        /// `if`, `with` or `range`.
        kind: &'static str,
        /// The condition.
        pipe: Pipe,
        /// What runs when it holds.
        list: Vec<Node>,
        /// What runs when not.
        other: Vec<Node>,
    },
}

/// A pipeline: commands joined by `|`, maybe declaring variables.
#[derive(Debug, Clone)]
struct Pipe {
    /// The variables declared (`$x :=`) or set (`$x =`).
    decl: Vec<String>,
    /// The commands.
    cmds: Vec<Vec<Arg>>,
    /// The line it is on.
    line: usize,
}

/// One word of a command.
#[derive(Debug, Clone)]
enum Arg {
    /// A function name.
    Func(String),
    /// `.A.B` (empty: `.`).
    Field(Vec<String>),
    /// `$x.A.B` (`$` alone is the data).
    Var(String, Vec<String>),
    /// `( pipeline ).A.B`.
    Sub(Box<Pipe>, Vec<String>),
    /// A constant.
    Const(Tv),
}

/// A parsed template.
#[derive(Debug, Clone)]
pub struct Template {
    /// Its pieces.
    nodes: Vec<Node>,
    /// Its name, for errors.
    name: String,
}

/// The tokens inside an action.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A word: a function, keyword, `true`, `false`, `nil`.
    Ident(String),
    /// `.A.B` or `.`.
    Field(Vec<String>),
    /// `$x` with its fields.
    Var(String, Vec<String>),
    /// A text constant.
    Str(String),
    /// A number constant.
    Num(Tv),
    /// `(`.
    Open,
    /// `)` with the fields after it.
    Close(Vec<String>),
    /// `|`.
    Bar,
    /// `:=`.
    Declare,
    /// `=`.
    Assign,
    /// `,`.
    Comma,
}

/// Reads a field chain `.A.B` starting at `s[i]` (a `.`); the names and where it ends.
fn field_chain(s: &[char], mut i: usize) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    while i < s.len() && s[i] == '.' {
        let start = i + 1;
        let mut j = start;
        while j < s.len() && (s[j].is_alphanumeric() || s[j] == '_') {
            j += 1;
        }
        if j == start {
            if names.is_empty() {
                return (names, j);
            }
            break;
        }
        names.push(s[start..j].iter().collect());
        i = j;
    }
    (names, i)
}

/// Go's escapes in a quoted text.
fn unquote(body: &str, line: usize) -> Result<String, String> {
    let mut out = String::new();
    let mut it = body.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let e = it.next().ok_or_else(|| format!("line {line}: unterminated quoted string"))?;
        match e {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'v' => out.push('\x0b'),
            '\\' | '"' | '\'' => out.push(e),
            'x' | 'u' | 'U' => {
                let n = match e {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let hex: String = it.by_ref().take(n).collect();
                let c = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
                out.push(c.ok_or_else(|| format!("line {line}: bad escape"))?);
            }
            _ => return Err(format!("line {line}: unknown escape sequence")),
        }
    }
    Ok(out)
}

/// Splits an action's text into tokens.
fn lex(src: &str, line: usize) -> Result<Vec<Tok>, String> {
    let s: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '.' if i + 1 >= s.len() || !s[i + 1].is_ascii_digit() => {
                let (names, j) = field_chain(&s, i);
                out.push(Tok::Field(names));
                i = j.max(i + 1);
            }
            '$' => {
                let mut j = i + 1;
                while j < s.len() && (s[j].is_alphanumeric() || s[j] == '_') {
                    j += 1;
                }
                let name: String = s[i..j].iter().collect();
                let (names, k) = field_chain(&s, j);
                out.push(Tok::Var(name, names));
                i = k;
            }
            '"' => {
                let mut j = i + 1;
                while j < s.len() && s[j] != '"' {
                    if s[j] == '\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j >= s.len() {
                    return Err(format!("line {line}: unterminated quoted string"));
                }
                out.push(Tok::Str(unquote(&s[i + 1..j].iter().collect::<String>(), line)?));
                i = j + 1;
            }
            '`' => {
                let j = (i + 1..s.len()).find(|&j| s[j] == '`').ok_or_else(|| format!("line {line}: unterminated raw quoted string"))?;
                out.push(Tok::Str(s[i + 1..j].iter().collect()));
                i = j + 1;
            }
            '\'' => {
                let j = (i + 1..s.len())
                    .find(|&j| s[j] == '\'' && s[j - 1] != '\\')
                    .ok_or_else(|| format!("line {line}: unterminated character constant"))?;
                let ch = unquote(&s[i + 1..j].iter().collect::<String>(), line)?;
                out.push(Tok::Num(Tv::Int(ch.chars().next().map(|c| c as i64).unwrap_or(0))));
                i = j + 1;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                let (names, j) = field_chain(&s, i + 1);
                out.push(Tok::Close(names));
                i = j.max(i + 1);
            }
            '|' => {
                out.push(Tok::Bar);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            ':' if s.get(i + 1) == Some(&'=') => {
                out.push(Tok::Declare);
                i += 2;
            }
            '=' => {
                out.push(Tok::Assign);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' => {
                let mut j = i + 1;
                while j < s.len()
                    && (s[j].is_ascii_alphanumeric()
                        || s[j] == '.'
                        || s[j] == '_'
                        || ((s[j] == '-' || s[j] == '+') && matches!(s[j - 1], 'e' | 'E')))
                {
                    j += 1;
                }
                let t: String = s[i..j].iter().collect::<String>().replace('_', "");
                let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    i64::from_str_radix(h, 16).ok().map(Tv::Int)
                } else if t.contains(['.', 'e', 'E']) {
                    t.parse::<f64>().ok().map(Tv::Float)
                } else {
                    t.parse::<i64>().ok().map(Tv::Int).or_else(|| t.parse::<u64>().ok().map(Tv::Uint))
                };
                out.push(Tok::Num(v.ok_or_else(|| format!("line {line}: bad number syntax: \"{t}\""))?));
                i = j;
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut j = i + 1;
                while j < s.len() && (s[j].is_alphanumeric() || s[j] == '_') {
                    j += 1;
                }
                out.push(Tok::Ident(s[i..j].iter().collect()));
                i = j;
            }
            _ => return Err(format!("line {line}: unexpected \"{c}\" in command")),
        }
    }
    Ok(out)
}

/// The names of the functions templates may call.
const FUNCS: &[&str] = &[
    // text/template
    "and",
    "or",
    "not",
    "len",
    "index",
    "print",
    "printf",
    "println",
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "html",
    "js",
    "urlquery",
    "slice",
    // sprig: text
    "upper",
    "lower",
    "title",
    "untitle",
    "trim",
    "trimAll",
    "trimPrefix",
    "trimSuffix",
    "replace",
    "contains",
    "hasPrefix",
    "hasSuffix",
    "repeat",
    "substr",
    "trunc",
    "quote",
    "squote",
    "cat",
    "nospace",
    "abbrev",
    "initials",
    "snakecase",
    "camelcase",
    "kebabcase",
    "indent",
    "nindent",
    "plural",
    "toString",
    "join",
    "split",
    "splitList",
    "splitn",
    "regexMatch",
    "regexFind",
    "regexFindAll",
    "regexReplaceAll",
    "regexReplaceAllLiteral",
    "regexSplit",
    "regexQuoteMeta",
    "b64enc",
    "b64dec",
    "sha1sum",
    "sha256sum",
    "urlquery",
    // sprig: defaults and flow
    "default",
    "empty",
    "coalesce",
    "ternary",
    "fail",
    "toJson",
    "toPrettyJson",
    "toRawJson",
    // sprig: numbers
    "add",
    "add1",
    "sub",
    "mul",
    "div",
    "mod",
    "max",
    "min",
    "addf",
    "add1f",
    "subf",
    "mulf",
    "divf",
    "maxf",
    "minf",
    "floor",
    "ceil",
    "round",
    "int",
    "int64",
    "float64",
    "atoi",
    "toDecimal",
    "seq",
    "until",
    // sprig: lists
    "list",
    "first",
    "last",
    "rest",
    "initial",
    "append",
    "prepend",
    "concat",
    "reverse",
    "uniq",
    "has",
    "without",
    "compact",
    "sortAlpha",
    // sprig: dates
    "now",
    "date",
    "unixEpoch",
];

/// What ended a list of nodes: `end` or `else`, the tokens after an `else`, and its line.
type Ender = (String, Vec<Tok>, usize);

/// Parses actions into nodes.
struct Parser {
    /// The text and actions, in order: `Err(text)` for text, `Ok((tokens, line))` for an action.
    items: Vec<Result<(Vec<Tok>, usize), String>>,
    /// The next item.
    pos: usize,
}

impl Parser {
    /// The nodes up to `{{ end }}` or `{{ else }}`; which of those ended it (with the rest of an `else`'s
    /// tokens).
    fn list(&mut self) -> Result<(Vec<Node>, Option<Ender>), String> {
        let mut nodes = Vec::new();
        while self.pos < self.items.len() {
            let item = self.items[self.pos].clone();
            self.pos += 1;
            match item {
                Err(text) => nodes.push(Node::Text(text)),
                Ok((toks, line)) => {
                    let first = match toks.first() {
                        Some(Tok::Ident(w)) => w.clone(),
                        _ => String::new(),
                    };
                    match first.as_str() {
                        "end" => return Ok((nodes, Some(("end".into(), Vec::new(), line)))),
                        "else" => return Ok((nodes, Some(("else".into(), toks[1..].to_vec(), line)))),
                        "if" | "with" | "range" => nodes.push(self.branch(&first, &toks[1..], line)?),
                        "define" | "template" | "block" | "break" | "continue" => {
                            return Err(format!("line {line}: {first} is not supported in macros"));
                        }
                        _ => nodes.push(Node::Action(pipe(&toks, line)?)),
                    }
                }
            }
        }
        Ok((nodes, None))
    }

    /// An `if`, `with` or `range` with what follows it.
    fn branch(&mut self, kind: &str, toks: &[Tok], line: usize) -> Result<Node, String> {
        let kind: &'static str = match kind {
            "if" => "if",
            "with" => "with",
            _ => "range",
        };
        let p = pipe(toks, line)?;
        let (list, end) = self.list()?;
        let mut other = Vec::new();
        match end {
            None => return Err(format!("line {line}: unexpected EOF")),
            Some((e, _, _)) if e == "end" => {}
            Some((_, rest, eline)) => match rest.first() {
                Some(Tok::Ident(w)) if (w == "if" || w == "with") && !rest.is_empty() => {
                    // {{ else if … }}: an if inside the else, ended by the same {{ end }}
                    other.push(self.branch(&w.clone(), &rest[1..], eline)?);
                }
                _ => {
                    let (o, end) = self.list()?;
                    if !matches!(end, Some((ref e, _, _)) if e == "end") {
                        return Err(format!("line {eline}: expected end; found {:?}", end.map(|e| e.0)));
                    }
                    other = o;
                }
            },
        }
        Ok(Node::Branch { kind, pipe: p, list, other })
    }
}

/// A pipeline from its tokens.
fn pipe(toks: &[Tok], line: usize) -> Result<Pipe, String> {
    let mut decl = Vec::new();
    let mut rest = toks;
    // $x := … or $x, $y := …
    let mut k = 0;
    let mut names = Vec::new();
    while let Some(Tok::Var(n, f)) = rest.get(k) {
        if !f.is_empty() {
            break;
        }
        names.push(n.clone());
        k += 1;
        if rest.get(k) == Some(&Tok::Comma) {
            k += 1;
            continue;
        }
        break;
    }
    if !names.is_empty() && matches!(rest.get(k), Some(Tok::Declare) | Some(Tok::Assign)) {
        decl = names;
        rest = &rest[k + 1..];
    }
    let mut cmds = Vec::new();
    let mut i = 0;
    let mut cmd = Vec::new();
    while i < rest.len() {
        match &rest[i] {
            Tok::Bar => {
                if cmd.is_empty() {
                    return Err(format!("line {line}: missing command"));
                }
                cmds.push(std::mem::take(&mut cmd));
                i += 1;
            }
            Tok::Open => {
                let mut depth = 1;
                let mut j = i + 1;
                while j < rest.len() {
                    match rest[j] {
                        Tok::Open => depth += 1,
                        Tok::Close(_) => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let Some(Tok::Close(fields)) = rest.get(j) else {
                    return Err(format!("line {line}: unclosed left paren"));
                };
                cmd.push(Arg::Sub(Box::new(pipe(&rest[i + 1..j], line)?), fields.clone()));
                i = j + 1;
            }
            Tok::Close(_) => return Err(format!("line {line}: unexpected right paren")),
            Tok::Ident(w) => {
                cmd.push(match w.as_str() {
                    "true" => Arg::Const(Tv::Bool(true)),
                    "false" => Arg::Const(Tv::Bool(false)),
                    "nil" => Arg::Const(Tv::Nil),
                    w if FUNCS.contains(&w) => Arg::Func(w.into()),
                    w => return Err(format!("line {line}: function \"{w}\" not defined")),
                });
                i += 1;
            }
            Tok::Field(f) => {
                cmd.push(Arg::Field(f.clone()));
                i += 1;
            }
            Tok::Var(n, f) => {
                cmd.push(Arg::Var(n.clone(), f.clone()));
                i += 1;
            }
            Tok::Str(s) => {
                cmd.push(Arg::Const(Tv::Str(s.clone())));
                i += 1;
            }
            Tok::Num(n) => {
                cmd.push(Arg::Const(n.clone()));
                i += 1;
            }
            Tok::Declare | Tok::Assign | Tok::Comma => return Err(format!("line {line}: unexpected token in command")),
        }
    }
    if !cmd.is_empty() {
        cmds.push(cmd);
    }
    if cmds.is_empty() {
        return Err(format!("line {line}: missing value for command"));
    }
    Ok(Pipe { decl, cmds, line })
}

impl Template {
    /// Parses `src`; the problem as Go words it when it can't be (`template: macro:1: function "x" not
    /// defined`).
    pub fn parse(name: &str, src: &str) -> Result<Template, String> {
        let mut items = Vec::new();
        let mut rest = src;
        let mut line = 1;
        let mut trim_next = false;
        while !rest.is_empty() {
            let Some(open) = rest.find("{{") else {
                let t = if trim_next { rest.trim_start() } else { rest };
                items.push(Err(t.to_string()));
                break;
            };
            let mut text = &rest[..open];
            if trim_next {
                text = text.trim_start();
            }
            let after = &rest[open + 2..];
            let trim_left = after.starts_with("- ") || after.starts_with("-\t") || after.starts_with("-\n");
            if trim_left {
                text = text.trim_end();
            }
            if !text.is_empty() {
                items.push(Err(text.to_string()));
            }
            line += rest[..open].matches('\n').count();
            let body_start = if trim_left { 2 } else { 0 };
            let after = &after[body_start..];
            let (close, body) = if after.trim_start().starts_with("/*") {
                let end = after.find("*/").ok_or_else(|| format!("template: {name}:{line}: unclosed comment"))?;
                let c =
                    after[end + 2..].find("}}").map(|c| c + end + 2).ok_or_else(|| format!("template: {name}:{line}: unclosed action"))?;
                (c, None)
            } else {
                let c = find_close(after).ok_or_else(|| format!("template: {name}:{line}: unclosed action"))?;
                (c, Some(&after[..c]))
            };
            let mut body = body;
            trim_next = false;
            if let Some(b) = body {
                let t = b.trim_end();
                if t.ends_with(" -") || t.ends_with("\t-") || t.ends_with("\n-") {
                    trim_next = true;
                    body = Some(&t[..t.len() - 1]);
                }
            } else if after[..close].trim_end().ends_with('-') {
                trim_next = true;
            }
            if let Some(b) = body {
                let toks = lex(b, line).map_err(|e| format!("template: {name}:{e}"))?;
                if toks.is_empty() {
                    return Err(format!("template: {name}:{line}: missing value for command"));
                }
                items.push(Ok((toks, line)));
                line += b.matches('\n').count();
            }
            rest = &after[close + 2..];
        }
        let mut p = Parser { items, pos: 0 };
        let (nodes, end) = p.list().map_err(|e| format!("template: {name}:{e}"))?;
        if let Some((what, _, l)) = end {
            return Err(format!("template: {name}:{l}: unexpected {{{{{what}}}}}"));
        }
        Ok(Template { nodes, name: name.to_string() })
    }

    /// Runs the template on `data`; the text, or the problem as Go words it.
    pub fn execute(&self, data: &Tv) -> Result<String, String> {
        let mut out = String::new();
        let mut vars = vec![("$".to_string(), data.clone())];
        self.run(&self.nodes, data, &mut vars, &mut out)?;
        Ok(out)
    }

    /// Runs `nodes` with `dot`.
    fn run(&self, nodes: &[Node], dot: &Tv, vars: &mut Vec<(String, Tv)>, out: &mut String) -> Result<(), String> {
        for n in nodes {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Action(p) => {
                    let v = self.pipe(p, dot, vars)?;
                    if p.decl.is_empty() {
                        out.push_str(&match v {
                            Tv::Nil => "<no value>".to_string(),
                            v => v.text(),
                        });
                    }
                }
                Node::Branch { kind, pipe, list, other } => {
                    let mark = vars.len();
                    let v = self.pipe(pipe, dot, vars)?;
                    match *kind {
                        "range" => {
                            let items: Vec<(Tv, Tv)> = match &v {
                                Tv::List(l) => l.iter().enumerate().map(|(i, x)| (Tv::Int(i as i64), x.clone())).collect(),
                                Tv::Map(m) => m.iter().map(|(k, x)| (Tv::Str(k.clone()), x.clone())).collect(),
                                Tv::Int(n) => (0..*n).map(|i| (Tv::Int(i), Tv::Int(i))).collect(),
                                Tv::Nil => Vec::new(),
                                other => return Err(self.err(pipe.line, &format!("range can't iterate over {}", other.text()))),
                            };
                            if items.is_empty() {
                                self.run(other, dot, vars, out)?;
                            }
                            for (k, x) in items {
                                match pipe.decl.as_slice() {
                                    [one] => set_var(vars, one, x.clone()),
                                    [a, b] => {
                                        set_var(vars, a, k);
                                        set_var(vars, b, x.clone());
                                    }
                                    _ => {}
                                }
                                self.run(list, &x, vars, out)?;
                            }
                        }
                        "with" => {
                            if v.truth() {
                                self.run(list, &v, vars, out)?;
                            } else {
                                self.run(other, dot, vars, out)?;
                            }
                        }
                        _ => {
                            if v.truth() {
                                self.run(list, dot, vars, out)?;
                            } else {
                                self.run(other, dot, vars, out)?;
                            }
                        }
                    }
                    vars.truncate(mark.max(1));
                }
            }
        }
        Ok(())
    }

    /// A problem while running, as Go words it.
    fn err(&self, line: usize, what: &str) -> String {
        format!("template: {}:{line}: executing \"{}\": {what}", self.name, self.name)
    }

    /// Runs a pipeline: each command's value is the last argument of the next.
    fn pipe(&self, p: &Pipe, dot: &Tv, vars: &mut Vec<(String, Tv)>) -> Result<Tv, String> {
        let mut last: Option<Tv> = None;
        for cmd in &p.cmds {
            last = Some(self.command(cmd, dot, vars, last, p.line)?);
        }
        let v = last.unwrap_or(Tv::Nil);
        for d in &p.decl {
            set_var(vars, d, v.clone());
        }
        Ok(v)
    }

    /// Runs one command.
    fn command(&self, cmd: &[Arg], dot: &Tv, vars: &mut Vec<(String, Tv)>, piped: Option<Tv>, line: usize) -> Result<Tv, String> {
        match &cmd[0] {
            Arg::Func(name) => {
                let mut args = Vec::new();
                for a in &cmd[1..] {
                    args.push(self.arg(a, dot, vars, line)?);
                }
                if let Some(p) = piped {
                    args.push(p);
                }
                call(name, &args).map_err(|e| self.err(line, &format!("error calling {name}: {e}")))
            }
            first => {
                if cmd.len() > 1 || piped.is_some() {
                    return Err(self.err(line, "can't give argument to non-function"));
                }
                self.arg(first, dot, vars, line)
            }
        }
    }

    /// The value of one argument.
    fn arg(&self, a: &Arg, dot: &Tv, vars: &mut Vec<(String, Tv)>, line: usize) -> Result<Tv, String> {
        match a {
            Arg::Const(v) => Ok(v.clone()),
            Arg::Field(names) => self.fields(dot.clone(), names, line),
            Arg::Var(n, names) => {
                let v = vars
                    .iter()
                    .rev()
                    .find(|(k, _)| k == n)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| self.err(line, &format!("undefined variable: {n}")))?;
                self.fields(v, names, line)
            }
            Arg::Sub(p, names) => {
                let v = self.pipe(p, dot, vars)?;
                self.fields(v, names, line)
            }
            Arg::Func(name) => call(name, &[]).map_err(|e| self.err(line, &format!("error calling {name}: {e}"))),
        }
    }

    /// `v.A.B`.
    fn fields(&self, mut v: Tv, names: &[String], line: usize) -> Result<Tv, String> {
        for n in names {
            v = match v {
                Tv::Map(m) => m.get(n).cloned().unwrap_or(Tv::Nil),
                Tv::Struct(t, m) => match m.get(n) {
                    Some(x) => x.clone(),
                    None => return Err(self.err(line, &format!("can't evaluate field {n} in type {t}"))),
                },
                Tv::Nil => return Err(self.err(line, &format!("nil pointer evaluating .{n}"))),
                other => return Err(self.err(line, &format!("can't evaluate field {n} in type {}", type_name(&other)))),
            };
        }
        Ok(v)
    }
}

/// The closing `}}` of an action, outside quotes.
fn find_close(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i + 1 < b.len() {
        match quote {
            Some(q) => {
                if b[i] == b'\\' && q != b'`' {
                    i += 1;
                } else if b[i] == q {
                    quote = None;
                }
            }
            None => {
                if b[i] == b'}' && b[i + 1] == b'}' {
                    return Some(i);
                }
                if matches!(b[i], b'"' | b'`' | b'\'') {
                    quote = Some(b[i]);
                }
            }
        }
        i += 1;
    }
    None
}

/// Sets (or declares) variable `n`.
fn set_var(vars: &mut Vec<(String, Tv)>, n: &str, v: Tv) {
    match vars.iter_mut().rev().find(|(k, _)| k == n) {
        Some(slot) => slot.1 = v,
        None => vars.push((n.to_string(), v)),
    }
}

/// A value's Go type name, for errors.
fn type_name(v: &Tv) -> &'static str {
    match v {
        Tv::Nil => "nil",
        Tv::Bool(_) => "bool",
        Tv::Int(_) => "int",
        Tv::Uint(_) => "uint64",
        Tv::Float(_) => "float64",
        Tv::Str(_) => "string",
        Tv::Bytes(_) => "[]uint8",
        Tv::List(_) => "[]string",
        Tv::Map(_) => "map[string]string",
        Tv::Struct(t, _) => t,
    }
}

/// Compares two values as Go's `eq`/`lt` do; `None` when they can't be compared.
fn compare(a: &Tv, b: &Tv) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Tv::Str(x), Tv::Str(y)) => Some(x.cmp(y)),
        (Tv::Bool(x), Tv::Bool(y)) => Some(x.cmp(y)),
        (Tv::Int(x), Tv::Int(y)) => Some(x.cmp(y)),
        (Tv::Uint(x), Tv::Uint(y)) => Some(x.cmp(y)),
        (Tv::Int(x), Tv::Uint(y)) => Some(if *x < 0 { std::cmp::Ordering::Less } else { (*x as u64).cmp(y) }),
        (Tv::Uint(x), Tv::Int(y)) => Some(if *y < 0 { std::cmp::Ordering::Greater } else { x.cmp(&(*y as u64)) }),
        (Tv::Float(x), Tv::Float(y)) => x.partial_cmp(y),
        (Tv::Nil, Tv::Nil) => Some(std::cmp::Ordering::Equal),
        _ => None,
    }
}

/// Go's `fmt.Sprintf`, for the verbs macros use: `%v %s %d %f %e %g %q %t %x %X %c %%`, with flags, width and
/// precision.
pub fn sprintf(format: &str, args: &[Tv]) -> String {
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    let mut n = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '%' {
            out.push(c);
            continue;
        }
        let (mut minus, mut plus, mut zero, mut space) = (false, false, false, false);
        while i < chars.len() && matches!(chars[i], '-' | '+' | '0' | ' ' | '#') {
            match chars[i] {
                '-' => minus = true,
                '+' => plus = true,
                '0' => zero = true,
                ' ' => space = true,
                _ => {}
            }
            i += 1;
        }
        let mut width = None;
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i > start {
            width = chars[start..i].iter().collect::<String>().parse::<usize>().ok();
        }
        let mut prec = None;
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            let s = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            prec = Some(chars[s..i].iter().collect::<String>().parse::<usize>().unwrap_or(0));
        }
        let Some(&verb) = chars.get(i) else {
            out.push_str("%!(NOVERB)");
            break;
        };
        i += 1;
        if verb == '%' {
            out.push('%');
            continue;
        }
        let Some(a) = args.get(n) else {
            let _ = write!(out, "%!{verb}(MISSING)");
            continue;
        };
        n += 1;
        let mut s = match (verb, a) {
            ('d', Tv::Int(x)) => x.to_string(),
            ('d', Tv::Uint(x)) => x.to_string(),
            ('f' | 'F', Tv::Float(x)) => format!("{:.*}", prec.unwrap_or(6), x),
            ('e', Tv::Float(x)) => {
                let e = format!("{:.*e}", prec.unwrap_or(6), x);
                let (m, ex) = e.split_once('e').unwrap_or((&e, "0"));
                go_exp(m, ex.parse().unwrap_or(0))
            }
            ('g', Tv::Float(x)) => go_float(*x),
            ('s' | 'v', v) => {
                let t = v.text();
                match prec {
                    Some(p) if verb == 's' => t.chars().take(p).collect(),
                    _ => t,
                }
            }
            ('q', Tv::Str(x)) => format!("{x:?}"),
            ('t', Tv::Bool(b)) => b.to_string(),
            ('x', Tv::Int(x)) => format!("{x:x}"),
            ('x', Tv::Uint(x)) => format!("{x:x}"),
            ('X', Tv::Int(x)) => format!("{x:X}"),
            ('X', Tv::Uint(x)) => format!("{x:X}"),
            ('x', Tv::Str(x)) => hex::encode(x),
            ('c', Tv::Int(x)) => char::from_u32(*x as u32).map(String::from).unwrap_or_default(),
            (verb, v) => format!("%!{verb}({}={})", type_name(v), v.text()),
        };
        if plus && matches!(a, Tv::Int(_) | Tv::Float(_)) && !s.starts_with('-') && verb != 's' && verb != 'v' {
            s.insert(0, '+');
        } else if space && matches!(a, Tv::Int(_) | Tv::Float(_)) && !s.starts_with('-') {
            s.insert(0, ' ');
        }
        if let Some(w) = width {
            let len = s.chars().count();
            if len < w {
                let pad = w - len;
                if minus {
                    s.push_str(&" ".repeat(pad));
                } else if zero && matches!(a, Tv::Int(_) | Tv::Uint(_) | Tv::Float(_)) {
                    let (sign, digits) = if s.starts_with(['-', '+']) { s.split_at(1) } else { ("", s.as_str()) };
                    s = format!("{sign}{}{digits}", "0".repeat(pad));
                } else {
                    s = format!("{}{s}", " ".repeat(pad));
                }
            }
        }
        out.push_str(&s);
    }
    if n < args.len() {
        out.push_str("%!(EXTRA ");
        out.push_str(&args[n..].iter().map(|a| format!("{}={}", type_name(a), a.text())).collect::<Vec<_>>().join(", "));
        out.push(')');
    }
    out
}

/// Go's `fmt.Sprint`: spaces only between two operands that aren't text.
fn sprint(args: &[Tv]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 && !matches!(a, Tv::Str(_)) && !matches!(args[i - 1], Tv::Str(_)) {
            out.push(' ');
        }
        out.push_str(&a.text());
    }
    out
}

/// The argument `i`, or a problem.
fn at(args: &[Tv], i: usize) -> Result<&Tv, String> {
    args.get(i).ok_or_else(|| format!("wrong number of args: want at least {}, got {}", i + 1, args.len()))
}

/// A regular expression from a sprig call (Go syntax).
fn re(p: &Tv) -> Result<regex::Regex, String> {
    super::rls::goregex::try_compile(&p.string()).ok_or_else(|| format!("error parsing regexp: {}", p.string()))
}

/// Calls function `name`.
fn call(name: &str, a: &[Tv]) -> Result<Tv, String> {
    use Tv::*;
    let s = |i: usize| at(a, i).map(Tv::string);
    let f = |i: usize| at(a, i).map(Tv::float);
    let n = |i: usize| at(a, i).map(Tv::int);
    Ok(match name {
        "and" => a.iter().find(|v| !v.truth()).or(a.last()).cloned().unwrap_or(Nil),
        "or" => a.iter().find(|v| v.truth()).or(a.last()).cloned().unwrap_or(Nil),
        "not" => Bool(!at(a, 0)?.truth()),
        "len" => Int(match at(a, 0)? {
            Str(x) => x.len(),
            List(l) => l.len(),
            Map(m) => m.len(),
            Bytes(b) => b.len(),
            other => return Err(format!("len of type {}", type_name(other))),
        } as i64),
        "index" => {
            let mut v = at(a, 0)?.clone();
            for k in &a[1..] {
                v = match (&v, k) {
                    (List(l), k) => l.get(k.int() as usize).cloned().ok_or_else(|| format!("index out of range: {}", k.int()))?,
                    (Map(m), k) => m.get(&k.string()).cloned().unwrap_or(Nil),
                    (other, _) => return Err(format!("can't index item of type {}", type_name(other))),
                };
            }
            v
        }
        "slice" => match at(a, 0)? {
            Str(x) => {
                let from = a.get(1).map(Tv::int).unwrap_or(0).max(0) as usize;
                let to = a.get(2).map(Tv::int).map(|t| t as usize).unwrap_or(x.len());
                Str(x.get(from..to).ok_or("slice index out of range")?.to_string())
            }
            List(l) => {
                let from = a.get(1).map(Tv::int).unwrap_or(0).max(0) as usize;
                let to = a.get(2).map(Tv::int).map(|t| t as usize).unwrap_or(l.len());
                List(l.get(from..to).ok_or("slice index out of range")?.to_vec())
            }
            other => return Err(format!("can't slice item of type {}", type_name(other))),
        },
        "print" => Str(sprint(a)),
        "println" => Str(a.iter().map(Tv::text).collect::<Vec<_>>().join(" ") + "\n"),
        "printf" => Str(sprintf(&s(0)?, &a[1..])),
        "eq" => {
            let first = at(a, 0)?;
            let mut eq = false;
            for b in &a[1..] {
                match compare(first, b) {
                    Some(o) => eq |= o.is_eq(),
                    None => return Err("incompatible types for comparison".into()),
                }
            }
            Bool(eq)
        }
        "ne" | "lt" | "le" | "gt" | "ge" => {
            let o = compare(at(a, 0)?, at(a, 1)?).ok_or("incompatible types for comparison")?;
            Bool(match name {
                "ne" => o.is_ne(),
                "lt" => o.is_lt(),
                "le" => o.is_le(),
                "gt" => o.is_gt(),
                _ => o.is_ge(),
            })
        }
        "html" => Str(html_escape::encode_safe(&sprint(a)).into_owned()),
        "js" => Str(sprint(a).replace('\\', "\\\\").replace('\'', "\\'").replace('"', "\\\"")),
        "urlquery" => Str(super::definition::query_escape(&sprint(a))),
        "upper" => Str(s(0)?.to_uppercase()),
        "lower" => Str(s(0)?.to_lowercase()),
        "title" => Str(s(0)?
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ")),
        "untitle" => Str(s(0)?
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ")),
        "trim" => Str(s(0)?.trim().to_string()),
        "trimAll" => Str(s(1)?.trim_matches(|c| s(0).unwrap_or_default().contains(c)).to_string()),
        "trimPrefix" => Str(s(1)?.strip_prefix(&s(0)?).map(str::to_string).unwrap_or(s(1)?)),
        "trimSuffix" => Str(s(1)?.strip_suffix(&s(0)?).map(str::to_string).unwrap_or(s(1)?)),
        "replace" => Str(s(2)?.replace(&s(0)?, &s(1)?)),
        "contains" => Bool(s(1)?.contains(&s(0)?)),
        "hasPrefix" => Bool(s(1)?.starts_with(&s(0)?)),
        "hasSuffix" => Bool(s(1)?.ends_with(&s(0)?)),
        "repeat" => Str(s(1)?.repeat(n(0)?.max(0) as usize)),
        "substr" => {
            let t = s(2)?;
            let (from, to) = (n(0)?, n(1)?);
            let from = if from < 0 { 0 } else { from as usize };
            let to = if to < 0 || to as usize > t.len() { t.len() } else { to as usize };
            Str(t.get(from.min(to)..to).unwrap_or("").to_string())
        }
        "trunc" => {
            let (c, t) = (n(0)?, s(1)?);
            let len = t.len() as i64;
            Str(if c < 0 && len + c > 0 {
                t.get((len + c) as usize..).unwrap_or("").to_string()
            } else if c >= 0 && len > c {
                t.get(..c as usize).unwrap_or("").to_string()
            } else {
                t
            })
        }
        "quote" => Str(a.iter().filter(|v| **v != Nil).map(|v| format!("{:?}", v.string())).collect::<Vec<_>>().join(" ")),
        "squote" => Str(a.iter().filter(|v| **v != Nil).map(|v| format!("'{}'", v.string())).collect::<Vec<_>>().join(" ")),
        "cat" => Str(a.iter().filter(|v| **v != Nil).map(Tv::string).collect::<Vec<_>>().join(" ")),
        "nospace" => Str(s(0)?.chars().filter(|c| !c.is_whitespace()).collect()),
        "abbrev" => {
            let (w, t) = (n(0)?, s(1)?);
            Str(if w < 4 || t.len() as i64 <= w { t } else { format!("{}...", t.get(..(w - 3) as usize).unwrap_or("")) })
        }
        "initials" => Str(s(0)?.split_whitespace().filter_map(|w| w.chars().next()).collect()),
        "snakecase" | "kebabcase" | "camelcase" => {
            let t = s(0)?;
            let words: Vec<String> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect();
            Str(match name {
                "snakecase" => words.join("_"),
                "kebabcase" => words.join("-"),
                _ => words
                    .iter()
                    .map(|w| {
                        let mut c = w.chars();
                        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                    })
                    .collect(),
            })
        }
        "indent" | "nindent" => {
            let pad = " ".repeat(n(0)?.max(0) as usize);
            let t = format!("{pad}{}", s(1)?.replace('\n', &format!("\n{pad}")));
            Str(if name == "nindent" { format!("\n{t}") } else { t })
        }
        "plural" => Str(if n(2)? == 1 { s(0)? } else { s(1)? }),
        "toString" => Str(s(0)?),
        "join" => Str(at(a, 1)?.strings().join(&s(0)?)),
        "splitList" => List(s(1)?.split(&s(0)?).map(|x| Str(x.to_string())).collect()),
        "split" => Map(s(1)?.split(&s(0)?).enumerate().map(|(i, x)| (format!("_{i}"), Str(x.to_string()))).collect()),
        "splitn" => {
            Map(s(2)?.splitn(n(1)?.max(0) as usize, &s(0)?).enumerate().map(|(i, x)| (format!("_{i}"), Str(x.to_string()))).collect())
        }
        "regexMatch" => Bool(re(at(a, 0)?)?.is_match(&s(1)?)),
        "regexFind" => Str(re(at(a, 0)?)?.find(&s(1)?).map(|m| m.as_str().to_string()).unwrap_or_default()),
        "regexFindAll" => {
            let lim = n(2)?;
            let t = s(1)?;
            let all = re(at(a, 0)?)?.find_iter(&t).map(|m| Str(m.as_str().to_string())).collect::<Vec<_>>();
            List(if lim < 0 { all } else { all.into_iter().take(lim as usize).collect() })
        }
        "regexReplaceAll" => Str(re(at(a, 0)?)?.replace_all(&s(1)?, s(2)?.as_str()).into_owned()),
        "regexReplaceAllLiteral" => Str(re(at(a, 0)?)?.replace_all(&s(1)?, regex::NoExpand(&s(2)?)).into_owned()),
        "regexSplit" => List(go_regex_split(&re(at(a, 0)?)?, &s(1)?, n(2)?).into_iter().map(Str).collect()),
        "regexQuoteMeta" => Str(regex::escape(&s(0)?)),
        "b64enc" => {
            use base64::Engine;
            Str(base64::engine::general_purpose::STANDARD.encode(s(0)?))
        }
        "b64dec" => {
            use base64::Engine;
            Str(base64::engine::general_purpose::STANDARD
                .decode(s(0)?)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|e| e.to_string()))
        }
        "sha1sum" => {
            use sha1::Digest;
            Str(hex::encode(sha1::Sha1::digest(s(0)?.as_bytes())))
        }
        "sha256sum" => {
            use sha2::Digest;
            Str(hex::encode(sha2::Sha256::digest(s(0)?.as_bytes())))
        }
        "default" => {
            let given = a.get(1).cloned().unwrap_or(Nil);
            if given.truth() {
                given
            } else {
                at(a, 0)?.clone()
            }
        }
        "empty" => Bool(!at(a, 0)?.truth()),
        "coalesce" => a.iter().find(|v| v.truth()).cloned().unwrap_or(Nil),
        "ternary" => {
            if at(a, 2)?.truth() {
                at(a, 0)?.clone()
            } else {
                at(a, 1)?.clone()
            }
        }
        "fail" => return Err(s(0)?),
        "toJson" | "toRawJson" | "toPrettyJson" => {
            let j = to_json(at(a, 0)?);
            Str(if name == "toPrettyJson" { serde_json::to_string_pretty(&j).unwrap_or_default() } else { j.to_string() })
        }
        "add" => Int(a.iter().map(Tv::int).sum()),
        "add1" => Int(n(0)? + 1),
        "sub" => Int(n(0)? - n(1)?),
        "mul" => Int(a.iter().map(Tv::int).product()),
        "div" => {
            let d = n(1)?;
            if d == 0 {
                return Err("integer divide by zero".into());
            }
            Int(n(0)? / d)
        }
        "mod" => {
            let d = n(1)?;
            if d == 0 {
                return Err("integer divide by zero".into());
            }
            Int(n(0)? % d)
        }
        "max" => Int(a.iter().map(Tv::int).max().unwrap_or(0)),
        "min" => Int(a.iter().map(Tv::int).min().unwrap_or(0)),
        "addf" => Float(a.iter().map(Tv::float).sum()),
        "add1f" => Float(f(0)? + 1.0),
        "subf" => Float(a.iter().skip(1).fold(f(0)?, |x, y| x - y.float())),
        "mulf" => Float(a.iter().skip(1).fold(f(0)?, |x, y| x * y.float())),
        "divf" => Float(a.iter().skip(1).fold(f(0)?, |x, y| x / y.float())),
        "maxf" => Float(a.iter().map(Tv::float).fold(f64::MIN, f64::max)),
        "minf" => Float(a.iter().map(Tv::float).fold(f64::MAX, f64::min)),
        "floor" => Float(f(0)?.floor()),
        "ceil" => Float(f(0)?.ceil()),
        "round" => {
            let p = 10f64.powi(n(1)? as i32);
            Float((f(0)? * p).round() / p)
        }
        "int" | "int64" => Int(n(0)?),
        "float64" => Float(f(0)?),
        "atoi" => Int(s(0)?.trim().parse().unwrap_or(0)),
        "toDecimal" => Int(i64::from_str_radix(&s(0)?, 8).unwrap_or(0)),
        "until" => List((0..n(0)?.max(0)).map(Int).collect()),
        "seq" => {
            let (from, to) = match a.len() {
                1 => (1, n(0)?),
                _ => (n(0)?, n(a.len() - 1)?),
            };
            Str((from..=to).map(|x| x.to_string()).collect::<Vec<_>>().join(" "))
        }
        "list" => List(a.to_vec()),
        "first" | "last" | "rest" | "initial" | "reverse" | "uniq" | "compact" | "sortAlpha" => {
            let l = match at(a, 0)? {
                List(l) => l.clone(),
                other => return Err(format!("Cannot find {name} on type {}", type_name(other))),
            };
            match name {
                "first" => l.first().cloned().unwrap_or(Nil),
                "last" => l.last().cloned().unwrap_or(Nil),
                "rest" => List(l.get(1..).unwrap_or(&[]).to_vec()),
                "initial" => List(l.get(..l.len().saturating_sub(1)).unwrap_or(&[]).to_vec()),
                "reverse" => List(l.into_iter().rev().collect()),
                "uniq" => {
                    let mut out: Vec<Tv> = Vec::new();
                    for x in l {
                        if !out.contains(&x) {
                            out.push(x);
                        }
                    }
                    List(out)
                }
                "compact" => List(l.into_iter().filter(Tv::truth).collect()),
                _ => {
                    let mut t = at(a, 0)?.strings();
                    t.sort();
                    List(t.into_iter().map(Str).collect())
                }
            }
        }
        "append" | "prepend" => {
            let mut l = match at(a, 0)? {
                List(l) => l.clone(),
                other => return Err(format!("Cannot {name} on type {}", type_name(other))),
            };
            let v = at(a, 1)?.clone();
            if name == "append" {
                l.push(v);
            } else {
                l.insert(0, v);
            }
            List(l)
        }
        "concat" => List(a.iter().flat_map(|v| if let List(l) = v { l.clone() } else { Vec::new() }).collect()),
        "has" => Bool(matches!(at(a, 1)?, List(l) if l.contains(at(a, 0)?))),
        "without" => match at(a, 0)? {
            List(l) => List(l.iter().filter(|x| !a[1..].contains(x)).cloned().collect()),
            other => return Err(format!("Cannot find without on type {}", type_name(other))),
        },
        "now" => Str(chrono::Local::now().to_rfc3339()),
        "unixEpoch" => Str(chrono::Local::now().timestamp().to_string()),
        "date" => Str(go_date(&s(0)?, &chrono::Local::now())),
        _ => return Err(format!("function \"{name}\" not defined")),
    })
}

/// Go's `Regexp.Split(s, n)`: the text between matches, at most `n` parts (`n < 0`: all; 0: none).
fn go_regex_split(re: &regex::Regex, s: &str, n: i64) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    if !re.as_str().is_empty() && s.is_empty() {
        return vec![String::new()];
    }
    let limit = if n < 0 { usize::MAX } else { n as usize };
    let mut out = Vec::new();
    let (mut beg, mut end) = (0, 0);
    for m in re.find_iter(s).take(limit) {
        if n > 0 && out.len() as i64 == n - 1 {
            break;
        }
        end = m.start();
        if m.end() != 0 {
            out.push(s[beg..end].to_string());
        }
        beg = m.end();
    }
    if end != s.len() {
        out.push(s[beg..].to_string());
    }
    out
}

/// A value as JSON (sprig's `toJson`).
fn to_json(v: &Tv) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Tv::Nil => J::Null,
        Tv::Bool(b) => J::Bool(*b),
        Tv::Int(n) => J::from(*n),
        Tv::Uint(n) => J::from(*n),
        Tv::Float(f) => J::from(*f),
        Tv::Str(s) => J::String(s.clone()),
        Tv::Bytes(b) => J::String(String::from_utf8_lossy(b).into_owned()),
        Tv::List(l) => J::Array(l.iter().map(to_json).collect()),
        Tv::Map(m) | Tv::Struct(_, m) => J::Object(m.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
    }
}

/// A time in Go's layout words (`2006-01-02 15:04:05`), for sprig's `date`.
fn go_date(layout: &str, t: &chrono::DateTime<chrono::Local>) -> String {
    let pairs = [
        ("2006", "%Y"),
        ("January", "%B"),
        ("Jan", "%b"),
        ("Monday", "%A"),
        ("Mon", "%a"),
        ("MST", "%Z"),
        ("01", "%m"),
        ("02", "%d"),
        ("15", "%H"),
        ("03", "%I"),
        ("04", "%M"),
        ("05", "%S"),
        ("06", "%y"),
        ("PM", "%p"),
        ("-0700", "%z"),
    ];
    let mut out = String::new();
    let mut rest = layout;
    'outer: while !rest.is_empty() {
        for (go, strf) in pairs {
            if let Some(r) = rest.strip_prefix(go) {
                out.push_str(&t.format(strf).to_string());
                rest = r;
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, data: Tv) -> Result<String, String> {
        Template::parse("t", src)?.execute(&data)
    }

    #[test]
    fn runs_go_templates() {
        let data = Tv::Struct(
            "domain.Macro",
            BTreeMap::from([
                ("A".to_string(), Tv::Str("x".into())),
                ("N".to_string(), Tv::Int(3)),
                ("L".to_string(), Tv::List(vec![Tv::Str("a".into()), Tv::Str("b".into())])),
            ]),
        );
        assert_eq!(run("{{ .A }}-{{.N}}", data.clone()).unwrap(), "x-3");
        assert_eq!(run("{{ if eq .N 3 }}three{{ else }}no{{ end }}", data.clone()).unwrap(), "three");
        assert_eq!(run("{{ if gt .N 5 }}big{{ else if gt .N 2 }}mid{{ else }}small{{ end }}", data.clone()).unwrap(), "mid");
        assert_eq!(run("{{ range .L }}[{{ . }}]{{ end }}", data.clone()).unwrap(), "[a][b]");
        assert_eq!(run("{{ range $i, $v := .L }}{{ $i }}{{ $v }}{{ end }}", data.clone()).unwrap(), "0a1b");
        assert_eq!(run("a  {{- .A -}}  b", data.clone()).unwrap(), "axb");
        assert_eq!(run("{{/* note */}}{{ .A | upper }}", data.clone()).unwrap(), "X");
        assert_eq!(run("{{ .L }}", data.clone()).unwrap(), "[a b]");
        assert_eq!(run(r#"{{ printf "%05.1f|%-4d|%s" 3.14159 7 "z" }}"#, data.clone()).unwrap(), "003.1|7   |z");
        assert!(run("{{ .Nope }}", data.clone()).is_err());
        assert!(run("{{ nope }}", data).is_err());
    }
}
