//! Turning blocky models and clips into rewrite-bridge types.
//!
//! # Models
//!
//! Every Hytale node becomes a bridge node (its pivot, named like the Hytale node) with the
//! node's `position` as translation and `orientation` as rotation, both scaled from pixels to
//! metres. Because a child's position is relative to its parent's pivot plus the parent's shape
//! offset (see [`crate::blockymodel`]), that offset is added to the child's translation.
//!
//! A node with a visible shape gets one more bridge node under its pivot, named `<name>/shape`,
//! with the shape's `offset` as translation and `stretch` as scale, and the shape's mesh hangs on
//! it. Keeping the stretch off the pivot node is what stops it from scaling the children, and it
//! is the node that `shapeStretch` clips drive. Each shape is one mesh with one material: the
//! model's texture, in a variant for double-sided shapes and one for shapes with a face marked
//! `transparent`.
//!
//! Hidden shapes (`visible: false`) get no mesh. Faces without a `textureLayout` entry are not
//! drawn. Without a texture the meshes have no UVs, because pixel UVs cannot be normalised.
//!
//! # Clips
//!
//! See [`crate::blockyanim`] for the data; here is how it lands in bridge channels, per Hytale
//! node name (all nodes with that name are driven):
//!
//! - `position`: translation channel, value = (rest position + delta) in metres;
//! - `orientation`: rotation channel, value = rest * delta, spherical interpolation;
//! - `shapeStretch`: scale channel on the `<name>/shape` node, value = stretch * delta;
//! - `shapeVisible` and `shapeUvOffset` have no bridge target and are dropped (counted in
//!   [`ClipReport`]).
//!
//! `smooth` keyframes of position and stretch become cubic-spline keyframes whose tangents
//! reproduce the Catmull-Rom curve of the Blockbench plugin's preview; a channel whose segments
//! are all linear stays linear. Looping and `holdLastKeyframe` are not representable and are
//! dropped.

use std::collections::HashMap;

use rewrite_bridge::{
    AlphaMode, Animation, Channel, Image, Interpolation, Material, Mesh, Model, Node, Target,
};

use crate::blockyanim::{BlockyAnim, Key, FPS};
use crate::blockymodel::{shape_faces, BlockyModel, FACE_TRIANGLES, V3};
use crate::image::Rgba;

pub fn to_image(img: &Rgba) -> Image {
    Image {
        width: img.width,
        height: img.height,
        rgba: img.data.clone(),
    }
}

/// A Hytale node's place in the bridge model
#[derive(Clone, Debug)]
pub struct RigNode {
    /// The pivot node
    pub pivot: usize,
    /// Rest position in pixels, already including the parent's shape offset
    pub rest_pos: [f32; 3],
    pub rest_rot: [f32; 4],
    /// The shape node and the shape's rest stretch
    pub shape: Option<(usize, [f32; 3])>,
}

pub struct Rig {
    pub model: Model,
    pub by_name: HashMap<String, Vec<RigNode>>,
    /// Metres per pixel
    pub scale: f32,
}

pub fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub fn qnorm(q: [f32; 4]) -> [f32; 4] {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l == 0.0 || !l.is_finite() {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
    }
}

/// Builds the bridge model for a blocky model. `scale` is metres per pixel. `texture` is the
/// image every shape is drawn with, if one could be found.
pub fn build_rig(
    bm: &BlockyModel,
    name: &str,
    scale: f32,
    texture: Option<&Rgba>,
    material_name: &str,
) -> Rig {
    let mut model = Model {
        name: name.to_string(),
        ..Model::default()
    };
    let mut by_name: HashMap<String, Vec<RigNode>> = HashMap::new();
    let mut variants: HashMap<(bool, bool), usize> = HashMap::new();
    let tex_alpha = texture.is_some_and(|t| t.has_alpha());

    struct Ctx<'a> {
        model: &'a mut Model,
        by_name: &'a mut HashMap<String, Vec<RigNode>>,
        variants: &'a mut HashMap<(bool, bool), usize>,
        texture: Option<&'a Rgba>,
        tex_alpha: bool,
        scale: f32,
        material_name: &'a str,
    }

    fn walk(c: &mut Ctx, n: &crate::blockymodel::Node, parent: Option<usize>, parent_offset: V3) {
        let s = c.scale;
        let rest_pos = [
            n.position.x + parent_offset.x,
            n.position.y + parent_offset.y,
            n.position.z + parent_offset.z,
        ];
        let rest_rot = qnorm(n.orientation.arr());
        let pivot = c.model.nodes.len();
        let mut node = Node::new(n.name.clone(), parent);
        node.translation = rest_pos.map(|v| v * s);
        node.rotation = rest_rot;
        c.model.nodes.push(node);

        let sh = &n.shape;
        let faces = if sh.visible {
            shape_faces(sh)
        } else {
            Vec::new()
        };
        let mut shape = None;
        if !faces.is_empty() {
            let mut sn = Node::new(format!("{}/shape", n.name), Some(pivot));
            sn.translation = sh.offset.arr().map(|v| v * s);
            sn.scale = sh.stretch.arr();
            let shape_idx = c.model.nodes.len();
            c.model.nodes.push(sn);
            shape = Some((shape_idx, sh.stretch.arr()));

            let transparent = faces.iter().any(|f| f.transparent);
            let key = (sh.double_sided, transparent);
            let material = *c.variants.entry(key).or_insert_with(|| {
                let mut m = Material {
                    name: format!(
                        "{}{}{}",
                        c.material_name,
                        if key.0 { " (double sided)" } else { "" },
                        if key.1 { " (transparent)" } else { "" }
                    ),
                    base_color: c.texture.map(to_image),
                    metallic: 0.0,
                    roughness: 1.0,
                    double_sided: key.0,
                    ..Material::default()
                };
                m.alpha = if key.1 {
                    AlphaMode::Blend
                } else if c.tex_alpha {
                    AlphaMode::Mask(0.5)
                } else {
                    AlphaMode::Opaque
                };
                c.model.materials.push(m);
                c.model.materials.len() - 1
            });
            let mut mesh = Mesh {
                name: n.name.clone(),
                node: shape_idx,
                material,
                ..Mesh::default()
            };
            for f in &faces {
                let base = mesh.positions.len() as u32;
                for i in 0..4 {
                    mesh.positions.push(f.positions[i].map(|v| v * s));
                    mesh.normals.push(f.normal);
                    if let Some(t) = c.texture {
                        mesh.uvs.push([
                            f.uv_px[i][0] / t.width as f32,
                            f.uv_px[i][1] / t.height as f32,
                        ]);
                    }
                }
                for tri in FACE_TRIANGLES {
                    mesh.indices.extend(tri.iter().map(|i| base + i));
                }
            }
            c.model.meshes.push(mesh);
        }
        c.by_name.entry(n.name.clone()).or_default().push(RigNode {
            pivot,
            rest_pos,
            rest_rot,
            shape,
        });
        for ch in &n.children {
            walk(c, ch, Some(pivot), sh.offset);
        }
    }

    let mut ctx = Ctx {
        model: &mut model,
        by_name: &mut by_name,
        variants: &mut variants,
        texture,
        tex_alpha,
        scale,
        material_name,
    };
    for n in &bm.nodes {
        walk(&mut ctx, n, None, V3::ZERO);
    }
    Rig {
        model,
        by_name,
        scale,
    }
}

/// What a clip conversion did not carry over
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipReport {
    /// Node names in the clip that the model does not have
    pub missing_nodes: Vec<String>,
    pub dropped_visible_keys: usize,
    pub dropped_uv_keys: usize,
}

struct Track {
    t: f32,
    v: Vec<f32>,
    smooth: bool,
}

fn dedupe(mut tr: Vec<Track>) -> Vec<Track> {
    tr.sort_by(|a, b| a.t.total_cmp(&b.t));
    let mut out: Vec<Track> = Vec::with_capacity(tr.len());
    for k in tr {
        match out.last_mut() {
            Some(last) if last.t == k.t => *last = k,
            _ => out.push(k),
        }
    }
    out
}

/// A translation or scale channel from keyframes. Linear unless a segment is smooth.
fn vec3_channel(node: usize, target: Target, tr: Vec<Track>) -> Option<Channel> {
    let tr = dedupe(tr);
    if tr.is_empty() {
        return None;
    }
    let n = tr.len();
    let seg_smooth = |k: usize| tr[k].smooth || tr[k + 1].smooth;
    let any_smooth = n >= 2 && (0..n - 1).any(seg_smooth);
    let times: Vec<f32> = tr.iter().map(|k| k.t).collect();
    if !any_smooth {
        let values = tr.iter().flat_map(|k| k.v.iter().copied()).collect();
        return Some(Channel::new(
            node,
            target,
            Interpolation::Linear,
            times,
            values,
        ));
    }
    // Hermite tangents per segment; glTF stores them as value per second, scaled by the segment
    // length inside the curve. A smooth segment k uses Catmull-Rom tangents (p[k+1]-p[k-1])/2 and
    // (p[k+2]-p[k])/2 with the ends doubled; a linear one uses the chord.
    let mut values = Vec::with_capacity(n * 9);
    let get = |k: usize, c: usize| tr[k].v[c];
    for k in 0..n {
        let mut a = [0.0f32; 3]; // in-tangent: belongs to segment k-1
        let mut b = [0.0f32; 3]; // out-tangent: belongs to segment k
        for c in 0..3 {
            if k > 0 {
                let dt = tr[k].t - tr[k - 1].t;
                let m = if seg_smooth(k - 1) {
                    let next = if k + 1 < n { k + 1 } else { k };
                    (get(next, c) - get(k - 1, c)) / 2.0
                } else {
                    get(k, c) - get(k - 1, c)
                };
                a[c] = m / dt;
            }
            if k + 1 < n {
                let dt = tr[k + 1].t - tr[k].t;
                let m = if seg_smooth(k) {
                    let prev = if k > 0 { k - 1 } else { k };
                    (get(k + 1, c) - get(prev, c)) / 2.0
                } else {
                    get(k + 1, c) - get(k, c)
                };
                b[c] = m / dt;
            }
        }
        values.extend_from_slice(&a);
        values.extend_from_slice(&tr[k].v);
        values.extend_from_slice(&b);
    }
    Some(Channel::new(
        node,
        target,
        Interpolation::CubicSpline,
        times,
        values,
    ))
}

fn rotation_channel(node: usize, tr: Vec<Track>) -> Option<Channel> {
    let tr = dedupe(tr);
    if tr.is_empty() {
        return None;
    }
    let times: Vec<f32> = tr.iter().map(|k| k.t).collect();
    let mut values: Vec<f32> = Vec::with_capacity(tr.len() * 4);
    for k in &tr {
        let mut q = [k.v[0], k.v[1], k.v[2], k.v[3]];
        // q and -q are the same rotation; keep neighbours in one hemisphere so that spherical
        // interpolation takes the short way
        if values.len() >= 4 {
            let p = &values[values.len() - 4..];
            if p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3] < 0.0 {
                q = q.map(|v| -v);
            }
        }
        values.extend_from_slice(&q);
    }
    Some(Channel::new(
        node,
        Target::Rotation,
        Interpolation::Linear,
        times,
        values,
    ))
}

fn smooth_of<T>(k: &Key<T>) -> bool {
    k.smooth()
}

/// Converts a clip for a rig. Names the clip `name`.
pub fn build_clip(rig: &Rig, anim: &BlockyAnim, name: &str) -> (Animation, ClipReport) {
    let s = rig.scale;
    let mut channels = Vec::new();
    let mut report = ClipReport::default();
    let mut longest = anim.duration_s();
    for (node_name, na) in &anim.node_animations {
        let Some(targets) = rig.by_name.get(node_name) else {
            report.missing_nodes.push(node_name.clone());
            continue;
        };
        report.dropped_visible_keys += na.shape_visible.len();
        report.dropped_uv_keys += na.shape_uv_offset.len();
        for rn in targets {
            if !na.position.is_empty() {
                let tr = na
                    .position
                    .iter()
                    .map(|k| Track {
                        t: k.time / FPS,
                        v: vec![
                            (rn.rest_pos[0] + k.delta.x) * s,
                            (rn.rest_pos[1] + k.delta.y) * s,
                            (rn.rest_pos[2] + k.delta.z) * s,
                        ],
                        smooth: smooth_of(k),
                    })
                    .collect();
                channels.extend(vec3_channel(rn.pivot, Target::Translation, tr));
            }
            if !na.orientation.is_empty() {
                let tr = na
                    .orientation
                    .iter()
                    .map(|k| Track {
                        t: k.time / FPS,
                        v: qnorm(qmul(rn.rest_rot, qnorm(k.delta.arr()))).to_vec(),
                        smooth: smooth_of(k),
                    })
                    .collect();
                channels.extend(rotation_channel(rn.pivot, tr));
            }
            if let (Some((shape, stretch)), false) = (rn.shape, na.shape_stretch.is_empty()) {
                let tr = na
                    .shape_stretch
                    .iter()
                    .map(|k| Track {
                        t: k.time / FPS,
                        v: vec![
                            stretch[0] * k.delta.x,
                            stretch[1] * k.delta.y,
                            stretch[2] * k.delta.z,
                        ],
                        smooth: smooth_of(k),
                    })
                    .collect();
                channels.extend(vec3_channel(shape, Target::Scale, tr));
            }
        }
    }
    for ch in &channels {
        if let Some(&t) = ch.times.last() {
            longest = longest.max(t);
        }
    }
    report.missing_nodes.sort();
    report.missing_nodes.dedup();
    (
        Animation {
            name: name.to_string(),
            duration_s: longest,
            channels,
        },
        report,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockymodel::BlockyModel;

    const MODEL: &str = r#"{"nodes":[
      {"id":"1","name":"Root","position":{"x":0,"y":10,"z":0},"orientation":{"x":0,"y":0,"z":0,"w":1},
       "shape":{"type":"box","offset":{"x":0,"y":5,"z":0},"stretch":{"x":-1,"y":1,"z":1},
                "settings":{"size":{"x":4,"y":10,"z":4}},
                "textureLayout":{"front":{"offset":{"x":0,"y":0},"mirror":{"x":false,"y":false},"angle":0}},
                "visible":true,"doubleSided":false},
       "children":[{"id":"2","name":"Arm","position":{"x":2,"y":3,"z":0},"orientation":{"x":0,"y":0,"z":0,"w":1},
                    "shape":{"type":"none","offset":{"x":0,"y":0,"z":0}}}]}]}"#;

    fn rig() -> Rig {
        let bm = BlockyModel::parse(MODEL.as_bytes()).unwrap();
        let tex = Rgba {
            width: 32,
            height: 32,
            data: vec![255; 32 * 32 * 4],
        };
        build_rig(&bm, "m", 1.0 / 64.0, Some(&tex), "tex")
    }

    #[test]
    fn children_hang_off_the_parent_pivot_plus_its_offset() {
        let r = rig();
        r.model.validate().unwrap();
        let arm = &r.model.nodes[r.by_name["Arm"][0].pivot];
        // (2, 3 + 5 offset, 0) px
        assert_eq!(arm.translation, [2.0 / 64.0, 8.0 / 64.0, 0.0]);
        let root = &r.model.nodes[r.by_name["Root"][0].pivot];
        assert_eq!(root.translation, [0.0, 10.0 / 64.0, 0.0]);
    }

    #[test]
    fn stretch_lives_on_the_shape_node_only() {
        let r = rig();
        let (shape, stretch) = r.by_name["Root"][0].shape.unwrap();
        assert_eq!(stretch, [-1.0, 1.0, 1.0]);
        assert_eq!(r.model.nodes[shape].scale, [-1.0, 1.0, 1.0]);
        assert_eq!(r.model.nodes[shape].translation, [0.0, 5.0 / 64.0, 0.0]);
        assert_eq!(r.model.nodes[r.by_name["Root"][0].pivot].scale, [1.0; 3]);
        assert_eq!(r.model.meshes.len(), 1);
        assert_eq!(r.model.meshes[0].node, shape);
        assert_eq!(r.model.meshes[0].uvs.len(), 4);
    }

    #[test]
    fn clips_add_position_and_compose_rotation_after_rest() {
        let r = rig();
        let json = br#"{"duration":60,"nodeAnimations":{
            "Root":{"position":[{"time":0,"delta":{"x":0,"y":0,"z":0}},{"time":60,"delta":{"x":6,"y":0,"z":0}}],
                    "orientation":[{"time":0,"delta":{"x":0,"y":0,"z":0,"w":1}}]},
            "Ghost":{"position":[{"time":0,"delta":{"x":1,"y":1,"z":1}}]}}}"#;
        let anim = BlockyAnim::parse(json).unwrap();
        let (clip, report) = build_clip(&r, &anim, "t");
        assert_eq!(report.missing_nodes, ["Ghost"]);
        assert_eq!(clip.duration_s, 1.0);
        clip.validate(r.model.nodes.len()).unwrap();
        let pos = clip
            .channels
            .iter()
            .find(|c| c.target == Target::Translation)
            .unwrap();
        assert_eq!(pos.interpolation, Interpolation::Linear);
        assert_eq!(pos.times, [0.0, 1.0]);
        assert_eq!(
            pos.values,
            [0.0, 10.0 / 64.0, 0.0, 6.0 / 64.0, 10.0 / 64.0, 0.0]
        );
    }

    #[test]
    fn smooth_positions_become_cubic_with_catmull_rom_tangents() {
        let tr = |v: [f32; 3], t: f32| Track {
            t,
            v: v.to_vec(),
            smooth: true,
        };
        let ch = vec3_channel(
            0,
            Target::Translation,
            vec![
                tr([0.0; 3], 0.0),
                tr([1.0, 0.0, 0.0], 1.0),
                tr([4.0, 0.0, 0.0], 2.0),
            ],
        )
        .unwrap();
        assert_eq!(ch.interpolation, Interpolation::CubicSpline);
        assert_eq!(ch.values.len(), 27);
        // key 1: in = out = (p2 - p0) / 2 per unit time = 2
        assert_eq!(ch.values[9], 2.0);
        assert_eq!(ch.values[15], 2.0);
        // first key's out-tangent uses the doubled end point: (p1 - p0) / 2
        assert_eq!(ch.values[6], 0.5);
    }

    #[test]
    fn rotations_stay_in_one_hemisphere() {
        let tr = |q: [f32; 4], t: f32| Track {
            t,
            v: q.to_vec(),
            smooth: false,
        };
        let ch = rotation_channel(
            0,
            vec![
                tr([0.0, 0.0, 0.0, 1.0], 0.0),
                tr([0.0, 0.0, 0.0, -1.0], 1.0),
            ],
        )
        .unwrap();
        assert_eq!(&ch.values[4..], [0.0, 0.0, 0.0, 1.0]);
    }
}
