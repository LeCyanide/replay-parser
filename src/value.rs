//! Decoding entity-def typed values from the wire.
//!
//! Strings and blobs borrow from the packet stream. A `USER_TYPE` is always a
//! length-prefixed blob on the wire, whatever inner type its definition
//! declares: `GUN_DIRECTIONS` (declared `UINT16`) and `NULLABLE_VECTOR3`
//! (declared `VECTOR3`) only decode when read that way. Interpreting the blob
//! is the job of whatever knows the converter. Variable-length data is
//! prefixed by a packed count: one byte, or `0xff` followed by a 24-bit
//! little-endian count for 255 and up.

use serde::Serialize;

use crate::defs::Primitive;
use crate::defs::Type;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value<'a> {
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Vector2([f32; 2]),
    Vector3([f32; 3]),
    Vector4([f32; 4]),
    /// `STRING` is bytes on the wire; most are ASCII.
    #[serde(serialize_with = "bytes_as_text")]
    String(&'a [u8]),
    #[serde(serialize_with = "bytes_as_text")]
    UnicodeString(&'a [u8]),
    #[serde(serialize_with = "bytes_as_len")]
    Blob(&'a [u8]),
    /// A pickle; decode with [`crate::pickle::from_slice`].
    #[serde(serialize_with = "bytes_as_len")]
    Python(&'a [u8]),
    List(Vec<Value<'a>>),
    Dict(Vec<(&'a str, Value<'a>)>),
    /// A nullable `FIXED_DICT` that was None.
    None,
}

fn bytes_as_text<S: serde::Serializer>(b: &&[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&String::from_utf8_lossy(b))
}

fn bytes_as_len<S: serde::Serializer>(b: &&[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("<{} bytes>", b.len()))
}

impl<'a> Value<'a> {
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::I64(v) => Some(v),
            Value::U64(v) => i64::try_from(v).ok(),
            _ => None,
        }
    }
    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Value::F32(v) => Some(v),
            Value::F64(v) => Some(v as f32),
            _ => None,
        }
    }
    pub fn get(&self, field: &str) -> Option<&Value<'a>> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| *k == field).map(|(_, v)| v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    #[error("ran out of bytes: wanted {needed}, {available} left")]
    Short { needed: usize, available: usize },
    #[error("nullable dict flag {0:#x} is neither 0 nor 1")]
    NoneFlag(u8),
}

pub(crate) fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8], ValueError> {
    if input.len() < n {
        return Err(ValueError::Short { needed: n, available: input.len() });
    }
    let (h, t) = input.split_at(n);
    *input = t;
    Ok(h)
}

fn arr<const N: usize>(input: &mut &[u8]) -> Result<[u8; N], ValueError> {
    Ok(take(input, N)?.try_into().expect("length checked"))
}

fn f32s<const N: usize>(input: &mut &[u8]) -> Result<[f32; N], ValueError> {
    let mut out = [0f32; N];
    for v in out.iter_mut() {
        *v = f32::from_le_bytes(arr(input)?);
    }
    Ok(out)
}

/// BigWorld's packed count.
pub(crate) fn packed_len(input: &mut &[u8]) -> Result<usize, ValueError> {
    let n = arr::<1>(input)?[0];
    if n != 0xff {
        return Ok(n as usize);
    }
    let b = arr::<3>(input)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], 0]) as usize)
}

fn prefixed<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], ValueError> {
    let n = packed_len(input)?;
    take(input, n)
}

pub fn decode<'a>(ty: &'a Type, input: &mut &'a [u8]) -> Result<Value<'a>, ValueError> {
    Ok(match ty {
        Type::Primitive(p) => match p {
            Primitive::U8 => Value::U64(u64::from(arr::<1>(input)?[0])),
            Primitive::U16 => Value::U64(u64::from(u16::from_le_bytes(arr(input)?))),
            Primitive::U32 => Value::U64(u64::from(u32::from_le_bytes(arr(input)?))),
            Primitive::U64 => Value::U64(u64::from_le_bytes(arr(input)?)),
            Primitive::I8 => Value::I64(i64::from(i8::from_le_bytes(arr(input)?))),
            Primitive::I16 => Value::I64(i64::from(i16::from_le_bytes(arr(input)?))),
            Primitive::I32 => Value::I64(i64::from(i32::from_le_bytes(arr(input)?))),
            Primitive::I64 => Value::I64(i64::from_le_bytes(arr(input)?)),
            Primitive::F32 => Value::F32(f32::from_le_bytes(arr(input)?)),
            Primitive::F64 => Value::F64(f64::from_le_bytes(arr(input)?)),
            Primitive::Vector2 => Value::Vector2(f32s(input)?),
            Primitive::Vector3 => Value::Vector3(f32s(input)?),
            Primitive::Vector4 => Value::Vector4(f32s(input)?),
            Primitive::String => Value::String(prefixed(input)?),
            Primitive::UnicodeString => Value::UnicodeString(prefixed(input)?),
            Primitive::Blob => Value::Blob(prefixed(input)?),
            Primitive::Python => Value::Python(prefixed(input)?),
        },
        Type::Array { of, size } | Type::Tuple { of, size } => {
            let n = match size {
                Some(n) => *n,
                None => packed_len(input)?,
            };
            // Each element takes at least a byte, except zero-size ones; cap
            // the reservation so a corrupt count cannot allocate wildly.
            let mut items = Vec::with_capacity(n.min(input.len()));
            for _ in 0..n {
                items.push(decode(of, input)?);
            }
            Value::List(items)
        }
        Type::FixedDict { allow_none, fields } => {
            if *allow_none {
                match arr::<1>(input)?[0] {
                    0 => return Ok(Value::None),
                    1 => {}
                    other => return Err(ValueError::NoneFlag(other)),
                }
            }
            let mut out = Vec::with_capacity(fields.len());
            for (name, t) in fields {
                out.push((name.as_str(), decode(t, input)?));
            }
            Value::Dict(out)
        }
        Type::User(_) => Value::Blob(prefixed(input)?),
        Type::Alias { inner, .. } => decode(inner, input)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_lengths() {
        let mut a: &[u8] = &[3, b'a', b'b', b'c'];
        assert_eq!(decode(&Type::Primitive(Primitive::String), &mut a).unwrap(), Value::String(b"abc"));
        let mut long = vec![0xff, 0x2c, 0x01, 0x00];
        long.extend(std::iter::repeat_n(7u8, 300));
        let mut b: &[u8] = &long;
        assert!(matches!(decode(&Type::Primitive(Primitive::Blob), &mut b).unwrap(), Value::Blob(x) if x.len() == 300));
        assert!(b.is_empty());
    }

    #[test]
    fn nullable_dicts() {
        let t = Type::FixedDict { allow_none: true, fields: vec![("a".into(), Type::Primitive(Primitive::U8))] };
        let mut none: &[u8] = &[0];
        assert_eq!(decode(&t, &mut none).unwrap(), Value::None);
        let mut some: &[u8] = &[1, 9];
        assert_eq!(decode(&t, &mut some).unwrap(), Value::Dict(vec![("a", Value::U64(9))]));
        let mut bad: &[u8] = &[2];
        assert_eq!(decode(&t, &mut bad), Err(ValueError::NoneFlag(2)));
    }
}
