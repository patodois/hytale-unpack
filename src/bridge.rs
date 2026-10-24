//! [`rewrite_bridge::Provider`] for Hytale.
//!
//! Asset ids are entry paths in `Assets.zip`, exactly as `list` prints them:
//!
//! | id | what | `load` / `load_animated` |
//! |----|------|--------------------------|
//! | `Server/Models/**.json` | an entity model definition (the player, NPCs, animals) | the model with its texture; `load_animated` adds every clip of its `AnimationSets` |
//! | `Common/**.blockymodel` | a bare model | the model with the texture found next to it; `load_animated` adds the clips the Blockbench plugin would auto-load |
//! | `Server/Item/Items/**.json` | an item that is a block | the block: a textured cube, or its custom model |
//! | `Server/Prefabs/**.prefab.json` | a building | `load_scene` only: its blocks, instanced |
//!
//! # Which clips belong to a model
//!
//! 1. For an entity model definition: its `AnimationSets` (with `Parent` inheritance, so a Trork
//!    gets the player's sets unless it overrides them). A set with one clip is named after the
//!    set (`Walk`); a set with several is named `Set/<clip file name>`.
//! 2. For a bare `.blockymodel` at `<dir>/<name>.blockymodel`: the plugin's rule, every
//!    `.blockyanim` in each folder of `<dir>/../Animations/`, named `<folder>/<file name>`. This is
//!    how `Common/NPC/Beast/Bear_Grizzly/Models/Model.blockymodel` finds
//!    `.../Bear_Grizzly/Animations/Default/*.blockyanim`.
//!
//! Clips address nodes by name; clip nodes the model lacks are skipped.
//!
//! # Textures
//!
//! A model definition names its texture; a block names it in `CustomModelTexture`. For a bare
//! model the plugin's search is used: a PNG in the model's folder whose name starts with the
//! model's name, else `Texture.png`, else a PNG in `<name>_Textures/`. Where a definition lists
//! several textures (random variants), the first is used.
//!
//! An entity definition's `GradientSet` / `GradientId` tints that texture ([`crate::image::tint`]);
//! the report says which gradient was used.
//!
//! # Attachments
//!
//! `load_animated` (and so `load`) wears the entity's `DefaultAttachments` on the body: the
//! attachment's nodes and meshes are added to the model, with its own texture (tinted by its own
//! `GradientSet` / `GradientId` where it has them), and its `isPiece` nodes merge into the body's
//! nodes of the same name, so everything follows the body's animation. See
//! [`crate::blockymodel`] for the rule. [`LoadReport`] lists what was worn and what was left out
//! with the reason: sets of random alternatives (`RandomAttachmentSets`) and weighted entries,
//! because the data gives no pick, and attachments whose pieces find no node to attach to.
//! [`Hytale::load_animated_with`] with `false` gives the bare body.
//!
//! # Blocks and prefabs
//!
//! A block model's origin is the bottom centre of its cell (the models' x and z span -16..16 px
//! and y 0..32 at 32 px per block). A `Cube` block is a 1 m cube on the same origin, one texture
//! per side, each side showing the whole texture. [`Hytale::load_scene_report`] places one
//! instance per non-empty cell:
//!
//! - cell `(x, y, z)` occupies `[x, x+1] x [y, y+1] x [z, z+1]` (the prefabs' own entities stand
//!   on the blocks under that rule, see the README) and is moved by minus the prefab's anchor, so
//!   the anchor cell is `[0, 1]` in x, y and z: its floor is at y = 0 and the origin is its
//!   lowest corner. A block model's origin is the bottom centre of its cell, so an unrotated
//!   block goes to `(x - ax + 0.5, y - ay, z - az + 0.5)`;
//! - `rotation` turns the block about its cell's centre, see [`crate::prefab::decode_rotation`];
//! - cells with a `filler` do not draw (they belong to a bigger block), `Empty` cells do not draw,
//!   and names that match no block are counted in the report and skipped;
//! - fluids and entities are not placed.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use rewrite_bridge::{
    AlphaMode, AnimatedModel, AssetInfo, AssetKind, BoxError, Instance, Material, Mesh, Model,
    Node, Provider, Scene, SceneModel, Transform,
};

use crate::assets::Assets;
use crate::blockyanim::BlockyAnim;
use crate::blockymodel::{
    shape_faces, BlockyModel, FaceLayout, Settings, Shape, ShapeType, FACE_TRIANGLES, V3,
};
use crate::defs::{AnimRef, AttachmentDef, BlockDesc, Defs, DrawType, GradientRef, ModelDef};
use crate::image::{decode_png, tint, Rgba};
use crate::prefab::{self, decode_rotation, PrefabBlock, Visit};
use crate::rig::{build_clip, build_rig, qmul, qnorm, to_image, Rig};

/// Sign applied to quarter turns of yaw, pitch and roll. See the README: yaw was checked against
/// placed furniture, pitch and roll could not be.
pub const ROTATION_SIGNS: [f32; 3] = [1.0, 1.0, 1.0];

/// An entity's own `GradientSet` tints its texture only when at least this share of the texels is
/// grey. This is a rule of this crate, not of the game's data: the 137 models that inherit the
/// player's gradient with a colour texture have 0.5 % grey texels at most, the 8 player-based ones
/// with a greyscale texture 93 %.
pub const GREYSCALE_SHARE: f32 = 0.5;

pub struct Hytale {
    assets: Assets,
    defs: Defs,
    images: Mutex<HashMap<String, Arc<Rgba>>>,
    listing: OnceLock<Vec<AssetInfo>>,
}

/// What a scene load skipped or dropped
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneReport {
    pub blocks: u64,
    pub placed: u64,
    pub empty: u64,
    pub fillers: u64,
    /// Block names that matched no drawable block, with their cell counts
    pub unresolved: BTreeMap<String, u64>,
    pub fluids: u64,
    pub entities: u64,
    pub anchor: [i32; 3],
}

/// What a model load left out
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub texture: Option<String>,
    /// The gradient (`Set/Id`) the entity's own texture was tinted with, if its definition names
    /// one that exists
    pub tint: Option<String>,
    pub clips: usize,
    pub dropped_visible_keys: usize,
    pub dropped_uv_keys: usize,
    /// Distinct clip node names the model lacks
    pub missing_nodes: Vec<String>,
    /// The attachments that were composed onto the model, in the order they were worn
    pub attachments: Vec<AttachmentNote>,
    /// What an entity definition lists as an attachment and was not composed, as `(what, why)`
    pub attachments_left_out: Vec<(String, String)>,
    /// Clips that were left out because the bridge would not accept them (a negative key time,
    /// for one), as `(name, why)`. The model itself still loads.
    pub skipped_clips: Vec<(String, String)>,
}

/// One attachment composed onto an entity model
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttachmentNote {
    /// Entry path of the attachment model
    pub model: String,
    /// Entry path of its texture
    pub texture: Option<String>,
    /// The gradient it was tinted with, as `Set/Id`
    pub tint: Option<String>,
    /// `isPiece` nodes merged into nodes of the base model
    pub pieces: usize,
    pub nodes_added: usize,
    pub meshes_added: usize,
    /// `isPiece` nodes that found no node of that name and were left out with what hangs below
    pub unmatched_pieces: Vec<String>,
}

fn kind_of_model(path: &str) -> AssetKind {
    if path.starts_with("Common/Blocks/") || path.starts_with("Common/Resources/") {
        AssetKind::Prop
    } else if path.contains("/Weapons/") {
        AssetKind::Weapon
    } else if path.starts_with("Common/NPC/")
        || path.starts_with("Common/Characters/")
        || path.starts_with("Common/Cosmetics/")
    {
        AssetKind::Character
    } else {
        AssetKind::Other
    }
}

/// The file name without its last extension: `Rock_Stone.json` is `Rock_Stone`,
/// `Houses_001.prefab.json` is `Houses_001.prefab`, and a name with dots of its own
/// (`Bush_Plants.Bush.Dead_001.prefab.json`) keeps them, so that two such files do not both
/// become `Bush_Plants`.
pub fn stem(path: &str) -> &str {
    let f = path.rsplit('/').next().unwrap_or(path);
    f.rsplit_once('.').map_or(f, |(stem, _)| stem)
}

/// The clip names of one animation set: the set's name when it has one clip, else
/// `Set/FileStem`.
///
/// A set may list the same file more than once and tell the entries apart only by their `Speed`
/// (Bat: `Fly` is `Fly.blockyanim` at the default speed, at 1.25 and at 1.5). Those clips have the
/// same name by the rule above, so each entry past the first of its name that carries a speed
/// other than 1 gets `@speed` appended (`Fly/Fly`, `Fly/Fly@1.25`, `Fly/Fly@1.5`). Entries that
/// are still equal after that (same file, same speed) are numbered `#2`, `#3`: the data has
/// nothing else to tell them apart.
fn clip_names(set: &str, refs: &[AnimRef]) -> Vec<String> {
    if refs.len() == 1 {
        return vec![set.to_string()];
    }
    let base: Vec<String> = refs
        .iter()
        .map(|r| format!("{set}/{}", stem(&Assets::common(&r.path))))
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(refs.len());
    for (i, r) in refs.iter().enumerate() {
        let repeated = base[..i].contains(&base[i]) || base[i + 1..].contains(&base[i]);
        let mut name = base[i].clone();
        if repeated {
            if let Some(speed) = r.speed.filter(|s| *s != 1.0) {
                name = format!("{name}@{speed}");
            }
        }
        let mut unique = name.clone();
        let mut n = 2;
        while out.contains(&unique) {
            unique = format!("{name}#{n}");
            n += 1;
        }
        out.push(unique);
    }
    out
}

fn berr(e: anyhow::Error) -> BoxError {
    format!("{e:#}").into()
}

impl Hytale {
    pub fn open(path: &Path) -> Result<Self> {
        let assets = Assets::open(path)?;
        let defs = Defs::new(&assets);
        Ok(Hytale {
            assets,
            defs,
            images: Mutex::new(HashMap::new()),
            listing: OnceLock::new(),
        })
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&Assets::default_path())
    }

    pub fn assets(&self) -> &Assets {
        &self.assets
    }

    pub fn defs(&self) -> &Defs {
        &self.defs
    }

    fn base_listing(&self) -> &Vec<AssetInfo> {
        self.listing.get_or_init(|| {
            let mut out = Vec::new();
            for n in self.assets.names() {
                if n.starts_with("Server/Models/") && n.ends_with(".json") {
                    // Definitions without a Model draw nothing: the showcase mannequins that are
                    // only particles. One that does not resolve stays listed, so that loading
                    // it reports why.
                    if self
                        .defs
                        .model_def(&self.assets, n)
                        .is_ok_and(|d| d.model.is_none())
                    {
                        continue;
                    }
                    out.push(AssetInfo {
                        id: n.to_string(),
                        name: stem(n).to_string(),
                        kind: AssetKind::Character,
                    });
                } else if n.starts_with("Common/") && n.ends_with(".blockymodel") {
                    let name = n
                        .trim_start_matches("Common/")
                        .trim_end_matches(".blockymodel")
                        .to_string();
                    out.push(AssetInfo {
                        id: n.to_string(),
                        name,
                        kind: kind_of_model(n),
                    });
                } else if n.starts_with("Server/Prefabs/") && n.ends_with(".prefab.json") {
                    let name = n
                        .trim_start_matches("Server/Prefabs/")
                        .trim_end_matches(".prefab.json")
                        .to_string();
                    out.push(AssetInfo {
                        id: n.to_string(),
                        name,
                        kind: AssetKind::Level,
                    });
                }
            }
            out.sort_by(|a, b| a.id.cmp(&b.id));
            out
        })
    }

    /// Whether an item file has a `BlockType` after inheritance. That does not make it a block:
    /// the editor's brush filters (`Filter_Rock`) and prefab placers carry a `BlockType` that only
    /// names a block list or a prefab list; see [`Hytale::is_drawable_block`].
    pub fn is_block_item(&self, path: &str) -> bool {
        self.defs
            .item(&self.assets, path)
            .map(|v| v.get("BlockType").is_some())
            .unwrap_or(false)
    }

    /// Whether an item file is a block with something to draw: a cube with at least one face
    /// texture, or a custom model. `DrawType: Empty` blocks (`Barrier`) and the editor items that
    /// only carry a list id are not, and the provider does not list them. A block whose
    /// definition does not resolve counts as drawable, so that loading it reports the error.
    pub fn is_drawable_block(&self, path: &str) -> bool {
        if !self.is_block_item(path) {
            return false;
        }
        match self.defs.block(&self.assets, stem(path)) {
            Ok(desc) => desc.is_some_and(|d| d.draws()),
            Err(_) => true,
        }
    }

    /// Drops the decoded textures kept for reuse; for long sweeps over many models.
    pub fn clear_cache(&self) {
        self.images.lock().unwrap().clear();
    }

    pub fn texture(&self, zip_path: &str) -> Result<Arc<Rgba>> {
        if let Some(t) = self.images.lock().unwrap().get(zip_path) {
            return Ok(t.clone());
        }
        let img = Arc::new(
            decode_png(&self.assets.read(zip_path)?).with_context(|| zip_path.to_string())?,
        );
        self.images
            .lock()
            .unwrap()
            .insert(zip_path.to_string(), img.clone());
        Ok(img)
    }

    /// The Blockbench plugin's texture search for a model: same folder, files starting with the
    /// model's name or `Texture.png`; then `<name>_Textures/`.
    pub fn discover_texture(&self, model_zip_path: &str) -> Option<String> {
        let (dir, file) = model_zip_path.rsplit_once('/')?;
        let name = file.trim_end_matches(".blockymodel");
        let lower = name.to_ascii_lowercase();
        let dir_prefix = format!("{dir}/");
        let mut same: Vec<&str> = self
            .assets
            .names_under(&dir_prefix)
            .filter(|n| {
                n[dir.len() + 1..].find('/').is_none() && n.to_ascii_lowercase().ends_with(".png")
            })
            .filter(|n| {
                let f = &n[dir.len() + 1..];
                f.to_ascii_lowercase().starts_with(&lower) || f == "Texture.png"
            })
            .collect();
        same.sort_by_key(|n| {
            let f = &n[dir.len() + 1..];
            (
                !f.eq_ignore_ascii_case(&format!("{name}.png")),
                !f.starts_with(name),
                f.to_string(),
            )
        });
        if let Some(t) = same.first() {
            return Some(t.to_string());
        }
        let sub = format!("{dir}/{name}_Textures/");
        let mut in_folder: Vec<&str> = self
            .assets
            .names_under(&sub)
            .filter(|n| n.to_ascii_lowercase().ends_with(".png"))
            .collect();
        in_folder.sort();
        in_folder.first().map(|s| s.to_string())
    }

    fn blocky(&self, zip_path: &str) -> Result<BlockyModel> {
        BlockyModel::parse(&self.assets.read(zip_path)?).with_context(|| zip_path.to_string())
    }

    fn rig_for(
        &self,
        model_zip: &str,
        texture_zip: Option<&str>,
        extra_scale: f32,
        gradient: Option<&GradientRef>,
        tinted: &mut Option<String>,
    ) -> Result<(Rig, Option<String>)> {
        let bm = self.blocky(model_zip)?;
        let ppu = bm.pixels_per_unit(model_zip);
        let tex_path = texture_zip
            .map(String::from)
            .or_else(|| self.discover_texture(model_zip));
        let mut tex = match &tex_path {
            Some(p) => Some(self.texture(p)?),
            None => None,
        };
        // An entity's own gradient is often inherited (every `Parent: Player` has the skin gradient)
        // by a model whose texture is painted in colour, with a few grey texels for outlines: only
        // a texture that is mostly grey is a greyscale texture to tint.
        let new = match (&tex, gradient) {
            (Some(t), Some(g)) if t.grey_share() >= GREYSCALE_SHARE => {
                self.tinted_texture(t.clone(), g).ok().map(|t2| (g, t2))
            }
            _ => None,
        };
        if let Some((g, t2)) = new {
            *tinted = Some(format!("{}/{}", g.set, g.id));
            tex = Some(Arc::new(t2));
        }
        let name = stem(model_zip).to_string();
        let label = tex_path
            .as_deref()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| "untextured".into());
        Ok((
            build_rig(&bm, &name, extra_scale / ppu, tex.as_deref(), &label),
            tex_path,
        ))
    }

    /// Loads an entity model definition, a bare model or a block item as an animated model, with
    /// the entity's default attachments worn.
    pub fn load_animated_report(&self, id: &str) -> Result<(AnimatedModel, LoadReport)> {
        self.load_animated_with(id, true)
    }

    /// [`Hytale::load_animated_report`], choosing whether an entity's default attachments are
    /// composed onto it (`false` gives the bare body, which is how every model loaded before
    /// attachments were read).
    pub fn load_animated_with(
        &self,
        id: &str,
        attachments: bool,
    ) -> Result<(AnimatedModel, LoadReport)> {
        let mut report = LoadReport::default();
        if id.starts_with("Server/Models/") && id.ends_with(".json") {
            let def = self.defs.model_def(&self.assets, id)?;
            let model_path = Assets::common(
                def.model
                    .as_deref()
                    .ok_or_else(|| anyhow!("{id}: no Model"))?,
            );
            let tex = def.texture.as_deref().map(Assets::common);
            let (mut rig, tex_path) = self.rig_for(
                &model_path,
                tex.as_deref(),
                1.0,
                def.gradient.as_ref(),
                &mut report.tint,
            )?;
            report.texture = tex_path;
            if attachments {
                self.compose_attachments(&mut rig, &def, &mut report);
            }
            let mut am = AnimatedModel::new(rig.model.clone());
            am.model.name = stem(id).to_string();
            self.add_def_clips(&rig, &def, &mut am, &mut report);
            am.validate()?;
            Ok((am, report))
        } else if id.ends_with(".blockymodel") {
            let (rig, tex_path) = self.rig_for(id, None, 1.0, None, &mut None)?;
            report.texture = tex_path;
            let mut am = AnimatedModel::new(rig.model.clone());
            self.add_folder_clips(&rig, id, &mut am, &mut report);
            am.validate()?;
            Ok((am, report))
        } else if id.starts_with("Server/Item/") && id.ends_with(".json") {
            let m = self
                .block_model(stem(id))?
                .ok_or_else(|| anyhow!("{id}: not a block that draws anything"))?;
            report.texture = m.materials.first().map(|m| m.name.clone());
            Ok(((*m).clone().into(), report))
        } else {
            bail!("{id}: not a model, model definition or block (prefabs load as scenes)");
        }
    }

    /// Wears the entity's `DefaultAttachments` on the rig; see [`Rig::attach`]. What is not worn is
    /// listed in `report.attachments_left_out` with the reason.
    fn compose_attachments(&self, rig: &mut Rig, def: &ModelDef, report: &mut LoadReport) {
        for (set, options) in &def.random_attachment_sets {
            let names: Vec<&str> = options.keys().map(String::as_str).collect();
            report.attachments_left_out.push((
                format!("RandomAttachmentSets/{set}"),
                format!(
                    "one of {} is picked per instance and the data gives no pick",
                    names.join(", ")
                ),
            ));
        }
        for a in &def.attachments {
            if let Some(w) = a.weight {
                report.attachments_left_out.push((
                    a.model.clone(),
                    format!("entry has a Weight ({w}), which the data does not explain"),
                ));
                continue;
            }
            match self.wear(rig, a) {
                Ok(note) if note.nodes_added == 0 => {
                    // every piece found no node to hang from, so nothing was added
                    let why = if note.unmatched_pieces.is_empty() {
                        "adds nothing: its pieces have no children".to_string()
                    } else {
                        format!(
                            "adds nothing: no node named {}",
                            note.unmatched_pieces.join(" or ")
                        )
                    };
                    report.attachments_left_out.push((a.model.clone(), why));
                }
                Ok(note) => report.attachments.push(note),
                Err(why) => report.attachments_left_out.push((a.model.clone(), why)),
            }
        }
    }

    fn tinted_texture(
        &self,
        tex: Arc<Rgba>,
        gradient: &GradientRef,
    ) -> std::result::Result<Rgba, String> {
        let path = self
            .defs
            .gradient_sets(&self.assets)
            .texture(gradient)
            .ok_or_else(|| {
                format!(
                    "gradient {}/{} is not in {}",
                    gradient.set,
                    gradient.id,
                    crate::defs::GRADIENT_SETS_PATH
                )
            })?;
        let g = self
            .texture(&Assets::common(path))
            .map_err(|e| format!("gradient {path}: {e:#}"))?;
        Ok(tint(&tex, &g))
    }

    fn wear(
        &self,
        rig: &mut Rig,
        a: &AttachmentDef,
    ) -> std::result::Result<AttachmentNote, String> {
        let model_path = Assets::common(&a.model);
        let bm = self.blocky(&model_path).map_err(|e| format!("{e:#}"))?;
        let ppu = bm.pixels_per_unit(&model_path);
        if (1.0 / ppu - rig.scale).abs() > 1e-9 {
            return Err(format!(
                "{ppu} pixels per unit, not the base model's {}",
                1.0 / rig.scale
            ));
        }
        let tex_path = a
            .texture
            .as_deref()
            .map(Assets::common)
            .or_else(|| self.discover_texture(&model_path));
        let mut tint_name = None;
        let tex: Option<Rgba> = match &tex_path {
            Some(p) => {
                let t = self.texture(p).map_err(|e| format!("{e:#}"))?;
                match &a.gradient {
                    Some(g) => {
                        tint_name = Some(format!("{}/{}", g.set, g.id));
                        Some(self.tinted_texture(t, g)?)
                    }
                    None => Some((*t).clone()),
                }
            }
            None => None,
        };
        let label = tex_path
            .as_deref()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| "untextured".into());
        let rep = rig.attach(&bm, tex.as_ref(), &label);
        Ok(AttachmentNote {
            model: model_path,
            texture: tex_path,
            tint: tint_name,
            pieces: rep.pieces,
            nodes_added: rep.nodes_added,
            meshes_added: rep.meshes_added,
            unmatched_pieces: rep.unmatched,
        })
    }

    fn add_clip(
        &self,
        rig: &Rig,
        am: &mut AnimatedModel,
        report: &mut LoadReport,
        zip_path: &str,
        name: String,
        cache: &mut HashMap<String, Arc<BlockyAnim>>,
    ) {
        let anim = match cache.get(zip_path) {
            Some(a) => a.clone(),
            None => match self
                .assets
                .read(zip_path)
                .and_then(|b| BlockyAnim::parse(&b).with_context(|| zip_path.to_string()))
            {
                Ok(a) => {
                    let a = Arc::new(a);
                    cache.insert(zip_path.to_string(), a.clone());
                    a
                }
                Err(_) => return,
            },
        };
        push_clip(rig, am, report, &anim, name);
    }

    fn add_def_clips(
        &self,
        rig: &Rig,
        def: &ModelDef,
        am: &mut AnimatedModel,
        report: &mut LoadReport,
    ) {
        let mut cache = HashMap::new();
        for (set, refs) in &def.animation_sets {
            let names = clip_names(set, refs);
            for (r, name) in refs.iter().zip(names) {
                let zip = Assets::common(&r.path);
                self.add_clip(rig, am, report, &zip, name, &mut cache);
            }
        }
    }

    fn add_folder_clips(
        &self,
        rig: &Rig,
        model_zip: &str,
        am: &mut AnimatedModel,
        report: &mut LoadReport,
    ) {
        let Some((dir, _)) = model_zip.rsplit_once('/') else {
            return;
        };
        let Some((parent, _)) = dir.rsplit_once('/') else {
            return;
        };
        let prefix = format!("{parent}/Animations/");
        let mut files: Vec<&str> = self
            .assets
            .names_under(&prefix)
            .filter(|n| n.ends_with(".blockyanim") && n[prefix.len()..].matches('/').count() == 1)
            .collect();
        files.sort();
        let mut cache = HashMap::new();
        for f in files {
            let name = f[prefix.len()..]
                .trim_end_matches(".blockyanim")
                .to_string();
            self.add_clip(rig, am, report, f, name, &mut cache);
        }
    }

    /// A block by item id (`Rock_Stone`), prefab cell name (`*Rail_State_Definitions_Corner_Left`)
    /// or item path. `None` when the name draws nothing.
    pub fn block_model(&self, name: &str) -> Result<Option<Arc<Model>>> {
        let name = if name.starts_with("Server/") {
            stem(name)
        } else {
            name
        };
        let Some(desc) = self.defs.block(&self.assets, name)? else {
            return Ok(None);
        };
        let mut model = Model {
            name: name.trim_start_matches('*').to_string(),
            nodes: vec![Node::new("block", None)],
            ..Model::default()
        };
        let mut drew = false;
        if matches!(desc.draw, DrawType::Cube | DrawType::CubeWithModel) {
            drew |= self.add_cube(&mut model, &desc)?;
        }
        if matches!(desc.draw, DrawType::Model | DrawType::CubeWithModel) {
            if let Some(m) = &desc.model {
                let model_zip = Assets::common(m);
                let tex = desc.model_texture.as_deref().map(Assets::common);
                let (rig, _) = self.rig_for(
                    &model_zip,
                    tex.as_deref(),
                    desc.model_scale,
                    None,
                    &mut None,
                )?;
                drew |= !rig.model.meshes.is_empty();
                append_model(&mut model, &rig.model);
            }
        }
        if !drew {
            return Ok(None);
        }
        model.validate()?;
        Ok(Some(Arc::new(model)))
    }

    fn add_cube(&self, model: &mut Model, desc: &BlockDesc) -> Result<bool> {
        // A 1 m cube standing on the cell's bottom centre: one mesh per texture
        let layout: BTreeMap<String, FaceLayout> =
            ["front", "back", "left", "right", "top", "bottom"]
                .iter()
                .map(|k| (k.to_string(), FaceLayout::default()))
                .collect();
        let shape = Shape {
            kind: ShapeType::Box,
            settings: Settings {
                size: Some(V3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                }),
                ..Settings::default()
            },
            texture_layout: layout,
            ..Shape::default()
        };
        let faces = shape_faces(&shape); // right, left, top, bottom, front, back = Side::ALL
        let mut by_tex: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, f) in desc.faces.iter().enumerate() {
            if let Some(t) = f {
                by_tex.entry(t.as_str()).or_default().push(i);
            }
        }
        let drew = !by_tex.is_empty();
        for (tex, sides) in by_tex {
            let img = self.texture(&Assets::common(tex))?;
            let mut m = Material {
                name: tex.rsplit('/').next().unwrap_or(tex).to_string(),
                base_color: Some(to_image(&img)),
                metallic: 0.0,
                roughness: 1.0,
                ..Material::default()
            };
            if img.has_alpha() {
                m.alpha = AlphaMode::Mask(0.5);
            }
            model.materials.push(m);
            let mut mesh = Mesh {
                name: format!("cube {tex}"),
                node: 0,
                material: model.materials.len() - 1,
                ..Mesh::default()
            };
            for s in sides {
                let f = &faces[s];
                let base = mesh.positions.len() as u32;
                for i in 0..4 {
                    let p = f.positions[i];
                    mesh.positions.push([p[0], p[1] + 0.5, p[2]]);
                    mesh.normals.push(f.normal);
                    mesh.uvs.push([(i % 2) as f32, (i / 2) as f32]);
                }
                for tri in FACE_TRIANGLES {
                    mesh.indices.extend(tri.iter().map(|i| base + i));
                }
            }
            model.meshes.push(mesh);
        }
        Ok(drew)
    }

    /// Loads a prefab as a scene, with a report of what was skipped.
    pub fn load_scene_report(&self, id: &str) -> Result<(Scene, SceneReport)> {
        if !(id.starts_with("Server/Prefabs/") && id.ends_with(".json")) {
            bail!("{id}: not a prefab");
        }
        struct Collector<'a> {
            hy: &'a Hytale,
            models: Vec<SceneModel>,
            index: HashMap<String, Option<usize>>,
            instances: Vec<Instance>,
            report: SceneReport,
            anchor: [i32; 3],
            /// The first block that could not be placed; no scene is returned then
            error: Option<anyhow::Error>,
        }
        impl Collector<'_> {
            fn place(&mut self, b: &PrefabBlock) {
                if self.error.is_some() {
                    return;
                }
                self.report.blocks += 1;
                if b.name == "Empty" {
                    self.report.empty += 1;
                    return;
                }
                if b.filler.unwrap_or(0) != 0 {
                    self.report.fillers += 1;
                    return;
                }
                let idx = match self.index.get(&b.name) {
                    Some(i) => *i,
                    None => {
                        let i = match self.hy.block_model(&b.name) {
                            Ok(Some(m)) => {
                                self.models.push(SceneModel::Inline((*m).clone()));
                                Some(self.models.len() - 1)
                            }
                            _ => None,
                        };
                        self.index.insert(b.name.clone(), i);
                        i
                    }
                };
                let Some(model) = idx else {
                    *self.report.unresolved.entry(b.name.clone()).or_default() += 1;
                    return;
                };
                match block_transform([b.x, b.y, b.z], self.anchor, b.rotation.unwrap_or(0)) {
                    Ok(t) => {
                        self.report.placed += 1;
                        self.instances.push(Instance::new(model, t));
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        impl Visit for Collector<'_> {
            fn block(&mut self, b: &PrefabBlock) {
                self.place(b);
            }
        }
        // Two passes over the entry's bytes (23 MB at most): the first for the anchor, which the
        // blocks are placed relative to, the second for the blocks. Neither keeps the document.
        let bytes = self.assets.read(id)?;
        struct Nop;
        impl Visit for Nop {}
        let anchor = prefab::stream(&bytes[..], &mut Nop)?.0.anchor;
        let mut c = Collector {
            hy: self,
            models: Vec::new(),
            index: HashMap::new(),
            instances: Vec::new(),
            report: SceneReport {
                anchor,
                ..SceneReport::default()
            },
            anchor,
            error: None,
        };
        let (_, counts) = prefab::stream(&bytes[..], &mut c)?;
        if let Some(e) = c.error {
            return Err(e.context(id.to_string()));
        }
        c.report.fluids = counts.fluids;
        c.report.entities = counts.entities;
        let scene = Scene {
            name: stem(id).trim_end_matches(".prefab").to_string(),
            models: c.models,
            instances: c.instances,
            ..Scene::default()
        };
        scene.validate()?;
        Ok((scene, c.report))
    }
}

/// Converts a clip for the rig and adds it to the model, unless the bridge would reject it. One
/// bad clip (a key at a negative time, say) must not fail a model that holds hundreds of good
/// ones: it is left out and named in the report.
fn push_clip(
    rig: &Rig,
    am: &mut AnimatedModel,
    report: &mut LoadReport,
    anim: &BlockyAnim,
    name: String,
) {
    let (clip, r) = build_clip(rig, anim, &name);
    if let Err(e) = clip.validate(am.model.nodes.len()) {
        report.skipped_clips.push((name, e.to_string()));
        return;
    }
    report.dropped_visible_keys += r.dropped_visible_keys;
    report.dropped_uv_keys += r.dropped_uv_keys;
    report.missing_nodes.extend(r.missing_nodes);
    report.missing_nodes.sort();
    report.missing_nodes.dedup();
    report.clips += 1;
    am.animations.push(clip);
}

fn append_model(into: &mut Model, from: &Model) {
    let node_base = into.nodes.len();
    let mat_base = into.materials.len();
    for n in &from.nodes {
        let mut n = n.clone();
        n.parent = Some(n.parent.map(|p| p + node_base).unwrap_or(0));
        into.nodes.push(n);
    }
    into.materials.extend(from.materials.iter().cloned());
    for m in &from.meshes {
        let mut m = m.clone();
        m.node += node_base;
        m.material += mat_base;
        into.meshes.push(m);
    }
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    // v' = q * (v, 0) * q^-1
    let qi = [-q[0], -q[1], -q[2], q[3]];
    let r = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qi);
    [r[0], r[1], r[2]]
}

/// The quaternion of a block `rotation` value: `Ry(yaw) * Rx(pitch) * Rz(roll)`.
pub fn block_rotation(rotation: u32) -> [f32; 4] {
    let (yaw, pitch, roll) = decode_rotation(rotation);
    let axis = |a: usize, quarter: u32, sign: f32| {
        let half = sign * quarter as f32 * std::f32::consts::FRAC_PI_4;
        let (s, c) = half.sin_cos();
        let mut q = [0.0, 0.0, 0.0, c];
        q[a] = s;
        q
    };
    qnorm(qmul(
        qmul(
            axis(1, yaw, ROTATION_SIGNS[0]),
            axis(0, pitch, ROTATION_SIGNS[1]),
        ),
        axis(2, roll, ROTATION_SIGNS[2]),
    ))
}

/// Where a block model (origin at the bottom centre of its cell) goes for a prefab cell: the
/// cell `[x, x+1]` moved by minus the anchor, the model origin at the middle of its floor. An
/// error when a coordinate minus the anchor does not fit 32 bits (the file's numbers are not
/// trusted to be small).
pub fn block_transform(cell: [i32; 3], anchor: [i32; 3], rotation: u32) -> Result<Transform> {
    let q = block_rotation(rotation);
    let mut centre = [0.0f32; 3];
    for k in 0..3 {
        let d = cell[k].checked_sub(anchor[k]).ok_or_else(|| {
            anyhow!("block at {cell:?} is too far from the anchor {anchor:?} to be placed")
        })?;
        centre[k] = d as f32 + 0.5;
    }
    // rotate about the cell centre: the model origin sits half a cell below it
    let off = rotate(q, [0.0, -0.5, 0.0]);
    Ok(Transform {
        translation: [centre[0] + off[0], centre[1] + off[1], centre[2] + off[2]],
        rotation: q,
        scale: [1.0; 3],
    })
}

impl Provider for Hytale {
    fn game(&self) -> &str {
        "Hytale"
    }

    fn list(&self, filter: &str) -> Result<Vec<AssetInfo>, BoxError> {
        let f = filter.to_ascii_lowercase();
        let mut out: Vec<AssetInfo> = self
            .base_listing()
            .iter()
            .filter(|a| {
                f.is_empty()
                    || a.id.to_ascii_lowercase().contains(&f)
                    || a.name.to_ascii_lowercase().contains(&f)
            })
            .cloned()
            .collect();
        let mut items: Vec<&str> = self
            .assets
            .names_under("Server/Item/Items/")
            .filter(|n| n.ends_with(".json"))
            .collect();
        items.sort();
        for n in items {
            let name = stem(n);
            if (f.is_empty() || n.to_ascii_lowercase().contains(&f)) && self.is_drawable_block(n) {
                out.push(AssetInfo {
                    id: n.to_string(),
                    name: name.to_string(),
                    kind: AssetKind::Prop,
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    fn load(&self, id: &str) -> Result<Model, BoxError> {
        Ok(self.load_animated(id)?.model)
    }

    fn load_animated(&self, id: &str) -> Result<AnimatedModel, BoxError> {
        self.load_animated_report(id).map(|r| r.0).map_err(berr)
    }

    fn load_scene(&self, id: &str) -> Result<Scene, BoxError> {
        self.load_scene_report(id).map(|r| r.0).map_err(berr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockymodel::BlockyModel;

    fn r(path: &str, speed: Option<f32>) -> AnimRef {
        AnimRef {
            path: path.into(),
            speed,
            looping: None,
        }
    }

    #[test]
    fn a_single_clip_takes_the_name_of_its_set() {
        assert_eq!(
            clip_names("Idle", &[r("A/Default/Idle.blockyanim", None)]),
            ["Idle"]
        );
    }

    #[test]
    fn several_clips_are_named_by_file() {
        let refs = [
            r("A/Jump_Far.blockyanim", None),
            r("A/Jump_Far2.blockyanim", None),
        ];
        assert_eq!(
            clip_names("JumpWalk", &refs),
            ["JumpWalk/Jump_Far", "JumpWalk/Jump_Far2"]
        );
    }

    #[test]
    fn one_file_at_several_speeds_is_told_apart_by_speed() {
        // Bat: Fly.blockyanim three times, at the default speed, 1.25 and 1.5
        let refs = [
            r("A/Fly.blockyanim", None),
            r("A/Fly.blockyanim", Some(1.25)),
            r("A/Fly.blockyanim", Some(1.5)),
            r("A/Fly_Glide.blockyanim", Some(2.0)),
        ];
        assert_eq!(
            clip_names("Fly", &refs),
            ["Fly/Fly", "Fly/Fly@1.25", "Fly/Fly@1.5", "Fly/Fly_Glide"]
        );
    }

    #[test]
    fn identical_entries_are_numbered_as_the_last_resort() {
        let refs = [
            r("A/Fly.blockyanim", None),
            r("A/Fly.blockyanim", Some(1.0)),
            r("A/Fly.blockyanim", None),
        ];
        assert_eq!(
            clip_names("Fly", &refs),
            ["Fly/Fly", "Fly/Fly#2", "Fly/Fly#3"]
        );
    }

    #[test]
    fn unrotated_blocks_sit_on_their_cell() {
        let t = block_transform([3, 5, -2], [3, 5, -2], 0).unwrap();
        assert_eq!(t.translation, [0.5, 0.0, 0.5]);
        assert_eq!(t.rotation, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_cell_is_moved_by_minus_the_anchor_into_x_to_x_plus_one() {
        // The anchor cell is [0, 1] in x, y and z, so its block model (origin at the bottom
        // centre) is at (0.5, 0, 0.5); one cell on, one metre on
        let t = block_transform([-7, 40, 9], [-7, 40, 9], 0).unwrap();
        assert_eq!(t.translation, [0.5, 0.0, 0.5]);
        let t = block_transform([-6, 42, 7], [-7, 40, 9], 0).unwrap();
        assert_eq!(t.translation, [1.5, 2.0, -1.5]);
    }

    #[test]
    fn a_cell_too_far_from_the_anchor_is_an_error() {
        // i32::MAX - (-1) does not fit: a panic in debug builds, a wrong block in release ones
        let e = block_transform([i32::MAX, 0, 0], [-1, 0, 0], 0).unwrap_err();
        assert!(e.to_string().contains("too far"), "{e}");
        assert!(block_transform([0, i32::MIN, 0], [0, 1, 0], 0).is_err());
        assert!(block_transform([0, 0, -2], [0, 0, i32::MAX], 0).is_err());
    }

    #[test]
    fn stem_cuts_at_the_last_dot() {
        assert_eq!(stem("Server/Item/Items/Rock_Stone.json"), "Rock_Stone");
        assert_eq!(stem("A/Fly.blockyanim"), "Fly");
        assert_eq!(stem("noextension"), "noextension");
        assert_eq!(stem("A/B_001.prefab.json"), "B_001.prefab");
        // Dots inside the name stay, so these two prefabs do not both become `Bush_Plants`
        assert_eq!(
            stem("P/Bush_Plants.Bush.Dead_001.prefab.json"),
            "Bush_Plants.Bush.Dead_001.prefab"
        );
        assert_ne!(
            stem("P/Bush_Plants.Bush.Dead_001.prefab.json"),
            stem("P/Bush_Plants.Bush.Dead_002.prefab.json")
        );
    }

    fn tiny_rig() -> Rig {
        let json = r#"{"nodes":[{"id":"1","name":"Root","position":{"x":0,"y":10,"z":0},
            "orientation":{"x":0,"y":0,"z":0,"w":1},
            "shape":{"type":"box","offset":{"x":0,"y":5,"z":0},"stretch":{"x":1,"y":1,"z":1},
                     "settings":{"size":{"x":4,"y":10,"z":4}},
                     "textureLayout":{"front":{"offset":{"x":0,"y":0},"mirror":{"x":false,"y":false},"angle":0}},
                     "visible":true,"doubleSided":false}}]}"#;
        let bm = BlockyModel::parse(json.as_bytes()).unwrap();
        build_rig(&bm, "m", 1.0 / 64.0, None, "none")
    }

    fn clip_json(time: i32) -> Vec<u8> {
        format!(
            r#"{{"duration":60,"nodeAnimations":{{"Root":{{"position":[
                 {{"time":{time},"delta":{{"x":0,"y":0,"z":0}}}},
                 {{"time":30,"delta":{{"x":1,"y":0,"z":0}}}}]}}}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn a_clip_the_bridge_rejects_is_skipped_and_reported_not_fatal() {
        let rig = tiny_rig();
        let hy_report = |t: i32| {
            let anim = BlockyAnim::parse(&clip_json(t)).unwrap();
            let mut am = AnimatedModel::new(rig.model.clone());
            let mut report = LoadReport::default();
            push_clip(&rig, &mut am, &mut report, &anim, format!("Clip{t}"));
            (am, report)
        };
        // The premise: a key at a negative time makes the bridge reject the clip, and with the
        // clip in the model, the whole model
        let anim = BlockyAnim::parse(&clip_json(-5)).unwrap();
        let (clip, _) = build_clip(&rig, &anim, "Bad");
        assert!(clip.validate(rig.model.nodes.len()).is_err());
        let mut whole = AnimatedModel::new(rig.model.clone());
        whole.animations.push(clip);
        assert!(whole.validate().is_err());
        // Skipped: the model still validates, and the report names the clip
        let (am, report) = hy_report(-5);
        assert!(am.animations.is_empty());
        assert_eq!(report.skipped_clips.len(), 1);
        assert!(
            report.skipped_clips[0].1.contains("Clip-5"),
            "{:?}",
            report.skipped_clips
        );
        assert_eq!(report.clips, 0);
        am.validate().unwrap();
        // A good clip is added as before
        let (am, report) = hy_report(0);
        assert_eq!(am.animations.len(), 1);
        assert_eq!(report.clips, 1);
        assert!(report.skipped_clips.is_empty());
        am.validate().unwrap();
    }

    #[test]
    fn rotation_is_about_the_cell_centre() {
        // Pitch of two quarter turns flips a block upside down: its origin (bottom centre) ends
        // up at the cell's top
        let t = block_transform([0, 0, 0], [0, 0, 0], 8).unwrap();
        assert!((t.translation[1] - 1.0).abs() < 1e-5, "{:?}", t.translation);
        assert!((t.translation[0] - 0.5).abs() < 1e-5);
        // Yaw only keeps the origin at the floor
        let t = block_transform([0, 0, 0], [0, 0, 0], 1).unwrap();
        assert!(t.translation[1].abs() < 1e-5);
    }

    #[test]
    fn four_yaw_quarter_turns_are_the_identity() {
        let q = block_rotation(0);
        assert_eq!(q, [0.0, 0.0, 0.0, 1.0]);
        let q1 = block_rotation(1);
        let q2 = block_rotation(2);
        let r = qmul(q1, q1);
        for k in 0..4 {
            assert!((r[k] - q2[k]).abs() < 1e-5);
        }
    }
}
