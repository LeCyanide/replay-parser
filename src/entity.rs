//! Entity-level decoding: turns the envelopes from [`crate::packet`] into
//! named method calls and property values, using [`EntityDefs`].
//!
//! The decoder keeps the state that decoding needs: each live entity's type,
//! and the current value of each of its client properties. Nested property
//! updates can only be decoded against that state (see [`crate::nested`]),
//! and it is also what a battle simulation reads.

use std::collections::HashMap;

use serde::Serialize;

use crate::defs::EntityDef;
use crate::defs::EntityDefs;
use crate::defs::Property;
use crate::nested::Bits;
use crate::nested::Change;
use crate::nested::NestedError;
use crate::nested::PathStep;
use crate::nested::default_value;
use crate::nested::index_bits;
use crate::packet::Decoded;
use crate::packet::Packet;
use crate::packet::Rot3;
use crate::packet::Vec3;
use crate::value::Value;
use crate::value::ValueError;
use crate::value::decode;

#[derive(Debug, Serialize)]
#[serde(tag = "event")]
pub enum Event<'a> {
    Create {
        entity_id: u32,
        entity: &'a str,
        position: Option<Vec3>,
        rotation: Option<Rot3>,
        properties: Vec<(&'a str, Value<'a>)>,
        /// Bytes after the declared properties of a player create packet.
        /// BasePlayerCreate carries component data here; CellPlayerCreate
        /// carried exactly `[0x02]` in every replay observed, meaning unknown.
        #[serde(skip)]
        trailing: &'a [u8],
    },
    Method {
        entity_id: u32,
        entity: &'a str,
        method: &'a str,
        args: Vec<(&'a str, Value<'a>)>,
    },
    Property {
        entity_id: u32,
        entity: &'a str,
        property: &'a str,
        value: Value<'a>,
    },
    /// A change somewhere inside a property. `path` leads from the property
    /// to the container `change` applies to.
    Nested {
        entity_id: u32,
        entity: &'a str,
        property: &'a str,
        path: Vec<PathStep<'a>>,
        change: Change<'a>,
    },
    Leave {
        entity_id: u32,
        entity: &'a str,
    },
}

/// Why an entity packet could not be decoded.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum EntityError {
    #[error("entity {0} was never created")]
    UnknownEntity(u32),
    #[error("entity type {0} is not in entities.xml")]
    UnknownType(u16),
    #[error("{entity} has no client method {index} ({count} defined)")]
    MethodIndex { entity: String, index: u32, count: usize },
    #[error("{entity} has no client property {index} ({count} defined)")]
    PropertyIndex { entity: String, index: u32, count: usize },
    #[error("{entity}.{member}: {error}")]
    Value { entity: String, member: String, error: ValueError },
    #[error("{entity}.{member}: {extra} bytes left after decoding")]
    Leftover { entity: String, member: String, extra: usize },
    #[error("{entity}.{member}: nested update: {error}")]
    Nested { entity: String, member: String, error: NestedError },
}

struct Live<'a> {
    def: &'a EntityDef,
    /// Indexed like `def.client_properties`; None until first set.
    props: Vec<Option<Value<'a>>>,
}

pub struct EntityDecoder<'a> {
    defs: &'a EntityDefs,
    live: HashMap<u32, Live<'a>>,
}

impl<'a> EntityDecoder<'a> {
    pub fn new(defs: &'a EntityDefs) -> Self {
        Self { defs, live: HashMap::new() }
    }

    pub fn entity_type(&self, entity_id: u32) -> Option<&'a EntityDef> {
        self.live.get(&entity_id).map(|l| l.def)
    }

    /// The current value of a client property, if it has been set.
    pub fn property(&self, entity_id: u32, name: &str) -> Option<&Value<'a>> {
        let live = self.live.get(&entity_id)?;
        let idx = live.def.client_properties.iter().position(|p| p.name == name)?;
        live.props.get(idx)?.as_ref()
    }

    /// Ids of live entities of the given type name.
    pub fn entities_of<'s>(&'s self, name: &'s str) -> impl Iterator<Item = u32> + 's {
        self.live.iter().filter(move |(_, l)| l.def.name == name).map(|(id, _)| *id)
    }

    fn register(&mut self, entity_id: u32, entity_type: u16) -> Result<&'a EntityDef, EntityError> {
        let def = self.defs.by_type_id(entity_type).ok_or(EntityError::UnknownType(entity_type))?;
        // A second create for a live id (re-entering the area of interest)
        // keeps known values; the create's own properties overwrite them.
        let live = self.live.entry(entity_id).or_insert_with(|| Live { def, props: Vec::new() });
        if !std::ptr::eq(live.def, def) {
            *live = Live { def, props: Vec::new() };
        }
        live.props.resize(def.client_properties.len(), None);
        Ok(def)
    }

    fn store(&mut self, entity_id: u32, properties: &[(&'a str, Value<'a>)]) {
        let Some(live) = self.live.get_mut(&entity_id) else { return };
        for (name, value) in properties {
            if let Some(i) = live.def.client_properties.iter().position(|p| p.name == *name) {
                live.props[i] = Some(value.clone());
            }
        }
    }

    fn lookup(&self, entity_id: u32) -> Result<&'a EntityDef, EntityError> {
        self.live.get(&entity_id).map(|l| l.def).ok_or(EntityError::UnknownEntity(entity_id))
    }

    /// Decodes a packet's entity payload and updates state. `Ok(None)` for
    /// packets that carry none. Creation packets register the entity even
    /// when their properties then fail to decode, so later packets for it
    /// still resolve.
    pub fn feed(&mut self, packet: &Packet<'a>) -> Result<Option<Event<'a>>, EntityError> {
        let Ok(decoded) = packet.decode() else { return Ok(None) };
        match decoded {
            Decoded::BasePlayerCreate { entity_id, entity_type, state, stub } => {
                let def = self.register(entity_id, entity_type)?;
                let (properties, trailing) =
                    if stub { (Vec::new(), state) } else { sequential(def, &def.base_player_properties, state)? };
                self.store(entity_id, &properties);
                Ok(Some(Event::Create {
                    entity_id,
                    entity: &def.name,
                    position: None,
                    rotation: None,
                    properties,
                    trailing,
                }))
            }
            Decoded::CellPlayerCreate { entity_id, position, rotation, state, .. } => {
                let def = self.lookup(entity_id)?;
                let (properties, trailing) = sequential(def, &def.cell_player_properties, state)?;
                self.store(entity_id, &properties);
                Ok(Some(Event::Create {
                    entity_id,
                    entity: &def.name,
                    position: Some(position),
                    rotation: Some(rotation),
                    properties,
                    trailing,
                }))
            }
            Decoded::EntityCreate { entity_id, entity_type, position, rotation, state, .. } => {
                let def = self.register(entity_id, entity_type)?;
                let mut input = state;
                let count = crate::value::take(&mut input, 1).map_err(|e| value_err(def, "<EntityCreate>", e))?[0];
                let mut properties = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let idx = crate::value::take(&mut input, 1).map_err(|e| value_err(def, "<EntityCreate>", e))?[0];
                    let prop = property(def, u32::from(idx))?;
                    let v = decode(&prop.ty, &mut input).map_err(|e| value_err(def, &prop.name, e))?;
                    properties.push((prop.name.as_str(), v));
                }
                if !input.is_empty() {
                    return Err(leftover(def, "<EntityCreate>", input.len()));
                }
                self.store(entity_id, &properties);
                Ok(Some(Event::Create {
                    entity_id,
                    entity: &def.name,
                    position: Some(position),
                    rotation: Some(rotation),
                    properties,
                    trailing: &[],
                }))
            }
            Decoded::EntityProperty { entity_id, property: idx, value } => {
                let def = self.lookup(entity_id)?;
                let prop = property(def, idx)?;
                let mut input = value;
                let v = decode(&prop.ty, &mut input).map_err(|e| value_err(def, &prop.name, e))?;
                if !input.is_empty() {
                    return Err(leftover(def, &prop.name, input.len()));
                }
                if let Some(live) = self.live.get_mut(&entity_id) {
                    live.props[idx as usize] = Some(v.clone());
                }
                Ok(Some(Event::Property { entity_id, entity: &def.name, property: &prop.name, value: v }))
            }
            Decoded::NestedPropertyUpdate { entity_id, is_slice, data } => {
                let live = self.live.get_mut(&entity_id).ok_or(EntityError::UnknownEntity(entity_id))?;
                let def = live.def;
                let nested = |member: &str, error| EntityError::Nested {
                    entity: def.name.clone(),
                    member: member.to_owned(),
                    error,
                };
                let mut bits = Bits::new(data);
                // The top level is always "descend into property N".
                if bits.read(1).map_err(|e| nested("<root>", e))? != 1 {
                    return Err(nested("<root>", NestedError::NotContainer("entity root without descend bit")));
                }
                let n = def.client_properties.len();
                let idx = bits.read(index_bits(n)).map_err(|e| nested("<root>", e))?;
                let prop = def.client_properties.get(idx).ok_or_else(|| EntityError::PropertyIndex {
                    entity: def.name.clone(),
                    index: idx as u32,
                    count: n,
                })?;
                // Some properties are first populated by a nested update.
                let slot = live.props[idx].get_or_insert_with(|| default_value(&prop.ty));
                let mut path = Vec::new();
                let change = crate::nested::apply(&prop.ty, slot, is_slice, &mut bits, &mut path)
                    .map_err(|e| nested(&prop.name, e))?;
                Ok(Some(Event::Nested { entity_id, entity: &def.name, property: &prop.name, path, change }))
            }
            Decoded::EntityMethod { entity_id, method: idx, args } => {
                let def = self.lookup(entity_id)?;
                let m = def.client_methods.get(idx as usize).ok_or_else(|| EntityError::MethodIndex {
                    entity: def.name.clone(),
                    index: idx,
                    count: def.client_methods.len(),
                })?;
                let mut input = args;
                let mut out = Vec::with_capacity(m.args.len());
                for (name, ty) in &m.args {
                    let v = decode(ty, &mut input).map_err(|e| value_err(def, &format!("{}({name})", m.name), e))?;
                    out.push((name.as_str(), v));
                }
                if !input.is_empty() {
                    return Err(leftover(def, &m.name, input.len()));
                }
                Ok(Some(Event::Method { entity_id, entity: &def.name, method: &m.name, args: out }))
            }
            Decoded::EntityLeave { entity_id } => {
                // Leaving the area of interest is not destruction: the entity
                // can come back with an EntityCreate or EntityEnter and keep
                // receiving updates, so its state is kept.
                let def = self.lookup(entity_id)?;
                Ok(Some(Event::Leave { entity_id, entity: &def.name }))
            }
            _ => Ok(None),
        }
    }
}

fn property(def: &EntityDef, idx: u32) -> Result<&Property, EntityError> {
    def.client_properties.get(idx as usize).ok_or_else(|| EntityError::PropertyIndex {
        entity: def.name.clone(),
        index: idx,
        count: def.client_properties.len(),
    })
}

type Props<'a> = Vec<(&'a str, Value<'a>)>;

fn sequential<'a>(
    def: &EntityDef,
    props: &'a [Property],
    state: &'a [u8],
) -> Result<(Props<'a>, &'a [u8]), EntityError> {
    let mut input = state;
    let mut out = Vec::with_capacity(props.len());
    for p in props {
        out.push((p.name.as_str(), decode(&p.ty, &mut input).map_err(|e| value_err(def, &p.name, e))?));
    }
    Ok((out, input))
}

fn value_err(def: &EntityDef, member: &str, error: ValueError) -> EntityError {
    EntityError::Value { entity: def.name.clone(), member: member.to_owned(), error }
}

fn leftover(def: &EntityDef, member: &str, extra: usize) -> EntityError {
    EntityError::Leftover { entity: def.name.clone(), member: member.to_owned(), extra }
}
