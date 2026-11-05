// Ported from go-shellquote v0.1.0 (github.com/Hellseher/go-shellquote, unquote.go), MIT, as autobrr v1.90.0 uses it.
//! Splitting a line of arguments into words the way `/bin/sh` does, without running a shell: autobrr's exec
//! actions and exec external filters give the program its arguments this way. Backslash escapes, single quotes and
//! double quotes work; nothing is expanded (no `$VAR`, no `*`, no `~`), so a release name can never run a command.

/// Why a line of arguments can't be split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// A `'` was opened and never closed.
    UnterminatedSingleQuote,
    /// A `"` was opened and never closed.
    UnterminatedDoubleQuote,
    /// The line ends with a lone `\`.
    UnterminatedEscape,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SplitError::UnterminatedSingleQuote => "unterminated single-quoted string",
            SplitError::UnterminatedDoubleQuote => "unterminated double-quoted string",
            SplitError::UnterminatedEscape => "unterminated backslash-escape",
        })
    }
}

/// The characters between words.
const SPLIT_CHARS: [char; 3] = [' ', '\n', '\t'];
/// The characters a backslash escapes inside double quotes.
const DOUBLE_ESCAPE_CHARS: [char; 5] = ['$', '`', '"', '\n', '\\'];

/// The words of `input`, split by `/bin/sh`'s rules (go-shellquote's `Split`).
pub fn split(input: &str) -> Result<Vec<String>, SplitError> {
    let mut words = Vec::new();
    let mut input = input;
    while let Some(c) = input.chars().next() {
        if SPLIT_CHARS.contains(&c) {
            input = &input[c.len_utf8()..];
            continue;
        }
        if c == '\\' {
            let next = &input[1..];
            match next.chars().next() {
                None => return Err(SplitError::UnterminatedEscape),
                Some('\n') => {
                    input = &next[1..];
                    continue;
                }
                Some(_) => {}
            }
        }
        let (word, rest) = split_word(input)?;
        words.push(word);
        input = rest;
    }
    Ok(words)
}

/// Where [`split_word`] is: outside quotes, after a backslash, inside single or double quotes.
enum State {
    Raw,
    Escape,
    Single,
    Double,
}

/// The first word of `input` and what follows it.
fn split_word(mut input: &str) -> Result<(String, &str), SplitError> {
    let mut buf = String::new();
    let mut state = State::Raw;
    loop {
        match state {
            State::Raw => {
                let mut next = None;
                for (i, c) in input.char_indices() {
                    match c {
                        '\'' => next = Some((i, c.len_utf8(), State::Single)),
                        '"' => next = Some((i, c.len_utf8(), State::Double)),
                        '\\' => next = Some((i, c.len_utf8(), State::Escape)),
                        c if SPLIT_CHARS.contains(&c) => {
                            buf.push_str(&input[..i]);
                            return Ok((buf, &input[i + c.len_utf8()..]));
                        }
                        _ => continue,
                    }
                    break;
                }
                match next {
                    Some((i, l, s)) => {
                        buf.push_str(&input[..i]);
                        input = &input[i + l..];
                        state = s;
                    }
                    None => {
                        buf.push_str(input);
                        return Ok((buf, ""));
                    }
                }
            }
            State::Escape => {
                let c = input.chars().next().ok_or(SplitError::UnterminatedEscape)?;
                // a backslash-escaped newline is left out entirely
                if c != '\n' {
                    buf.push(c);
                }
                input = &input[c.len_utf8()..];
                state = State::Raw;
            }
            State::Single => {
                let i = input.find('\'').ok_or(SplitError::UnterminatedSingleQuote)?;
                buf.push_str(&input[..i]);
                input = &input[i + 1..];
                state = State::Raw;
            }
            State::Double => {
                // `start` is where the text not yet copied begins; `i` walks the input
                let bytes = input;
                let mut start = 0;
                let mut it = bytes.char_indices().peekable();
                let mut closed = None;
                while let Some((i, c)) = it.next() {
                    if c == '"' {
                        buf.push_str(&bytes[start..i]);
                        closed = Some(i + 1);
                        break;
                    } else if c == '\\' {
                        if let Some(&(j, c2)) = it.peek() {
                            it.next();
                            if DOUBLE_ESCAPE_CHARS.contains(&c2) {
                                buf.push_str(&bytes[start..i]);
                                // an escaped newline is left out, backslash and all
                                if c2 != '\n' {
                                    buf.push(c2);
                                }
                                start = j + c2.len_utf8();
                            }
                        }
                    }
                }
                match closed {
                    Some(end) => {
                        input = &bytes[end..];
                        state = State::Raw;
                    }
                    None => return Err(SplitError::UnterminatedDoubleQuote),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! go-shellquote's unquote_test.go.
    use super::*;

    #[test]
    fn simple_split() {
        let cases: [(&str, &[&str]); 12] = [
            ("hello", &["hello"]),
            ("hello goodbye", &["hello", "goodbye"]),
            ("hello   goodbye", &["hello", "goodbye"]),
            ("glob* test?", &["glob*", "test?"]),
            ("don\\'t you know the dewey decimal system\\?", &["don't", "you", "know", "the", "dewey", "decimal", "system?"]),
            ("'don'\\''t you know the dewey decimal system?'", &["don't you know the dewey decimal system?"]),
            ("one '' two", &["one", "", "two"]),
            ("text with\\\na backslash-escaped newline", &["text", "witha", "backslash-escaped", "newline"]),
            ("text \"with\na\" quoted newline", &["text", "with\na", "quoted", "newline"]),
            ("\"quoted\\d\\\\\\\" text with\\\na backslash-escaped newline\"", &["quoted\\d\\\" text witha backslash-escaped newline"]),
            ("text with an escaped \\\n newline in the middle", &["text", "with", "an", "escaped", "newline", "in", "the", "middle"]),
            ("foo\"bar\"baz", &["foobarbaz"]),
        ];
        for (input, want) in cases {
            assert_eq!(split(input).unwrap(), want.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{input:?}");
        }
    }

    #[test]
    fn error_split() {
        let cases = [
            ("don't worry", SplitError::UnterminatedSingleQuote),
            ("'test'\\''ing", SplitError::UnterminatedSingleQuote),
            ("\"foo'bar", SplitError::UnterminatedDoubleQuote),
            ("foo\\", SplitError::UnterminatedEscape),
            ("   \\", SplitError::UnterminatedEscape),
        ];
        for (input, want) in cases {
            assert_eq!(split(input).unwrap_err(), want, "{input:?}");
        }
    }
}
