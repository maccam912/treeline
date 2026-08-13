//! Deterministic whorls and branch arms for one pine crown.

use glam::Vec3;
use treeline_ecology::{ProceduralTree, TreeCondition};

use crate::tree_mesh::TreeFrame;
use crate::vertex::{f64_as_f32, hash_fraction, usize_as_f32};

#[derive(Debug)]
pub(super) struct PineCrown {
    pub(super) tree: ProceduralTree,
    pub(super) frame: TreeFrame,
    pub(super) up: Vec3,
    pub(super) tangent: Vec3,
    pub(super) across: Vec3,
    pub(super) start_fraction: f32,
    pub(super) length: f32,
    pub(super) radius: f32,
    pub(super) layer_count: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PineLayer {
    pub(super) index: usize,
    pub(super) crown_fraction: f32,
    pub(super) center: Vec3,
    pub(super) reach: f32,
    pub(super) spacing: f32,
    pub(super) turn: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PineArm {
    pub(super) tip: Vec3,
    pub(super) foliage_start: Vec3,
    pub(super) foliage_radius: f32,
    pub(super) seed: u64,
}

impl PineCrown {
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
        let length = trunk_length * (1.0 - start_fraction);
        Some(Self {
            tree,
            frame,
            up,
            tangent,
            across,
            start_fraction,
            length,
            radius,
            layer_count: layer_count(tree, length),
        })
    }

    pub(super) fn layer(&self, index: usize) -> PineLayer {
        let count = usize_as_f32(self.layer_count);
        let unit = (usize_as_f32(index) + 0.55) / (count + 0.35);
        let shaped = (1.0 - libm::powf(1.0 - unit, 1.28)) * 0.95;
        let crown_fraction = (shaped + (signed(self.tree.id, layer_lane(index, 0)) * 0.18 / count))
            .clamp(0.05, 0.93);
        let stem_fraction = self.start_fraction + ((1.0 - self.start_fraction) * crown_fraction);
        let profile_exponent = match self.tree.condition {
            TreeCondition::Ancient => 0.26,
            TreeCondition::Sapling => 0.60,
            _ => 0.38,
        };
        let shade = (0.70 + (crown_fraction * 2.5)).min(1.0);
        let irregular = 0.84 + (draw(self.tree.id, layer_lane(index, 1)) * 0.20);
        let damage = 1.0 - (f64_as_f32(self.tree.damage_fraction) * 0.34);
        let reach = (self.radius
            * libm::powf(1.0 - crown_fraction, profile_exponent)
            * shade
            * irregular
            * damage)
            .clamp(self.radius * 0.10, self.radius);
        PineLayer {
            index,
            crown_fraction,
            center: self.frame.base + (self.frame.trunk_vector * stem_fraction),
            reach,
            spacing: self.length / count,
            turn: f64_as_f32(self.tree.rotation_turns)
                + (usize_as_f32(index) * 0.381_966)
                + (signed(self.tree.id, layer_lane(index, 2)) * 0.12),
        }
    }

    pub(super) fn branch_count(&self, layer: PineLayer) -> usize {
        let density = f64_as_f32(self.tree.genotype.branch_density_fraction);
        3 + usize::from(density > 0.82)
            + usize::from(draw(self.tree.id, layer_lane(layer.index, 3)) > 0.68)
    }

    pub(super) fn arm(&self, layer: PineLayer, index: usize) -> Option<PineArm> {
        let branch_count = self.branch_count(layer);
        let missing_chance = 0.07 + (f64_as_f32(self.tree.damage_fraction) * 0.52);
        if index >= 2 && draw(self.tree.id, arm_lane(layer.index, index, 0)) < missing_chance {
            return None;
        }
        let around = usize_as_f32(index) / usize_as_f32(branch_count);
        let turn =
            layer.turn + around + (signed(self.tree.id, arm_lane(layer.index, index, 1)) * 0.055);
        let (sine, cosine) = libm::sincosf(turn * std::f32::consts::TAU);
        let radial = (self.tangent * cosine) + (self.across * sine);
        let root = layer.center
            + (self.up
                * signed(self.tree.id, arm_lane(layer.index, index, 2))
                * layer.spacing
                * 0.10);
        let genotype_angle = f64_as_f32(self.tree.genotype.branching_angle_radians);
        let angle = (1.49 - (layer.crown_fraction * 0.78)
            + ((genotype_angle - 1.05) * 0.18)
            + (signed(self.tree.id, arm_lane(layer.index, index, 3)) * 0.10))
            .clamp(0.56, 1.54);
        let direction =
            ((radial * libm::sinf(angle)) + (self.up * libm::cosf(angle))).normalize_or(radial);
        let length =
            layer.reach * (0.90 + (draw(self.tree.id, arm_lane(layer.index, index, 4)) * 0.10));
        let sag = length
            * (0.015 + ((1.0 - layer.crown_fraction) * 0.055))
            * (0.72 + (draw(self.tree.id, arm_lane(layer.index, index, 5)) * 0.52));
        let mut tip = root + (direction * length) - (Vec3::Y * sag);
        let bare = if self.tree.condition == TreeCondition::Sapling {
            0.08 + (draw(self.tree.id, arm_lane(layer.index, index, 6)) * 0.08)
        } else {
            0.12 + (draw(self.tree.id, arm_lane(layer.index, index, 6)) * 0.12)
        };
        let mut foliage_start = root.lerp(tip, bare);
        let mut foliage_radius = foliage_start.distance(tip)
            * (0.115 + (f64_as_f32(self.tree.genotype.leaf_density_fraction) * 0.025));
        foliage_radius = foliage_radius.max(0.018).min(layer.reach * 0.18);
        let apex_y = self.frame.base.y + self.frame.trunk_vector.y;
        tip.y = tip.y.min(apex_y - (foliage_radius * 1.25));
        foliage_start = root.lerp(tip, bare);
        foliage_radius = (foliage_start.distance(tip)
            * (0.115 + (f64_as_f32(self.tree.genotype.leaf_density_fraction) * 0.025)))
            .max(0.018)
            .min(layer.reach * 0.18);
        Some(PineArm {
            tip,
            foliage_start,
            foliage_radius,
            seed: arm_seed(self.tree.id, layer.index, index),
        })
    }
}

fn crown_start(tree: ProceduralTree) -> f32 {
    if tree.condition == TreeCondition::Sapling {
        return 0.16;
    }
    let ratio = f64_as_f32(tree.crown_radius_meters / tree.height_meters.max(0.01));
    let openness = ((ratio - 0.10) / 0.09).clamp(0.0, 1.0);
    let condition = match tree.condition {
        TreeCondition::Ancient => 0.08,
        TreeCondition::WindDamaged => 0.04,
        _ => 0.0,
    };
    (0.56 - (openness * 0.25) + condition).clamp(0.28, 0.62)
}

fn layer_count(tree: ProceduralTree, crown_length: f32) -> usize {
    if tree.condition == TreeCondition::Sapling {
        return 3;
    }
    let mut count = 4;
    for threshold in [7.0, 9.5, 12.0, 15.0] {
        count += usize::from(crown_length >= threshold);
    }
    count
}

fn draw(id: u64, lane: u64) -> f32 {
    hash_fraction(id, lane)
}

fn signed(id: u64, lane: u64) -> f32 {
    draw(id, lane) - 0.5
}

fn layer_lane(layer: usize, property: u64) -> u64 {
    0x4c41_5945_5200_0000 ^ (u64::try_from(layer).expect("pine layer fits u64") << 8) ^ property
}

fn arm_lane(layer: usize, branch: usize, property: u64) -> u64 {
    0x4152_4d00_0000_0000
        ^ (u64::try_from(layer).expect("pine layer fits u64") << 16)
        ^ (u64::try_from(branch).expect("pine branch fits u64") << 8)
        ^ property
}

fn arm_seed(id: u64, layer: usize, branch: usize) -> u64 {
    id.rotate_left(23) ^ arm_lane(layer, branch, 0x5eed).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}
