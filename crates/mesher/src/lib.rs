//! Mesh output shared by the first Marching Cubes implementation and Transvoxel.

/// Renderer-neutral indexed triangle mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_well_formed(&self) -> bool {
        let Ok(vertex_count) = u32::try_from(self.positions.len()) else {
            return false;
        };
        self.positions.len() == self.normals.len()
            && self.indices.len() % 3 == 0
            && self.indices.iter().all(|&index| index < vertex_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_triangle_indices_are_rejected() {
        let mesh = Mesh {
            positions: vec![[0.0; 3]],
            normals: vec![[0.0, 1.0, 0.0]],
            indices: vec![0, 0],
        };
        assert!(!mesh.is_well_formed());
    }
}
