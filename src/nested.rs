//! Nested property updates: a change to one field or element somewhere
//! inside a property, instead of the whole property.
//!
//! The payload is a big-endian bit stream naming a path, then a byte-aligned
//! value:
//!
//! ```text
//! 1, index            descend one level (a FIXED_DICT field or an ARRAY
//!                     element); if that child is a scalar, the value follows
//!                     and the change ends here
//! 0, index[, index2]  the change is at this container: set one dict field,
//!                     set one array element, or (slice) replace the element
//!                     range index..index2 with however many elements follow
//!                     (none removes the range)
//! ```
//!
//! An index into a dict takes `ceil(log2(field count))` bits; into an array,
//! `ceil(log2(current length))`, or `ceil(log2(length + 1))` for a slice,
//! which may name one past the end. Because array widths depend on the
//! current length, decoding needs the property's current value.

use serde::Serialize;

use crate::defs::Type;
use crate::value::Value;
use crate::value::ValueError;
use crate::value::decode;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum PathStep<'a> {
    Field(&'a str),
    Index(usize),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Change<'a> {
    SetField { field: &'a str, value: Value<'a> },
    SetElement { index: usize, value: Value<'a> },
    Splice { start: usize, end: usize, values: Vec<Value<'a>> },
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum NestedError {
    #[error("path runs past the end of the bit stream")]
    Bits,
    #[error("index {index} is out of range for {what} of {len}")]
    Index { what: &'static str, index: usize, len: usize },
    #[error("path descends into a {0}, which has no fields or elements")]
    NotContainer(&'static str),
    #[error("path descends into a None dict")]
    NoneDict,
    #[error("set-element carries {0} values, expected exactly 1")]
    ElementCount(usize),
    #[error("value: {0}")]
    Value(#[from] ValueError),
    #[error("{0} bytes left after the value")]
    Leftover(usize),
}

pub(crate) struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    pub fn read(&mut self, count: u32) -> Result<usize, NestedError> {
        let mut v = 0usize;
        for _ in 0..count {
            let byte = *self.data.get(self.bit / 8).ok_or(NestedError::Bits)?;
            v = (v << 1) | usize::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(v)
    }

    /// Skips to the next byte boundary and returns everything from there.
    pub fn rest(&mut self) -> &'a [u8] {
        let start = self.bit.div_ceil(8).min(self.data.len());
        self.bit = self.data.len() * 8;
        &self.data[start..]
    }
}

/// Bits needed to index `n` choices.
pub(crate) fn index_bits(n: usize) -> u32 {
    n.max(1).next_power_of_two().trailing_zeros()
}

/// Strips aliases and user types down to the structural type.
fn peel(ty: &Type) -> &Type {
    match ty {
        Type::Alias { inner, .. } => peel(inner),
        other => other,
    }
}

fn is_scalar(ty: &Type) -> bool {
    !matches!(peel(ty), Type::Array { .. } | Type::Tuple { .. } | Type::FixedDict { .. })
}

/// Decodes one value that must use up `bytes` exactly.
fn exact<'a>(ty: &'a Type, mut bytes: &'a [u8]) -> Result<Value<'a>, NestedError> {
    let v = decode(ty, &mut bytes)?;
    if !bytes.is_empty() {
        return Err(NestedError::Leftover(bytes.len()));
    }
    Ok(v)
}

/// The value a property has before anything sets it.
pub fn default_value(ty: &Type) -> Value<'_> {
    match peel(ty) {
        Type::Primitive(_) | Type::User(_) => match crate::value::decode(ty, &mut &ZEROES[..]) {
            Ok(v) => v,
            Err(_) => Value::None,
        },
        Type::Array { of, size } | Type::Tuple { of, size } => {
            Value::List((0..size.unwrap_or(0)).map(|_| default_value(of)).collect())
        }
        Type::FixedDict { allow_none: true, .. } => Value::None,
        Type::FixedDict { allow_none: false, fields } => {
            Value::Dict(fields.iter().map(|(n, t)| (n.as_str(), default_value(t))).collect())
        }
        Type::Alias { .. } => unreachable!("peeled"),
    }
}

/// Enough zero bytes to decode any scalar as its zero value (a zero-length
/// string or blob, 0, 0.0, a zero vector).
static ZEROES: [u8; 16] = [0; 16];

/// Applies one nested update to `value` (of type `ty`) and reports where and
/// what changed.
pub(crate) fn apply<'a>(
    ty: &'a Type,
    value: &mut Value<'a>,
    is_slice: bool,
    bits: &mut Bits<'a>,
    path: &mut Vec<PathStep<'a>>,
) -> Result<Change<'a>, NestedError> {
    let descend = bits.read(1)? == 1;
    match peel(ty) {
        Type::FixedDict { allow_none, fields } => {
            if *allow_none && *value == Value::None {
                // Setting a field of a None dict materialises it first.
                *value = Value::Dict(fields.iter().map(|(n, t)| (n.as_str(), default_value(t))).collect());
            }
            let Value::Dict(entries) = value else { return Err(NestedError::NoneDict) };
            let index = bits.read(index_bits(fields.len()))?;
            let (field, field_ty) =
                fields.get(index).ok_or(NestedError::Index { what: "dict", index, len: fields.len() })?;
            let slot = match entries.iter_mut().position(|(k, _)| *k == field.as_str()) {
                Some(i) => &mut entries[i].1,
                None => {
                    entries.push((field.as_str(), default_value(field_ty)));
                    &mut entries.last_mut().expect("just pushed").1
                }
            };
            if descend && !is_scalar(field_ty) {
                path.push(PathStep::Field(field));
                return apply(field_ty, slot, is_slice, bits, path);
            }
            let v = exact(field_ty, bits.rest())?;
            *slot = v.clone();
            Ok(Change::SetField { field, value: v })
        }
        Type::Array { of, .. } | Type::Tuple { of, .. } => {
            let Value::List(items) = value else { return Err(NestedError::NotContainer("non-list value")) };
            if descend {
                let index = bits.read(index_bits(items.len()))?;
                if index >= items.len() {
                    return Err(NestedError::Index { what: "array", index, len: items.len() });
                }
                if !is_scalar(of) {
                    path.push(PathStep::Index(index));
                    return apply(of, &mut items[index], is_slice, bits, path);
                }
                let v = exact(of, bits.rest())?;
                items[index] = v.clone();
                return Ok(Change::SetElement { index, value: v });
            }
            let width = index_bits(items.len() + usize::from(is_slice));
            let start = bits.read(width)?;
            let end = if is_slice { Some(bits.read(width)?) } else { None };
            let mut rest = bits.rest();
            let mut values = Vec::new();
            while !rest.is_empty() {
                values.push(decode(of, &mut rest)?);
            }
            match end {
                Some(end) => {
                    if start > end || end > items.len() {
                        return Err(NestedError::Index { what: "slice end", index: end, len: items.len() });
                    }
                    items.splice(start..end, values.iter().cloned());
                    Ok(Change::Splice { start, end, values })
                }
                None => {
                    if values.len() != 1 {
                        return Err(NestedError::ElementCount(values.len()));
                    }
                    if start >= items.len() {
                        return Err(NestedError::Index { what: "array", index: start, len: items.len() });
                    }
                    let v = values.pop().expect("length checked");
                    items[start] = v.clone();
                    Ok(Change::SetElement { index: start, value: v })
                }
            }
        }
        _ => Err(NestedError::NotContainer("scalar")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::Primitive;

    fn u8t() -> Type {
        Type::Primitive(Primitive::U8)
    }

    #[test]
    fn index_widths() {
        assert_eq!([0, 1, 2, 3, 4, 5, 8, 9].map(index_bits), [0, 0, 1, 2, 2, 3, 3, 4]);
    }

    #[test]
    fn sets_a_field_two_levels_down() {
        // { a: u8, b: { x: u8, y: [u8] } }; set b.y[1] = 7 where y has 3 elements.
        let inner = Type::FixedDict {
            allow_none: false,
            fields: vec![("x".into(), u8t()), ("y".into(), Type::Array { of: Box::new(u8t()), size: None })],
        };
        let ty = Type::FixedDict { allow_none: false, fields: vec![("a".into(), u8t()), ("b".into(), inner)] };
        let mut v = Value::Dict(vec![
            ("a", Value::U64(0)),
            ("b", Value::Dict(vec![("x", Value::U64(0)), ("y", Value::List(vec![Value::U64(0); 3]))])),
        ]);
        // bits: 1 (descend) 1 (field b) | 1 (descend) 1 (field y) | 1 (descend) 01 (index 1), pad, then 0x07
        let data = [0b1111_1010, 0x07];
        let mut path = Vec::new();
        let change = apply(&ty, &mut v, false, &mut Bits::new(&data), &mut path).unwrap();
        assert_eq!(path, [PathStep::Field("b"), PathStep::Field("y")]);
        assert_eq!(change, Change::SetElement { index: 1, value: Value::U64(7) });
        assert_eq!(
            v.get("b").unwrap().get("y").unwrap(),
            &Value::List(vec![Value::U64(0), Value::U64(7), Value::U64(0)])
        );
    }

    #[test]
    fn slice_appends_and_removes() {
        let ty = Type::Array { of: Box::new(u8t()), size: None };
        let mut v = Value::List(vec![Value::U64(1), Value::U64(2)]);
        // 0 (here), slice over len 2 -> width 2: start=2 (10) end=2 (10), pad; append 9, 9.
        let data = [0b0101_0000, 9, 9];
        let change = apply(&ty, &mut v, true, &mut Bits::new(&data), &mut Vec::new()).unwrap();
        assert_eq!(change, Change::Splice { start: 2, end: 2, values: vec![Value::U64(9), Value::U64(9)] });
        assert_eq!(v, Value::List(vec![Value::U64(1), Value::U64(2), Value::U64(9), Value::U64(9)]));
        // remove 0..3 from len 4 -> width 3: 0 000 011, no values.
        let data = [0b0000_0110];
        apply(&ty, &mut v, true, &mut Bits::new(&data), &mut Vec::new()).unwrap();
        assert_eq!(v, Value::List(vec![Value::U64(9)]));
    }
}
