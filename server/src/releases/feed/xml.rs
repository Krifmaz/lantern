//! A small XML tree for reading feeds the way Go's `encoding/xml` and gofeed see them: elements are matched by
//! their local name (a `torznab:attr` is an `attr`), each keeps its prefix, attributes, the text directly in it
//! and its inner markup as written (gofeed's custom elements keep that).

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// One element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    /// The local name (`attr` for `torznab:attr`).
    pub name: String,
    /// The namespace prefix as written (`torznab`), empty when there is none.
    pub prefix: String,
    /// The attributes by local name, as written order.
    pub attrs: Vec<(String, String)>,
    /// The child elements.
    pub children: Vec<Node>,
    /// The text directly inside, entities and CDATA read.
    pub text: String,
    /// The inner markup as written.
    pub inner: String,
}

impl Node {
    /// Attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Every child named `name`.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The text of the first child named `name` (empty without one).
    pub fn child_text(&self, name: &str) -> String {
        self.child(name).map(|c| c.text.clone()).unwrap_or_default()
    }
}

/// `e` as a node without children yet.
fn open(e: &BytesStart<'_>) -> Result<Node, String> {
    let qname = e.name();
    let name = String::from_utf8_lossy(qname.local_name().as_ref()).to_string();
    let prefix = qname.prefix().map(|p| String::from_utf8_lossy(p.as_ref()).to_string()).unwrap_or_default();
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|e| format!("XML syntax error: {e}"))?;
        let key = String::from_utf8_lossy(a.key.local_name().as_ref()).to_string();
        let value = a.unescape_value().map_err(|e| format!("XML syntax error: {e}"))?.to_string();
        attrs.push((key, value));
    }
    Ok(Node { name, prefix, attrs, ..Node::default() })
}

/// The root element of `src`, or the XML problem in words.
pub fn parse(src: &str) -> Result<Node, String> {
    let mut r = Reader::from_str(src);
    let mut stack: Vec<(Node, usize)> = Vec::new();
    let mut root: Option<Node> = None;
    loop {
        let before = r.buffer_position() as usize;
        let ev = r.read_event().map_err(|e| format!("XML syntax error: {e}"))?;
        match ev {
            Event::Start(e) => {
                let n = open(&e)?;
                stack.push((n, r.buffer_position() as usize));
            }
            Event::Empty(e) => {
                let n = open(&e)?;
                match stack.last_mut() {
                    Some((p, _)) => p.children.push(n),
                    None if root.is_none() => root = Some(n),
                    None => {}
                }
            }
            Event::End(_) => {
                let Some((mut n, start)) = stack.pop() else { return Err("XML syntax error: unexpected end element".into()) };
                n.inner = src.get(start..before).unwrap_or("").to_string();
                match stack.last_mut() {
                    Some((p, _)) => p.children.push(n),
                    None if root.is_none() => root = Some(n),
                    None => {}
                }
            }
            Event::Text(t) => {
                if let Some((n, _)) = stack.last_mut() {
                    n.text.push_str(&t.unescape().map_err(|e| format!("XML syntax error: {e}"))?);
                }
            }
            Event::CData(c) => {
                if let Some((n, _)) = stack.last_mut() {
                    n.text.push_str(&String::from_utf8_lossy(&c.into_inner()));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("XML syntax error: unexpected EOF".into());
    }
    root.ok_or_else(|| "EOF".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_local_names_text_and_inner_markup() {
        let n = parse(r#"<?xml version="1.0"?><rss xmlns:t="x"><channel><t:attr name="a" value="1&amp;2"/><item><title><![CDATA[A&B]]></title><torrent><x>1</x></torrent></item></channel></rss>"#).unwrap();
        let ch = n.child("channel").unwrap();
        let attr = ch.child("attr").unwrap();
        assert_eq!((attr.prefix.as_str(), attr.attr("value")), ("t", Some("1&2")));
        let item = ch.child("item").unwrap();
        assert_eq!(item.child_text("title"), "A&B");
        assert_eq!(item.child("torrent").unwrap().inner, "<x>1</x>");
        assert!(parse("<a><b></a>").is_err());
    }
}
