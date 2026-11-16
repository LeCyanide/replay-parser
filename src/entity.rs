//! Entity-level decoding: turns the envelopes from [`crate::packet`] into
//! named method calls and property values, using [`EntityDefs`].
//!
//! The decoder keeps the one piece of state that decoding needs: which entity
//! type each live entity id has, learned from its creation packet.

use std::collections::HashMap;

use serde::Serialize;

use crate::defs::EntityDef;
use crate::defs::EntityDefs;
use crate::defs::Property;
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
}

pub struct EntityDecoder<'d> {
    defs: &'d EntityDefs,
    types: HashMap<u32, u16>,
}

impl<'d> EntityDecoder<'d> {
    pub fn new(defs: &'d EntityDefs) -> Self {
        Self { defs, types: HashMap::new() }
    }

    pub fn entity_type(&self, entity_id: u32) -> Option<&'d EntityDef> {
        self.types.get(&entity_id).and_then(|t| self.defs.by_type_id(*t))
    }

    /// Decodes a packet's entity payload. `Ok(None)` for packets that carry
    /// none. Creation packets register the entity even when their
    /// properties then fail to decode, so later packets for it still resolve.
    pub fn feed<'a>(&mut self, packet: &Packet<'a>) -> Result<Option<Event<'a>>, EntityError>
    where
        'd: 'a,
    {
        let Ok(decoded) = packet.decode() else { return Ok(None) };
        match decoded {
            Decoded::BasePlayerCreate { entity_id, entity_type, state, stub } => {
                self.types.insert(entity_id, entity_type);
                let def = self.def(entity_type)?;
                if stub {
                    return Ok(Some(Event::Create {
                        entity_id,
                        entity: &def.name,
                        position: None,
                        rotation: None,
                        properties: Vec::new(),
                        trailing: state,
                    }));
                }
                let (properties, trailing) = sequential(def, &def.base_player_properties, state)?;
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
                let t = *self.types.get(&entity_id).ok_or(EntityError::UnknownEntity(entity_id))?;
                let def = self.def(t)?;
                let (properties, trailing) = sequential(def, &def.cell_player_properties, state)?;
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
                self.types.insert(entity_id, entity_type);
                let def = self.def(entity_type)?;
                let mut input = state;
                let count = crate::value::take(&mut input, 1).map_err(|e| value_err(def, "<EntityCreate>", e))?[0];
                let mut properties = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let idx = crate::value::take(&mut input, 1).map_err(|e| value_err(def, "<EntityCreate>", e))?[0];
                    let prop = property(def, u32::from(idx))?;
                    properties.push((
                        prop.name.as_str(),
                        decode(&prop.ty, &mut input).map_err(|e| value_err(def, &prop.name, e))?,
                    ));
                }
                if !input.is_empty() {
                    return Err(leftover(def, "<EntityCreate>", input.len()));
                }
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
                Ok(Some(Event::Property { entity_id, entity: &def.name, property: &prop.name, value: v }))
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
                self.types.remove(&entity_id);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn def(&self, t: u16) -> Result<&'d EntityDef, EntityError> {
        self.defs.by_type_id(t).ok_or(EntityError::UnknownType(t))
    }

    fn lookup(&self, entity_id: u32) -> Result<&'d EntityDef, EntityError> {
        let t = *self.types.get(&entity_id).ok_or(EntityError::UnknownEntity(entity_id))?;
        self.def(t)
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
