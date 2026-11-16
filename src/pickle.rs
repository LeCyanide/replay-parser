//! A Python pickle reader that builds a value tree without running any code.
//!
//! The game uses pickles in two places: `PYTHON`-typed entity arguments in the
//! packet stream, and `GameParams.data`. Both are written by the client's
//! Python 2, so strings arrive either as `str` (bytes) or `unicode`, and class
//! instances arrive as `GLOBAL` + `REDUCE`/`NEWOBJ` + `BUILD`. Instances are
//! kept as [`Value::Object`] (class, constructor args, state) instead of being
//! constructed: the reader never imports or calls anything.
//!
//! Protocols 0 through 5 are accepted. Persistent ids and the extension
//! registry are not, since nothing the game writes uses them.

use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    /// An integer that does not fit i64, as little-endian two's complement.
    BigInt(Vec<u8>),
    Float(f64),
    /// Python 2 `str` / Python 3 `bytes`.
    Bytes(Vec<u8>),
    /// Python 2 `unicode` / Python 3 `str`.
    Str(String),
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Set(Vec<Value>),
    /// A reference to a module attribute, usually a class.
    Global {
        module: String,
        name: String,
    },
    /// `callable(*args)` that was not evaluated, plus any state `BUILD` set.
    Object {
        callable: Box<Value>,
        args: Box<Value>,
        state: Option<Box<Value>>,
    },
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Bytes(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Looks up a dict entry by string key (`str` or `unicode`).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(entries) => entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The `__dict__` of an object, or the value itself if it is a dict.
    /// Python 2 `copy_reg._reconstructor` objects keep their attributes here.
    pub fn attrs(&self) -> Option<&Value> {
        match self {
            Value::Object { state: Some(s), .. } => match s.as_ref() {
                // BUILD with a (dict, slots) tuple.
                Value::Tuple(t) if t.len() == 2 => t.first(),
                other => Some(other),
            },
            Value::Dict(_) => Some(self),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PickleError {
    #[error("pickle ends early at byte {0}")]
    Eof(usize),
    #[error("unsupported opcode 0x{op:02x} at byte {at}")]
    Opcode { op: u8, at: usize },
    #[error("stack underflow at byte {0}")]
    Underflow(usize),
    #[error("no MARK on the stack at byte {0}")]
    NoMark(usize),
    #[error("memo slot {0} was never written")]
    Memo(usize),
    #[error("malformed {what} at byte {at}")]
    Malformed { what: &'static str, at: usize },
    #[error("{0} applied to a value of the wrong type")]
    Type(&'static str),
}

/// Stack entries and memo slots share one cell, because pickle memoises a
/// container *before* filling it (`EMPTY_LIST`, `PUT`, then `APPENDS`): a later
/// `GET` must see the filled container. A container is only copied out of its
/// cell when a parent consumes it, by which point it is complete.
type Slot = Rc<RefCell<Value>>;

fn take(slot: Slot) -> Value {
    Rc::try_unwrap(slot).map(RefCell::into_inner).unwrap_or_else(|shared| shared.borrow().clone())
}

enum Item {
    Mark,
    Val(Slot),
}

struct Machine<'a> {
    data: &'a [u8],
    pos: usize,
    stack: Vec<Item>,
    memo: Vec<Option<Slot>>,
}

pub fn from_slice(data: &[u8]) -> Result<Value, PickleError> {
    let mut m = Machine { data, pos: 0, stack: Vec::new(), memo: Vec::new() };
    m.run()
}

impl<'a> Machine<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], PickleError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or(PickleError::Eof(self.pos))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, PickleError> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, PickleError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, PickleError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, PickleError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn line(&mut self) -> Result<&'a [u8], PickleError> {
        let rest = &self.data[self.pos..];
        let nl = rest.iter().position(|&b| b == b'\n').ok_or(PickleError::Eof(self.data.len()))?;
        self.pos += nl + 1;
        Ok(&rest[..nl])
    }
    fn line_str(&mut self, what: &'static str) -> Result<&'a str, PickleError> {
        let at = self.pos;
        std::str::from_utf8(self.line()?).map_err(|_| PickleError::Malformed { what, at })
    }

    fn push(&mut self, v: Value) {
        self.stack.push(Item::Val(Rc::new(RefCell::new(v))));
    }
    fn pop(&mut self) -> Result<Slot, PickleError> {
        match self.stack.pop() {
            Some(Item::Val(v)) => Ok(v),
            _ => Err(PickleError::Underflow(self.pos)),
        }
    }
    fn top(&mut self) -> Result<&mut Slot, PickleError> {
        match self.stack.last_mut() {
            Some(Item::Val(v)) => Ok(v),
            _ => Err(PickleError::Underflow(self.pos)),
        }
    }
    /// Everything above the topmost MARK, which is removed.
    fn pop_mark(&mut self) -> Result<Vec<Value>, PickleError> {
        let at = self.pos;
        let idx = self.stack.iter().rposition(|i| matches!(i, Item::Mark)).ok_or(PickleError::NoMark(at))?;
        let items = self.stack.split_off(idx + 1);
        self.stack.pop();
        Ok(items
            .into_iter()
            .map(|i| match i {
                Item::Val(v) => take(v),
                Item::Mark => unreachable!("only the topmost mark was searched for"),
            })
            .collect())
    }
    fn memo_put(&mut self, idx: usize) -> Result<(), PickleError> {
        let v = self.top()?.clone();
        if self.memo.len() <= idx {
            self.memo.resize(idx + 1, None);
        }
        self.memo[idx] = Some(v);
        Ok(())
    }
    fn memo_get(&mut self, idx: usize) -> Result<(), PickleError> {
        let v = self.memo.get(idx).cloned().flatten().ok_or(PickleError::Memo(idx))?;
        self.stack.push(Item::Val(v));
        Ok(())
    }

    fn append(&mut self, items: Vec<Value>) -> Result<(), PickleError> {
        match &mut *self.top()?.borrow_mut() {
            Value::List(l) => l.extend(items),
            Value::Set(s) => s.extend(items),
            // Lists of objects that define `extend`/`append` (rare) are kept
            // as their constructor call; the appended items are dropped onto a
            // list in their state so nothing is lost.
            Value::Object { state, .. } => {
                let s = state.get_or_insert_with(|| Box::new(Value::List(Vec::new())));
                match s.as_mut() {
                    Value::List(l) => l.extend(items),
                    _ => return Err(PickleError::Type("APPEND")),
                }
            }
            _ => return Err(PickleError::Type("APPEND")),
        }
        Ok(())
    }

    fn set_items(&mut self, items: Vec<Value>) -> Result<(), PickleError> {
        if items.len() % 2 != 0 {
            return Err(PickleError::Malformed { what: "SETITEMS pairs", at: self.pos });
        }
        let top = self.top()?.clone();
        let mut target = top.borrow_mut();
        let Value::Dict(d) = &mut *target else { return Err(PickleError::Type("SETITEM")) };
        let mut it = items.into_iter();
        while let (Some(k), Some(v)) = (it.next(), it.next()) {
            // Later keys replace earlier ones, as in Python.
            match d.iter_mut().find(|(ek, _)| *ek == k) {
                Some(slot) => slot.1 = v,
                None => d.push((k, v)),
            }
        }
        Ok(())
    }

    fn run(&mut self) -> Result<Value, PickleError> {
        loop {
            let at = self.pos;
            let op = self.u8()?;
            match op {
                // ---- framing and protocol
                0x80 => {
                    self.u8()?; // PROTO
                }
                0x95 => {
                    self.u64()?; // FRAME: a size hint only
                }
                b'.' => return self.pop().map(take), // STOP

                // ---- stack
                b'(' => self.stack.push(Item::Mark),
                b'0' => {
                    self.stack.pop().ok_or(PickleError::Underflow(at))?;
                }
                b'1' => {
                    self.pop_mark()?; // POP_MARK
                }
                b'2' => {
                    let v = self.top()?.clone(); // DUP
                    self.stack.push(Item::Val(v));
                }

                // ---- memo
                b'p' => {
                    let idx = parse_int_line(self.line_str("PUT")?, at)?;
                    self.memo_put(idx as usize)?;
                }
                b'q' => {
                    let idx = self.u8()? as usize;
                    self.memo_put(idx)?;
                }
                b'r' => {
                    let idx = self.u32()? as usize;
                    self.memo_put(idx)?;
                }
                0x94 => {
                    let idx = self.memo.len(); // MEMOIZE
                    self.memo_put(idx)?;
                }
                b'g' => {
                    let idx = parse_int_line(self.line_str("GET")?, at)?;
                    self.memo_get(idx as usize)?;
                }
                b'h' => {
                    let idx = self.u8()? as usize;
                    self.memo_get(idx)?;
                }
                b'j' => {
                    let idx = self.u32()? as usize;
                    self.memo_get(idx)?;
                }

                // ---- constants
                b'N' => self.push(Value::None),
                0x88 => self.push(Value::Bool(true)),
                0x89 => self.push(Value::Bool(false)),

                // ---- numbers
                b'I' => {
                    let s = self.line_str("INT")?;
                    // Protocol 0 spells booleans as INT 01 / 00.
                    let v = match s {
                        "01" => Value::Bool(true),
                        "00" => Value::Bool(false),
                        s => Value::Int(parse_int_line(s, at)?),
                    };
                    self.push(v);
                }
                b'J' => {
                    let v = self.u32()? as i32;
                    self.push(Value::Int(i64::from(v)));
                }
                b'K' => {
                    let v = self.u8()?;
                    self.push(Value::Int(i64::from(v)));
                }
                b'M' => {
                    let v = self.u16()?;
                    self.push(Value::Int(i64::from(v)));
                }
                b'L' => {
                    let s = self.line_str("LONG")?;
                    let s = s.strip_suffix('L').unwrap_or(s);
                    let v = match s.parse::<i64>() {
                        Ok(v) => Value::Int(v),
                        Err(_) => decimal_to_long(s).ok_or(PickleError::Malformed { what: "LONG", at })?,
                    };
                    self.push(v);
                }
                0x8a => {
                    let n = self.u8()? as usize; // LONG1
                    let b = self.bytes(n)?;
                    self.push(long_from_bytes(b));
                }
                0x8b => {
                    let n = self.u32()? as usize; // LONG4
                    let b = self.bytes(n)?;
                    self.push(long_from_bytes(b));
                }
                b'F' => {
                    let s = self.line_str("FLOAT")?;
                    let v = s.trim().parse::<f64>().map_err(|_| PickleError::Malformed { what: "FLOAT", at })?;
                    self.push(Value::Float(v));
                }
                b'G' => {
                    let v = f64::from_be_bytes(self.bytes(8)?.try_into().unwrap());
                    self.push(Value::Float(v));
                }

                // ---- strings
                b'S' => {
                    let s = self.line()?;
                    let v = unquote_repr(s).ok_or(PickleError::Malformed { what: "STRING", at })?;
                    self.push(Value::Bytes(v));
                }
                b'T' => {
                    let n = self.u32()? as usize; // BINSTRING
                    let b = self.bytes(n)?.to_vec();
                    self.push(Value::Bytes(b));
                }
                b'U' => {
                    let n = self.u8()? as usize; // SHORT_BINSTRING
                    let b = self.bytes(n)?.to_vec();
                    self.push(Value::Bytes(b));
                }
                b'B' => {
                    let n = self.u32()? as usize; // BINBYTES
                    let b = self.bytes(n)?.to_vec();
                    self.push(Value::Bytes(b));
                }
                b'C' => {
                    let n = self.u8()? as usize; // SHORT_BINBYTES
                    let b = self.bytes(n)?.to_vec();
                    self.push(Value::Bytes(b));
                }
                0x8e => {
                    let n = self.u64()? as usize; // BINBYTES8
                    let b = self.bytes(n)?.to_vec();
                    self.push(Value::Bytes(b));
                }
                0x96 => {
                    let n = self.u64()? as usize; // BYTEARRAY8
                    let b = self.bytes(n)?.to_vec();
                    self.push(Value::Bytes(b));
                }
                b'V' => {
                    let s = self.line()?; // UNICODE, raw-unicode-escape
                    self.push(Value::Str(raw_unicode_unescape(s)));
                }
                b'X' => {
                    let n = self.u32()? as usize;
                    let s = self.utf8(n)?;
                    self.push(Value::Str(s));
                }
                0x8c => {
                    let n = self.u8()? as usize; // SHORT_BINUNICODE
                    let s = self.utf8(n)?;
                    self.push(Value::Str(s));
                }
                0x8d => {
                    let n = self.u64()? as usize; // BINUNICODE8
                    let s = self.utf8(n)?;
                    self.push(Value::Str(s));
                }

                // ---- containers
                b']' => self.push(Value::List(Vec::new())),
                b'l' => {
                    let items = self.pop_mark()?;
                    self.push(Value::List(items));
                }
                b'a' => {
                    let v = take(self.pop()?);
                    self.append(vec![v])?;
                }
                b'e' => {
                    let items = self.pop_mark()?;
                    self.append(items)?;
                }
                b')' => self.push(Value::Tuple(Vec::new())),
                b't' => {
                    let items = self.pop_mark()?;
                    self.push(Value::Tuple(items));
                }
                0x85..=0x87 => {
                    let n = (op - 0x84) as usize; // TUPLE1..3
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(take(self.pop()?));
                    }
                    items.reverse();
                    self.push(Value::Tuple(items));
                }
                b'}' => self.push(Value::Dict(Vec::new())),
                b'd' => {
                    let items = self.pop_mark()?;
                    self.push(Value::Dict(Vec::new()));
                    self.set_items(items)?;
                }
                b's' => {
                    let v = take(self.pop()?);
                    let k = take(self.pop()?);
                    self.set_items(vec![k, v])?;
                }
                b'u' => {
                    let items = self.pop_mark()?;
                    self.set_items(items)?;
                }
                0x8f => self.push(Value::Set(Vec::new())), // EMPTY_SET
                0x90 => {
                    let items = self.pop_mark()?; // ADDITEMS
                    self.append(items)?;
                }
                0x91 => {
                    let items = self.pop_mark()?; // FROZENSET
                    self.push(Value::Set(items));
                }

                // ---- objects (recorded, never evaluated)
                b'c' => {
                    let module = self.line_str("GLOBAL module")?.to_owned();
                    let name = self.line_str("GLOBAL name")?.to_owned();
                    self.push(Value::Global { module, name });
                }
                0x93 => {
                    let name = take(self.pop()?); // STACK_GLOBAL
                    let module = take(self.pop()?);
                    match (module, name) {
                        (Value::Str(module), Value::Str(name)) => self.push(Value::Global { module, name }),
                        _ => return Err(PickleError::Type("STACK_GLOBAL")),
                    }
                }
                b'R' | 0x81 => {
                    let args = take(self.pop()?); // REDUCE / NEWOBJ
                    let callable = take(self.pop()?);
                    self.push(Value::Object { callable: Box::new(callable), args: Box::new(args), state: None });
                }
                0x92 => {
                    let _kwargs = self.pop()?; // NEWOBJ_EX
                    let args = take(self.pop()?);
                    let callable = take(self.pop()?);
                    self.push(Value::Object { callable: Box::new(callable), args: Box::new(args), state: None });
                }
                b'i' => {
                    let module = self.line_str("INST module")?.to_owned();
                    let name = self.line_str("INST name")?.to_owned();
                    let args = self.pop_mark()?;
                    self.push(Value::Object {
                        callable: Box::new(Value::Global { module, name }),
                        args: Box::new(Value::Tuple(args)),
                        state: None,
                    });
                }
                b'o' => {
                    let mut items = self.pop_mark()?; // OBJ
                    if items.is_empty() {
                        return Err(PickleError::Underflow(at));
                    }
                    let callable = items.remove(0);
                    self.push(Value::Object {
                        callable: Box::new(callable),
                        args: Box::new(Value::Tuple(items)),
                        state: None,
                    });
                }
                b'b' => {
                    let new_state = take(self.pop()?); // BUILD
                    match &mut *self.top()?.borrow_mut() {
                        Value::Object { state, .. } => *state = Some(Box::new(new_state)),
                        // BUILD on a plain dict merges, as `__setstate__` would.
                        Value::Dict(d) => {
                            if let Value::Dict(extra) = new_state {
                                d.extend(extra);
                            }
                        }
                        _ => return Err(PickleError::Type("BUILD")),
                    }
                }

                _ => return Err(PickleError::Opcode { op, at }),
            }
        }
    }

    /// Python allows lone surrogates in unicode objects, which are not valid
    /// UTF-8; they become U+FFFD rather than failing the whole pickle.
    fn utf8(&mut self, n: usize) -> Result<String, PickleError> {
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
}

fn parse_int_line(s: &str, at: usize) -> Result<i64, PickleError> {
    s.trim().parse::<i64>().map_err(|_| PickleError::Malformed { what: "integer", at })
}

fn long_from_bytes(b: &[u8]) -> Value {
    if b.is_empty() {
        return Value::Int(0);
    }
    if b.len() <= 8 {
        let negative = b[b.len() - 1] & 0x80 != 0;
        let mut buf = if negative { [0xffu8; 8] } else { [0u8; 8] };
        buf[..b.len()].copy_from_slice(b);
        Value::Int(i64::from_le_bytes(buf))
    } else {
        Value::BigInt(b.to_vec())
    }
}

/// A decimal integer too large for i64, as [`Value::BigInt`] in the same
/// minimal two's-complement form `LONG1` uses.
fn decimal_to_long(s: &str) -> Option<Value> {
    let (negative, digits) = match s.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Magnitude, little-endian bytes.
    let mut mag: Vec<u8> = vec![0];
    for d in digits.bytes() {
        let mut carry = u32::from(d - b'0');
        for byte in mag.iter_mut() {
            let v = u32::from(*byte) * 10 + carry;
            *byte = v as u8;
            carry = v >> 8;
        }
        while carry > 0 {
            mag.push(carry as u8);
            carry >>= 8;
        }
    }
    if negative {
        // Two's complement: invert and add one.
        let mut carry = 1u16;
        for byte in mag.iter_mut() {
            let v = u16::from(!*byte) + carry;
            *byte = v as u8;
            carry = v >> 8;
        }
    }
    // Trim to the shortest form that keeps the sign bit right.
    let fill = if negative { 0xff } else { 0x00 };
    while mag.len() > 1 && mag[mag.len() - 1] == fill && (mag[mag.len() - 2] & 0x80 != 0) == negative {
        mag.pop();
    }
    if (mag[mag.len() - 1] & 0x80 != 0) != negative {
        mag.push(fill);
    }
    Some(long_from_bytes(&mag))
}

/// Decodes a Python 2 `repr()` of a str: quoted, with backslash escapes.
fn unquote_repr(s: &[u8]) -> Option<Vec<u8>> {
    let q = *s.first()?;
    if (q != b'\'' && q != b'"') || s.len() < 2 || *s.last()? != q {
        return None;
    }
    let body = &s[1..s.len() - 1];
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        i += 1;
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let e = *body.get(i)?;
        i += 1;
        match e {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'\\' => out.push(b'\\'),
            b'\'' => out.push(b'\''),
            b'"' => out.push(b'"'),
            b'x' => {
                let hex = std::str::from_utf8(body.get(i..i + 2)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b'0'..=b'7' => {
                let start = i - 1;
                let mut end = start + 1;
                while end < body.len() && end < start + 3 && body[end].is_ascii_digit() && body[end] < b'8' {
                    end += 1;
                }
                let oct = std::str::from_utf8(&body[start..end]).ok()?;
                out.push(u8::from_str_radix(oct, 8).ok()?);
                i = end;
            }
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
    }
    Some(out)
}

/// Python's raw-unicode-escape: bytes are Latin-1 except `\uXXXX` and
/// `\UXXXXXXXX`.
fn raw_unicode_unescape(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\\' && i + 1 < s.len() && (s[i + 1] == b'u' || s[i + 1] == b'U') {
            let n = if s[i + 1] == b'u' { 4 } else { 8 };
            if let Some(cp) = s
                .get(i + 2..i + 2 + n)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .and_then(char::from_u32)
            {
                out.push(cp);
                i += 2 + n;
                continue;
            }
        }
        out.push(s[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_protocol_2_dict() {
        // pickle.dumps({'a': 1, 'b': [1.5, None, True]}, 2) under Python 2.
        let p = b"\x80\x02}q\x00(U\x01aq\x01K\x01U\x01bq\x02]q\x03(G?\xf8\x00\x00\x00\x00\x00\x00N\x88eu.";
        let v = from_slice(p).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Int(1)));
        assert_eq!(v.get("b"), Some(&Value::List(vec![Value::Float(1.5), Value::None, Value::Bool(true)])));
    }

    #[test]
    fn long1_sign_extends() {
        assert_eq!(long_from_bytes(&[0xff]), Value::Int(-1));
        assert_eq!(long_from_bytes(&[0x00, 0x80]), Value::Int(-32768));
        assert_eq!(long_from_bytes(&[0xff, 0x00]), Value::Int(255));
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        for input in [&b""[..], b"\x80", b"\x80\x02(", b"\xff", b"\x80\x02h\x05.", b"\x80\x02a."] {
            assert!(from_slice(input).is_err(), "{input:?}");
        }
    }
}
