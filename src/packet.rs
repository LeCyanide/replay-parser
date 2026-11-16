//! The decrypted packet stream.
//!
//! Every packet has a 12-byte header:
//!
//! ```text
//! payload_size u32
//! type         u32   wire id; its meaning depends on the client version
//! clock        f32   seconds of battle time
//! payload      u8[payload_size]
//! ```
//!
//! [`Packets`] walks the stream and classifies each packet. [`Packet::decode`]
//! then decodes payloads whose layout is fixed by the engine rather than by the
//! game's entity definitions. Entity creation, method calls and property
//! updates are decoded down to their envelope (entity id, method/property
//! index, raw argument bytes); the arguments need the entity definitions.

use serde::Serialize;

use crate::meta::Version;

/// What a packet is, independent of which wire id carried it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Kind {
    BasePlayerCreate,
    BasePlayerCreateStub,
    CellPlayerCreate,
    EntityControl,
    EntityEnter,
    EntityLeave,
    EntityCreate,
    EntityProperty,
    EntityMethod,
    Position,
    ServerTick,
    ServerTimestamp,
    InitFlag,
    InitMarker,
    Version,
    GunMarker,
    PlayerNetStats,
    OwnShip,
    BattleResults,
    NestedPropertyUpdate,
    Camera,
    CameraMode,
    Map,
    NonVolatilePosition,
    PlayerOrientation,
    CameraFreeLook,
    SetWeaponLock,
    SubController,
    CruiseState,
    ShotTracking,
    /// Wire id 0xffffffff, 16 bytes, clock 0. Observed as the final packet of
    /// every finished replay in the corpus; its contents are not decoded.
    StreamEnd,
    Unknown,
}

/// From 12.6.0 a `BattleResults` packet occupies 0x22 and every id from there
/// up moved one slot higher. Gate on the semantic version: regional clients
/// share a version but not a build number.
pub const MODERN_LAYOUT_SINCE: (u32, u32, u32) = (12, 6, 0);

impl Kind {
    pub fn from_wire(raw: u32, version: Version) -> Kind {
        if raw == u32::MAX {
            return Kind::StreamEnd;
        }
        if version.semver() >= MODERN_LAYOUT_SINCE { Self::modern(raw) } else { Self::legacy(raw) }
    }

    fn common(raw: u32) -> Option<Kind> {
        Some(match raw {
            0x00 => Kind::BasePlayerCreate,
            0x01 => Kind::CellPlayerCreate,
            0x02 => Kind::EntityControl,
            0x03 => Kind::EntityEnter,
            0x04 => Kind::EntityLeave,
            0x05 => Kind::EntityCreate,
            0x07 => Kind::EntityProperty,
            0x08 => Kind::EntityMethod,
            0x0a => Kind::Position,
            0x0e => Kind::ServerTick,
            0x0f => Kind::ServerTimestamp,
            0x10 => Kind::InitFlag,
            0x13 => Kind::InitMarker,
            0x16 => Kind::Version,
            0x18 => Kind::GunMarker,
            0x1d => Kind::PlayerNetStats,
            0x20 => Kind::OwnShip,
            0x26 => Kind::BasePlayerCreateStub,
            _ => return None,
        })
    }

    fn modern(raw: u32) -> Kind {
        Self::common(raw).unwrap_or(match raw {
            0x22 => Kind::BattleResults,
            0x23 => Kind::NestedPropertyUpdate,
            0x25 => Kind::Camera,
            0x27 => Kind::CameraMode,
            0x28 => Kind::Map,
            0x2a => Kind::NonVolatilePosition,
            0x2c => Kind::PlayerOrientation,
            0x2f => Kind::CameraFreeLook,
            0x30 => Kind::SetWeaponLock,
            0x31 => Kind::SubController,
            0x32 => Kind::CruiseState,
            0x33 => Kind::ShotTracking,
            _ => Kind::Unknown,
        })
    }

    /// Pre-12.6 layout. No replay of that era is in the test corpus, so this
    /// table is unverified here.
    fn legacy(raw: u32) -> Kind {
        Self::common(raw).unwrap_or(match raw {
            0x22 => Kind::NestedPropertyUpdate,
            0x24 => Kind::Camera,
            0x27 => Kind::Map,
            0x29 => Kind::NonVolatilePosition,
            0x2b => Kind::PlayerOrientation,
            0x2e => Kind::CameraFreeLook,
            0x2f => Kind::SetWeaponLock,
            0x30 => Kind::SubController,
            0x31 => Kind::CruiseState,
            0x32 => Kind::ShotTracking,
            _ => Kind::Unknown,
        })
    }
}

/// One framed packet, payload undecoded.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    pub kind: Kind,
    pub wire_type: u32,
    pub clock: f32,
    pub payload: &'a [u8],
}

/// Iterator over the packets of a decrypted stream. Stops at the first header
/// or payload that runs past the end; [`Packets::trailing`] then reports how
/// many bytes were left unread (non-zero for a truncated replay).
pub struct Packets<'a> {
    rest: &'a [u8],
    version: Version,
}

impl<'a> Packets<'a> {
    pub fn new(stream: &'a [u8], version: Version) -> Self {
        Self { rest: stream, version }
    }

    pub fn trailing(&self) -> usize {
        self.rest.len()
    }
}

impl<'a> Iterator for Packets<'a> {
    type Item = Packet<'a>;

    fn next(&mut self) -> Option<Packet<'a>> {
        let mut r = Reader(self.rest);
        let size = r.u32().ok()? as usize;
        let wire_type = r.u32().ok()?;
        let clock = r.f32().ok()?;
        let payload = r.bytes(size).ok()?;
        self.rest = r.0;
        Some(Packet { kind: Kind::from_wire(wire_type, self.version), wire_type, clock, payload })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Euler angles in radians. Yaw is the heading the minimap cares about.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rot3 {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

/// Payloads decoded as far as the engine fixes their layout.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind")]
pub enum Decoded<'a> {
    /// The base half of the recording player's Avatar. `state` is the base
    /// properties, which need entity definitions to read.
    BasePlayerCreate {
        entity_id: u32,
        entity_type: u16,
        #[serde(skip)]
        state: &'a [u8],
    },
    CellPlayerCreate {
        entity_id: u32,
        space_id: u32,
        vehicle_id: u32,
        position: Vec3,
        rotation: Rot3,
        #[serde(skip)]
        state: &'a [u8],
    },
    EntityControl {
        entity_id: u32,
        controlled: bool,
    },
    EntityEnter {
        entity_id: u32,
        space_id: u32,
        vehicle_id: u32,
    },
    EntityLeave {
        entity_id: u32,
    },
    /// An entity coming into the recording client's area of interest.
    /// `state` is its client properties: a u8 count, then (u8 index, value)
    /// pairs whose encoding the entity definitions decide.
    EntityCreate {
        entity_id: u32,
        entity_type: u16,
        space_id: u32,
        vehicle_id: u32,
        position: Vec3,
        rotation: Rot3,
        #[serde(skip)]
        state: &'a [u8],
    },
    EntityProperty {
        entity_id: u32,
        property: u32,
        #[serde(skip)]
        value: &'a [u8],
    },
    EntityMethod {
        entity_id: u32,
        method: u32,
        #[serde(skip)]
        args: &'a [u8],
    },
    NestedPropertyUpdate {
        entity_id: u32,
        is_slice: bool,
        #[serde(skip)]
        data: &'a [u8],
    },
    Position {
        entity_id: u32,
        space_id: u32,
        position: Vec3,
        /// Dead-reckoning velocity; zero on offline-position updates.
        direction: Vec3,
        rotation: Rot3,
        on_ground: bool,
    },
    /// The recording player's own ship, sent at a higher rate than
    /// `Position`. `parent_id` is non-zero when the position is relative to
    /// another entity.
    PlayerOrientation {
        entity_id: u32,
        parent_id: u32,
        position: Vec3,
        rotation: Rot3,
    },
    NonVolatilePosition {
        entity_id: u32,
        space_id: u32,
        position: Vec3,
        rotation: Rot3,
    },
    Version {
        version: &'a str,
    },
    Map {
        space_id: u32,
        arena_id: i64,
        name: &'a str,
    },
    ServerTick {
        rate: f64,
    },
    ServerTimestamp {
        timestamp: f64,
    },
    OwnShip {
        entity_id: u32,
    },
    BattleResults {
        json: &'a str,
    },
    Camera {
        rotation: [f32; 4],
        camera_position: Vec3,
        fov: f32,
        position: Vec3,
        direction: Vec3,
    },
    CameraMode {
        mode: u32,
    },
    CameraFreeLook {
        freelook: bool,
    },
    CruiseState {
        key: u32,
        value: i32,
    },
    /// Recognised, but nothing here decodes it yet.
    Opaque,
}

/// Why a payload did not match the layout its kind implies.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("{kind:?} payload ended early: wanted {needed} more bytes, {available} left")]
    Short { kind: Kind, needed: usize, available: usize },
    #[error("{kind:?} payload has {extra} bytes past its layout")]
    Trailing { kind: Kind, extra: usize },
    #[error("{kind:?} carries a string that is not UTF-8")]
    NotUtf8 { kind: Kind },
}

struct Reader<'a>(&'a [u8]);

/// Shortfall without a kind; [`Packet::decode`] attaches it.
struct Short {
    needed: usize,
    available: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Short> {
        if self.0.len() < n {
            return Err(Short { needed: n, available: self.0.len() });
        }
        let (h, t) = self.0.split_at(n);
        self.0 = t;
        Ok(h)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Short> {
        Ok(self.bytes(N)?.try_into().expect("length checked"))
    }
    fn u8(&mut self) -> Result<u8, Short> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, Short> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, Short> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn i32(&mut self) -> Result<i32, Short> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    fn i64(&mut self) -> Result<i64, Short> {
        Ok(i64::from_le_bytes(self.array()?))
    }
    fn f32(&mut self) -> Result<f32, Short> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    fn f64(&mut self) -> Result<f64, Short> {
        Ok(f64::from_le_bytes(self.array()?))
    }
    fn vec3(&mut self) -> Result<Vec3, Short> {
        Ok(Vec3 { x: self.f32()?, y: self.f32()?, z: self.f32()? })
    }
    fn rot3(&mut self) -> Result<Rot3, Short> {
        Ok(Rot3 { yaw: self.f32()?, pitch: self.f32()?, roll: self.f32()? })
    }
    /// u32 length, then that many bytes.
    fn blob(&mut self) -> Result<&'a [u8], Short> {
        let n = self.u32()? as usize;
        self.bytes(n)
    }
}

impl<'a> Packet<'a> {
    /// Decodes the payload as far as its layout is fixed. Fails when the
    /// payload is shorter or longer than that layout, which is how a wrong
    /// wire-id table shows itself.
    pub fn decode(&self) -> Result<Decoded<'a>, DecodeError> {
        let kind = self.kind;
        let mut r = Reader(self.payload);
        let short = |s: Short| DecodeError::Short { kind, needed: s.needed, available: s.available };
        let utf8 = |b: &'a [u8]| std::str::from_utf8(b).map_err(|_| DecodeError::NotUtf8 { kind });

        let decoded = (|| -> Result<Result<Decoded<'a>, DecodeError>, Short> {
            Ok(Ok(match kind {
                Kind::BasePlayerCreate => {
                    let entity_id = r.u32()?;
                    let entity_type = r.u16()?;
                    let state = r.bytes(r.0.len())?;
                    Decoded::BasePlayerCreate { entity_id, entity_type, state }
                }
                Kind::CellPlayerCreate => Decoded::CellPlayerCreate {
                    entity_id: r.u32()?,
                    space_id: r.u32()?,
                    vehicle_id: r.u32()?,
                    position: r.vec3()?,
                    rotation: r.rot3()?,
                    state: r.blob()?,
                },
                Kind::EntityControl => Decoded::EntityControl { entity_id: r.u32()?, controlled: r.u8()? != 0 },
                Kind::EntityEnter => {
                    Decoded::EntityEnter { entity_id: r.u32()?, space_id: r.u32()?, vehicle_id: r.u32()? }
                }
                Kind::EntityLeave => Decoded::EntityLeave { entity_id: r.u32()? },
                Kind::EntityCreate => Decoded::EntityCreate {
                    entity_id: r.u32()?,
                    entity_type: r.u16()?,
                    space_id: r.u32()?,
                    vehicle_id: r.u32()?,
                    position: r.vec3()?,
                    rotation: r.rot3()?,
                    state: r.blob()?,
                },
                Kind::EntityProperty => {
                    Decoded::EntityProperty { entity_id: r.u32()?, property: r.u32()?, value: r.blob()? }
                }
                Kind::EntityMethod => Decoded::EntityMethod { entity_id: r.u32()?, method: r.u32()?, args: r.blob()? },
                Kind::NestedPropertyUpdate => {
                    Decoded::NestedPropertyUpdate { entity_id: r.u32()?, is_slice: r.u8()? != 0, data: r.blob()? }
                }
                Kind::Position => Decoded::Position {
                    entity_id: r.u32()?,
                    space_id: r.u32()?,
                    position: r.vec3()?,
                    direction: r.vec3()?,
                    rotation: r.rot3()?,
                    on_ground: r.u8()? != 0,
                },
                Kind::PlayerOrientation => Decoded::PlayerOrientation {
                    entity_id: r.u32()?,
                    parent_id: r.u32()?,
                    position: r.vec3()?,
                    rotation: r.rot3()?,
                },
                Kind::NonVolatilePosition => Decoded::NonVolatilePosition {
                    entity_id: r.u32()?,
                    space_id: r.u32()?,
                    position: r.vec3()?,
                    rotation: r.rot3()?,
                },
                Kind::Version => match utf8(r.blob()?) {
                    Ok(version) => Decoded::Version { version },
                    Err(e) => return Ok(Err(e)),
                },
                Kind::Map => {
                    let space_id = r.u32()?;
                    let arena_id = r.i64()?;
                    let _unknown = (r.u32()?, r.u32()?);
                    let _blob = r.bytes(128)?;
                    let name = match utf8(r.blob()?) {
                        Ok(n) => n,
                        Err(e) => return Ok(Err(e)),
                    };
                    let _matrix = r.bytes(64)?;
                    let _flag = r.u8()?;
                    Decoded::Map { space_id, arena_id, name }
                }
                Kind::ServerTick => Decoded::ServerTick { rate: r.f64()? },
                Kind::ServerTimestamp => Decoded::ServerTimestamp { timestamp: r.f64()? },
                Kind::OwnShip => Decoded::OwnShip { entity_id: r.u32()? },
                Kind::BattleResults => match utf8(r.blob()?) {
                    Ok(json) => Decoded::BattleResults { json },
                    Err(e) => return Ok(Err(e)),
                },
                Kind::Camera => {
                    // Pre-~0.11 clients omit the f32 between fov and position
                    // (56 bytes instead of 60).
                    let has_extra = r.0.len() >= 60;
                    let rotation = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                    let camera_position = r.vec3()?;
                    let fov = r.f32()?;
                    if has_extra {
                        r.f32()?;
                    }
                    Decoded::Camera { rotation, camera_position, fov, position: r.vec3()?, direction: r.vec3()? }
                }
                Kind::CameraMode => Decoded::CameraMode { mode: r.u32()? },
                Kind::CameraFreeLook => Decoded::CameraFreeLook { freelook: r.u8()? != 0 },
                Kind::CruiseState => Decoded::CruiseState { key: r.u32()?, value: r.i32()? },
                _ => {
                    r.0 = &[];
                    Decoded::Opaque
                }
            }))
        })()
        .map_err(short)??;

        if !r.0.is_empty() {
            return Err(DecodeError::Trailing { kind, extra: r.0.len() });
        }
        Ok(decoded)
    }
}
