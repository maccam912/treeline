//! Bark appearance, derived from species and condition.

use treeline_ecology::{BarkStyle, ProceduralTree, TreeCondition, TreeFunctionalGroup};

use crate::vertex::{
    SURFACE_KIND_OAK_BARK, SURFACE_KIND_PINE_BARK, SURFACE_KIND_SOLID, f64_as_f32, hash_fraction,
    hash_lane,
};

pub(crate) fn bark_color(tree: ProceduralTree) -> [f32; 4] {
    let base = match tree.genotype.bark_style {
        BarkStyle::Scaly => [0.25, 0.18, 0.11],
        BarkStyle::Smooth => [0.43, 0.40, 0.33],
        BarkStyle::Furrowed => [0.20, 0.14, 0.085],
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

/// Shaded inner needles and sunlit outer tips for one pine foliage lobe.
pub(crate) fn pine_foliage_colors(tree: ProceduralTree, seed: u64) -> [[f32; 4]; 2] {
    let density = f64_as_f32(tree.genotype.leaf_density_fraction);
    let tone = (hash_fraction(seed, 0x0043_4f4c_4f52) - 0.5) * 0.045;
    let inner = [0.035 + tone, 0.13 + tone, 0.055 + (tone * 0.55), 1.0];
    let outer = [
        0.078 + tone + (density * 0.018),
        0.255 + tone + (density * 0.045),
        0.105 + (tone * 0.55) + (density * 0.018),
        1.0,
    ];
    [inner, outer].map(|mut color| {
        for channel in &mut color[..3] {
            *channel = channel.clamp(0.0, 1.0);
        }
        color
    })
}

/// Cool interior shade and sunlit exterior leaves for one broadleaf cloudlet.
pub(crate) fn broadleaf_foliage_colors(
    tree: ProceduralTree,
    seed: u64,
    exposure: f32,
) -> [[f32; 4]; 2] {
    let density = f64_as_f32(tree.genotype.leaf_density_fraction);
    let tone = (hash_fraction(seed, 0x4252_4f41_444c_4546) - 0.5) * 0.055;
    let exposure = exposure.clamp(0.0, 1.0);
    let (inner, outer) = match tree.genotype.functional_group {
        TreeFunctionalGroup::ColdDeciduous => ([0.055, 0.16, 0.050, 1.0], [0.19, 0.39, 0.105, 1.0]),
        TreeFunctionalGroup::TemperateBroadleaf => {
            ([0.038, 0.14, 0.030, 1.0], [0.135, 0.35, 0.070, 1.0])
        }
        TreeFunctionalGroup::EvergreenNeedleleaf => unreachable!("broadleaf color on a conifer"),
    };
    [inner, outer].map(|mut color| {
        color[0] += tone + (exposure * 0.025);
        color[1] += tone + (exposure * 0.055) + (density * 0.025);
        color[2] += (tone * 0.55) + (exposure * 0.012);
        for channel in &mut color[..3] {
            *channel = channel.clamp(0.0, 1.0);
        }
        color
    })
}
