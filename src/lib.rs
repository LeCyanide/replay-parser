//! World of Warships replay parsing, independent of landaire's wows-toolkit.
//!
//! Layers, bottom up:
//!
//! 1. [`container`] — the `.wowsreplay` file: arena-info JSON, then a
//!    Blowfish/XOR-chained, zlib-compressed packet stream.
//! 2. [`packet`] — framing of that stream into typed packets, and decoding of
//!    every payload whose layout does not depend on the game's entity
//!    definitions. Entity method/property payloads are returned raw.
//!
//! Decoding those raw payloads needs the entity definitions shipped with the
//! game client and is the next layer up; it is not here yet.

pub mod container;
pub mod error;
pub mod meta;
pub mod packet;
pub mod pickle;

pub use container::Replay;
pub use error::Error;
pub use meta::ReplayMeta;
pub use meta::Version;
