//! Deterministic adjacency structures shared by the CPU and GPU kernels.
//!
//! Every list is built by a stable pass over faces in ascending order, so a
//! vertex always visits its incident faces and corners in the same order. The
//! kernels therefore add the same terms in the same order whatever the thread
//! count or the number of GPUs.

use glam::Vec3;

/// Compact representation of a face list: face `f` owns the corners
/// `corners[face_offsets[f]..face_offsets[f + 1]]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FaceGraph {
    face_offsets: Vec<u32>,
    corners: Vec<u32>,
    corner_faces: Vec<u32>,
}

impl FaceGraph {
    /// Build from any iterator of faces, in polygon order.
    pub fn from_faces<'a, I>(faces: I) -> Self
    where
        I: IntoIterator<Item = &'a [u32]>,
    {
        let mut face_offsets = Vec::with_capacity(64);
        face_offsets.push(0);
        let mut corners = Vec::new();
        let mut corner_faces = Vec::new();
        for (index, face) in faces.into_iter().enumerate() {
            for &corner in face {
                corners.push(corner);
                corner_faces.push(index as u32);
            }
            face_offsets.push(corners.len() as u32);
        }
        Self {
            face_offsets,
            corners,
            corner_faces,
        }
    }

    pub fn face_count(&self) -> usize {
        self.face_offsets.len().saturating_sub(1)
    }

    pub fn corner_count(&self) -> usize {
        self.corners.len()
    }

    pub fn face_offsets(&self) -> &[u32] {
        &self.face_offsets
    }

    pub fn corners(&self) -> &[u32] {
        &self.corners
    }

    pub fn corner_faces(&self) -> &[u32] {
        &self.corner_faces
    }

    /// Corner range of one face.
    pub fn face_range(&self, face: usize) -> std::ops::Range<usize> {
        let start = self.face_offsets[face] as usize;
        let end = self.face_offsets[face + 1] as usize;
        start..end
    }

    /// Vertices of one face.
    pub fn face(&self, face: usize) -> &[u32] {
        &self.corners[self.face_range(face)]
    }

    /// Newell normal of a face, normalised. This is the reference formula: the
    /// CPU and GPU kernels must both use it, or their normals would diverge.
    pub fn newell_normal(&self, positions: &[Vec3], face: usize) -> Vec3 {
        let corners = self.face(face);
        if corners.len() < 3 {
            return Vec3::ZERO;
        }
        let mut normal = Vec3::ZERO;
        for i in 0..corners.len() {
            let a = positions[corners[i] as usize];
            let b = positions[corners[(i + 1) % corners.len()] as usize];
            normal.x += (a.y - b.y) * (a.z + b.z);
            normal.y += (a.z - b.z) * (a.x + b.x);
            normal.z += (a.x - b.x) * (a.y + b.y);
        }
        normal.normalize_or_zero()
    }
}

/// Vertex to corner adjacency: vertex `v` owns
/// `corners[offsets[v]..offsets[v + 1]]`, in ascending corner (and therefore
/// ascending face) order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VertexCorners {
    offsets: Vec<u32>,
    corners: Vec<u32>,
}

impl VertexCorners {
    /// Counting sort over the face graph: ascending corner order is preserved.
    pub fn build(vertex_count: usize, graph: &FaceGraph) -> Self {
        let mut counts = vec![0u32; vertex_count + 1];
        for &vertex in graph.corners() {
            let index = vertex as usize;
            if index < vertex_count {
                counts[index + 1] += 1;
            }
        }
        for index in 0..vertex_count {
            counts[index + 1] += counts[index];
        }
        let offsets = counts.clone();
        let mut cursor = counts;
        let mut corners = vec![0u32; graph.corner_count()];
        for (index, &vertex) in graph.corners().iter().enumerate() {
            let slot = cursor[vertex as usize] as usize;
            corners[slot] = index as u32;
            cursor[vertex as usize] += 1;
        }
        Self { offsets, corners }
    }

    pub fn vertex_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    pub fn corners(&self) -> &[u32] {
        &self.corners
    }

    pub fn range(&self, vertex: usize) -> std::ops::Range<usize> {
        self.offsets[vertex] as usize..self.offsets[vertex + 1] as usize
    }
}

/// Vertex to face adjacency, in ascending face order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VertexFaces {
    offsets: Vec<u32>,
    faces: Vec<u32>,
}

impl VertexFaces {
    pub fn build(vertex_count: usize, graph: &FaceGraph) -> Self {
        let mut counts = vec![0u32; vertex_count + 1];
        for &vertex in graph.corners() {
            let index = vertex as usize;
            if index < vertex_count {
                counts[index + 1] += 1;
            }
        }
        for index in 0..vertex_count {
            counts[index + 1] += counts[index];
        }
        let offsets = counts.clone();
        let mut cursor = counts;
        let mut faces = vec![0u32; graph.corner_count()];
        for (face, &vertex) in graph.corners().iter().enumerate() {
            let vertex = vertex as usize;
            let slot = cursor[vertex] as usize;
            faces[slot] = graph.corner_faces()[face];
            cursor[vertex] += 1;
        }
        Self { offsets, faces }
    }

    pub fn vertex_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    pub fn faces(&self) -> &[u32] {
        &self.faces
    }

    pub fn range(&self, vertex: usize) -> std::ops::Range<usize> {
        self.offsets[vertex] as usize..self.offsets[vertex + 1] as usize
    }
}

/// Unique edge-adjacent neighbours of every vertex, in ascending index order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Neighbours {
    offsets: Vec<u32>,
    values: Vec<u32>,
}

impl Neighbours {
    /// Build from the face graph. Neighbours are deduplicated and sorted, so a
    /// smoothing pass reads them in a reproducible order.
    pub fn build(vertex_count: usize, graph: &FaceGraph) -> Self {
        let mut lists: Vec<Vec<u32>> = vec![Vec::new(); vertex_count];
        for face in 0..graph.face_count() {
            let corners = graph.face(face);
            let count = corners.len();
            for i in 0..count {
                let a = corners[i] as usize;
                let b = corners[(i + 1) % count] as usize;
                if a < vertex_count {
                    lists[a].push(b as u32);
                }
                if b < vertex_count {
                    lists[b].push(a as u32);
                }
            }
        }
        let mut offsets = Vec::with_capacity(vertex_count + 1);
        offsets.push(0);
        let mut values = Vec::new();
        for list in &mut lists {
            list.sort_unstable();
            list.dedup();
            values.extend_from_slice(list);
            offsets.push(values.len() as u32);
        }
        Self { offsets, values }
    }

    pub fn vertex_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    pub fn values(&self) -> &[u32] {
        &self.values
    }

    pub fn range(&self, vertex: usize) -> std::ops::Range<usize> {
        self.offsets[vertex] as usize..self.offsets[vertex + 1] as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> FaceGraph {
        FaceGraph::from_faces([&[0u32, 1, 2][..], &[0, 2, 3][..]])
    }

    #[test]
    fn faces_are_indexed_by_corner_range() {
        let graph = square();
        assert_eq!(graph.face_count(), 2);
        assert_eq!(graph.corner_count(), 6);
        assert_eq!(graph.face(0), &[0, 1, 2]);
        assert_eq!(graph.face(1), &[0, 2, 3]);
        assert_eq!(graph.corner_faces(), &[0, 0, 0, 1, 1, 1]);
    }

    #[test]
    fn vertex_corners_are_in_ascending_order() {
        let graph = square();
        let corners = VertexCorners::build(4, &graph);
        // Corners: [0,1,2] then [0,2,3]. Vertex 0 owns corners 0 and 3,
        // vertex 1 owns corner 1, vertex 2 owns corners 2 and 4.
        assert_eq!(corners.range(0), 0..2);
        assert_eq!(&corners.corners()[0..2], &[0, 3]);
        assert_eq!(corners.range(1), 2..3);
        assert_eq!(&corners.corners()[2..3], &[1]);
        assert_eq!(corners.range(2), 3..5);
        assert_eq!(&corners.corners()[3..5], &[2, 4]);
    }

    #[test]
    fn neighbours_are_sorted_and_unique() {
        let graph = square();
        let neighbours = Neighbours::build(4, &graph);
        // Vertex 0 touches 1, 2 and 3 across the two triangles.
        assert_eq!(neighbours.range(0), 0..3);
        assert_eq!(&neighbours.values()[0..3], &[1, 2, 3]);
        assert_eq!(neighbours.range(1), 3..5);
        assert_eq!(&neighbours.values()[3..5], &[0, 2]);
    }

    #[test]
    fn vertex_faces_follow_face_order() {
        let graph = square();
        let faces = VertexFaces::build(4, &graph);
        assert_eq!(faces.range(1), 2..3);
        assert_eq!(&faces.faces()[2..3], &[0]);
        assert_eq!(faces.range(2), 3..5);
        assert_eq!(&faces.faces()[3..5], &[0, 1]);
    }
}
