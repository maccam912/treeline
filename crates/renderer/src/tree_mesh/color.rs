//! Bark and foliage appearance, derived from species and condition.

use treeline_ecology::{BarkStyle, ProceduralTree, TreeCondition, TreeFunctionalGroup};

use crate::vertex::{
    SURFACE_KIND_OAK_BARK, SURFACE_KIND_PINE_BARK, SURFACE_KIND_SOLID, f64_as_f32, hash_lane,
};

pub(crate) fn tree_has_foliage(tree: ProceduralTree) -> bool {
    !matches!(
        tree.condition,
        TreeCondition::DeadStanding | TreeCondition::StormBroken
    )
}

pub(crate) fn bark_color(tree: ProceduralTree) -> [f32; 4] {
    let base = match tree.genotype.bark_style {
        BarkStyle::Scaly => [0.25, 0.18, 0.11],
        BarkStyle::Smooth => [0.43, 0.40, 0.33],
        BarkStyle::Furrowed => [0.27, 0.20, 0.14],
    };
    let bleaching = if tree.condition == TreeCondition::DeadStanding {
        0.46
    } else {
        f64_as_f32(tree.damage_fraction) * 0.12
    };
    [
        base[0] + bleaching,
        base[1] + bleaching,
        base[2] + (bleaching * 0.88),
        1.0,
    ]
}

pub(crate) fn foliage_color(tree: ProceduralTree) -> [f32; 4] {
    let base = match tree.genotype.functional_group {
        TreeFunctionalGroup::EvergreenNeedleleaf => [0.055, 0.24, 0.12],
        TreeFunctionalGroup::ColdDeciduous => [0.25, 0.43, 0.12],
        TreeFunctionalGroup::TemperateBroadleaf => [0.10, 0.36, 0.075],
    };
    let variation = (hash_lane(tree.id, 31) - 0.5) * 0.10;
    let damage = f64_as_f32(tree.damage_fraction);
    [
        (base[0] + variation + (damage * 0.12)).clamp(0.0, 1.0),
        (base[1] + variation - (damage * 0.10)).clamp(0.0, 1.0),
        (base[2] + (variation * 0.5) - (damage * 0.04)).clamp(0.0, 1.0),
        1.0,
    ]
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CylinderMaterial {
    pub(crate) surface_kind: f32,
    pub(crate) seed: f32,
}

impl CylinderMaterial {
    pub(crate) const UNTEXTURED: Self = Self {
        surface_kind: SURFACE_KIND_SOLID,
        seed: 0.0,
    };
}

pub(crate) fn bark_cylinder_material(tree: ProceduralTree, lane: usize) -> CylinderMaterial {
    let surface_kind = match tree.genotype.functional_group {
        TreeFunctionalGroup::EvergreenNeedleleaf => SURFACE_KIND_PINE_BARK,
        TreeFunctionalGroup::ColdDeciduous | TreeFunctionalGroup::TemperateBroadleaf => {
            SURFACE_KIND_OAK_BARK
        }
    };
    CylinderMaterial {
        surface_kind,
        seed: hash_lane(tree.id.rotate_left(29), lane + 41),
    }
}
