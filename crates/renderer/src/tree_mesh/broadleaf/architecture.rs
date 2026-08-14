//! Deterministic forked scaffolds and terminal leaf cloudlets.

mod clusters;

use glam::Vec3;
use treeline_ecology::{CrownShape, ProceduralTree, TreeCondition};

use crate::tree_mesh::TreeFrame;
use crate::vertex::{f64_as_f32, hash_fraction, usize_as_f32};

pub(super) use clusters::LeafCluster;

#[derive(Debug)]
pub(super) struct BroadleafCrown {
    pub(super) tree: ProceduralTree,
    pub(super) frame: TreeFrame,
    pub(super) up: Vec3,
    pub(super) tangent: Vec3,
    pub(super) across: Vec3,
    pub(super) start_fraction: f32,
    pub(super) length: f32,
    pub(super) radius: f32,
    pub(super) fan_count: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Scaffold {
    pub(super) root: Vec3,
    pub(super) elbow: Vec3,
    pub(super) forks: [Vec3; 2],
    pub(super) tips: [Vec3; 2],
    pub(super) root_radius: f32,
    pub(super) elbow_radius: f32,
    pub(super) fork_radius: f32,
    pub(super) tip_radius: f32,
}

impl BroadleafCrown {
    pub(super) fn new(tree: ProceduralTree, frame: TreeFrame) -> Option<Self> {
        let trunk_length = frame.trunk_vector.length();
        let radius = f64_as_f32(tree.crown_radius_meters);
        if trunk_length <= f32::EPSILON || radius <= f32::EPSILON {
            return None;
        }
        let up = frame.trunk_vector / trunk_length;
        let reference = if up.y.abs() < 0.92 { Vec3::Y } else { Vec3::X };
        let tangent = up.cross(reference).normalize_or(Vec3::X);
        let across = up.cross(tangent).normalize_or(Vec3::Z);
        let start_fraction = crown_start(tree);
        let branch_density = f64_as_f32(tree.genotype.branch_density_fraction);
        Some(Self {
            tree,
            frame,
            up,
            tangent,
            across,
            start_fraction,
            length: trunk_length * (1.0 - start_fraction),
            radius,
            fan_count: match (tree.condition, tree.genotype.crown_shape) {
                (TreeCondition::Sapling, _) => 2,
                (_, CrownShape::Rounded) => {
                    3 + usize::from(branch_density >= 0.68) + usize::from(branch_density >= 0.86)
                }
                (_, CrownShape::Columnar) => 2 + usize::from(branch_density >= 0.62),
                (_, CrownShape::Conical) => {
                    unreachable!("conical crown in the broadleaf grammar")
                }
            },
        })
    }

    pub(super) fn trunk_end_fraction(&self) -> f32 {
        let share = match self.tree.genotype.crown_shape {
            CrownShape::Columnar => 0.30,
            CrownShape::Rounded => 0.20,
            CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
        };
        (self.start_fraction + ((1.0 - self.start_fraction) * share)).clamp(0.48, 0.78)
    }

    pub(super) fn junction(&self) -> Vec3 {
        self.frame.base + (self.frame.trunk_vector * self.trunk_end_fraction())
    }

    pub(super) fn leader_bend(&self) -> Vec3 {
        let apex = self.cluster(self.cluster_count() - 1).center;
        let sweep = (self.tangent * signed(self.tree.id, 0x4c45_4144_4552_5f58))
            + (self.across * signed(self.tree.id, 0x4c45_4144_4552_5f5a));
        self.junction().lerp(apex, 0.54) + (sweep * self.radius * 0.13)
    }

    pub(super) fn scaffold(&self, index: usize) -> Scaffold {
        let count = usize_as_f32(self.fan_count);
        let ordinal = usize_as_f32(index);
        let turn = f64_as_f32(self.tree.rotation_turns)
            + (ordinal / count)
            + (signed(self.tree.id, scaffold_lane(index, 0)) * 0.065);
        let radial = self.radial(turn);
        let side = self.up.cross(radial).normalize_or(self.across);
        let root_fraction = self.start_fraction
            + ((1.0 - self.start_fraction)
                * (0.055
                    + (ordinal / count * 0.13)
                    + (signed(self.tree.id, scaffold_lane(index, 1)) * 0.025)));
        let root = self.frame.base + (self.frame.trunk_vector * root_fraction);
        let branching_angle = f64_as_f32(self.tree.genotype.branching_angle_radians);
        let opening = ((branching_angle - 0.62) / 0.66).clamp(0.0, 1.0);
        let profile_reach = match self.tree.genotype.crown_shape {
            CrownShape::Columnar => 0.39 + (opening * 0.05),
            CrownShape::Rounded => 0.40 + (opening * 0.04),
            CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
        };
        let damage = 1.0 - (f64_as_f32(self.tree.damage_fraction) * 0.34);
        let reach = self.radius
            * (profile_reach + (signed(self.tree.id, scaffold_lane(index, 2)) * 0.08))
            * damage;
        let elbow = root
            + (radial * reach * 0.38)
            + (side * reach * signed(self.tree.id, scaffold_lane(index, 9)) * 0.18)
            + (self.up
                * self.length
                * (0.14 + (draw(self.tree.id, scaffold_lane(index, 3)) * 0.07)));
        let first_rise = (0.10
            + (draw(self.tree.id, scaffold_lane(index, 4)) * 0.22)
            + ((1.0 - opening) * 0.06))
            .clamp(0.10, 0.38);
        let second_rise = (0.43
            + (draw(self.tree.id, scaffold_lane(index, 5)) * 0.27)
            + ((1.0 - opening) * 0.04))
            .clamp(0.40, 0.76);
        let tips = [
            self.crown_point(first_rise, radial * reach * 0.92 + side * reach * 0.12),
            self.crown_point(
                second_rise,
                radial * reach * 0.74
                    - side * reach * (0.28 + (draw(self.tree.id, scaffold_lane(index, 6)) * 0.16)),
            ),
        ];
        let bend = side * reach * (0.045 + (draw(self.tree.id, scaffold_lane(index, 10)) * 0.055));
        let forks = [
            elbow.lerp(tips[0], 0.48)
                + bend
                + (self.up * self.length * signed(self.tree.id, scaffold_lane(index, 11)) * 0.025),
            elbow.lerp(tips[1], 0.53) - (bend * 0.78)
                + (radial * reach * signed(self.tree.id, scaffold_lane(index, 12)) * 0.045),
        ];
        let trunk_radius = lerp(
            self.frame.trunk_radius,
            self.frame.trunk_top_radius,
            root_fraction,
        );
        let weight = match self.tree.genotype.crown_shape {
            CrownShape::Columnar => 0.28,
            CrownShape::Rounded => 0.50,
            CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
        };
        let root_radius = (trunk_radius * weight).max(0.018);
        Scaffold {
            root,
            elbow,
            forks,
            tips,
            root_radius,
            elbow_radius: (root_radius * 0.66).max(0.012),
            fork_radius: (root_radius * 0.34).max(0.008),
            tip_radius: (root_radius * 0.13).max(0.005),
        }
    }

    fn crown_point(&self, crown_fraction: f32, radial_offset: Vec3) -> Vec3 {
        self.frame.base
            + (self.frame.trunk_vector
                * (self.start_fraction + ((1.0 - self.start_fraction) * crown_fraction)))
            + radial_offset
    }

    fn radial(&self, turn: f32) -> Vec3 {
        let (sine, cosine) = libm::sincosf(turn * std::f32::consts::TAU);
        (self.tangent * cosine) + (self.across * sine)
    }
}

fn crown_start(tree: ProceduralTree) -> f32 {
    if tree.condition == TreeCondition::Sapling {
        return 0.30;
    }
    let ratio = f64_as_f32(tree.crown_radius_meters / tree.height_meters.max(0.01));
    let typical = match tree.genotype.crown_shape {
        CrownShape::Columnar => 0.20,
        CrownShape::Rounded => 0.26,
        CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
    };
    let openness = ((ratio / typical - 0.70) / 0.42).clamp(0.0, 1.0);
    let (closed, open) = match tree.genotype.crown_shape {
        CrownShape::Columnar => (0.62, 0.46),
        CrownShape::Rounded => (0.54, 0.32),
        CrownShape::Conical => unreachable!("conical crown in the broadleaf grammar"),
    };
    let condition = match tree.condition {
        TreeCondition::Ancient => -0.035,
        TreeCondition::WindDamaged => 0.025,
        _ => 0.0,
    };
    (lerp(closed, open, openness) + condition).clamp(0.30, 0.68)
}

fn draw(id: u64, lane: u64) -> f32 {
    hash_fraction(id, lane)
}

fn signed(id: u64, lane: u64) -> f32 {
    draw(id, lane) - 0.5
}

fn scaffold_lane(index: usize, property: u64) -> u64 {
    0x5343_4146_464f_4c00
        ^ (u64::try_from(index).expect("broadleaf scaffold fits u64") << 8)
        ^ property
}

fn lerp(start: f32, end: f32, amount: f32) -> f32 {
    start + ((end - start) * amount)
}
