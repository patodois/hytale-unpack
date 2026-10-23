//! `.blockymodel`: Hytale's model format.
//!
//! The files are JSON. Facts below come from reading the files in the install and from the
//! source of the official Blockbench plugin ([JannisX11/hytale-blockbench-plugin], GPL-3.0,
//! copyright Hypixel Studios Canada Inc.), which was read to learn what the fields mean. This
//! crate contains none of the plugin's code.
//!
//! ```text
//! { "nodes": [ Node, ... ],            // roots
//!   "format": "prop" | "character",    // optional, see "Scale"
//!   "lod": "auto",                     // optional, ignored
//!   "lodFriendly": bool, "editor": "blockbench" }   // optional, ignored
//!
//! Node = { "id": "12",                 // string, unique in the file
//!          "name": "R-Forearm",        // animations address nodes by name; names may repeat
//!          "position": {x,y,z},        // pixels
//!          "orientation": {x,y,z,w},   // unit quaternion
//!          "shape": Shape,
//!          "children": [ Node, ... ] }
//!
//! Shape = { "type": "box" | "quad" | "none",
//!           "offset": {x,y,z},         // pixels: the shape's centre relative to the node
//!           "stretch": {x,y,z},        // per-axis scale of the shape about its centre, may be negative
//!           "settings": { "size": {x,y,z}   // box; a quad has only x and y
//!                       , "normal": "+Z"    // quad only: the side it faces
//!                       , "isPiece": bool   // see "Attachments"
//!                       , "isStaticBox": bool },  // editor bookkeeping
//!           "textureLayout": { "front"|"back"|"left"|"right"|"top"|"bottom":
//!                                { "offset": {x,y}, "mirror": {x,y bool}, "angle": 0|90|180|270,
//!                                  "transparent"?: bool, "lockUVs"?: bool } },
//!           "visible": bool, "doubleSided": bool,
//!           "shadingMode": "flat" | "standard" | "fullbright" | "reflective",
//!           "unwrapMode": "custom" | "full" | "collapsed" }
//! ```
//!
//! # Scale
//!
//! All lengths are pixels. Hytale's modelling guide and the plugin set the pixel density at
//! **64 px per unit for characters and attachments and 32 px per unit for props and blocks**. The
//! plugin picks the density from the file's `format` field (`"prop"` or `"character"`); when the
//! field is missing it uses props for paths with a `Blocks` folder and characters otherwise.
//! [`BlockyModel::pixels_per_unit`] does the same. The unit is the game's block. The data never
//! says how long a block is; the bridge wants metres, and this crate takes 1 block = 1 m, which
//! fits the player's `Server/Models/Human/Player.json` (hitbox 1.85 high, eye height 1.6) against
//! its model (about 1.89 tall at 64 px/unit).
//!
//! # Hierarchy
//!
//! A node's `position` is its pivot, relative to its parent's pivot **plus the parent's
//! `shape.offset`**: the plugin subtracts the parent's offset when it writes a file and adds it
//! when it reads one, and the numbers only make sense that way (a calf hangs at the bottom of a
//! thigh whose shape is centred 11 px below its pivot). The node rotates about its pivot, and
//! its shape and its children rotate with it. The offset is taken even when the shape type is
//! `none`.
//!
//! The shape is drawn centred on `pivot + offset`, with `stretch` applied about that centre. A
//! stretch does not affect children. A negative stretch mirrors the shape (the left limbs reuse
//! the right limbs' geometry that way).
//!
//! # Boxes, quads and UVs
//!
//! A box of `size` is built from `from = -size/2` to `to = size/2` before stretch. The six faces
//! use `textureLayout`; a face with no entry is not drawn. A quad is a box of zero thickness
//! along its normal (`size` x,y become the extents of the other two axes: for X, the z extent is
//! `size.x`; for Y, the z extent is `size.y`), only its normal face is drawn, and it reads the
//! layout entry named `front`.
//!
//! Face names: `front` is +Z, `back` is -Z, `right` is +X, `left` is -X, `top` is +Y, `bottom`
//! is -Y. UVs are pixels in the texture, origin top-left, divided by the texture's own width and
//! height (the plugin uses a per-texture UV size). For a face the layout gives the corner
//! `offset`; the face covers `w x h` pixels, where `w x h` is `size.x x size.y` for front/back,
//! `size.z x size.y` for left/right and `size.x x size.z` for top/bottom. `mirror` flips an axis
//! (the rectangle extends towards negative u or v) and `angle` rotates the face's texture by
//! quarter turns. [`face_uvs`] has the exact mapping.
//!
//! `unwrapMode` is `custom` for 98% of nodes and `full` or `collapsed` for the rest; all of them
//! carry a `textureLayout`, which is what this crate uses. `shadingMode`, `doubleSided` and
//! a face's `transparent` flag are read and kept; what the engine does with them is not in the
//! data, see the README.
//!
//! # Attachments
//!
//! Cosmetics and equipment are separate models whose nodes with `isPiece: true` attach to the
//! node of the same name in the model they are worn on (the plugin's tooltip: "the node will be
//! attached to a node of the same name when displayed as an attachment in-game"). This crate
//! reads the flag but does not compose attachments.
//!
//! The plugin limits a model to 255 nodes (one per group plus one per extra cube); the largest
//! file in the install has 239.
//!
//! [JannisX11/hytale-blockbench-plugin]: https://github.com/JannisX11/hytale-blockbench-plugin

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct V3 {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
}

impl V3 {
    pub const ZERO: V3 = V3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: V3 = V3 {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };
    pub fn arr(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

/// A quaternion as stored in the files (x, y, z, w).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Quat {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    #[serde(default = "one")]
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }
}

impl Quat {
    pub fn arr(self) -> [f32; 4] {
        [self.x, self.y, self.z, self.w]
    }
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn v3_one() -> V3 {
    V3::ONE
}

fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum S {
        S(String),
        N(i64),
    }
    Ok(match S::deserialize(d)? {
        S::S(s) => s,
        S::N(n) => n.to_string(),
    })
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct BlockyModel {
    pub nodes: Vec<Node>,
    pub format: Option<String>,
    pub lod: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Node {
    #[serde(deserialize_with = "string_or_number", default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub position: V3,
    #[serde(default)]
    pub orientation: Quat,
    #[serde(default)]
    pub shape: Shape,
    #[serde(default)]
    pub children: Vec<Node>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeType {
    Box,
    Quad,
    #[default]
    None,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Shape {
    #[serde(rename = "type")]
    pub kind: ShapeType,
    pub offset: V3,
    #[serde(default = "v3_one")]
    pub stretch: V3,
    pub settings: Settings,
    pub texture_layout: BTreeMap<String, FaceLayout>,
    #[serde(default = "yes")]
    pub visible: bool,
    pub double_sided: bool,
    pub shading_mode: Option<String>,
    pub unwrap_mode: Option<String>,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            kind: ShapeType::None,
            offset: V3::ZERO,
            stretch: V3::ONE,
            settings: Settings::default(),
            texture_layout: BTreeMap::new(),
            visible: true,
            double_sided: false,
            shading_mode: None,
            unwrap_mode: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub size: Option<V3>,
    pub normal: Option<String>,
    pub is_piece: bool,
    pub is_static_box: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct V2 {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct Mirror {
    #[serde(default)]
    pub x: bool,
    #[serde(default)]
    pub y: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FaceLayout {
    pub offset: V2,
    pub mirror: Mirror,
    pub angle: i32,
    pub transparent: bool,
    #[serde(rename = "lockUVs")]
    pub lock_uvs: bool,
}

/// A node with its tree position resolved, parents before children.
#[derive(Clone, Debug)]
pub struct FlatNode<'a> {
    pub node: &'a Node,
    pub parent: Option<usize>,
}

impl BlockyModel {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Nodes in depth-first order, parents before children.
    pub fn flatten(&self) -> Vec<FlatNode<'_>> {
        fn walk<'a>(n: &'a Node, parent: Option<usize>, out: &mut Vec<FlatNode<'a>>) {
            let me = out.len();
            out.push(FlatNode { node: n, parent });
            for c in &n.children {
                walk(c, Some(me), out);
            }
        }
        let mut out = Vec::new();
        for n in &self.nodes {
            walk(n, None, &mut out);
        }
        out
    }

    /// 64 for characters and attachments, 32 for props and blocks. `path` is the entry path, used
    /// only when the file has no `format` field.
    pub fn pixels_per_unit(&self, path: &str) -> f32 {
        match self.format.as_deref() {
            Some("prop") => 32.0,
            Some(_) => 64.0,
            None if path.split('/').any(|p| p == "Blocks") => 32.0,
            None => 64.0,
        }
    }

    /// Problems that make a file unusable or suspicious, for `check`. Empty when the file is fine.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        let flat = self.flatten();
        if flat.len() > 255 {
            p.push(format!(
                "{} nodes, more than the 255 Hytale displays",
                flat.len()
            ));
        }
        for f in &flat {
            let s = &f.node.shape;
            match s.kind {
                ShapeType::Box if s.settings.size.is_none() => {
                    p.push(format!("{}: box without size", f.node.name))
                }
                ShapeType::Quad if s.settings.size.is_none() => {
                    p.push(format!("{}: quad without size", f.node.name))
                }
                _ => {}
            }
            for (k, l) in &s.texture_layout {
                if !FACE_KEYS.contains(&k.as_str()) {
                    p.push(format!("{}: unknown face {k}", f.node.name));
                }
                if !matches!(l.angle, 0 | 90 | 180 | 270) {
                    p.push(format!("{}: face {k} angle {}", f.node.name, l.angle));
                }
            }
            let q = f.node.orientation;
            let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
            if (len - 1.0).abs() > 1e-2 {
                p.push(format!("{}: orientation length {len}", f.node.name));
            }
        }
        p
    }
}

const FACE_KEYS: [&str; 6] = ["front", "back", "left", "right", "top", "bottom"];

/// One drawn face of a shape, before stretch: positions in pixels about the shape's centre.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Layout name (`front`, ...) of the side this face is on
    pub side: &'static str,
    pub positions: [[f32; 3]; 4],
    pub normal: [f32; 3],
    /// UVs in pixels, same order as `positions`
    pub uv_px: [[f32; 2]; 4],
    pub transparent: bool,
}

/// Triangles of a face's four vertices, counter-clockwise seen from outside.
pub const FACE_TRIANGLES: [[u32; 3]; 2] = [[0, 2, 1], [2, 3, 1]];

/// A box side: its name, outward normal and corner signs
type Side = (&'static str, [f32; 3], [[f32; 3]; 4]);

/// side, outward normal, and the four corners as signs (-1 = from, +1 = to) of x, y, z. Corner 0
/// is the top-left of the face seen from outside, 1 top-right, 2 bottom-left, 3 bottom-right.
const SIDES: [Side; 6] = [
    (
        "right",
        [1.0, 0.0, 0.0],
        [
            [1.0, 1.0, 1.0],
            [1.0, 1.0, -1.0],
            [1.0, -1.0, 1.0],
            [1.0, -1.0, -1.0],
        ],
    ),
    (
        "left",
        [-1.0, 0.0, 0.0],
        [
            [-1.0, 1.0, -1.0],
            [-1.0, 1.0, 1.0],
            [-1.0, -1.0, -1.0],
            [-1.0, -1.0, 1.0],
        ],
    ),
    (
        "top",
        [0.0, 1.0, 0.0],
        [
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
    ),
    (
        "bottom",
        [0.0, -1.0, 0.0],
        [
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
        ],
    ),
    (
        "front",
        [0.0, 0.0, 1.0],
        [
            [-1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
        ],
    ),
    (
        "back",
        [0.0, 0.0, -1.0],
        [
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [1.0, -1.0, -1.0],
            [-1.0, -1.0, -1.0],
        ],
    ),
];

/// The corner UVs, in pixels, of a face of extent `(w, h)` with the given layout.
///
/// Corner `i` of a face sits at face coordinates `(s, t)`: `(0, 0)`, `(1, 0)`, `(0, 1)`, `(1, 1)`.
/// Its texel is found by stepping `s * w` along u and `t * h` along v from the layout's
/// `offset`, each step negated when that axis is mirrored, and then turning that step by
/// `angle` (quarter turns, `(u, v) -> (-v, u)` each) about the offset. So `offset` is always
/// where corner 0 lands.
pub fn face_uvs(layout: &FaceLayout, w: f32, h: f32) -> [[f32; 2]; 4] {
    let sign = |m: bool| if m { -1.0 } else { 1.0 };
    let (du, dv) = (w * sign(layout.mirror.x), h * sign(layout.mirror.y));
    let turns = layout.angle.rem_euclid(360) / 90;
    [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].map(|[s, t]: [f32; 2]| {
        let mut step = [s * du, t * dv];
        for _ in 0..turns {
            step = [-step[1], step[0]];
        }
        [layout.offset.x + step[0], layout.offset.y + step[1]]
    })
}

/// The faces a shape draws. Empty for `none` shapes, shapes without a size, and boxes whose
/// layout names no face.
pub fn shape_faces(shape: &Shape) -> Vec<Face> {
    let Some(size) = shape.settings.size else {
        return Vec::new();
    };
    let mut faces = Vec::new();
    match shape.kind {
        ShapeType::None => {}
        ShapeType::Box => {
            let (sx, sy, sz) = (size.x, size.y, size.z);
            for (side, normal, corners) in SIDES {
                let Some(layout) = shape.texture_layout.get(side) else {
                    continue;
                };
                let (w, h) = match side {
                    "left" | "right" => (sz, sy),
                    "top" | "bottom" => (sx, sz),
                    _ => (sx, sy),
                };
                faces.push(make_face(side, normal, corners, [sx, sy, sz], layout, w, h));
            }
        }
        ShapeType::Quad => {
            let normal = shape.settings.normal.as_deref().unwrap_or("+Z");
            let side = match normal {
                "+X" => "right",
                "-X" => "left",
                "+Y" => "top",
                "-Y" => "bottom",
                "-Z" => "back",
                _ => "front",
            };
            // The quad's two extents become the box extents of the other two axes
            let dims = match &normal[normal.len().saturating_sub(1)..] {
                "X" => [0.0, size.y, size.x],
                "Y" => [size.x, 0.0, size.y],
                _ => [size.x, size.y, 0.0],
            };
            let Some(layout) = shape.texture_layout.get("front") else {
                return faces;
            };
            let (w, h) = match side {
                "left" | "right" => (dims[2], dims[1]),
                "top" | "bottom" => (dims[0], dims[2]),
                _ => (dims[0], dims[1]),
            };
            let (_, n, corners) = SIDES.iter().find(|s| s.0 == side).copied().unwrap();
            faces.push(make_face(side, n, corners, dims, layout, w, h));
        }
    }
    faces
}

fn make_face(
    side: &'static str,
    normal: [f32; 3],
    corners: [[f32; 3]; 4],
    dims: [f32; 3],
    layout: &FaceLayout,
    w: f32,
    h: f32,
) -> Face {
    let mut positions = [[0.0; 3]; 4];
    for (p, c) in positions.iter_mut().zip(corners) {
        *p = [
            c[0] * dims[0] / 2.0,
            c[1] * dims[1] / 2.0,
            c[2] * dims[2] / 2.0,
        ];
    }
    Face {
        side,
        positions,
        normal,
        uv_px: face_uvs(layout, w, h),
        transparent: layout.transparent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(x: f32, y: f32, mx: bool, my: bool, angle: i32) -> FaceLayout {
        FaceLayout {
            offset: V2 { x, y },
            mirror: Mirror { x: mx, y: my },
            angle,
            ..FaceLayout::default()
        }
    }

    #[test]
    fn plain_face_covers_its_rectangle() {
        let uv = face_uvs(&layout(10.0, 20.0, false, false, 0), 4.0, 6.0);
        assert_eq!(uv, [[10.0, 20.0], [14.0, 20.0], [10.0, 26.0], [14.0, 26.0]]);
    }

    #[test]
    fn mirror_extends_towards_smaller_u() {
        let uv = face_uvs(&layout(10.0, 20.0, true, false, 0), 4.0, 6.0);
        assert_eq!(uv, [[10.0, 20.0], [6.0, 20.0], [10.0, 26.0], [6.0, 26.0]]);
    }

    #[test]
    fn a_quarter_turn_swaps_the_covered_rectangle() {
        // angle 90: the face covers 6 x 4 pixels, taller than wide, and the corners rotate
        let uv = face_uvs(&layout(0.0, 0.0, false, false, 90), 4.0, 6.0);
        let (us, vs): (Vec<f32>, Vec<f32>) = uv.iter().map(|p| (p[0], p[1])).unzip();
        let span = |v: &[f32]| {
            v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert_eq!((span(&us), span(&vs)), (6.0, 4.0));
        // four quarter turns come back to the start
        let a = face_uvs(&layout(3.0, 5.0, false, true, 0), 4.0, 6.0);
        let b = face_uvs(&layout(3.0, 5.0, false, true, 360), 4.0, 6.0);
        assert_eq!(a, b);
    }

    #[test]
    fn faces_wind_counter_clockwise_from_outside() {
        let shape = Shape {
            kind: ShapeType::Box,
            settings: Settings {
                size: Some(V3 {
                    x: 2.0,
                    y: 4.0,
                    z: 6.0,
                }),
                ..Settings::default()
            },
            texture_layout: FACE_KEYS
                .iter()
                .map(|k| (k.to_string(), FaceLayout::default()))
                .collect(),
            ..Shape::default()
        };
        let faces = shape_faces(&shape);
        assert_eq!(faces.len(), 6);
        for f in &faces {
            for t in FACE_TRIANGLES {
                let p: Vec<[f32; 3]> = t.iter().map(|&i| f.positions[i as usize]).collect();
                let a = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let b = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let n = [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ];
                let d = n[0] * f.normal[0] + n[1] * f.normal[1] + n[2] * f.normal[2];
                assert!(d > 0.0, "{} winds clockwise", f.side);
            }
        }
    }

    #[test]
    fn quads_draw_only_their_normal_side() {
        let mut shape = Shape {
            kind: ShapeType::Quad,
            settings: Settings {
                size: Some(V3 {
                    x: 12.0,
                    y: 7.0,
                    z: 0.0,
                }),
                normal: Some("-X".into()),
                ..Settings::default()
            },
            ..Shape::default()
        };
        shape
            .texture_layout
            .insert("front".into(), FaceLayout::default());
        let faces = shape_faces(&shape);
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].side, "left");
        // the quad's x extent runs along z
        let zs: Vec<f32> = faces[0].positions.iter().map(|p| p[2]).collect();
        assert!(zs.contains(&6.0) && zs.contains(&-6.0));
        assert!(faces[0].positions.iter().all(|p| p[0] == 0.0));
    }

    #[test]
    fn density_follows_format_then_path() {
        let m = |f: Option<&str>| BlockyModel {
            format: f.map(String::from),
            ..BlockyModel::default()
        };
        assert_eq!(
            m(Some("prop")).pixels_per_unit("Common/Characters/a.blockymodel"),
            32.0
        );
        assert_eq!(
            m(Some("character")).pixels_per_unit("Common/Blocks/a.blockymodel"),
            64.0
        );
        assert_eq!(m(None).pixels_per_unit("Common/Blocks/a.blockymodel"), 32.0);
        assert_eq!(m(None).pixels_per_unit("Common/NPC/a.blockymodel"), 64.0);
    }
}
