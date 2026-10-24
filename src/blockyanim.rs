//! `.blockyanim`: Hytale's animation clips.
//!
//! JSON again; fields as found in the install (6,736 files) and as the Blockbench plugin reads
//! them (see [`crate::blockymodel`] for the plugin's credit):
//!
//! ```text
//! { "formatVersion": 1,            // 4 files lack it
//!   "duration": 60,                // frames
//!   "holdLastKeyframe": false,     // true: the clip stops on its last keyframe, false: it loops
//!   "nodeAnimations": {
//!     "<node name>": {             // nodes are addressed by name
//!       "position":     [ { "time": 0, "delta": {x,y,z},   "interpolationType": "smooth" } ],
//!       "orientation":  [ { "time": 0, "delta": {x,y,z,w}, "interpolationType": "smooth" } ],
//!       "shapeStretch": [ { "time": 0, "delta": {x,y,z} } ],
//!       "shapeVisible": [ { "time": 0, "delta": true } ],
//!       "shapeUvOffset":[ { "time": 0, "delta": {x,y} } ] } } }
//! ```
//!
//! - Times are **frames at 60 per second**, `duration` too.
//! - `position` is added to the node's rest position (pixels, like the model); `orientation` is
//!   a quaternion composed after the rest orientation (`rest * delta`, which is what the plugin's
//!   preview does with its quaternion interpolation); `shapeStretch` multiplies the shape's
//!   stretch (1,1,1 is the rest value); `shapeVisible` shows or hides the shape; `shapeUvOffset`
//!   slides the shape's texture (pixels).
//! - `interpolationType` is `smooth` or `linear`; a missing one means linear. Before the first
//!   keyframe and after the last, a channel holds its first and last value.
//! - Several nodes of a model can share a name; a clip drives all of them.
//!
//! `crate::bridge` turns these into bridge clips. Smooth position and stretch keyframes become
//! Catmull-Rom splines, as in the plugin's preview (three.js's `SplineCurve` with its end points
//! doubled); rotations are always interpolated spherically between keyframes, as in the plugin
//! for Hytale formats.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::blockymodel::{Quat, V3};

pub const FPS: f32 = 60.0;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BlockyAnim {
    pub format_version: Option<u32>,
    /// Frames
    pub duration: f32,
    pub hold_last_keyframe: bool,
    pub node_animations: BTreeMap<String, NodeAnim>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeAnim {
    pub position: Vec<Key<V3>>,
    pub orientation: Vec<Key<Quat>>,
    pub shape_stretch: Vec<Key<V3>>,
    pub shape_visible: Vec<Key<bool>>,
    pub shape_uv_offset: Vec<Key<Uv>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct Uv {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Key<T> {
    /// Frames
    pub time: f32,
    pub delta: T,
    #[serde(default)]
    pub interpolation_type: Option<String>,
}

impl<T> Key<T> {
    pub fn smooth(&self) -> bool {
        self.interpolation_type.as_deref() == Some("smooth")
    }
}

impl BlockyAnim {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    pub fn duration_s(&self) -> f32 {
        self.duration / FPS
    }

    /// Total keyframes over all channels
    pub fn key_count(&self) -> usize {
        self.node_animations
            .values()
            .map(|n| {
                n.position.len()
                    + n.orientation.len()
                    + n.shape_stretch.len()
                    + n.shape_visible.len()
                    + n.shape_uv_offset.len()
            })
            .sum()
    }

    /// Problems for `check`: empty when the file is fine.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if !self.duration.is_finite() || self.duration < 0.0 {
            p.push(format!("duration {}", self.duration));
        }
        for (name, n) in &self.node_animations {
            let times = |v: Vec<f32>, what: &str, p: &mut Vec<String>| {
                if v.windows(2).any(|w| w[1] < w[0]) {
                    p.push(format!("{name}: {what} keyframes out of order"));
                }
                if v.iter()
                    .any(|t| !t.is_finite() || *t < 0.0 || *t > self.duration)
                {
                    p.push(format!("{name}: {what} keyframe outside 0..duration"));
                }
            };
            times(
                n.position.iter().map(|k| k.time).collect(),
                "position",
                &mut p,
            );
            times(
                n.orientation.iter().map(|k| k.time).collect(),
                "orientation",
                &mut p,
            );
            times(
                n.shape_stretch.iter().map(|k| k.time).collect(),
                "shapeStretch",
                &mut p,
            );
            times(
                n.shape_visible.iter().map(|k| k.time).collect(),
                "shapeVisible",
                &mut p,
            );
            times(
                n.shape_uv_offset.iter().map(|k| k.time).collect(),
                "shapeUvOffset",
                &mut p,
            );
            for k in &n.orientation {
                let q = k.delta;
                let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
                if (len - 1.0).abs() > 1e-2 {
                    p.push(format!("{name}: orientation delta length {len}"));
                    break;
                }
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_clip() {
        let json = br#"{"formatVersion":1,"duration":60,"holdLastKeyframe":true,"nodeAnimations":{
            "Head":{"orientation":[{"time":0,"delta":{"x":0,"y":0,"z":0,"w":1},"interpolationType":"smooth"}],
                    "shapeVisible":[{"time":3,"delta":false}],
                    "position":[{"time":0,"delta":{"x":1,"y":2,"z":3}}]}}}"#;
        let a = BlockyAnim::parse(json).unwrap();
        assert_eq!(a.duration_s(), 1.0);
        assert!(a.hold_last_keyframe);
        let head = &a.node_animations["Head"];
        assert!(head.orientation[0].smooth());
        assert!(!head.position[0].smooth());
        assert!(!head.shape_visible[0].delta);
        assert_eq!(a.key_count(), 3);
        assert!(a.problems().is_empty());
    }

    #[test]
    fn reports_keyframes_outside_the_clip() {
        let json = br#"{"duration":10,"nodeAnimations":{"A":{"position":[{"time":11,"delta":{"x":0,"y":0,"z":0}}]}}}"#;
        assert_eq!(BlockyAnim::parse(json).unwrap().problems().len(), 1);
    }
}
