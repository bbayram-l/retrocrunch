//! Blender bridge + native FBX import, end to end. Skipped when Blender isn't installed.

use glam::{Mat4, Quat, Vec2, Vec3};
use retrocrunch::export::{self, ExportSettings};
use retrocrunch::mesh::{Material, Model, Vertex};
use retrocrunch::rig::{Channel, Clip, Interpolation, Property, Rig, RigNode, Skin};

/// A textured, skinned two-bone column with one bend clip.
fn column() -> Model {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let rings = 8;
    for r in 0..=rings {
        let y = r as f32 / rings as f32 * 2.0;
        let t = (y - 0.8).clamp(0.0, 0.4) / 0.4;
        for s in 0..8 {
            let a = s as f32 / 8.0 * std::f32::consts::TAU;
            let p = Vec3::new(a.cos() * 0.2, y, a.sin() * 0.2);
            vertices.push(Vertex {
                pos: p,
                normal: Vec3::new(a.cos(), 0.0, a.sin()),
                uv: Vec2::new(s as f32 / 8.0, y / 2.0),
                color: [1.0; 4],
                joints: [0, 1, 0, 0],
                weights: [1.0 - t, t, 0.0, 0.0],
            });
        }
    }
    for r in 0..rings {
        for s in 0..8u32 {
            let (a, b) = (r * 8 + s, r * 8 + (s + 1) % 8);
            indices.extend_from_slice(&[a, a + 8, b, b, a + 8, b + 8]);
        }
    }
    let piece = retrocrunch::mesh::Piece {
        name: "column".into(),
        vertices,
        indices,
        material: Some(0),
        skin: Some(0),
        ..Default::default()
    };
    let nodes = vec![
        RigNode {
            name: "base".into(),
            parent: None,
            children: vec![1],
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        },
        RigNode {
            name: "top".into(),
            parent: Some(0),
            children: vec![],
            translation: Vec3::Y,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        },
    ];
    let bend = Quat::from_rotation_z(1.0);
    let rig = Rig {
        nodes,
        roots: vec![0],
        skins: vec![Skin {
            name: "skin".into(),
            joints: vec![0, 1],
            inverse_bind: vec![Mat4::IDENTITY, Mat4::from_translation(-Vec3::Y)],
            skeleton: Some(0),
        }],
        clips: vec![Clip {
            name: "bend".into(),
            channels: vec![Channel {
                node: 1,
                property: Property::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: [Quat::IDENTITY.to_array(), bend.to_array()].concat(),
            }],
        }],
    };
    let tex = image::RgbaImage::from_fn(16, 16, |x, y| {
        image::Rgba([(x * 16) as u8, (y * 16) as u8, 90, 255])
    });
    Model {
        name: "column".into(),
        pieces: vec![piece],
        materials: vec![Material {
            name: "mat".into(),
            albedo: Some(tex),
            ..Default::default()
        }],
        rig: Some(rig),
        ..Default::default()
    }
}

#[test]
fn fbx_export_via_blender_reimports_natively_with_rig_clip_and_texture() {
    if retrocrunch::blender::find_blender().is_none() {
        eprintln!("Blender not installed: skipping");
        return;
    }
    let dir = std::env::temp_dir().join("retrocrunch_fbx_it");
    let _ = std::fs::remove_dir_all(&dir);
    let s = ExportSettings {
        glb: true,
        fbx: true,
        godot_scene: false,
        preview: false,
        ..Default::default()
    };
    let files = export::export_model(&column(), &dir, &s).unwrap();
    let fbx = files
        .iter()
        .find(|f| f.extension().is_some_and(|e| e == "fbx"))
        .expect("FBX written");
    let back = Model::load(fbx).unwrap();
    assert!(back.is_rigged(), "skin survives FBX");
    let r = back.rig.as_ref().unwrap();
    assert!(r.skins[0].joints.len() >= 2);
    assert!(
        r.clips.iter().any(|c| c.duration() > 0.5),
        "bend clip survives FBX"
    );
    assert_eq!(back.tri_count(), column().tri_count());
    assert!(
        back.materials.iter().any(|m| m.albedo.is_some()),
        "texture reaches the FBX"
    );
    // Same pose from FBX and from the original data: the top ring moves by the bend.
    let pose = |m: &Model| -> Vec3 {
        let rig = m.rig.as_ref().unwrap();
        let mats = rig.skin_matrices(&rig.world(&rig.pose(Some(0), 1.0)));
        let p = &m.pieces[0];
        // Centroid of the top ring (vertex order may differ after the FBX round trip).
        let top: Vec<Vec3> = p
            .vertices
            .iter()
            .filter(|v| v.pos.y > 1.99)
            .map(|v| {
                retrocrunch::rig::skin_point(v.pos, v.joints, v.weights, &mats[p.skin.unwrap()])
            })
            .collect();
        assert!(!top.is_empty());
        top.iter().copied().sum::<Vec3>() / top.len() as f32
    };
    let (a, b) = (pose(&column()), pose(&back));
    assert!((a - b).length() < 0.05, "posed top differs: {a} vs {b}");
}

#[test]
fn remesh_first_evens_out_triangles_on_static_models() {
    if retrocrunch::blender::find_blender().is_none() {
        eprintln!("Blender not installed: skipped");
        return;
    }
    // A static, textured, sliver-heavy plank: long thin triangles along its length.
    let mut m = column();
    m.rig = None;
    for p in &mut m.pieces {
        p.skin = None;
    }
    let quality = |m: &Model| {
        let mut q = Vec::new();
        for p in &m.pieces {
            for t in p.indices.chunks_exact(3) {
                let [a, b, c] = [0, 1, 2].map(|k| p.vertices[t[k] as usize].pos);
                let e2 =
                    (b - a).length_squared() + (c - b).length_squared() + (a - c).length_squared();
                q.push(4.0 * 3f32.sqrt() * 0.5 * (b - a).cross(c - a).length() / e2.max(1e-12));
            }
        }
        q.iter().sum::<f32>() / q.len().max(1) as f32
    };
    let ps = retrocrunch::process::PieceSettings::defaults_for(&m);
    let base = retrocrunch::process::ConvertSettings {
        target_tris: 60,
        ..Default::default()
    };
    let plain = retrocrunch::process::convert(&m, &ps, &base, None, &|_, _| {});
    let remeshed = retrocrunch::process::convert(
        &m,
        &ps,
        &retrocrunch::process::ConvertSettings {
            remesh: true,
            ..base.clone()
        },
        None,
        &|_, _| {},
    );
    assert!(
        remeshed.stats.warnings.is_empty(),
        "{:?}",
        remeshed.stats.warnings
    );
    assert!(
        remeshed.stats.atlas.is_some(),
        "textures rebaked into an atlas"
    );
    let (qp, qr) = (quality(&plain.model), quality(&remeshed.model));
    assert!(
        qr > qp,
        "remeshed triangles should be better shaped: {qr} vs {qp}"
    );
    let tris = remeshed.model.tri_count();
    assert!((30..=90).contains(&tris), "budget respected: {tris}");

    // Texture-only changes reuse the remesh; a new budget runs Blender again.
    use std::sync::atomic::Ordering;
    let runs = retrocrunch::blender::REMESH_RUNS.load(Ordering::Relaxed);
    let mut texture_change = retrocrunch::process::ConvertSettings {
        remesh: true,
        ..base.clone()
    };
    texture_change.texture.colors = 4;
    texture_change.texture.max_size = 64;
    let again = retrocrunch::process::convert(&m, &ps, &texture_change, None, &|_, _| {});
    assert_eq!(
        retrocrunch::blender::REMESH_RUNS.load(Ordering::Relaxed),
        runs,
        "cached"
    );
    assert_eq!(again.model.tri_count(), remeshed.model.tri_count());
    let budget_change = retrocrunch::process::ConvertSettings {
        remesh: true,
        target_tris: 40,
        ..base.clone()
    };
    retrocrunch::process::convert(&m, &ps, &budget_change, None, &|_, _| {});
    assert_eq!(
        retrocrunch::blender::REMESH_RUNS.load(Ordering::Relaxed),
        runs + 1,
        "new budget remeshes"
    );
}
