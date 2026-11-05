// Ported from go-rencode v0.1.8 (github.com/gdm85/go-rencode, encoder.go, decoder.go, rencode_generated.go), GPL-3.0.
//! rencode, the compact encoding Deluge's daemon speaks: typed values with short forms for small numbers,
//! strings, lists and dictionaries.

/// The longest a big number may be, written in base 10.
pub const MAX_INT_LENGTH: usize = 64;
const CHR_LIST: u8 = 59;
const CHR_DICT: u8 = 60;
const CHR_INT: u8 = 61;
const CHR_INT1: u8 = 62;
const CHR_INT2: u8 = 63;
const CHR_INT4: u8 = 64;
const CHR_INT8: u8 = 65;
const CHR_FLOAT32: u8 = 66;
const CHR_FLOAT64: u8 = 44;
const CHR_TRUE: u8 = 67;
const CHR_FALSE: u8 = 68;
const CHR_NONE: u8 = 69;
const CHR_TERM: u8 = 127;
const INT_POS_FIXED_START: u8 = 0;
const INT_POS_FIXED_COUNT: u8 = 44;
const DICT_FIXED_START: u8 = 102;
const DICT_FIXED_COUNT: u8 = 25;
const INT_NEG_FIXED_START: u8 = 70;
const INT_NEG_FIXED_COUNT: u8 = 32;
const STR_FIXED_START: u8 = 128;
const STR_FIXED_COUNT: u8 = 64;
const LIST_FIXED_START: u8 = STR_FIXED_START + STR_FIXED_COUNT;
const LIST_FIXED_COUNT: u8 = 64;

/// A rencode value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Nothing (Python's `None`).
    None,
    /// True or false.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A number too big for 64 bits, in base 10.
    Big(String),
    /// A 32-bit float.
    F32(f32),
    /// A 64-bit float.
    F64(f64),
    /// Bytes (rencode's strings).
    Bytes(Vec<u8>),
    /// A list.
    List(Vec<Value>),
    /// A dictionary, in order.
    Dict(Vec<(Value, Value)>),
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Bytes(s.as_bytes().to_vec())
    }
}

impl Value {
    /// The bytes as text, when this is bytes.
    pub fn as_str(&self) -> Option<String> {
        match self {
            Value::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }

    /// The number, when this is one.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// Encodes `v` onto `out`.
pub fn encode(out: &mut Vec<u8>, v: &Value) -> Result<(), String> {
    match v {
        Value::None => out.push(CHR_NONE),
        Value::Bool(b) => out.push(if *b { CHR_TRUE } else { CHR_FALSE }),
        Value::Int(x) => {
            let x = *x;
            if (i8::MIN as i64..=i8::MAX as i64).contains(&x) {
                let x = x as i8;
                if (0..INT_POS_FIXED_COUNT as i8).contains(&x) {
                    out.push(INT_POS_FIXED_START + x as u8);
                } else if (-(INT_NEG_FIXED_COUNT as i8)..0).contains(&x) {
                    out.push((INT_NEG_FIXED_START as i8 - 1 - x) as u8);
                } else {
                    out.push(CHR_INT1);
                    out.push(x as u8);
                }
            } else if (i16::MIN as i64..=i16::MAX as i64).contains(&x) {
                out.push(CHR_INT2);
                out.extend((x as i16).to_be_bytes());
            } else if (i32::MIN as i64..=i32::MAX as i64).contains(&x) {
                out.push(CHR_INT4);
                out.extend((x as i32).to_be_bytes());
            } else {
                out.push(CHR_INT8);
                out.extend(x.to_be_bytes());
            }
        }
        Value::Big(s) => {
            if s.len() > MAX_INT_LENGTH {
                return Err(format!("Number is longer than {MAX_INT_LENGTH} characters"));
            }
            out.push(CHR_INT);
            out.extend(s.as_bytes());
            out.push(CHR_TERM);
        }
        Value::F32(f) => {
            out.push(CHR_FLOAT32);
            out.extend(f.to_be_bytes());
        }
        Value::F64(f) => {
            out.push(CHR_FLOAT64);
            out.extend(f.to_be_bytes());
        }
        Value::Bytes(b) => {
            if b.len() < STR_FIXED_COUNT as usize {
                out.push(STR_FIXED_START + b.len() as u8);
            } else {
                out.extend(format!("{}:", b.len()).as_bytes());
            }
            out.extend(b);
        }
        Value::List(l) => {
            let fixed = l.len() < LIST_FIXED_COUNT as usize;
            out.push(if fixed { LIST_FIXED_START + l.len() as u8 } else { CHR_LIST });
            for x in l {
                encode(out, x)?;
            }
            if !fixed {
                out.push(CHR_TERM);
            }
        }
        Value::Dict(d) => {
            let fixed = d.len() < DICT_FIXED_COUNT as usize;
            out.push(if fixed { DICT_FIXED_START + d.len() as u8 } else { CHR_DICT });
            for (k, x) in d {
                encode(out, k)?;
                encode(out, x)?;
            }
            if !fixed {
                out.push(CHR_TERM);
            }
        }
    }
    Ok(())
}

/// Encodes one value.
pub fn to_bytes(v: &Value) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    encode(&mut out, v)?;
    Ok(out)
}

/// A reader over encoded bytes.
pub struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// A decoder over `data`.
    pub fn new(data: &'a [u8]) -> Decoder<'a> {
        Decoder { data, pos: 0 }
    }

    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.pos).ok_or("EOF")?;
        self.pos += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("unexpected EOF".into());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn until(&mut self, delim: u8) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        loop {
            let b = self.byte()?;
            if b == delim {
                return Ok(out);
            }
            out.push(b);
        }
    }

    /// The next value.
    pub fn value(&mut self) -> Result<Value, String> {
        let t = self.byte()?;
        self.decode(t)
    }

    fn decode(&mut self, t: u8) -> Result<Value, String> {
        Ok(match t {
            CHR_TRUE => Value::Bool(true),
            CHR_FALSE => Value::Bool(false),
            CHR_NONE => Value::None,
            CHR_INT1 => Value::Int(self.byte()? as i8 as i64),
            CHR_INT2 => Value::Int(i16::from_be_bytes(self.take(2)?.try_into().unwrap_or_default()) as i64),
            CHR_INT4 => Value::Int(i32::from_be_bytes(self.take(4)?.try_into().unwrap_or_default()) as i64),
            CHR_INT8 => Value::Int(i64::from_be_bytes(self.take(8)?.try_into().unwrap_or_default())),
            CHR_INT => {
                let s = String::from_utf8_lossy(&self.until(CHR_TERM)?).into_owned();
                match s.parse::<i64>() {
                    Ok(i) => Value::Int(i),
                    Err(_) if !s.is_empty() && s.trim_start_matches('-').bytes().all(|b| b.is_ascii_digit()) => Value::Big(s),
                    Err(_) => return Err("expected integer".into()),
                }
            }
            CHR_FLOAT32 => Value::F32(f32::from_be_bytes(self.take(4)?.try_into().unwrap_or_default())),
            CHR_FLOAT64 => Value::F64(f64::from_be_bytes(self.take(8)?.try_into().unwrap_or_default())),
            CHR_LIST => {
                let mut l = Vec::new();
                loop {
                    let t = self.byte()?;
                    if t == CHR_TERM {
                        break;
                    }
                    l.push(self.decode(t)?);
                }
                Value::List(l)
            }
            CHR_DICT => {
                let mut d = Vec::new();
                loop {
                    let t = self.byte()?;
                    if t == CHR_TERM {
                        break;
                    }
                    let k = self.decode(t)?;
                    let t = self.byte()?;
                    if t == CHR_TERM {
                        d.push((k, Value::None));
                        break;
                    }
                    d.push((k, self.decode(t)?));
                }
                Value::Dict(d)
            }
            t if t < INT_POS_FIXED_START + INT_POS_FIXED_COUNT => Value::Int((t - INT_POS_FIXED_START) as i64),
            t if (INT_NEG_FIXED_START..INT_NEG_FIXED_START + INT_NEG_FIXED_COUNT).contains(&t) => {
                Value::Int(-((t - INT_NEG_FIXED_START) as i64 + 1))
            }
            t if (STR_FIXED_START..STR_FIXED_START + STR_FIXED_COUNT).contains(&t) => {
                Value::Bytes(self.take((t - STR_FIXED_START) as usize)?.to_vec())
            }
            t if (b'1'..=b'9').contains(&t) => {
                let mut n = vec![t];
                n.extend(self.until(b':')?);
                let size: usize = String::from_utf8_lossy(&n).parse().map_err(|e| format!("{e}"))?;
                Value::Bytes(self.take(size)?.to_vec())
            }
            t if t >= LIST_FIXED_START => {
                let mut l = Vec::new();
                for _ in 0..(t - LIST_FIXED_START) {
                    l.push(self.value()?);
                }
                Value::List(l)
            }
            t if (DICT_FIXED_START..DICT_FIXED_START + DICT_FIXED_COUNT).contains(&t) => {
                let mut d = Vec::new();
                for _ in 0..(t - DICT_FIXED_START) {
                    let k = self.value()?;
                    d.push((k, self.value()?));
                }
                Value::Dict(d)
            }
            _ => Value::None,
        })
    }
}

/// Decodes one value from `data`.
pub fn from_bytes(data: &[u8]) -> Result<Value, String> {
    Decoder::new(data).value()
}

#[cfg(test)]
mod tests {
    //! go-rencode's rencode_test.go: every value comes back as it went in.
    use super::*;

    fn round(v: Value) -> Value {
        from_bytes(&to_bytes(&v).unwrap()).unwrap()
    }

    #[test]
    fn numbers_round_trip() {
        for i in [10i64, -10, 100, -100, 27123, -27123, 7483648, -7483648, 8223372036854775808, -8223372036854775808, 45] {
            assert_eq!(round(Value::Int(i)), Value::Int(i), "{i}");
        }
        assert_eq!(to_bytes(&Value::Int(10)).unwrap(), vec![10], "a small number is one byte");
        assert_eq!(to_bytes(&Value::Int(-10)).unwrap(), vec![79]);
        assert_eq!(to_bytes(&Value::Int(100)).unwrap(), vec![CHR_INT1, 100]);
        let big = "590295810358705651680".to_string(); // u64::MAX * 32
        assert_eq!(round(Value::Big(big.clone())), Value::Big(big));
        assert_eq!(round(Value::F32(1234.56)), Value::F32(1234.56));
        assert_eq!(round(Value::F64(1234.56)), Value::F64(1234.56));
    }

    #[test]
    fn strings_none_and_bools_round_trip() {
        for s in ["foobarbaz".to_string(), "f".repeat(255), "fööbar".into(), String::new()] {
            assert_eq!(round(s.as_str().into()), Value::Bytes(s.clone().into_bytes()));
        }
        assert_eq!(round(Value::None), Value::None);
        assert_eq!(round(Value::Bool(true)), Value::Bool(true));
        assert_eq!(round(Value::Bool(false)), Value::Bool(false));
        assert_eq!(round(Value::Bytes(vec![0, 255, 3])), Value::Bytes(vec![0, 255, 3]));
    }

    #[test]
    fn dictionaries_round_trip() {
        let fixed = Value::Dict(vec![("abcdefghijk".into(), Value::Int(1234)), (Value::Bool(false), "bäz".into())]);
        assert_eq!(round(fixed.clone()), fixed);
        let nested_list = Value::List(vec![Value::Bool(true), "carrot".into()]);
        let mut d = Vec::new();
        for i in 0..120 {
            d.push((format!("abcde {i}").as_str().into(), "foo".into()));
            d.push((format!("fghijk {i}").as_str().into(), fixed.clone()));
            d.push((format!("z {i}").as_str().into(), nested_list.clone()));
        }
        let big = Value::Dict(d);
        assert_eq!(round(big.clone()), big);
        let long = Value::List((0..100).map(Value::Int).collect());
        assert_eq!(round(long.clone()), long);
    }
}
