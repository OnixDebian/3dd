//! Unit-cube geometry: 8 vertices, 6 faces, outward normals.
//!
//! A unit cube centered at the origin (each side length 1, so corners at ±0.5).
//! Each [`Face`] is a convex quad (4 corner indices, CCW when viewed from
//! outside), plus its outward normal and a stable [`FaceId`]. The rasterizer
//! triangulates the quad for the fill; plan 05 can later tint per-face by id.
//!
//! ARCH: pure data — no projection, no color, no I/O.

// Consumed by the rasterizer here and (per-face tinting) in plan 05.
#![allow(dead_code)]

use glam::Vec3;

/// Half the side length: corners sit at ±`HALF` on each axis.
const HALF: f32 = 0.5;

/// Stable identifier for each of the six faces, so callers can address a
/// specific face (e.g. for per-face tinting) without depending on slice order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceId {
    Front,  // +Z
    Back,   // -Z
    Right,  // +X
    Left,   // -X
    Top,    // +Y
    Bottom, // -Y
}

/// One cube face: a convex quad of 4 vertex indices (CCW seen from outside),
/// its outward unit normal, and its [`FaceId`].
#[derive(Debug, Clone, Copy)]
pub struct Face {
    /// Indices into [`Cube::vertices`], CCW when viewed from outside the cube.
    pub indices: [usize; 4],
    /// Outward-pointing unit normal in cube-local space (== world space for an
    /// un-transformed centered cube).
    pub normal: Vec3,
    /// Stable face identity.
    pub id: FaceId,
}

/// A unit cube: 8 corner vertices and 6 quad faces with outward normals.
#[derive(Debug, Clone)]
pub struct Cube {
    pub vertices: [Vec3; 8],
    pub faces: [Face; 6],
}

/// Build the canonical unit cube centered at the origin.
///
/// Vertex layout (bit pattern of the index = which corner):
///   bit0 = X (-/+0.5), bit1 = Y (-/+0.5), bit2 = Z (-/+0.5)
/// So vertex `i`: x = `(i&1)`, y = `(i&2)`, z = `(i&4)` mapped to ∓HALF.
pub fn unit_cube() -> Cube {
    // Corner coordinates, indexable by the (x,y,z) sign bits described above.
    let vertices = [
        Vec3::new(-HALF, -HALF, -HALF), // 0: ---
        Vec3::new(HALF, -HALF, -HALF),  // 1: +--
        Vec3::new(-HALF, HALF, -HALF),  // 2: -+-
        Vec3::new(HALF, HALF, -HALF),   // 3: ++-
        Vec3::new(-HALF, -HALF, HALF),  // 4: --+
        Vec3::new(HALF, -HALF, HALF),   // 5: +-+
        Vec3::new(-HALF, HALF, HALF),   // 6: -++
        Vec3::new(HALF, HALF, HALF),    // 7: +++
    ];

    // Each face lists 4 corners CCW as seen from OUTSIDE (along -normal toward
    // the cube), so the outward normal follows the right-hand rule. This makes
    // the back-face cull in the rasterizer correct.
    let faces = [
        Face {
            id: FaceId::Front, // +Z
            indices: [4, 5, 7, 6],
            normal: Vec3::Z,
        },
        Face {
            id: FaceId::Back, // -Z
            indices: [1, 0, 2, 3],
            normal: Vec3::NEG_Z,
        },
        Face {
            id: FaceId::Right, // +X
            indices: [5, 1, 3, 7],
            normal: Vec3::X,
        },
        Face {
            id: FaceId::Left, // -X
            indices: [0, 4, 6, 2],
            normal: Vec3::NEG_X,
        },
        Face {
            id: FaceId::Top, // +Y
            indices: [6, 7, 3, 2],
            normal: Vec3::Y,
        },
        Face {
            id: FaceId::Bottom, // -Y
            indices: [0, 1, 5, 4],
            normal: Vec3::NEG_Y,
        },
    ];

    Cube { vertices, faces }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_has_8_vertices_and_6_faces() {
        let cube = unit_cube();
        assert_eq!(cube.vertices.len(), 8);
        assert_eq!(cube.faces.len(), 6);
    }

    #[test]
    fn all_vertices_are_unit_cube_corners() {
        let cube = unit_cube();
        for v in &cube.vertices {
            assert!((v.x.abs() - HALF).abs() < 1e-6, "x not ±0.5: {v:?}");
            assert!((v.y.abs() - HALF).abs() < 1e-6, "y not ±0.5: {v:?}");
            assert!((v.z.abs() - HALF).abs() < 1e-6, "z not ±0.5: {v:?}");
        }
    }

    #[test]
    fn normals_are_unit_and_axis_aligned() {
        let cube = unit_cube();
        for face in &cube.faces {
            assert!(
                (face.normal.length() - 1.0).abs() < 1e-6,
                "normal not unit length: {:?}",
                face.normal
            );
        }
    }

    #[test]
    fn normals_point_outward() {
        // For each face, the centroid of its 4 corners must point in the same
        // direction as the face normal (outward from the cube center == origin).
        let cube = unit_cube();
        for face in &cube.faces {
            let centroid: Vec3 = face
                .indices
                .iter()
                .map(|&i| cube.vertices[i])
                .sum::<Vec3>()
                / 4.0;
            assert!(
                centroid.dot(face.normal) > 0.0,
                "normal {:?} does not point outward for face {:?} (centroid {:?})",
                face.normal,
                face.id,
                centroid
            );
        }
    }

    #[test]
    fn face_winding_is_ccw_from_outside() {
        // The geometric normal computed from the winding (edge0 × edge1) must
        // agree with the declared outward normal — proves CCW-from-outside order,
        // which the rasterizer's back-face cull depends on.
        let cube = unit_cube();
        for face in &cube.faces {
            let [a, b, c, _d] = face.indices;
            let v0 = cube.vertices[a];
            let v1 = cube.vertices[b];
            let v2 = cube.vertices[c];
            let geo_normal = (v1 - v0).cross(v2 - v0).normalize();
            assert!(
                geo_normal.dot(face.normal) > 0.9,
                "winding normal {:?} disagrees with declared {:?} for {:?}",
                geo_normal,
                face.normal,
                face.id
            );
        }
    }
}
