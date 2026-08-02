//! Assertions shared by the meshing tests.

use treeline_coordinates::stable_hash;

use crate::Mesh;

pub fn assert_front_facing(mesh: &Mesh) {
    for (triangle_index, triangle) in mesh.indices.chunks_exact(3).enumerate() {
        let positions = [triangle[0], triangle[1], triangle[2]]
            .map(|index| mesh.positions[usize::try_from(index).expect("test index fits usize")]);
        let first = [
            positions[1][0] - positions[0][0],
            positions[1][1] - positions[0][1],
            positions[1][2] - positions[0][2],
        ];
        let second = [
            positions[2][0] - positions[0][0],
            positions[2][1] - positions[0][1],
            positions[2][2] - positions[0][2],
        ];
        let geometric_normal = [
            (first[1] * second[2]) - (first[2] * second[1]),
            (first[2] * second[0]) - (first[0] * second[2]),
            (first[0] * second[1]) - (first[1] * second[0]),
        ];
        if geometric_normal
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            <= f64::EPSILON
        {
            continue;
        }
        let vertex_normal = triangle.iter().fold([0.0; 3], |sum, &index| {
            let normal = mesh.normals[usize::try_from(index).expect("test index fits usize")];
            [sum[0] + normal[0], sum[1] + normal[1], sum[2] + normal[2]]
        });
        let agreement = geometric_normal
            .into_iter()
            .zip(vertex_normal)
            .map(|(geometric, vertex)| geometric * f64::from(vertex))
            .sum::<f64>();
        assert!(
            agreement > 0.0,
            "triangle {triangle_index} faces away from its vertex normals"
        );
    }
}

pub fn mesh_fingerprint(mesh: &Mesh) -> u64 {
    let mut words = Vec::with_capacity(
        (mesh.positions.len() * 3) + (mesh.normals.len() * 3) + mesh.indices.len() + 3,
    );
    words.push(u64::try_from(mesh.positions.len()).expect("test mesh length fits u64"));
    words.push(u64::try_from(mesh.normals.len()).expect("test mesh length fits u64"));
    words.push(u64::try_from(mesh.indices.len()).expect("test mesh length fits u64"));
    words.extend(
        mesh.positions
            .iter()
            .flatten()
            .map(|component| component.to_bits()),
    );
    words.extend(
        mesh.normals
            .iter()
            .flatten()
            .map(|component| u64::from(component.to_bits())),
    );
    words.extend(mesh.indices.iter().map(|&index| u64::from(index)));
    stable_hash(&words)
}
