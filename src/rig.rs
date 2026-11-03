//! Skeletons, skins and animation clips carried through the pipeline (glTF semantics).
//!
//! Skinned vertex positions are kept in bind space. Rest/animated poses are evaluated
//! on the CPU for preview: `skin_matrix[j] = world(joint j) * inverse_bind[j]`.

use glam::{Mat4, Quat, Vec3};

#[derive(Clone, Debug)]
pub struct RigNode {
    pub name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl RigNode {
    pub fn local(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Clone, Debug)]
pub struct Skin {
    pub name: String,
    /// Rig node index per joint.
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
    pub skeleton: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
    /// Morph-target weights; the value is the number of targets.
    /// For these channels `Channel::node` is a piece `morph_group`, not a rig node.
    Weights(usize),
}

impl Property {
    pub fn components(self) -> usize {
        match self {
            Self::Rotation => 4,
            Self::Weights(n) => n.max(1),
            _ => 3,
        }
    }

    pub fn gltf_path(self) -> &'static str {
        match self {
            Self::Translation => "translation",
            Self::Rotation => "rotation",
            Self::Scale => "scale",
            Self::Weights(_) => "weights",
        }
    }

    pub fn is_transform(self) -> bool {
        !matches!(self, Self::Weights(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Step,
    CubicSpline,
}

impl Interpolation {
    pub fn gltf_name(self) -> &'static str {
        match self {
            Self::Linear => "LINEAR",
            Self::Step => "STEP",
            Self::CubicSpline => "CUBICSPLINE",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub node: usize,
    pub property: Property,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// Flattened keyframe values. CUBICSPLINE stores (in-tangent, value, out-tangent) per key.
    pub values: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub channels: Vec<Channel>,
}

impl Clip {
    pub fn duration(&self) -> f32 {
        self.channels
            .iter()
            .filter_map(|c| c.times.last().copied())
            .fold(0.0, f32::max)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Rig {
    pub nodes: Vec<RigNode>,
    pub roots: Vec<usize>,
    pub skins: Vec<Skin>,
    pub clips: Vec<Clip>,
}

type Trs = (Vec3, Quat, Vec3);

impl Channel {
    /// Sample at time `t` (clamped to the key range); returns `components()` values.
    pub fn sample_vec(&self, t: f32) -> Vec<f32> {
        let comps = self.property.components();
        let cubic = self.interpolation == Interpolation::CubicSpline;
        let (stride, slot) = if cubic { (3, 1) } else { (1, 0) };
        let n = self.times.len();
        let read = |key: usize, slot: usize| -> &[f32] {
            let base = (key * stride + slot) * comps;
            &self.values[base..base + comps]
        };
        let rotation = self.property == Property::Rotation;
        if n == 0 || self.values.len() < n * stride * comps {
            return if rotation {
                vec![0.0, 0.0, 0.0, 1.0]
            } else {
                vec![0.0; comps]
            };
        }
        if n == 1 || t <= self.times[0] {
            return read(0, slot).to_vec();
        }
        if t >= self.times[n - 1] {
            return read(n - 1, slot).to_vec();
        }
        let k = self
            .times
            .partition_point(|&x| x <= t)
            .saturating_sub(1)
            .min(n - 2);
        let (t0, t1) = (self.times[k], self.times[k + 1]);
        let dt = (t1 - t0).max(1e-9);
        let u = (t - t0) / dt;
        let (a, b) = (read(k, slot), read(k + 1, slot));
        match self.interpolation {
            Interpolation::Step => a.to_vec(),
            Interpolation::Linear if rotation => {
                let q = Quat::from_slice(a).slerp(Quat::from_slice(b), u);
                q.normalize().to_array().to_vec()
            }
            Interpolation::Linear => a.iter().zip(b).map(|(x, y)| x + (y - x) * u).collect(),
            Interpolation::CubicSpline => {
                let (out_tan, in_tan) = (read(k, 2), read(k + 1, 0));
                let (u2, u3) = (u * u, u * u * u);
                let (h00, h10, h01, h11) = (
                    2.0 * u3 - 3.0 * u2 + 1.0,
                    u3 - 2.0 * u2 + u,
                    -2.0 * u3 + 3.0 * u2,
                    u3 - u2,
                );
                let v: Vec<f32> = (0..comps)
                    .map(|i| h00 * a[i] + h10 * dt * out_tan[i] + h01 * b[i] + h11 * dt * in_tan[i])
                    .collect();
                if rotation {
                    Quat::from_slice(&v).normalize().to_array().to_vec()
                } else {
                    v
                }
            }
        }
    }

    /// Transform channels: the first 4 components (unused ones are 0).
    pub fn sample(&self, t: f32) -> [f32; 4] {
        let v = self.sample_vec(t);
        std::array::from_fn(|i| v.get(i).copied().unwrap_or(0.0))
    }
}

impl Rig {
    pub fn joint_count(&self) -> usize {
        self.skins.iter().map(|s| s.joints.len()).sum()
    }

    /// Local TRS of every node: rest pose, overridden by `clip` at time `t`.
    pub fn pose(&self, clip: Option<usize>, t: f32) -> Vec<Trs> {
        let mut trs: Vec<Trs> = self
            .nodes
            .iter()
            .map(|n| (n.translation, n.rotation, n.scale))
            .collect();
        if let Some(c) = clip.and_then(|c| self.clips.get(c)) {
            for ch in c.channels.iter().filter(|c| c.property.is_transform()) {
                let Some(slot) = trs.get_mut(ch.node) else {
                    continue;
                };
                let v = ch.sample(t);
                match ch.property {
                    Property::Translation => slot.0 = Vec3::new(v[0], v[1], v[2]),
                    Property::Rotation => slot.1 = Quat::from_array(v).normalize(),
                    Property::Scale => slot.2 = Vec3::new(v[0], v[1], v[2]),
                    Property::Weights(_) => {}
                }
            }
        }
        trs
    }

    /// Morph weights of `group` at time `t`, if the clip animates them.
    pub fn morph_weights(&self, clip: Option<usize>, t: f32, group: usize) -> Option<Vec<f32>> {
        let c = clip.and_then(|c| self.clips.get(c))?;
        c.channels
            .iter()
            .find(|ch| matches!(ch.property, Property::Weights(_)) && ch.node == group)
            .map(|ch| ch.sample_vec(t))
    }

    /// Nodes moved by any transform channel of any clip.
    pub fn animated_nodes(&self) -> std::collections::HashSet<usize> {
        self.clips
            .iter()
            .flat_map(|c| c.channels.iter())
            .filter(|ch| ch.property.is_transform())
            .map(|ch| ch.node)
            .collect()
    }

    /// The (skin, joint slot) a static mesh under rig node `node` should follow:
    /// its nearest joint ancestor, else its nearest animated ancestor, which becomes a
    /// joint of the synthetic rigid skin `RIGID_SKIN`. None = the mesh stays static.
    pub fn follow_target(&mut self, node: usize) -> Option<(usize, usize)> {
        if let Some(j) = self.joint_ancestor(node) {
            return Some(j);
        }
        let animated = self.animated_nodes();
        let mut cur = Some(node);
        let mut guard = 0;
        while let Some(n) = cur {
            if animated.contains(&n) {
                let rest = self.world(&self.pose(None, 0.0));
                let si = match self.skins.iter().position(|s| s.name == RIGID_SKIN) {
                    Some(i) => i,
                    None => {
                        self.skins.push(Skin {
                            name: RIGID_SKIN.into(),
                            joints: Vec::new(),
                            inverse_bind: Vec::new(),
                            skeleton: None,
                        });
                        self.skins.len() - 1
                    }
                };
                let skin = &mut self.skins[si];
                skin.joints.push(n);
                skin.inverse_bind.push(rest[n].inverse());
                return Some((si, skin.joints.len() - 1));
            }
            cur = self.nodes.get(n).and_then(|x| x.parent);
            guard += 1;
            if guard > 512 {
                break;
            }
        }
        None
    }

    /// World matrices for a pose (parents are resolved recursively, any node order).
    pub fn world(&self, pose: &[Trs]) -> Vec<Mat4> {
        let mut world: Vec<Option<Mat4>> = vec![None; self.nodes.len()];
        fn resolve(
            i: usize,
            rig: &Rig,
            pose: &[Trs],
            world: &mut Vec<Option<Mat4>>,
            depth: usize,
        ) -> Mat4 {
            if let Some(m) = world[i] {
                return m;
            }
            let (t, r, s) = pose[i];
            let local = Mat4::from_scale_rotation_translation(s, r, t);
            let m = match rig.nodes[i].parent {
                Some(p) if depth < 512 => resolve(p, rig, pose, world, depth + 1) * local,
                _ => local,
            };
            world[i] = Some(m);
            m
        }
        (0..self.nodes.len())
            .map(|i| resolve(i, self, pose, &mut world, 0))
            .collect()
    }

    /// Per-skin joint matrices for a pose.
    pub fn skin_matrices(&self, world: &[Mat4]) -> Vec<Vec<Mat4>> {
        self.skins
            .iter()
            .map(|s| {
                s.joints
                    .iter()
                    .zip(&s.inverse_bind)
                    .map(|(&j, ibm)| world.get(j).copied().unwrap_or(Mat4::IDENTITY) * *ibm)
                    .collect()
            })
            .collect()
    }

    /// Uniform scale of the whole rig (node translations, bind matrices, translation keys).
    pub fn scale(&mut self, s: f32) {
        for n in &mut self.nodes {
            n.translation *= s;
        }
        for sk in &mut self.skins {
            for m in &mut sk.inverse_bind {
                m.w_axis.x *= s;
                m.w_axis.y *= s;
                m.w_axis.z *= s;
            }
        }
        for c in &mut self.clips {
            for ch in c
                .channels
                .iter_mut()
                .filter(|ch| ch.property == Property::Translation)
            {
                ch.values.iter_mut().for_each(|v| *v *= s);
            }
        }
    }

    /// Nearest ancestor of `node` (inclusive) that is a joint of some skin: (skin, joint slot).
    pub fn joint_ancestor(&self, node: usize) -> Option<(usize, usize)> {
        let mut cur = Some(node);
        let mut guard = 0;
        while let Some(n) = cur {
            for (si, s) in self.skins.iter().enumerate() {
                if let Some(j) = s.joints.iter().position(|&x| x == n) {
                    return Some((si, j));
                }
            }
            cur = self.nodes.get(n).and_then(|x| x.parent);
            guard += 1;
            if guard > 512 {
                break;
            }
        }
        None
    }
}

/// Name of the skin psx-forge creates for node-animated (non-skinned) meshes.
pub const RIGID_SKIN: &str = "psx_rigid";

/// Normalise up to 4 influences; zero-weight slots get joint 0.
pub fn normalize_influences(joints: [u16; 4], weights: [f32; 4]) -> ([u16; 4], [f32; 4]) {
    let sum: f32 = weights.iter().map(|w| w.max(0.0)).sum();
    if sum <= 1e-8 {
        return ([joints[0], 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    }
    let mut j = joints;
    let mut w = weights.map(|w| w.max(0.0) / sum);
    for k in 0..4 {
        if w[k] == 0.0 {
            j[k] = 0;
        }
    }
    // Keep the largest influence first (handy for dominant-joint checks).
    let mut order = [0usize, 1, 2, 3];
    order.sort_by(|&a, &b| w[b].total_cmp(&w[a]));
    let (j2, w2) = (order.map(|o| j[o]), order.map(|o| w[o]));
    w = w2;
    (j2, w)
}

/// Keep the 4 strongest of an arbitrary list of (joint, weight) pairs.
pub fn top4(pairs: &[(u16, f32)]) -> ([u16; 4], [f32; 4]) {
    let mut merged: Vec<(u16, f32)> = Vec::with_capacity(pairs.len());
    for &(j, w) in pairs {
        if w <= 0.0 {
            continue;
        }
        match merged.iter_mut().find(|(mj, _)| *mj == j) {
            Some(e) => e.1 += w,
            None => merged.push((j, w)),
        }
    }
    merged.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut joints = [0u16; 4];
    let mut weights = [0f32; 4];
    for (k, (j, w)) in merged.into_iter().take(4).enumerate() {
        joints[k] = j;
        weights[k] = w;
    }
    normalize_influences(joints, weights)
}

/// Influences of a point between two vertices (used by subdivision).
pub fn blend_influences(
    a: ([u16; 4], [f32; 4]),
    b: ([u16; 4], [f32; 4]),
    t: f32,
) -> ([u16; 4], [f32; 4]) {
    let mut pairs = Vec::with_capacity(8);
    for k in 0..4 {
        pairs.push((a.0[k], a.1[k] * (1.0 - t)));
        pairs.push((b.0[k], b.1[k] * t));
    }
    top4(&pairs)
}

/// Linear blend skinning of one position.
#[inline]
pub fn skin_point(p: Vec3, joints: [u16; 4], weights: [f32; 4], mats: &[Mat4]) -> Vec3 {
    let mut out = Vec3::ZERO;
    let mut total = 0.0;
    for k in 0..4 {
        let w = weights[k];
        if w <= 0.0 {
            continue;
        }
        if let Some(m) = mats.get(joints[k] as usize) {
            out += m.transform_point3(p) * w;
            total += w;
        }
    }
    if total > 0.0 { out / total } else { p }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Two-bone chain along +Y with a 90° bend clip on the second joint.
    pub fn two_bone_rig() -> Rig {
        let nodes = vec![
            RigNode {
                name: "root".into(),
                parent: None,
                children: vec![1],
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            RigNode {
                name: "tip".into(),
                parent: Some(0),
                children: vec![],
                translation: Vec3::Y,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ];
        let skins = vec![Skin {
            name: "skin".into(),
            joints: vec![0, 1],
            inverse_bind: vec![Mat4::IDENTITY, Mat4::from_translation(-Vec3::Y)],
            skeleton: Some(0),
        }];
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let clips = vec![Clip {
            name: "bend".into(),
            channels: vec![Channel {
                node: 1,
                property: Property::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: [Quat::IDENTITY.to_array(), q.to_array()].concat(),
            }],
        }];
        Rig {
            nodes,
            roots: vec![0],
            skins,
            clips,
        }
    }

    #[test]
    fn bind_pose_skinning_is_identity() {
        let rig = two_bone_rig();
        let mats = rig.skin_matrices(&rig.world(&rig.pose(None, 0.0)));
        let p = Vec3::new(0.3, 1.7, 0.1);
        assert!((skin_point(p, [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], &mats[0]) - p).length() < 1e-5);
    }

    #[test]
    fn animation_bends_tip_joint() {
        let rig = two_bone_rig();
        let mats = rig.skin_matrices(&rig.world(&rig.pose(Some(0), 1.0)));
        // A point one unit above the tip joint rotates 90° about Z around (0,1,0).
        let p = skin_point(
            Vec3::new(0.0, 2.0, 0.0),
            [1, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
            &mats[0],
        );
        assert!((p - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-4, "{p}");
        // Half way: 45 degrees.
        let mats = rig.skin_matrices(&rig.world(&rig.pose(Some(0), 0.5)));
        let p = skin_point(
            Vec3::new(0.0, 2.0, 0.0),
            [1, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
            &mats[0],
        );
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!((p - Vec3::new(-s, 1.0 + s, 0.0)).length() < 1e-4, "{p}");
    }

    #[test]
    fn step_and_cubic_sampling() {
        let mut ch = Channel {
            node: 0,
            property: Property::Translation,
            interpolation: Interpolation::Step,
            times: vec![0.0, 1.0],
            values: vec![0., 0., 0., 2., 0., 0.],
        };
        assert_eq!(ch.sample(0.9)[0], 0.0);
        assert_eq!(ch.sample(1.5)[0], 2.0);
        // Cubic with zero tangents: smoothstep between keys.
        ch.interpolation = Interpolation::CubicSpline;
        ch.values = vec![
            0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 2., 0., 0., 0., 0., 0.,
        ];
        assert!((ch.sample(0.5)[0] - 1.0).abs() < 1e-5);
        assert!(ch.sample(0.25)[0] < 0.5);
    }

    #[test]
    fn influence_blending_keeps_four_normalised() {
        let a = ([1, 2, 3, 4], [0.4, 0.3, 0.2, 0.1]);
        let b = ([5, 6, 1, 0], [0.7, 0.3, 0.0, 0.0]);
        let (j, w) = blend_influences(a, b, 0.5);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(j[0], 5);
        assert!(w.windows(2).all(|p| p[0] >= p[1]));
    }

    #[test]
    fn animated_node_without_skin_gets_rigid_skin() {
        let mut rig = two_bone_rig();
        rig.skins.clear();
        // Tip is animated (bend clip) but no skin exists yet.
        let (si, j) = rig
            .follow_target(1)
            .expect("animated node must be followed");
        assert_eq!(rig.skins[si].name, RIGID_SKIN);
        assert_eq!(rig.skins[si].joints[j], 1);
        // At rest the rigid joint's skin matrix is identity (meshes stay in rest-world).
        let rest = rig.skin_matrices(&rig.world(&rig.pose(None, 0.0)));
        assert!(rest[si][j].abs_diff_eq(Mat4::IDENTITY, 1e-5));
        // The root is not animated and has no animated ancestor: stays static.
        assert_eq!(rig.follow_target(0), None);
        // Second mesh under the same node reuses the joint slot list (no new skin).
        let again = rig.follow_target(1).unwrap();
        assert_eq!(again.0, si);
    }

    #[test]
    fn weights_channel_samples_all_targets() {
        let ch = Channel {
            node: 7,
            property: Property::Weights(3),
            interpolation: Interpolation::Linear,
            times: vec![0.0, 2.0],
            values: vec![0.0, 1.0, 0.0, 1.0, 0.0, 0.5],
        };
        let v = ch.sample_vec(1.0);
        assert_eq!(v.len(), 3);
        assert!(
            (v[0] - 0.5).abs() < 1e-6 && (v[1] - 0.5).abs() < 1e-6 && (v[2] - 0.25).abs() < 1e-6
        );
        let rig = Rig {
            clips: vec![Clip {
                name: "c".into(),
                channels: vec![ch],
            }],
            ..Default::default()
        };
        assert_eq!(
            rig.morph_weights(Some(0), 2.0, 7).unwrap(),
            vec![1.0, 0.0, 0.5]
        );
        assert!(rig.morph_weights(Some(0), 2.0, 8).is_none());
        // Weight channels never touch node transforms.
        assert!(rig.pose(Some(0), 1.0).is_empty());
    }

    #[test]
    fn rig_scale_keeps_skinning_consistent() {
        let mut rig = two_bone_rig();
        rig.scale(2.0);
        let mats = rig.skin_matrices(&rig.world(&rig.pose(Some(0), 1.0)));
        let p = skin_point(
            Vec3::new(0.0, 4.0, 0.0),
            [1, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
            &mats[0],
        );
        assert!((p - Vec3::new(-2.0, 2.0, 0.0)).length() < 1e-4, "{p}");
    }
}
