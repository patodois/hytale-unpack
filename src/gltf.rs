//! A small glTF 2.0 writer for rewrite-bridge types.
//!
//! [`write_animated`] writes a model with its clips as glTF nodes, meshes, materials and
//! animations. [`write_scene`] writes a scene whose models are inline: each distinct mesh is
//! stored once and every instance is a node that references it, with the instance transform
//! folded into the mesh's own node transform. Images are PNG files next to the `.gltf`, shared
//! between materials that use identical pixels.
//!
//! Textures use nearest-neighbour sampling and clamp-to-edge wrapping, because the textures are
//! pixel art and the Blockbench plugin's default wrap for Hytale formats is clamp.
//!
//! Output is `<name>.gltf`, `<name>.bin` and `<name>_<n>.png`.

use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use rewrite_bridge::{AlphaMode, AnimatedModel, Interpolation, Model, Scene, SceneModel, Target, Transform};
use serde_json::{json, Value};

use crate::image::encode_png;

pub type Mat4 = [f32; 16]; // column-major

pub fn mat_identity() -> Mat4 {
    [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    o
}

/// `T * R * S`
pub fn mat_from_trs(t: [f32; 3], q: [f32; 4], s: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz, xy, xz, yz, wx, wy, wz) = (x * x, y * y, z * z, x * y, x * z, y * z, w * x, w * y, w * z);
    let r = [
        [1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy)],
        [2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx)],
        [2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy)],
    ];
    let mut m = mat_identity();
    for c in 0..3 {
        for row in 0..3 {
            m[c * 4 + row] = r[c][row] * s[c];
        }
    }
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

/// Splits `T * R * S` back into parts; a negative determinant goes into the x scale.
pub fn mat_to_trs(m: &Mat4) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let col = |c: usize| [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]];
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let (c0, c1, c2) = (col(0), col(1), col(2));
    let mut s = [len(c0), len(c1), len(c2)];
    let cross = [c0[1] * c1[2] - c0[2] * c1[1], c0[2] * c1[0] - c0[0] * c1[2], c0[0] * c1[1] - c0[1] * c1[0]];
    if cross[0] * c2[0] + cross[1] * c2[1] + cross[2] * c2[2] < 0.0 {
        s[0] = -s[0];
    }
    let n = |v: [f32; 3], d: f32| if d == 0.0 { [0.0; 3] } else { [v[0] / d, v[1] / d, v[2] / d] };
    let (r0, r1, r2) = (n(c0, s[0]), n(c1, s[1]), n(c2, s[2]));
    // rotation matrix with columns r0, r1, r2 to quaternion
    let tr = r0[0] + r1[1] + r2[2];
    let q = if tr > 0.0 {
        let k = (tr + 1.0).sqrt() * 2.0;
        [(r1[2] - r2[1]) / k, (r2[0] - r0[2]) / k, (r0[1] - r1[0]) / k, 0.25 * k]
    } else if r0[0] > r1[1] && r0[0] > r2[2] {
        let k = (1.0 + r0[0] - r1[1] - r2[2]).sqrt() * 2.0;
        [0.25 * k, (r1[0] + r0[1]) / k, (r2[0] + r0[2]) / k, (r1[2] - r2[1]) / k]
    } else if r1[1] > r2[2] {
        let k = (1.0 + r1[1] - r0[0] - r2[2]).sqrt() * 2.0;
        [(r1[0] + r0[1]) / k, 0.25 * k, (r2[1] + r1[2]) / k, (r2[0] - r0[2]) / k]
    } else {
        let k = (1.0 + r2[2] - r0[0] - r1[1]).sqrt() * 2.0;
        [(r2[0] + r0[2]) / k, (r2[1] + r1[2]) / k, 0.25 * k, (r0[1] - r1[0]) / k]
    };
    ([m[12], m[13], m[14]], crate::rig::qnorm(q), s)
}

pub fn transform_matrix(t: &Transform) -> Mat4 {
    mat_from_trs(t.translation, t.rotation, t.scale)
}

/// World matrices of a model's nodes
pub fn world_matrices(model: &Model) -> Vec<Mat4> {
    let mut w: Vec<Mat4> = Vec::with_capacity(model.nodes.len());
    for n in &model.nodes {
        let local = mat_from_trs(n.translation, n.rotation, n.scale);
        w.push(match n.parent {
            Some(p) => mat_mul(&w[p], &local),
            None => local,
        });
    }
    w
}

#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    meshes: Vec<Value>,
    nodes: Vec<Value>,
    materials: Vec<Value>,
    textures: Vec<Value>,
    images: Vec<Value>,
    image_files: Vec<(String, Vec<u8>)>,
    image_ids: HashMap<(u32, u32, u64), usize>,
    animations: Vec<Value>,
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(&mut self, bytes: &[u8], target: Option<u32>, component: u32, count: usize, ty: &str, minmax: Option<([f32; 3], [f32; 3])>) -> usize {
        let view = self.view(bytes, target);
        let mut a = json!({"bufferView": view, "componentType": component, "count": count, "type": ty});
        if let Some((lo, hi)) = minmax {
            a["min"] = json!(lo);
            a["max"] = json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn floats(&mut self, data: &[f32], count: usize, ty: &str) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.accessor(&bytes, None, 5126, count, ty, None)
    }

    fn image(&mut self, stem: &str, img: &rewrite_bridge::Image) -> Result<usize> {
        let key = (img.width, img.height, hash_bytes(&img.rgba));
        if let Some(&t) = self.image_ids.get(&key) {
            return Ok(t);
        }
        let n = self.image_files.len();
        let file = format!("{stem}_{n}.png");
        self.image_files.push((file.clone(), encode_png(img.width, img.height, &img.rgba)?));
        self.images.push(json!({"uri": file}));
        self.textures.push(json!({"sampler": 0, "source": self.images.len() - 1}));
        let tex = self.textures.len() - 1;
        self.image_ids.insert(key, tex);
        Ok(tex)
    }

    /// Adds a model's materials and meshes. Returns (material base, mesh index per model mesh).
    fn add_model_data(&mut self, stem: &str, model: &Model) -> Result<(Vec<usize>, Vec<usize>)> {
        let mut mats = Vec::new();
        for m in &model.materials {
            let mut pbr = json!({"baseColorFactor": m.base_color_factor, "metallicFactor": m.metallic, "roughnessFactor": m.roughness});
            if let Some(img) = &m.base_color {
                let tex = self.image(stem, img)?;
                pbr["baseColorTexture"] = json!({"index": tex});
            }
            let mut mat = json!({"name": m.name, "pbrMetallicRoughness": pbr, "doubleSided": m.double_sided});
            match m.alpha {
                AlphaMode::Opaque => {}
                AlphaMode::Mask(c) => {
                    mat["alphaMode"] = json!("MASK");
                    mat["alphaCutoff"] = json!(c);
                }
                AlphaMode::Blend => mat["alphaMode"] = json!("BLEND"),
            }
            if m.emissive_factor != [0.0; 3] {
                mat["emissiveFactor"] = json!(m.emissive_factor.map(|f| f.min(1.0)));
            }
            self.materials.push(mat);
            mats.push(self.materials.len() - 1);
        }
        let mut meshes = Vec::new();
        for mesh in &model.meshes {
            let n = mesh.positions.len();
            let flat: Vec<f32> = mesh.positions.iter().flatten().copied().collect();
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &mesh.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let bytes: Vec<u8> = flat.iter().flat_map(|f| f.to_le_bytes()).collect();
            let pos = self.accessor(&bytes, Some(34962), 5126, n, "VEC3", Some((lo, hi)));
            let mut attrs = json!({"POSITION": pos});
            if !mesh.normals.is_empty() {
                let flat: Vec<f32> = mesh.normals.iter().flatten().copied().collect();
                let bytes: Vec<u8> = flat.iter().flat_map(|f| f.to_le_bytes()).collect();
                attrs["NORMAL"] = json!(self.accessor(&bytes, Some(34962), 5126, n, "VEC3", None));
            }
            if !mesh.uvs.is_empty() {
                let flat: Vec<f32> = mesh.uvs.iter().flatten().copied().collect();
                let bytes: Vec<u8> = flat.iter().flat_map(|f| f.to_le_bytes()).collect();
                attrs["TEXCOORD_0"] = json!(self.accessor(&bytes, Some(34962), 5126, n, "VEC2", None));
            }
            let bytes: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let idx = self.accessor(&bytes, Some(34963), 5125, mesh.indices.len(), "SCALAR", None);
            self.meshes.push(json!({"name": mesh.name, "primitives": [{"attributes": attrs, "indices": idx, "material": mats[mesh.material], "mode": 4}]}));
            meshes.push(self.meshes.len() - 1);
        }
        Ok((mats, meshes))
    }

    fn finish(mut self, name: &str, out_dir: &Path, scene_nodes: Vec<usize>) -> Result<PathBuf> {
        fs::create_dir_all(out_dir)?;
        for (file, bytes) in &self.image_files {
            fs::write(out_dir.join(file), bytes)?;
        }
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let bin_name = format!("{name}.bin");
        fs::write(out_dir.join(&bin_name), &self.bin)?;
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "hytale-unpack"},
            "scene": 0,
            "scenes": [{"nodes": scene_nodes}],
            "nodes": self.nodes,
            "buffers": [{"uri": bin_name, "byteLength": self.bin.len()}],
        });
        let obj = doc.as_object_mut().unwrap();
        let mut put = |k: &str, v: Vec<Value>| {
            if !v.is_empty() {
                obj.insert(k.to_string(), Value::Array(v));
            }
        };
        put("bufferViews", self.views);
        put("accessors", self.accessors);
        put("meshes", self.meshes);
        put("materials", self.materials);
        put("textures", self.textures);
        put("images", self.images);
        put("animations", self.animations);
        if !self.image_files.is_empty() {
            // 9728 NEAREST, 33071 CLAMP_TO_EDGE
            obj.insert("samplers".into(), json!([{"magFilter": 9728, "minFilter": 9728, "wrapS": 33071, "wrapT": 33071}]));
        }
        let path = out_dir.join(format!("{name}.gltf"));
        fs::write(&path, serde_json::to_vec_pretty(&doc)?)?;
        Ok(path)
    }
}

fn node_json(n: &rewrite_bridge::Node, children: &[usize]) -> Value {
    let mut v = json!({"name": n.name});
    if n.translation != [0.0; 3] {
        v["translation"] = json!(n.translation);
    }
    if n.rotation != [0.0, 0.0, 0.0, 1.0] {
        v["rotation"] = json!(n.rotation);
    }
    if n.scale != [1.0; 3] {
        v["scale"] = json!(n.scale);
    }
    if !children.is_empty() {
        v["children"] = json!(children);
    }
    v
}

/// Writes a model with its clips. Returns the `.gltf` path.
pub fn write_animated(am: &AnimatedModel, name: &str, out_dir: &Path) -> Result<PathBuf> {
    am.validate()?;
    let mut b = Builder::default();
    let model = &am.model;
    let (_, meshes) = b.add_model_data(name, model)?;
    let mut children: Vec<Vec<usize>> = vec![vec![]; model.nodes.len()];
    let mut roots = Vec::new();
    for (i, n) in model.nodes.iter().enumerate() {
        match n.parent {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }
    for (i, n) in model.nodes.iter().enumerate() {
        b.nodes.push(node_json(n, &children[i]));
    }
    for (mi, mesh) in model.meshes.iter().enumerate() {
        b.nodes[mesh.node]["mesh"] = json!(meshes[mi]);
        if b.nodes[mesh.node].get("mesh").is_some() && model.meshes.iter().filter(|m| m.node == mesh.node).count() > 1 {
            bail!("node {} carries more than one mesh", mesh.node);
        }
    }
    for clip in &am.animations {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for ch in &clip.channels {
            let input = {
                let bytes: Vec<u8> = ch.times.iter().flat_map(|f| f.to_le_bytes()).collect();
                let lo = ch.times.iter().cloned().fold(f32::MAX, f32::min);
                let hi = ch.times.iter().cloned().fold(f32::MIN, f32::max);
                let a = b.accessor(&bytes, None, 5126, ch.times.len(), "SCALAR", None);
                b.accessors[a]["min"] = json!([lo]);
                b.accessors[a]["max"] = json!([hi]);
                a
            };
            let comps = ch.target.components();
            let per = if ch.interpolation == Interpolation::CubicSpline { 3 } else { 1 };
            let output = b.floats(&ch.values, ch.times.len() * per, if comps == 4 { "VEC4" } else { "VEC3" });
            let interp = match ch.interpolation {
                Interpolation::Step => "STEP",
                Interpolation::Linear => "LINEAR",
                Interpolation::CubicSpline => "CUBICSPLINE",
            };
            samplers.push(json!({"input": input, "output": output, "interpolation": interp}));
            let path = match ch.target {
                Target::Translation => "translation",
                Target::Rotation => "rotation",
                Target::Scale => "scale",
                _ => bail!("unsupported animation target"),
            };
            channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": ch.node, "path": path}}));
        }
        if !channels.is_empty() {
            b.animations.push(json!({"name": clip.name, "samplers": samplers, "channels": channels}));
        }
    }
    b.finish(name, out_dir, roots)
}

/// Writes a scene made of inline models. Returns the `.gltf` path.
pub fn write_scene(scene: &Scene, name: &str, out_dir: &Path) -> Result<PathBuf> {
    scene.validate()?;
    let mut b = Builder::default();
    // per model: for each mesh, (gltf mesh index, mesh world matrix)
    let mut per_model: Vec<Vec<(usize, Mat4)>> = Vec::new();
    for m in &scene.models {
        let SceneModel::Inline(model) = m else {
            bail!("write_scene needs inline models");
        };
        let (_, meshes) = b.add_model_data(name, model)?;
        let world = world_matrices(model);
        per_model.push(model.meshes.iter().enumerate().map(|(i, mesh)| (meshes[i], world[mesh.node])).collect());
    }
    let mut roots = Vec::new();
    for inst in &scene.instances {
        let im = transform_matrix(&inst.transform);
        for (mesh, w) in &per_model[inst.model] {
            let (t, r, s) = mat_to_trs(&mat_mul(&im, w));
            let mut v = json!({"mesh": mesh, "translation": t});
            if r != [0.0, 0.0, 0.0, 1.0] {
                v["rotation"] = json!(r);
            }
            if s != [1.0; 3] {
                v["scale"] = json!(s);
            }
            b.nodes.push(v);
            roots.push(b.nodes.len() - 1);
        }
    }
    b.finish(name, out_dir, roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trs_round_trips_through_a_matrix() {
        let q = crate::rig::qnorm([0.2, -0.4, 0.1, 0.8]);
        for s in [[1.0, 2.0, 3.0], [-1.0, 2.0, 0.5], [1.0, 1.0, -2.0]] {
            let m = mat_from_trs([1.0, 2.0, 3.0], q, s);
            let (t, r2, s2) = mat_to_trs(&m);
            let m2 = mat_from_trs(t, r2, s2);
            for i in 0..16 {
                assert!((m[i] - m2[i]).abs() < 1e-5, "{s:?} element {i}");
            }
        }
    }

    #[test]
    fn matrices_compose_parent_first() {
        let a = mat_from_trs([1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [2.0; 3]);
        let b = mat_from_trs([1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        // child translated by 1 inside a parent scaled by 2 and moved by 1
        assert_eq!(mat_mul(&a, &b)[12], 3.0);
    }
}
