//! The JSON definitions that tie models, textures, animations and blocks together.
//!
//! # Inheritance
//!
//! Definition files may carry `"Parent": "<id>"`, where the id is the file name without
//! extension of another definition of the same family (`Server/Models/**` for entity models,
//! `Server/Item/**` for items; 637 stems exist in more than one place under `Server/Item`, in
//! which case `Server/Item/Items` wins). The child inherits everything: objects are merged key by
//! key, recursively, and any other value (strings, numbers, arrays) is replaced by the child's.
//! For example `Server/Models/Intelligent/Trork/Trork.json` has `"Parent": "Player"` and so gets
//! the player's animation sets; 243 of 438 model definitions have a parent.
//!
//! # Entity models (`Server/Models/**.json`)
//!
//! ```text
//! { "Parent": "Player",
//!   "Model":   "Characters/Player.blockymodel",              // relative to Common/
//!   "Texture": "Characters/Player_Textures/Player_Greyscale.png",
//!   "HitBox": {...}, "EyeHeight": 1.6, "MinScale": 1, "MaxScale": 1, ...,
//!   "DefaultAttachments": [ { "Model": ..., "Texture": ..., "GradientSet"?: "Hair", "GradientId"?: "Brown",
//!                             "Weight"?: 0.9 } ],
//!   "RandomAttachmentSets": { "<set>": { "<option>": { "Model": ..., "Texture": ..., "Weight"? } } },
//!   "GradientSet": "Skin", "GradientId": "02",
//!   "AnimationSets": { "Idle": { "Animations": [ { "Animation": "Characters/Animations/Default/Idle.blockyanim",
//!                                                  "Speed": 0.5, "Looping": true, ... } ] }, ... } }
//! ```
//!
//! This is the data link between a model and its clips: the sets name the clips (`Idle`, `Walk`,
//! `Run`, ...) and give their files.
//!
//! # Attachments and gradients
//!
//! `DefaultAttachments` lists the models worn by every instance of the entity (eyes, hair, armour,
//! weapons, hats), each with its own texture. A model's `Parent` chain replaces the whole list, as
//! it does for any array: 230 of the 436 entity models have a non-empty list after inheritance.
//! `RandomAttachmentSets` names sets of alternatives (`"Fleece": {"Wool": {...}}`, an option `{}`
//! meaning none) of which one is picked per instance; the data gives no pick, so they are only
//! read, never composed. An entry with a `Weight` (a probability or a share) is in the same
//! position. How the models are glued on is in [`crate::blockymodel`] ("Attachments").
//!
//! `GradientSet` and `GradientId` name a tint: `Cosmetics/CharacterCreator/GradientSets.json`
//! lists every set with its gradients, each a path to a 256 x 16 PNG ([`GradientSets`]). The
//! textures that go with a gradient are greyscale in the tinted regions; see [`crate::image::tint`].
//!
//! # Items and blocks (`Server/Item/Items/**.json`)
//!
//! An item with a `BlockType` is a placeable block. After inheritance, `BlockType.DrawType` is
//! `Cube` (six textures from `Textures`), `Model` (`CustomModel` + `CustomModelTexture`),
//! `CubeWithModel` (both), or `Empty`. `Textures` entries have `All`, `Sides`, `UpDown`, `Up`,
//! `Down`, `North`, `South`, `East`, `West` and a `Weight` (random variants); specific faces
//! beat `Sides`/`UpDown`, which beat `All`. `CustomModelScale` scales a model.
//!
//! A `BlockType` may itself carry `"Parent": "<item id>"`, inheriting that item's block type.
//!
//! A prefab cell may name a block *state*: `*Rail_State_Definitions_Corner_Left` is the item
//! `Rail` with `BlockType.State.Definitions.Corner_Left` merged over its `BlockType`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Result};
use serde_json::{Map, Value};

use crate::assets::Assets;

/// Deep merge: objects key by key, anything else replaced by `over`.
pub fn merge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(a), Value::Object(b)) => {
            let mut out: Map<String, Value> = a.clone();
            for (k, v) in b {
                let merged = match a.get(k) {
                    Some(old) => merge(old, v),
                    None => v.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        _ => over.clone(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Family {
    Models,
    Items,
}

/// Where the tint gradients are: `Cosmetics/CharacterCreator/GradientSets.json`.
pub const GRADIENT_SETS_PATH: &str = "Cosmetics/CharacterCreator/GradientSets.json";

/// The gradient sets of the character creator: (set, gradient id) -> the gradient's PNG, as a path
/// relative to `Common/`. Gradients also carry a `BaseColor`, the swatch colour of the picker,
/// which is not read.
#[derive(Clone, Debug, Default)]
pub struct GradientSets {
    textures: HashMap<(String, String), String>,
}

impl GradientSets {
    pub fn from_json(v: &Value) -> Self {
        let mut textures = HashMap::new();
        for set in v.as_array().into_iter().flatten() {
            let Some(id) = set.get("Id").and_then(Value::as_str) else {
                continue;
            };
            let Some(grads) = set.get("Gradients").and_then(Value::as_object) else {
                continue;
            };
            for (gid, g) in grads {
                if let Some(t) = g.get("Texture").and_then(Value::as_str) {
                    textures.insert((id.to_string(), gid.clone()), t.to_string());
                }
            }
        }
        GradientSets { textures }
    }

    /// The gradient's PNG, relative to `Common/`
    pub fn texture(&self, g: &GradientRef) -> Option<&str> {
        self.textures
            .get(&(g.set.clone(), g.id.clone()))
            .map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}

pub struct Defs {
    models: HashMap<String, String>,
    items: HashMap<String, String>,
    cache: Mutex<HashMap<(Family, String), Arc<Value>>>,
    gradients: OnceLock<GradientSets>,
}

fn stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".json").unwrap_or(file)
}

impl Defs {
    pub fn new(assets: &Assets) -> Self {
        let mut models = HashMap::new();
        let mut items: HashMap<String, String> = HashMap::new();
        for n in assets.names() {
            if !n.ends_with(".json") {
                continue;
            }
            if n.starts_with("Server/Models/") {
                models
                    .entry(stem(n).to_string())
                    .or_insert_with(|| n.to_string());
            } else if n.starts_with("Server/Item/") {
                let s = stem(n).to_string();
                let in_items = n.starts_with("Server/Item/Items/");
                match items.get(&s) {
                    Some(old) if old.starts_with("Server/Item/Items/") || !in_items => {}
                    _ => {
                        items.insert(s, n.to_string());
                    }
                }
            }
        }
        Defs {
            models,
            items,
            cache: Mutex::new(HashMap::new()),
            gradients: OnceLock::new(),
        }
    }

    /// The character creator's gradient sets, read on first use (empty when the file is missing).
    pub fn gradient_sets(&self, assets: &Assets) -> &GradientSets {
        self.gradients.get_or_init(|| {
            assets
                .read_json::<Value>(GRADIENT_SETS_PATH)
                .map(|v| GradientSets::from_json(&v))
                .unwrap_or_default()
        })
    }

    pub fn model_paths(&self) -> impl Iterator<Item = &str> {
        self.models.values().map(|s| s.as_str())
    }

    pub fn item_path(&self, id: &str) -> Option<&str> {
        self.items.get(id).map(|s| s.as_str())
    }

    pub fn model_path(&self, id: &str) -> Option<&str> {
        self.models.get(id).map(|s| s.as_str())
    }

    fn resolve(
        &self,
        assets: &Assets,
        family: Family,
        path: &str,
        depth: u32,
    ) -> Result<Arc<Value>> {
        let key = (family, path.to_string());
        if let Some(v) = self.cache.lock().unwrap().get(&key) {
            return Ok(v.clone());
        }
        if depth > 32 {
            bail!("{path}: Parent chain too deep");
        }
        let own: Value = assets.read_json(path)?;
        let merged = match own.get("Parent").and_then(Value::as_str) {
            Some(parent) => {
                let table = if family == Family::Models {
                    &self.models
                } else {
                    &self.items
                };
                let ppath = table
                    .get(parent)
                    .ok_or_else(|| anyhow!("{path}: Parent {parent:?} not found"))?;
                let base = self.resolve(assets, family, ppath, depth + 1)?;
                let mut own = own;
                if let Value::Object(m) = &mut own {
                    m.remove("Parent");
                }
                merge(&base, &own)
            }
            None => own,
        };
        let v = Arc::new(merged);
        self.cache.lock().unwrap().insert(key, v.clone());
        Ok(v)
    }

    /// An entity model definition (`Server/Models/**.json`) with its parents merged in.
    pub fn model_def(&self, assets: &Assets, path: &str) -> Result<ModelDef> {
        let v = self.resolve(assets, Family::Models, path, 0)?;
        Ok(ModelDef::from_json(&v))
    }

    /// An item definition (`Server/Item/Items/**.json`) with its parents merged in.
    pub fn item(&self, assets: &Assets, path: &str) -> Result<Arc<Value>> {
        self.resolve(assets, Family::Items, path, 0)
    }

    /// An item's `BlockType` with the item's own parents and the `BlockType`'s `Parent` (another
    /// item whose block type it inherits, e.g. `Plant_Crop_Mushroom_Block_Blue_Branch` from
    /// `Wood_Oak_Branch_Long`) merged in.
    fn block_type(&self, assets: &Assets, id: &str, depth: u32) -> Result<Option<Value>> {
        if depth > 32 {
            bail!("{id}: BlockType Parent chain too deep");
        }
        let path = self
            .items
            .get(id)
            .ok_or_else(|| anyhow!("{id}: no such item"))?;
        let item = self.item(assets, path)?;
        let Some(bt) = item.get("BlockType") else {
            return Ok(None);
        };
        let mut bt = bt.clone();
        if let Some(parent) = bt.get("Parent").and_then(Value::as_str).map(String::from) {
            if let Some(base) = self.block_type(assets, &parent, depth + 1)? {
                if let Value::Object(m) = &mut bt {
                    m.remove("Parent");
                }
                bt = merge(&base, &bt);
            }
        }
        Ok(Some(bt))
    }

    /// The block a prefab cell or item id names, or `None` for things that draw nothing
    /// (`Empty`, items that are not blocks).
    pub fn block(&self, assets: &Assets, name: &str) -> Result<Option<BlockDesc>> {
        if name == "Empty" {
            return Ok(None);
        }
        let (id, state) = match name
            .strip_prefix('*')
            .and_then(|n| n.split_once("_State_Definitions_"))
        {
            Some((id, state)) => (id, Some(state)),
            None => (name, None),
        };
        let Some(mut bt) = self.block_type(assets, id, 0)? else {
            return Ok(None);
        };
        if let Some(state) = state {
            let def = bt
                .pointer(&format!("/State/Definitions/{state}"))
                .cloned()
                .ok_or_else(|| anyhow!("{name}: {id} has no state {state}"))?;
            bt = merge(&bt, &def);
        }
        Ok(Some(BlockDesc::from_json(&bt)))
    }
}

/// One clip reference of an animation set
#[derive(Clone, Debug, PartialEq)]
pub struct AnimRef {
    /// Path relative to `Common/`
    pub path: String,
    pub speed: Option<f32>,
    pub looping: Option<bool>,
}

/// A `GradientSet` / `GradientId` pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GradientRef {
    pub set: String,
    pub id: String,
}

/// One attached model of an entity definition (`DefaultAttachments`, or an option of
/// `RandomAttachmentSets`).
#[derive(Clone, Debug, PartialEq)]
pub struct AttachmentDef {
    /// Relative to `Common/`
    pub model: String,
    /// Relative to `Common/`
    pub texture: Option<String>,
    pub gradient: Option<GradientRef>,
    /// Present on weighted entries only
    pub weight: Option<f32>,
}

impl AttachmentDef {
    /// `None` for `{}` (the "no attachment" option) and for entries without a model.
    pub fn from_json(a: &Value) -> Option<Self> {
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(String::from);
        Some(AttachmentDef {
            model: s("Model")?,
            texture: s("Texture"),
            gradient: GradientRef::from_json(a),
            weight: a.get("Weight").and_then(Value::as_f64).map(|x| x as f32),
        })
    }
}

impl GradientRef {
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(GradientRef {
            set: v.get("GradientSet")?.as_str()?.to_string(),
            id: v.get("GradientId")?.as_str()?.to_string(),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModelDef {
    /// Relative to `Common/`
    pub model: Option<String>,
    pub texture: Option<String>,
    /// Set name -> clips
    pub animation_sets: BTreeMap<String, Vec<AnimRef>>,
    /// `DefaultAttachments`, in file order, without the empty `{}` entries
    pub attachments: Vec<AttachmentDef>,
    /// `RandomAttachmentSets`: set name -> option name -> the model (`None` for `{}`)
    pub random_attachment_sets: BTreeMap<String, BTreeMap<String, Option<AttachmentDef>>>,
    /// The entity's own `GradientSet` / `GradientId`
    pub gradient: Option<GradientRef>,
    pub hit_box_height: Option<f32>,
}

impl ModelDef {
    pub fn from_json(v: &Value) -> Self {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        let mut def = ModelDef {
            model: s("Model"),
            texture: s("Texture"),
            ..ModelDef::default()
        };
        if let Some(sets) = v.get("AnimationSets").and_then(Value::as_object) {
            for (name, set) in sets {
                let anims = set
                    .get("Animations")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|a| {
                                Some(AnimRef {
                                    path: a.get("Animation")?.as_str()?.to_string(),
                                    speed: a.get("Speed").and_then(Value::as_f64).map(|x| x as f32),
                                    looping: a.get("Looping").and_then(Value::as_bool),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if !anims.is_empty() {
                    def.animation_sets.insert(name.clone(), anims);
                }
            }
        }
        if let Some(att) = v.get("DefaultAttachments").and_then(Value::as_array) {
            def.attachments = att.iter().filter_map(AttachmentDef::from_json).collect();
        }
        if let Some(sets) = v.get("RandomAttachmentSets").and_then(Value::as_object) {
            for (set, options) in sets {
                let Some(options) = options.as_object() else {
                    continue;
                };
                def.random_attachment_sets.insert(
                    set.clone(),
                    options
                        .iter()
                        .map(|(k, o)| (k.clone(), AttachmentDef::from_json(o)))
                        .collect(),
                );
            }
        }
        def.gradient = GradientRef::from_json(v);
        def.hit_box_height = v
            .pointer("/HitBox/Max/Y")
            .and_then(Value::as_f64)
            .zip(v.pointer("/HitBox/Min/Y").and_then(Value::as_f64))
            .map(|(a, b)| (a - b) as f32);
        def
    }
}

/// Which side of a cube a texture goes on. Hytale's compass is read as `North` = -Z, `South` = +Z,
/// `East` = +X, `West` = -X (the Blockbench plugin's mapping of model faces).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    East,
    West,
    Up,
    Down,
    South,
    North,
}

impl Side {
    pub const ALL: [Side; 6] = [
        Side::East,
        Side::West,
        Side::Up,
        Side::Down,
        Side::South,
        Side::North,
    ];
}

#[derive(Clone, Debug, PartialEq)]
pub enum DrawType {
    Cube,
    Model,
    CubeWithModel,
    Empty,
    Other(Option<String>),
}

#[derive(Clone, Debug)]
pub struct BlockDesc {
    pub draw: DrawType,
    /// Paths relative to `Common/`, one per [`Side::ALL`] entry, for cube draw types
    pub faces: [Option<String>; 6],
    pub model: Option<String>,
    pub model_texture: Option<String>,
    pub model_scale: f32,
    pub opacity: Option<String>,
}

impl BlockDesc {
    /// Whether the block has anything to draw: a cube with a texture on at least one face, or a
    /// custom model. `Empty` blocks, blocks of an unknown draw type and items whose `BlockType`
    /// only names a list (the editor's brush filters and prefab placers) have not.
    pub fn draws(&self) -> bool {
        let cube = matches!(self.draw, DrawType::Cube | DrawType::CubeWithModel)
            && self.faces.iter().any(Option::is_some);
        let model =
            matches!(self.draw, DrawType::Model | DrawType::CubeWithModel) && self.model.is_some();
        cube || model
    }

    pub fn from_json(bt: &Value) -> Self {
        let draw = match bt.get("DrawType").and_then(Value::as_str) {
            Some("Cube") => DrawType::Cube,
            Some("Model") => DrawType::Model,
            Some("CubeWithModel") => DrawType::CubeWithModel,
            Some("Empty") => DrawType::Empty,
            other => DrawType::Other(other.map(String::from)),
        };
        let mut faces: [Option<String>; 6] = Default::default();
        if let Some(t) = bt
            .get("Textures")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
        {
            let get = |k: &str| t.get(k).and_then(Value::as_str).map(String::from);
            let (all, sides, updown) = (get("All"), get("Sides"), get("UpDown"));
            let pick = |specific: Option<String>, group: &Option<String>| {
                specific.or_else(|| group.clone()).or_else(|| all.clone())
            };
            faces[0] = pick(get("East"), &sides);
            faces[1] = pick(get("West"), &sides);
            faces[2] = pick(get("Up"), &updown);
            faces[3] = pick(get("Down"), &updown);
            faces[4] = pick(get("South"), &sides);
            faces[5] = pick(get("North"), &sides);
        }
        BlockDesc {
            draw,
            faces,
            model: bt
                .get("CustomModel")
                .and_then(Value::as_str)
                .map(String::from),
            model_texture: bt
                .get("CustomModelTexture")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|t| t.get("Texture"))
                .and_then(Value::as_str)
                .map(String::from),
            model_scale: bt
                .get("CustomModelScale")
                .and_then(Value::as_f64)
                .map(|x| x as f32)
                .unwrap_or(1.0),
            opacity: bt.get("Opacity").and_then(Value::as_str).map(String::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_is_deep_for_objects_and_replaces_the_rest() {
        let base = json!({"A": {"x": 1, "y": [1, 2]}, "B": 1});
        let over = json!({"A": {"y": [3]}, "C": 2});
        assert_eq!(
            merge(&base, &over),
            json!({"A": {"x": 1, "y": [3]}, "B": 1, "C": 2})
        );
    }

    #[test]
    fn cube_faces_prefer_specific_over_group_over_all() {
        let bt = json!({"DrawType": "Cube", "Textures": [{"All": "a.png", "Sides": "s.png", "Up": "u.png", "Weight": 1}]});
        let b = BlockDesc::from_json(&bt);
        assert_eq!(b.draw, DrawType::Cube);
        let f: Vec<_> = b.faces.iter().map(|f| f.as_deref().unwrap()).collect();
        // east, west, up, down, south, north
        assert_eq!(f, ["s.png", "s.png", "u.png", "a.png", "s.png", "s.png"]);
    }

    #[test]
    fn only_blocks_with_a_texture_or_a_model_draw() {
        let draws = |bt| BlockDesc::from_json(&bt).draws();
        assert!(draws(
            json!({"DrawType": "Cube", "Textures": [{"All": "a.png"}]})
        ));
        assert!(draws(
            json!({"DrawType": "Model", "CustomModel": "Blocks/A.blockymodel"})
        ));
        assert!(!draws(
            json!({"DrawType": "Empty", "Opacity": "Transparent"})
        ));
        assert!(!draws(json!({"DrawType": "Cube"})));
        // the editor's brush filters name a block list and nothing else
        assert!(!draws(json!({"BlockListAssetId": "TreeWoodAndLeaves"})));
        assert!(!draws(json!({"PrefabListAssetId": "Trees"})));
    }

    #[test]
    fn model_blocks_read_model_texture_and_scale() {
        let bt = json!({"DrawType": "Model", "CustomModel": "Blocks/A.blockymodel",
                        "CustomModelTexture": [{"Texture": "Blocks/A.png", "Weight": 1}], "CustomModelScale": 0.5});
        let b = BlockDesc::from_json(&bt);
        assert_eq!(b.model.as_deref(), Some("Blocks/A.blockymodel"));
        assert_eq!(b.model_texture.as_deref(), Some("Blocks/A.png"));
        assert_eq!(b.model_scale, 0.5);
    }

    #[test]
    fn model_defs_list_their_animation_sets() {
        let v = json!({"Model": "M.blockymodel", "Texture": "T.png",
            "AnimationSets": {"Idle": {"Animations": [{"Animation": "a/Idle.blockyanim", "Speed": 0.5, "Looping": true}]},
                              "Empty": {}}});
        let d = ModelDef::from_json(&v);
        assert_eq!(d.animation_sets.len(), 1);
        assert_eq!(
            d.animation_sets["Idle"][0],
            AnimRef {
                path: "a/Idle.blockyanim".into(),
                speed: Some(0.5),
                looping: Some(true)
            }
        );
    }

    #[test]
    fn model_defs_read_attachments_random_sets_and_gradients() {
        let v = json!({"Model": "M.blockymodel", "GradientSet": "Skin", "GradientId": "02",
            "DefaultAttachments": [
                {"Model": "A.blockymodel", "Texture": "A.png", "GradientSet": "Hair", "GradientId": "Brown"},
                {"Model": "B.blockymodel", "Texture": "B.png", "Weight": 0.9},
                {},
                {"Model": "C.blockymodel"}],
            "RandomAttachmentSets": {"Fleece": {"Wool": {"Model": "W.blockymodel", "Texture": "W.png"}, "None": {}}}});
        let d = ModelDef::from_json(&v);
        assert_eq!(
            d.gradient,
            Some(GradientRef {
                set: "Skin".into(),
                id: "02".into()
            })
        );
        // the empty entry is dropped; the others keep file order
        let models: Vec<_> = d.attachments.iter().map(|a| a.model.as_str()).collect();
        assert_eq!(models, ["A.blockymodel", "B.blockymodel", "C.blockymodel"]);
        assert_eq!(d.attachments[0].texture.as_deref(), Some("A.png"));
        assert_eq!(d.attachments[0].gradient.as_ref().unwrap().id, "Brown");
        assert_eq!(d.attachments[1].weight, Some(0.9));
        assert_eq!(d.attachments[2].texture, None);
        let fleece = &d.random_attachment_sets["Fleece"];
        assert_eq!(fleece["Wool"].as_ref().unwrap().model, "W.blockymodel");
        assert!(fleece["None"].is_none());
    }

    #[test]
    fn a_child_list_of_attachments_replaces_the_parents() {
        let base =
            json!({"DefaultAttachments": [{"Model": "A.blockymodel"}, {"Model": "B.blockymodel"}]});
        let child = json!({"DefaultAttachments": [{"Model": "C.blockymodel"}]});
        let d = ModelDef::from_json(&merge(&base, &child));
        assert_eq!(d.attachments.len(), 1);
        assert_eq!(d.attachments[0].model, "C.blockymodel");
        let empty = json!({"DefaultAttachments": []});
        assert!(ModelDef::from_json(&merge(&base, &empty))
            .attachments
            .is_empty());
    }

    #[test]
    fn gradient_sets_map_set_and_id_to_a_texture() {
        let v = json!([
            {"Id": "Hair", "Gradients": {"Brown": {"BaseColor": ["#5d2014"], "Texture": "TintGradients/Hair/Brown.png"}}},
            {"Id": "Skin", "Gradients": {"02": {"Texture": "TintGradients/Skin_Tones/02.png"}, "bad": {}}},
            {"Gradients": {}}]);
        let g = GradientSets::from_json(&v);
        assert_eq!(g.len(), 2);
        let r = |s: &str, i: &str| GradientRef {
            set: s.into(),
            id: i.into(),
        };
        assert_eq!(
            g.texture(&r("Hair", "Brown")),
            Some("TintGradients/Hair/Brown.png")
        );
        assert_eq!(
            g.texture(&r("Skin", "02")).unwrap(),
            "TintGradients/Skin_Tones/02.png"
        );
        assert_eq!(g.texture(&r("Skin", "Brown")), None);
    }

    #[test]
    fn stems_drop_directory_and_extension() {
        assert_eq!(stem("Server/Models/Human/Player.json"), "Player");
        // Only the extension goes: a dot inside the name stays
        assert_eq!(stem("Server/Models/Kind.Variant.json"), "Kind.Variant");
    }
}
