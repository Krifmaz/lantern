// Ported from Prowlarr v2.7.0.5723: the way .NET's XDocument writes the documents of
// src/NzbDrone.Core/IndexerSearch/NewznabResults.cs, src/NzbDrone.Core/Indexers/IndexerCapabilities.cs and
// src/Prowlarr.Api.V1/Indexers/NewznabController.cs (CreateErrorXML), GPL-3.0.
//! A small XML writer that writes exactly as Prowlarr's answers look: the declaration on its own line, two spaces of
//! indent, `<empty />` with a space, text inline, and the same characters escaped. Apps compare nothing byte for
//! byte, but the golden tests do, so Lantern's answers can be checked against Prowlarr's.

/// What an element holds: other elements and text.
#[derive(Debug, Clone)]
pub enum Node {
    /// An element.
    Elem(Elem),
    /// Text (an empty one still makes `<a></a>` rather than `<a />`).
    Text(String),
}

/// One element: its name (with its prefix, `torznab:attr`), attributes in order, and what it holds.
#[derive(Debug, Clone)]
pub struct Elem {
    /// The name.
    pub name: String,
    /// The attributes, in order.
    pub attrs: Vec<(String, String)>,
    /// What it holds.
    pub children: Vec<Node>,
}

impl Elem {
    /// An empty element called `name`.
    pub fn new(name: &str) -> Elem {
        Elem { name: name.to_string(), attrs: Vec::new(), children: Vec::new() }
    }

    /// With attribute `k`.
    pub fn attr(mut self, k: &str, v: &str) -> Elem {
        self.attrs.push((k.to_string(), v.to_string()));
        self
    }

    /// With element `e` inside.
    pub fn child(mut self, e: Elem) -> Elem {
        self.children.push(Node::Elem(e));
        self
    }

    /// With each of `es` inside.
    pub fn children(mut self, es: impl IntoIterator<Item = Elem>) -> Elem {
        self.children.extend(es.into_iter().map(Node::Elem));
        self
    }

    /// With text `t` inside (`None` leaves it empty: `<a />`, as .NET writes a null value).
    pub fn text(mut self, t: Option<&str>) -> Elem {
        if let Some(t) = t {
            self.children.push(Node::Text(t.to_string()));
        }
        self
    }

    /// The whole document: the declaration, a line break, the element (`XDocument.Declaration + NewLine + doc`).
    pub fn document(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        self.write(&mut out, 0, true);
        out
    }

    /// Writes the element at `depth`; `indent` is false inside an element that holds text (.NET writes such
    /// content as it is, without line breaks).
    fn write(&self, out: &mut String, depth: usize, indent: bool) {
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            out.push_str(&escape_attr(v));
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str(" />");
            return;
        }
        out.push('>');
        let mixed = self.children.iter().any(|c| matches!(c, Node::Text(_)));
        let inner = indent && !mixed;
        for c in &self.children {
            if inner {
                out.push('\n');
                out.push_str(&"  ".repeat(depth + 1));
            }
            match c {
                Node::Elem(e) => e.write(out, depth + 1, inner),
                Node::Text(t) => out.push_str(&escape_text(t)),
            }
        }
        if inner {
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }
}

/// Whether .NET's writer takes `c` at all (XML 1.0's characters); others are left out rather than failing.
fn allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}')
}

/// Text as .NET writes it: `&`, `<` and `>` escaped, line breaks as `\n`.
pub fn escape_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                o.push('\n');
            }
            c if allowed(c) => o.push(c),
            _ => {}
        }
    }
    o
}

/// An attribute value as .NET writes it: `&`, `<`, `>` and `"` escaped, tabs and line breaks as character
/// references.
pub fn escape_attr(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\t' => o.push_str("&#x9;"),
            '\n' => o.push_str("&#xA;"),
            '\r' => o.push_str("&#xD;"),
            c if allowed(c) => o.push(c),
            _ => {}
        }
    }
    o
}

/// A Newznab error answer: `<error code="…" description="…" />` (`NewznabController.CreateErrorXML`).
pub fn error_xml(code: u16, description: &str) -> String {
    Elem::new("error").attr("code", &code.to_string()).attr("description", description).document()
}

/// A number as .NET writes a `double` in XML (`XmlConvert.ToString`): no `.0` on whole numbers.
pub fn double(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_like_dotnet() {
        let e = Elem::new("a")
            .attr("x", "1 & \"2\"\n")
            .child(Elem::new("b").text(Some("t<'>")))
            .child(Elem::new("c").text(None))
            .child(Elem::new("d").text(Some("")))
            .child(Elem::new("e").child(Elem::new("f")));
        assert_eq!(
            e.document(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a x=\"1 &amp; &quot;2&quot;&#xA;\">\n  <b>t&lt;'&gt;</b>\n  <c />\n  <d></d>\n  <e>\n    <f />\n  </e>\n</a>"
        );
        assert_eq!(
            error_xml(200, "Missing parameter (t)"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<error code=\"200\" description=\"Missing parameter (t)\" />"
        );
        assert_eq!((double(1.0), double(0.5)), ("1".to_string(), "0.5".to_string()));
    }
}
