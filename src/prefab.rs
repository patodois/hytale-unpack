//! `*.prefab.json`: Hytale's building blueprints (`Server/Prefabs/**`, 7,829 files, 1.8 GB of
//! JSON; the biggest is 23 MB).
//!
//! ```text
//! { "version": 8, "blockIdVersion": 11,
//!   "anchorX": 4, "anchorY": 10, "anchorZ": 4,       // the cell that is pasted at the target position
//!   "blocks":   [ { "x":0, "y":10, "z":2, "name":"Rock_Slate",
//!                   "rotation": 8,                   // optional, see below
//!                   "filler": 1024,                  // optional
//!                   "support": 15,                   // optional, ignored
//!                   "components": {...} } ],         // optional, block entity data, ignored
//!   "entities": [ { "Components": { "Transform": {"Position": {X,Y,Z}, "Rotation": {...}},
//!                                    "Model": {"Model": {"Id": ...}}, ... } } ],
//!   "fluids":   [ { "x":1, "y":1, "z":-4, "name":"Water_Source", "level":1 } ] }
//! ```
//!
//! All files seen have `version` 8 and `blockIdVersion` 11. Block cells are integer
//! coordinates, +Y up, in the prefab's own frame; `name` is an item id (a file stem under
//! `Server/Item/Items`, see [`crate::blocks`]).
//!
//! # `rotation`
//!
//! A small integer, absent for no rotation. In the 2.6 million blocks sampled it takes the values
//! 1 to 11 and no others. The values fall into three groups of four, `0..=3`, `4..=7` and
//! `8..=11`, so it reads as `yaw + 4 * pitch` with yaw and pitch in quarter turns (a roll would
//! add `16 * roll`, which never occurs in the sample). That reading is an inference from the
//! numbers, supported by what the data does with it: stalactites are always `8` (pitch 2, upside
//! down), tree trunks lie along an axis with `4` and `5`, bookcases use `1..=3`. See
//! [`decode_rotation`] for the axes and signs this crate uses and how they were checked.
//!
//! # `filler`
//!
//! Present on cells that belong to a bigger block (doors, beds, wide furniture). Values such as
//! `1`, `31`, `32`, `992`, `1024`, `31744` are packed small signed offsets, 5 bits per axis (x in
//! the low bits, then y, then z), read as the offset towards the cell that carries the model.
//! This crate only needs to know that a cell with a non-zero filler does not draw the model
//! again.
//!
//! The reader streams: [`stream`] visits blocks one by one without holding the document, which
//! is what keeps `check` at a few megabytes.

use std::fmt;
use std::io::{BufReader, Read};

use anyhow::Result;
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

/// One cell of a prefab.
#[derive(Clone, Debug, Deserialize)]
pub struct PrefabBlock {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub name: String,
    #[serde(default)]
    pub rotation: Option<u32>,
    #[serde(default)]
    pub filler: Option<u32>,
    #[serde(default)]
    pub support: Option<u32>,
    /// Only whether block data exists; its content is ignored
    #[serde(rename = "components", default, deserialize_with = "present")]
    pub has_components: bool,
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(d)?;
    Ok(true)
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrefabFluid {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub name: String,
    #[serde(default)]
    pub level: Option<u32>,
}

/// Header fields of a prefab
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrefabHeader {
    pub version: Option<i64>,
    pub block_id_version: Option<i64>,
    pub anchor: [i32; 3],
}

/// A whole prefab held in memory. Only for modest prefabs: use [`stream`] for large ones.
#[derive(Clone, Debug, Default)]
pub struct Prefab {
    pub header: PrefabHeader,
    pub blocks: Vec<PrefabBlock>,
    pub fluids: Vec<PrefabFluid>,
    pub entities: Vec<serde_json::Value>,
}

/// What [`stream`] counted that it was not asked to hand over.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub blocks: u64,
    pub fluids: u64,
    pub entities: u64,
}

/// Callbacks for [`stream`]. All have empty defaults.
pub trait Visit {
    fn block(&mut self, _b: &PrefabBlock) {}
    fn fluid(&mut self, _f: &PrefabFluid) {}
    fn entity(&mut self, _e: &serde_json::Value) {}
    /// Whether [`Visit::entity`] wants the entity's content (otherwise it is skipped, not parsed)
    fn wants_entities(&self) -> bool {
        false
    }
}

/// Parses a prefab from a reader, calling `visit` for each block, fluid and (if wanted) entity as
/// it goes by.
pub fn stream<R: Read>(reader: R, visit: &mut dyn Visit) -> Result<(PrefabHeader, Counts)> {
    let mut de = serde_json::Deserializer::from_reader(BufReader::with_capacity(1 << 16, reader));
    let mut out = (PrefabHeader::default(), Counts::default());
    DocSeed { visit, out: &mut out }.deserialize(&mut de)?;
    de.end()?;
    Ok(out)
}

/// Parses a prefab from bytes into memory.
pub fn parse(bytes: &[u8]) -> Result<Prefab> {
    struct Collect(Prefab);
    impl Visit for Collect {
        fn block(&mut self, b: &PrefabBlock) {
            self.0.blocks.push(b.clone());
        }
        fn fluid(&mut self, f: &PrefabFluid) {
            self.0.fluids.push(f.clone());
        }
        fn entity(&mut self, e: &serde_json::Value) {
            self.0.entities.push(e.clone());
        }
        fn wants_entities(&self) -> bool {
            true
        }
    }
    let mut c = Collect(Prefab::default());
    let (header, _) = stream(bytes, &mut c)?;
    c.0.header = header;
    Ok(c.0)
}

struct DocSeed<'a> {
    visit: &'a mut dyn Visit,
    out: &'a mut (PrefabHeader, Counts),
}

impl<'de, 'a> DeserializeSeed<'de> for DocSeed<'a> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de, 'a> Visitor<'de> for DocSeed<'a> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a prefab object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let DocSeed { visit, out } = self;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "version" => out.0.version = map.next_value()?,
                "blockIdVersion" => out.0.block_id_version = map.next_value()?,
                "anchorX" => out.0.anchor[0] = map.next_value()?,
                "anchorY" => out.0.anchor[1] = map.next_value()?,
                "anchorZ" => out.0.anchor[2] = map.next_value()?,
                "blocks" => map.next_value_seed(ListSeed::<PrefabBlock> { f: &mut |b| visit.block(&b), n: &mut out.1.blocks })?,
                "fluids" => map.next_value_seed(ListSeed::<PrefabFluid> { f: &mut |b| visit.fluid(&b), n: &mut out.1.fluids })?,
                "entities" => {
                    if visit.wants_entities() {
                        map.next_value_seed(ListSeed::<serde_json::Value> { f: &mut |e| visit.entity(&e), n: &mut out.1.entities })?
                    } else {
                        map.next_value_seed(ListSeed::<IgnoredAny> { f: &mut |_| {}, n: &mut out.1.entities })?
                    }
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

struct ListSeed<'a, T> {
    f: &'a mut dyn FnMut(T),
    n: &'a mut u64,
}

impl<'de, 'a, T: Deserialize<'de>> DeserializeSeed<'de> for ListSeed<'a, T> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, 'a, T: Deserialize<'de>> Visitor<'de> for ListSeed<'a, T> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(item) = seq.next_element::<T>()? {
            *self.n += 1;
            (self.f)(item);
        }
        Ok(())
    }
}

/// A block's rotation as quarter turns of yaw (about +Y) and pitch (about +X), and of roll
/// (about +Z).
///
/// `rotation = yaw + 4 * pitch + 16 * roll`, each in `0..4`. The model is rotated by roll first,
/// then pitch, then yaw: `R = Ry(yaw) * Rx(pitch) * Rz(roll)`, which is what makes `5`
/// (pitch 1, yaw 1) a trunk along X where `4` is along Z. Positive angles turn counter-clockwise
/// looking down the axis towards the origin, as in a right-handed system; see the README for how
/// the yaw sign was checked against placed furniture and for the pitch sign's status.
pub fn decode_rotation(rotation: u32) -> (u32, u32, u32) {
    (rotation % 4, (rotation / 4) % 4, (rotation / 16) % 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"version":8,"blockIdVersion":11,"anchorX":4,"anchorY":10,"anchorZ":-4,
        "blocks":[{"x":0,"y":10,"z":2,"name":"Rock_Slate"},
                  {"x":1,"y":10,"z":2,"name":"Furniture_Village_Bookcase","rotation":2,"support":15,"filler":1024,
                   "components":{"Components":{"FarmingBlock":{}}}}],
        "fluids":[{"x":1,"y":1,"z":-4,"name":"Water_Source","level":1}],
        "entities":[{"Components":{"Nameplate":{"Text":"Klops"}}}],
        "unknown":[1,2,3]}"#;

    #[test]
    fn parses_a_prefab() {
        let p = parse(DOC.as_bytes()).unwrap();
        assert_eq!(p.header.anchor, [4, 10, -4]);
        assert_eq!(p.header.version, Some(8));
        assert_eq!(p.blocks.len(), 2);
        assert_eq!(p.blocks[1].rotation, Some(2));
        assert_eq!(p.blocks[1].filler, Some(1024));
        assert!(p.blocks[1].has_components && !p.blocks[0].has_components);
        assert_eq!(p.fluids.len(), 1);
        assert_eq!(p.entities.len(), 1);
    }

    #[test]
    fn streaming_counts_without_collecting() {
        struct Names(Vec<String>);
        impl Visit for Names {
            fn block(&mut self, b: &PrefabBlock) {
                self.0.push(b.name.clone());
            }
        }
        let mut v = Names(vec![]);
        let (_, counts) = stream(DOC.as_bytes(), &mut v).unwrap();
        assert_eq!(counts, Counts { blocks: 2, fluids: 1, entities: 1 });
        assert_eq!(v.0, ["Rock_Slate", "Furniture_Village_Bookcase"]);
    }

    #[test]
    fn rejects_truncated_documents() {
        assert!(parse(&DOC.as_bytes()[..100]).is_err());
    }

    #[test]
    fn rotation_splits_into_quarter_turns() {
        assert_eq!(decode_rotation(0), (0, 0, 0));
        assert_eq!(decode_rotation(5), (1, 1, 0));
        assert_eq!(decode_rotation(11), (3, 2, 0));
        assert_eq!(decode_rotation(16 + 2), (2, 0, 1));
    }
}
