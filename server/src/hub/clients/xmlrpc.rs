//! A small XML-RPC writer and reader, for rTorrent: calls with text, number, base64 and list parameters, and
//! answers read into JSON values (a fault becomes an error with its message).

use base64::Engine;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{Map, Value};

/// A parameter of a call.
#[derive(Debug, Clone)]
pub enum Param {
    /// Text.
    Str(String),
    /// A whole number.
    Int(i64),
    /// Bytes, sent as base64.
    Bytes(Vec<u8>),
    /// A list.
    List(Vec<Param>),
}

/// The XML of `p`'s value.
fn value(p: &Param, out: &mut String) {
    out.push_str("<value>");
    match p {
        Param::Str(s) => {
            out.push_str("<string>");
            out.push_str(&quick_xml::escape::escape(s.as_str()));
            out.push_str("</string>");
        }
        Param::Int(n) => out.push_str(&format!("<i8>{n}</i8>")),
        Param::Bytes(b) => {
            out.push_str("<base64>");
            out.push_str(&base64::engine::general_purpose::STANDARD.encode(b));
            out.push_str("</base64>");
        }
        Param::List(l) => {
            out.push_str("<array><data>");
            for x in l {
                value(x, out);
            }
            out.push_str("</data></array>");
        }
    }
    out.push_str("</value>");
}

/// The body of a call to `method` with `params`.
pub fn call(method: &str, params: &[Param]) -> String {
    let mut out = format!("<?xml version=\"1.0\"?><methodCall><methodName>{}</methodName><params>", quick_xml::escape::escape(method));
    for p in params {
        out.push_str("<param>");
        value(p, &mut out);
        out.push_str("</param>");
    }
    out.push_str("</params></methodCall>");
    out
}

/// A list or a struct being read.
enum Frame {
    /// A list's items so far.
    List(Vec<Value>),
    /// A struct's members so far, and the name of the member being read.
    Struct(Map<String, Value>, Option<String>),
}

/// Adds `v` to the innermost list or struct, or makes it the answer.
fn attach(frames: &mut [Frame], answer: &mut Option<Value>, v: Value) {
    match frames.last_mut() {
        Some(Frame::List(l)) => l.push(v),
        Some(Frame::Struct(m, name)) => {
            m.insert(name.take().unwrap_or_default(), v);
        }
        None => *answer = Some(v),
    }
}

/// The answer of a call body as JSON; the fault's message as the error.
pub fn parse(body: &str) -> Result<Value, String> {
    let mut r = Reader::from_str(body);
    let mut frames: Vec<Frame> = Vec::new();
    let mut answer: Option<Value> = None;
    let mut text = String::new();
    let mut kind = String::new();
    let mut open = false;
    let mut fault = false;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"fault" => fault = true,
                b"value" => {
                    text.clear();
                    kind.clear();
                    open = true;
                }
                b"array" => {
                    frames.push(Frame::List(Vec::new()));
                    open = false;
                }
                b"struct" => {
                    frames.push(Frame::Struct(Map::new(), None));
                    open = false;
                }
                b"name" => text.clear(),
                t @ (b"string" | b"int" | b"i4" | b"i8" | b"boolean" | b"double" | b"base64" | b"dateTime.iso8601") => {
                    kind = String::from_utf8_lossy(t).into_owned();
                    text.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if matches!(e.name().as_ref(), b"value" | b"string") {
                    attach(&mut frames, &mut answer, Value::String(String::new()));
                    open = false;
                }
            }
            Ok(Event::Text(t)) => text.push_str(&t.unescape().map(|s| s.into_owned()).unwrap_or_default()),
            Ok(Event::CData(t)) => text.push_str(&String::from_utf8_lossy(&t)),
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"name" => {
                    if let Some(Frame::Struct(_, name)) = frames.last_mut() {
                        *name = Some(text.trim().to_string());
                    }
                }
                b"string" | b"int" | b"i4" | b"i8" | b"boolean" | b"double" | b"base64" | b"dateTime.iso8601" => {
                    let t = text.trim();
                    let v = match kind.as_str() {
                        "int" | "i4" | "i8" => t.parse::<i64>().map(Value::from).unwrap_or(Value::Null),
                        "boolean" => Value::Bool(t == "1"),
                        "double" => t.parse::<f64>().map(Value::from).unwrap_or(Value::Null),
                        "base64" => Value::String(
                            base64::engine::general_purpose::STANDARD
                                .decode(t)
                                .map(|b| String::from_utf8_lossy(&b).into_owned())
                                .unwrap_or_default(),
                        ),
                        _ => Value::String(text.clone()),
                    };
                    attach(&mut frames, &mut answer, v);
                    open = false;
                }
                b"value" => {
                    if open {
                        attach(&mut frames, &mut answer, Value::String(text.clone()));
                    }
                    open = false;
                }
                b"array" | b"struct" => {
                    let v = match frames.pop() {
                        Some(Frame::List(l)) => Value::Array(l),
                        Some(Frame::Struct(m, _)) => Value::Object(m),
                        None => Value::Null,
                    };
                    attach(&mut frames, &mut answer, v);
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("the answer is not XML-RPC Lantern can read ({e})")),
            _ => {}
        }
    }
    let answer = answer.ok_or_else(|| "the answer had no value".to_string())?;
    if fault {
        return Err(answer.get("faultString").and_then(Value::as_str).unwrap_or("it gave no reason").to_string());
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_calls_and_reads_answers() {
        let c =
            call("load.raw_start", &[Param::Str("".into()), Param::Bytes(b"d4:infoe".to_vec()), Param::Str("d.custom1.set=a&b".into())]);
        assert!(c.contains("<methodName>load.raw_start</methodName>"));
        assert!(c.contains("<base64>ZDQ6aW5mb2U=</base64>"));
        assert!(c.contains("d.custom1.set=a&amp;b"));
        let a = "<?xml version=\"1.0\"?><methodResponse><params><param><value><array><data>\
                 <value><array><data><value><string>H1</string></value><value><i8>5</i8></value><value>plain</value><value/></data></array></value>\
                 </data></array></value></param></params></methodResponse>";
        assert_eq!(parse(a).unwrap(), json!([["H1", 5, "plain", ""]]));
        let f = "<methodResponse><fault><value><struct><member><name>faultCode</name><value><int>-501</int></value></member>\
                 <member><name>faultString</name><value><string>Could not find info-hash.</string></value></member></struct></value></fault></methodResponse>";
        assert_eq!(parse(f).unwrap_err(), "Could not find info-hash.");
    }
}
