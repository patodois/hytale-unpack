//! Reader for Hytale's game data: `Assets.zip`, `.blockymodel`, `.blockyanim` and prefabs.
//!
//! No game data is included in this crate, and it is not affiliated with Hypixel Studios. It
//! reads files from a copy of the game you have installed.
//!
//! - [`assets`]: the zip archive, read entry by entry;
//! - [`blockymodel`], [`blockyanim`], [`prefab`]: the formats, with their documentation;
//! - [`defs`]: the JSON definitions that link models to textures, clips and blocks;
//! - [`image`]: PNG textures;
//! - with the `bridge` feature (on by default): `rig` and `gltf` convert to
//!   [rewrite-bridge](https://github.com/patodois/rewrite-bridge) types and glTF, and
//!   `bridge::Hytale` is a `Provider`.

pub mod assets;
pub mod blockyanim;
pub mod blockymodel;
pub mod check;
pub mod defs;
pub mod image;
pub mod prefab;

#[cfg(feature = "bridge")]
pub mod bridge;
#[cfg(feature = "bridge")]
pub mod gltf;
#[cfg(feature = "bridge")]
pub mod rig;

pub use assets::Assets;
