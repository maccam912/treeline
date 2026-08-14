//! Branch-attached foliage cluster placement shared by both distance tiers.

use glam::Vec3;
use treeline_ecology::{CrownShape, TreeCondition};

use super::{BroadleafCrown, draw};
use crate::vertex::f64_as_f32;

#[derive(Clone, Copy, Debug)]
pub(in crate::tree_mesh::broadleaf) struct LeafCluster {
    pub(in crate::tree_mesh::broadleaf) center: Vec3,
    pub(in crate::tree_mesh::broadleaf) up: Vec3,
    pub(in crate::tree_mesh::broadleaf) long: Vec3,
    pub(in crate::tree_mesh::broadleaf) across: Vec3,
    pub(in crate::tree_mesh::broadleaf) exposure: f32,
    pub(in crate::tree_mesh::broadleaf) seed: u64,
}

impl BroadleafCrown {
    pub(in crate::tree_mesh::broadleaf) fn cluster_count(&self) -> usize {
        (self.fan_count * 2) + 1
    }

    pub(in crate::tree_mesh::broadleaf) fn cluster(&self, index: usize) -> LeafCluster {
        let apex = index + 1 == self.cluster_count();
        let seed = self.tree.id.rotate_left(41) ^ cluster_lane(index, 0x5eed);
        let (mut center, radial, exposure) = if apex {
            (self.frame.base + self.frame.trunk_vector, self.tangent, 1.0)
        } else {
            let scaffold = self.scaffold(index / 2);
            let center = scaffold.tips[index % 2];
            let offset =
                center - (self.frame.base + (self.frame.trunk_vector * self.start_fraction));
            let radial = (offset - (self.up * offset.dot(self.up))).normalize_or(self.tangent);
            let exposure = ((center - self.frame.base).dot(self.up)
                / self.frame.trunk_vector.length())
            .clamp(self.start_fraction, 1.0);
            (center, radial, exposure)
        };
        let profile = match self.tree.genotype.crown_shape {
            CrownShape::Columnar => 0.280,
            CrownShape::Rounded => 0.530,
            CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
        };
        let density = f64_as_f32(self.tree.genotype.leaf_density_fraction);
        let scale = profile
            * (0.90 + (draw(self.tree.id, cluster_lane(index, 1)) * 0.20))
            * (0.92 + (exposure * 0.12))
            * (0.92 + (density * 0.10));
        let width = self.radius * scale;
        let height_ratio = match self.tree.genotype.crown_shape {
            CrownShape::Columnar => 1.00,
            CrownShape::Rounded => 0.96,
            CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
        };
        let height = (width * (height_ratio + (draw(self.tree.id, cluster_lane(index, 2)) * 0.14)))
            .min(self.length * 0.19)
            .max(0.06);
        if apex {
            center -= self.up * height * 1.02;
        }
        let side = self.up.cross(radial).normalize_or(self.across);
        let across_scale = match self.tree.genotype.crown_shape {
            CrownShape::Columnar => 0.72,
            CrownShape::Rounded => 0.94,
            CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
        };
        self.fit_below_canopy(LeafCluster {
            center,
            up: self.up * height,
            long: radial * width,
            across: side
                * width
                * (across_scale + (draw(self.tree.id, cluster_lane(index, 3)) * 0.12)),
            exposure,
            seed,
        })
    }

    pub(in crate::tree_mesh::broadleaf) fn cluster_present(&self, index: usize) -> bool {
        let apex = index + 1 == self.cluster_count();
        if apex && self.tree.condition == TreeCondition::StormBroken {
            return false;
        }
        let missing = f64_as_f32(self.tree.damage_fraction) * if apex { 0.24 } else { 0.58 };
        draw(self.tree.id, cluster_lane(index, 7)) >= missing
    }

    pub(in crate::tree_mesh::broadleaf) fn branch_cluster(&self, index: usize) -> LeafCluster {
        let scaffold = self.scaffold(index);
        let terminal_center = scaffold.tips[0].lerp(scaffold.tips[1], 0.5);
        let template = self.cluster(index * 2);
        let (up_scale, long_scale, across_scale) = match self.tree.genotype.crown_shape {
            CrownShape::Columnar => (0.66, 0.62, 0.68),
            CrownShape::Rounded => (0.78, 0.96, 0.98),
            CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
        };
        self.fit_below_canopy(LeafCluster {
            center: scaffold.elbow.lerp(terminal_center, 0.58),
            up: template.up * up_scale,
            long: template.long * long_scale,
            across: template.across * across_scale,
            exposure: (template.exposure - 0.12).max(0.0),
            seed: template.seed ^ 0x4252_414e_4348_5f46,
        })
    }

    fn fit_below_canopy(&self, mut cluster: LeafCluster) -> LeafCluster {
        let canopy_top = self.frame.base.y + self.frame.trunk_vector.y;
        let clearance = (canopy_top - cluster.center.y).max(0.0);
        let vertical_extent = cluster.up.y.abs() + cluster.long.y.abs() + cluster.across.y.abs();
        if vertical_extent > clearance && vertical_extent > f32::EPSILON {
            let scale = (clearance / vertical_extent).clamp(0.0, 1.0);
            cluster.up *= scale;
            cluster.long *= scale;
            cluster.across *= scale;
        }
        cluster
    }
}

fn cluster_lane(index: usize, property: u64) -> u64 {
    0x434c_5553_5445_5200
        ^ (u64::try_from(index).expect("broadleaf cluster fits u64") << 8)
        ^ property
}
