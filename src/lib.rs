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
//! 3. [`defs`] + [`value`] — the client's entity definitions (`scripts/`
//!    XML) and the typed values they describe.
//! 4. [`entity`] — named method calls and property values from step 2's
//!    envelopes.
//! 5. [`world`] — battle state built from those events: ships, deaths,
//!    scores, capture points, smoke.
//!
//! [`res`] reads game files from an install (resource packages, text
//! catalogues), including the entity defs for step 3.
//!
//! [`pickle`] reads the Python pickles found in `PYTHON` values and in
//! GameParams.

pub mod container;
pub mod defs;
pub mod draw;
pub mod entity;
pub mod error;
pub mod gameparams;
pub mod meta;
pub mod nested;
pub mod packet;
pub mod pickle;
pub mod res;
pub mod value;
pub mod world;

pub use container::Replay;
pub use error::Error;
pub use meta::ReplayMeta;
pub use meta::Version;
