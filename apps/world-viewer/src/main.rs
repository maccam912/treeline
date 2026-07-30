use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use treeline_coordinates::{WorldIdentity, stable_hash};
use treeline_ecology::{ForestDistribution, Soil};
use treeline_geography::{Climate, RegionalProfile};
use treeline_renderer::terrain_tier;
use treeline_terrain::{SurfaceField, WildernessTerrain};
use treeline_voxel::LodLevel;
use treeline_world::{CURRENT_GENERATOR_VERSION, GeneratedWorldTerrain, GenerationPriority};

const DEFAULT_OUTPUT: &str = "artifacts/world-quality";
const DEFAULT_REGION_COUNT: usize = 12;
const DEFAULT_SEEDS: [u64; 3] = [0x5eed, 0xa11c_e5ed, 0xd15c_0a7e];
const AUDIT_SPACING_METERS: f64 = 4_000.0;
const AUDIT_GRID_EDGE: usize = 7;
const VIEW_SEARCH_SPACING_METERS: f64 = 2_000.0;
const VIEW_SEARCH_EDGE: usize = 9;
const PANEL_WIDTH: usize = 48;
const PANEL_HEIGHT: usize = 36;
const CONTACT_SCALE: usize = 3;
const PANEL_SPAN_METERS: f64 = 12_000.0;
const CONTACT_COLUMNS: usize = 7;
const DOMAIN_AUDIT_REGION: u64 = 0x574f_524c_445f_4155;
const DOMAIN_VIEW_DIRECTION: u64 = 0x5649_4557_5f44_4952;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "audit")
    {
        let config = AuditConfig::parse(&arguments[1..])?;
        run_audit(&config)?;
        return Ok(());
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "help" || argument == "--help")
    {
        print_usage();
        return Ok(());
    }

    let near_spacing = LodLevel::new(0).spacing_meters();
    let horizon_tier = terrain_tier(30_000.0);
    let first_job = GenerationPriority::Horizon;
    println!(
        "World viewer: {near_spacing}m near samples, {horizon_tier:?}, first job {first_job:?}"
    );
    println!("Run `cargo run -p world-viewer -- audit` for the Phase 5 world-quality survey.");
    Ok(())
}

fn print_usage() {
    println!(
        "world-viewer audit [--output PATH] [--regions COUNT] [--seeds LIST] [--accept]\n\
         Seeds are comma-separated decimal or 0x-prefixed u64 values. Existing baselines are\n\
         compared but retained unless --accept is supplied."
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuditConfig {
    output: PathBuf,
    regions_per_seed: usize,
    seeds: Vec<u64>,
    accept_baseline: bool,
}

impl AuditConfig {
    fn parse(arguments: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut config = Self {
            output: PathBuf::from(DEFAULT_OUTPUT),
            regions_per_seed: DEFAULT_REGION_COUNT,
            seeds: DEFAULT_SEEDS.to_vec(),
            accept_baseline: false,
        };
        let mut index = 0;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "--output" => {
                    index += 1;
                    config.output =
                        PathBuf::from(arguments.get(index).ok_or("--output requires a path")?);
                }
                "--regions" => {
                    index += 1;
                    config.regions_per_seed = arguments
                        .get(index)
                        .ok_or("--regions requires a count")?
                        .parse()?;
                    if config.regions_per_seed == 0 {
                        return Err("--regions must be greater than zero".into());
                    }
                }
                "--seeds" => {
                    index += 1;
                    config.seeds = parse_seeds(
                        arguments
                            .get(index)
                            .ok_or("--seeds requires a comma-separated list")?,
                    )?;
                }
                "--accept" => config.accept_baseline = true,
                unknown => return Err(format!("unknown audit argument: {unknown}").into()),
            }
            index += 1;
        }
        Ok(config)
    }
}

fn parse_seeds(value: &str) -> Result<Vec<u64>, Box<dyn Error>> {
    let seeds = value
        .split(',')
        .map(str::trim)
        .filter(|seed| !seed.is_empty())
        .map(|seed| {
            seed.strip_prefix("0x").map_or_else(
                || seed.parse::<u64>(),
                |hexadecimal| u64::from_str_radix(hexadecimal, 16),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if seeds.is_empty() {
        return Err("at least one seed is required".into());
    }
    Ok(seeds)
}

fn run_audit(config: &AuditConfig) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(&config.output)?;
    let mut descriptors = Vec::with_capacity(config.seeds.len() * config.regions_per_seed);
    let mut suites = Vec::with_capacity(config.seeds.len());

    for &seed in &config.seeds {
        let world = WorldIdentity::new(seed, CURRENT_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        for region_index in 0..config.regions_per_seed {
            let center = audit_region_center(seed, region_index);
            descriptors.push(sample_descriptor(&terrain, region_index, center)?);
        }
        let suite_center = audit_region_center(seed, 0);
        suites.push(capture_viewpoint_suite(&terrain, suite_center)?);
    }

    let novelty = NoveltyReport::analyze(&descriptors);
    let contact_sheet = render_contact_sheet(&suites)?;
    let descriptor_csv = descriptors_csv(&descriptors);
    let report = markdown_report(config, &descriptors, &novelty, &suites);
    let fingerprint = audit_fingerprint(
        descriptor_csv.as_bytes(),
        &contact_sheet.pixels,
        CURRENT_GENERATOR_VERSION,
    );

    fs::write(config.output.join("descriptors.csv"), descriptor_csv)?;
    fs::write(config.output.join("report.md"), report)?;
    write_ppm(
        &config.output.join("contact-sheet.ppm"),
        contact_sheet.width,
        contact_sheet.height,
        &contact_sheet.pixels,
    )?;
    update_baseline(&config.output, fingerprint, config.accept_baseline)?;

    println!(
        "World-quality audit wrote {} descriptors and {} viewpoint frames to {}",
        descriptors.len(),
        suites.len() * ViewpointKind::ALL.len(),
        config.output.display()
    );
    println!("fingerprint {fingerprint:016x}; {}", novelty.summary());
    Ok(())
}

fn audit_region_center(seed: u64, region_index: usize) -> [f64; 2] {
    let index = u64::try_from(region_index).expect("region index fits u64");
    let key = stable_hash(&[seed, DOMAIN_AUDIT_REGION, index]);
    let x_cell = signed_audit_cell(key);
    let z_cell = signed_audit_cell(key.rotate_left(29));
    [
        index_as_f64(x_cell) * 64_000.0,
        index_as_f64(z_cell) * 64_000.0,
    ]
}

fn signed_audit_cell(value: u64) -> i64 {
    let bounded = i64::try_from(value % 2_001).expect("bounded cell fits i64");
    bounded - 1_000
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(value: i64) -> f64 {
    value as f64
}

#[derive(Clone, Debug)]
struct LandscapeDescriptor {
    seed: u64,
    region_index: usize,
    center: [f64; 2],
    mean_elevation: f64,
    relief: f64,
    roughness: f64,
    mean_temperature: f64,
    precipitation: f64,
    canopy: f64,
    moisture: f64,
    river_fraction: f64,
    lake_fraction: f64,
    wetland_fraction: f64,
    reef_fraction: f64,
    cave_fraction: f64,
    family: LandscapeFamily,
}

impl LandscapeDescriptor {
    fn vector(&self) -> [f64; 9] {
        [
            normalize(self.mean_elevation, -200.0, 2_000.0),
            normalize(self.relief, 0.0, 2_000.0),
            normalize(self.roughness, 0.0, 500.0),
            normalize(self.mean_temperature, -20.0, 35.0),
            normalize(self.precipitation, 250.0, 2_500.0),
            self.canopy,
            self.moisture,
            self.river_fraction,
            (self.lake_fraction + self.wetland_fraction + self.reef_fraction + self.cave_fraction)
                .clamp(0.0, 1.0),
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum LandscapeFamily {
    Alpine,
    AridHighland,
    ForestedMountain,
    WetLowland,
    Coast,
    ReefCoast,
    Karst,
    TemperateUpland,
    OpenPlain,
}

impl LandscapeFamily {
    const ALL: [Self; 9] = [
        Self::Alpine,
        Self::AridHighland,
        Self::ForestedMountain,
        Self::WetLowland,
        Self::Coast,
        Self::ReefCoast,
        Self::Karst,
        Self::TemperateUpland,
        Self::OpenPlain,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Alpine => "alpine",
            Self::AridHighland => "arid highland",
            Self::ForestedMountain => "forested mountain",
            Self::WetLowland => "wet lowland",
            Self::Coast => "coast",
            Self::ReefCoast => "reef coast",
            Self::Karst => "karst",
            Self::TemperateUpland => "temperate upland",
            Self::OpenPlain => "open plain",
        }
    }
}

fn sample_descriptor(
    terrain: &GeneratedWorldTerrain,
    region_index: usize,
    center: [f64; 2],
) -> Result<LandscapeDescriptor, Box<dyn Error>> {
    let world = terrain.world();
    let mut elevations = Vec::with_capacity(AUDIT_GRID_EDGE * AUDIT_GRID_EDGE);
    let mut temperatures = 0.0;
    let mut precipitation = 0.0;
    let mut canopy = 0.0;
    let mut moisture = 0.0;
    let mut river_hits = 0;
    let mut lake_hits = 0;
    let mut wetland = 0.0;
    let mut reef = 0.0;
    let mut cave_hits = 0;
    let half = i32::try_from(AUDIT_GRID_EDGE / 2).expect("audit grid is small");

    for grid_z in -half..=half {
        for grid_x in -half..=half {
            let x = center[0] + (f64::from(grid_x) * AUDIT_SPACING_METERS);
            let z = center[1] + (f64::from(grid_z) * AUDIT_SPACING_METERS);
            elevations.push(
                terrain
                    .surface_height(x, z)
                    .ok_or("terrain sample unavailable")?,
            );
            let climate = Climate::new(world)
                .sample(x, z)
                .ok_or("climate sample unavailable")?;
            let soil = Soil::new(world)
                .sample(x, z)
                .ok_or("soil sample unavailable")?;
            let forest = ForestDistribution::new(world)
                .sample(x, z)
                .ok_or("forest sample unavailable")?;
            temperatures += climate.mean_temperature_celsius;
            precipitation += climate.annual_precipitation_millimeters;
            canopy += forest.canopy_cover_fraction;
            moisture += soil.surface_moisture;
            river_hits += usize::from(
                terrain
                    .river_influence_at(x, z)
                    .is_some_and(|river| river.distance_meters <= river.valley_half_width_meters),
            );
            lake_hits += usize::from(terrain.lake_surface_at(x, z).is_some());
            wetland += terrain
                .wetland_at(x, z)
                .map_or(0.0, |sample| sample.coverage_fraction);
            reef += terrain
                .reef_at(x, z)
                .map_or(0.0, |sample| sample.coverage_fraction);
            cave_hits += usize::from(terrain.cave_map_at(x, z).is_some());
        }
    }

    let sample_count = usize_as_f64(elevations.len());
    let mean_elevation = elevations.iter().sum::<f64>() / sample_count;
    let minimum = elevations.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = elevations.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let variance = elevations
        .iter()
        .map(|elevation| {
            let delta = elevation - mean_elevation;
            delta * delta
        })
        .sum::<f64>()
        / sample_count;
    let mut descriptor = LandscapeDescriptor {
        seed: world.seed,
        region_index,
        center,
        mean_elevation,
        relief: maximum - minimum,
        roughness: libm::sqrt(variance),
        mean_temperature: temperatures / sample_count,
        precipitation: precipitation / sample_count,
        canopy: canopy / sample_count,
        moisture: moisture / sample_count,
        river_fraction: usize_as_f64(river_hits) / sample_count,
        lake_fraction: usize_as_f64(lake_hits) / sample_count,
        wetland_fraction: wetland / sample_count,
        reef_fraction: reef / sample_count,
        cave_fraction: usize_as_f64(cave_hits) / sample_count,
        family: LandscapeFamily::OpenPlain,
    };
    descriptor.family = classify_landscape(&descriptor, terrain, center);
    Ok(descriptor)
}

fn classify_landscape(
    descriptor: &LandscapeDescriptor,
    terrain: &GeneratedWorldTerrain,
    center: [f64; 2],
) -> LandscapeFamily {
    let profile = RegionalProfile::sample(terrain.world(), center[0], center[1]);
    if descriptor.reef_fraction > 0.05 {
        LandscapeFamily::ReefCoast
    } else if descriptor.mean_elevation < 30.0 && descriptor.lake_fraction > 0.08 {
        LandscapeFamily::Coast
    } else if profile.is_some_and(|profile| profile.karst_probability > 0.72)
        && descriptor.cave_fraction > 0.0
    {
        LandscapeFamily::Karst
    } else if descriptor.mean_elevation > 850.0 && descriptor.mean_temperature < 4.0 {
        LandscapeFamily::Alpine
    } else if descriptor.relief > 650.0 && descriptor.moisture < 0.32 {
        LandscapeFamily::AridHighland
    } else if descriptor.relief > 550.0 && descriptor.canopy > 0.36 {
        LandscapeFamily::ForestedMountain
    } else if descriptor.wetland_fraction > 0.18 || descriptor.moisture > 0.72 {
        LandscapeFamily::WetLowland
    } else if descriptor.relief > 220.0 {
        LandscapeFamily::TemperateUpland
    } else {
        LandscapeFamily::OpenPlain
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

fn normalize(value: f64, minimum: f64, maximum: f64) -> f64 {
    ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
}

#[derive(Clone, Debug)]
struct NoveltyReport {
    closest_pair: Option<(usize, usize, f64)>,
    suspicious_pairs: Vec<(usize, usize, f64)>,
    family_counts: BTreeMap<LandscapeFamily, usize>,
    outliers: Vec<String>,
}

impl NoveltyReport {
    fn analyze(descriptors: &[LandscapeDescriptor]) -> Self {
        let mut closest_pair = None;
        let mut suspicious_pairs = Vec::new();
        for left in 0..descriptors.len() {
            for right in (left + 1)..descriptors.len() {
                let distance =
                    descriptor_distance(descriptors[left].vector(), descriptors[right].vector());
                if closest_pair.is_none_or(|(_, _, current)| distance < current) {
                    closest_pair = Some((left, right, distance));
                }
                if distance < 0.075 {
                    suspicious_pairs.push((left, right, distance));
                }
            }
        }
        let mut family_counts = BTreeMap::new();
        let mut outliers = Vec::new();
        for (index, descriptor) in descriptors.iter().enumerate() {
            *family_counts.entry(descriptor.family).or_default() += 1;
            if descriptor.relief > 2_400.0 {
                outliers.push(format!(
                    "region {index}: extreme relief {:.0} m",
                    descriptor.relief
                ));
            }
            if descriptor.mean_elevation < -180.0 || descriptor.mean_elevation > 2_300.0 {
                outliers.push(format!(
                    "region {index}: implausible mean elevation {:.0} m",
                    descriptor.mean_elevation
                ));
            }
            if descriptor.canopy > 0.75 && descriptor.moisture < 0.18 {
                outliers.push(format!(
                    "region {index}: dense canopy ({:.2}) in very dry soil ({:.2})",
                    descriptor.canopy, descriptor.moisture
                ));
            }
        }
        Self {
            closest_pair,
            suspicious_pairs,
            family_counts,
            outliers,
        }
    }

    fn summary(&self) -> String {
        format!(
            "{} suspicious pair(s), {} outlier(s), {} represented family/families",
            self.suspicious_pairs.len(),
            self.outliers.len(),
            self.family_counts.len()
        )
    }
}

fn descriptor_distance(left: [f64; 9], right: [f64; 9]) -> f64 {
    let squared = left
        .into_iter()
        .zip(right)
        .map(|(left, right)| {
            let delta = left - right;
            delta * delta
        })
        .sum::<f64>();
    libm::sqrt(squared / 9.0)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ViewpointKind {
    Valley,
    Ridge,
    River,
    Forest,
    LakeShore,
    Cave,
    Summit,
}

impl ViewpointKind {
    const ALL: [Self; 7] = [
        Self::Valley,
        Self::Ridge,
        Self::River,
        Self::Forest,
        Self::LakeShore,
        Self::Cave,
        Self::Summit,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Valley => "valley",
            Self::Ridge => "ridge",
            Self::River => "river",
            Self::Forest => "forest",
            Self::LakeShore => "lake shore",
            Self::Cave => "cave",
            Self::Summit => "summit",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Viewpoint {
    kind: ViewpointKind,
    position: [f64; 2],
    score: f64,
    qualified: bool,
}

#[derive(Clone, Debug)]
struct ViewpointSuite {
    seed: u64,
    viewpoints: Vec<Viewpoint>,
}

fn capture_viewpoint_suite(
    terrain: &GeneratedWorldTerrain,
    center: [f64; 2],
) -> Result<ViewpointSuite, Box<dyn Error>> {
    let world = terrain.world();
    let mut best = ViewpointKind::ALL.map(|kind| Viewpoint {
        kind,
        position: center,
        score: f64::NEG_INFINITY,
        qualified: false,
    });
    let half = i32::try_from(VIEW_SEARCH_EDGE / 2).expect("view search grid is small");
    for grid_z in -half..=half {
        for grid_x in -half..=half {
            let position = [
                center[0] + (f64::from(grid_x) * VIEW_SEARCH_SPACING_METERS),
                center[1] + (f64::from(grid_z) * VIEW_SEARCH_SPACING_METERS),
            ];
            let height = terrain
                .surface_height(position[0], position[1])
                .ok_or("viewpoint terrain sample unavailable")?;
            let base = WildernessTerrain::new(world)
                .erosion_at(position[0], position[1])
                .ok_or("viewpoint erosion sample unavailable")?;
            let forest = ForestDistribution::new(world)
                .sample(position[0], position[1])
                .ok_or("viewpoint forest sample unavailable")?;
            let profile = RegionalProfile::sample(world, position[0], position[1])
                .ok_or("viewpoint profile sample unavailable")?;
            let river = terrain.river_influence_at(position[0], position[1]);
            let lake = terrain.lake_surface_at(position[0], position[1]);
            let cave = terrain.cave_map_at(position[0], position[1]);
            let scores = [
                (
                    -height + (base.sediment_deposition_meters * 10.0),
                    base.slope < 0.08,
                ),
                (
                    height + (base.slope * 900.0) + (profile.uplift * 500.0),
                    base.slope > 0.04,
                ),
                (
                    river.map_or(-1_000.0, |river| {
                        river.segment.discharge_cubic_meters_per_second
                            - (river.distance_meters / river.valley_half_width_meters.max(1.0))
                    }),
                    river.is_some_and(|river| {
                        river.distance_meters <= river.valley_half_width_meters
                    }),
                ),
                (
                    forest.canopy_cover_fraction * 1_000.0,
                    forest.canopy_cover_fraction > 0.35,
                ),
                (
                    lake.map_or(-1_000.0, |lake| -lake.water_depth_meters.abs()),
                    lake.is_some(),
                ),
                (
                    cave.map_or(-1_000.0, |cave| -cave.horizontal_distance_meters),
                    cave.is_some(),
                ),
                (height, height > 600.0),
            ];
            for (candidate, (score, qualified)) in best.iter_mut().zip(scores) {
                let replaces = (qualified && !candidate.qualified)
                    || (qualified == candidate.qualified && score > candidate.score);
                if replaces {
                    candidate.position = position;
                    candidate.score = score;
                    candidate.qualified = qualified;
                }
            }
        }
    }
    Ok(ViewpointSuite {
        seed: world.seed,
        viewpoints: best.to_vec(),
    })
}

#[derive(Clone, Debug)]
struct RasterImage {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

fn render_contact_sheet(suites: &[ViewpointSuite]) -> Result<RasterImage, Box<dyn Error>> {
    let width = PANEL_WIDTH * CONTACT_SCALE * CONTACT_COLUMNS;
    let height = PANEL_HEIGHT * CONTACT_SCALE * suites.len();
    let mut pixels = vec![0; width * height * 3];
    for (row, suite) in suites.iter().enumerate() {
        let world = WorldIdentity::new(suite.seed, CURRENT_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        for (column, viewpoint) in suite.viewpoints.iter().enumerate() {
            let panel = render_panel(&terrain, *viewpoint)?;
            blit_panel(&mut pixels, width, row, column, &panel);
        }
    }
    Ok(RasterImage {
        width,
        height,
        pixels,
    })
}

fn render_panel(
    terrain: &GeneratedWorldTerrain,
    viewpoint: Viewpoint,
) -> Result<RasterImage, Box<dyn Error>> {
    let mut heights = Vec::with_capacity(PANEL_WIDTH * PANEL_HEIGHT);
    let mut colors = Vec::with_capacity(PANEL_WIDTH * PANEL_HEIGHT);
    let direction_key = stable_hash(&[
        terrain.world().seed,
        DOMAIN_VIEW_DIRECTION,
        viewpoint.kind as u64,
    ]);
    let quarter_turn = usize::try_from(direction_key & 3).expect("quarter turn fits usize");
    for pixel_y in 0..PANEL_HEIGHT {
        for pixel_x in 0..PANEL_WIDTH {
            let local = panel_local_position(pixel_x, pixel_y, quarter_turn);
            let x = viewpoint.position[0] + local[0];
            let z = viewpoint.position[1] + local[1];
            let height = terrain
                .surface_height(x, z)
                .ok_or("contact-sheet terrain sample unavailable")?;
            heights.push(height);
            colors.push(map_color(terrain, x, z, height)?);
        }
    }
    let mut pixels = Vec::with_capacity(PANEL_WIDTH * PANEL_HEIGHT * 3);
    for pixel_y in 0..PANEL_HEIGHT {
        for pixel_x in 0..PANEL_WIDTH {
            let index = pixel_y * PANEL_WIDTH + pixel_x;
            let left = heights[pixel_y * PANEL_WIDTH + pixel_x.saturating_sub(1)];
            let right = heights[pixel_y * PANEL_WIDTH + (pixel_x + 1).min(PANEL_WIDTH - 1)];
            let down = heights[pixel_y.saturating_sub(1) * PANEL_WIDTH + pixel_x];
            let up = heights[(pixel_y + 1).min(PANEL_HEIGHT - 1) * PANEL_WIDTH + pixel_x];
            let shade = (0.76 + ((left - right + down - up) / 240.0)).clamp(0.38, 1.18);
            let mut color = colors[index].map(|channel| (channel * shade).clamp(0.0, 1.0));
            if !viewpoint.qualified {
                color = [
                    color[0] * 0.72 + 0.18,
                    color[1] * 0.72 + 0.10,
                    color[2] * 0.72 + 0.18,
                ];
            }
            pixels.extend(color.map(float_channel));
        }
    }
    Ok(RasterImage {
        width: PANEL_WIDTH,
        height: PANEL_HEIGHT,
        pixels,
    })
}

fn panel_local_position(pixel_x: usize, pixel_y: usize, quarter_turn: usize) -> [f64; 2] {
    let x = ((usize_as_f64(pixel_x) + 0.5) / usize_as_f64(PANEL_WIDTH) - 0.5) * PANEL_SPAN_METERS;
    let z = ((usize_as_f64(pixel_y) + 0.5) / usize_as_f64(PANEL_HEIGHT) - 0.5) * PANEL_SPAN_METERS;
    match quarter_turn {
        0 => [x, z],
        1 => [-z, x],
        2 => [-x, -z],
        _ => [z, -x],
    }
}

fn map_color(
    terrain: &GeneratedWorldTerrain,
    x: f64,
    z: f64,
    elevation: f64,
) -> Result<[f64; 3], Box<dyn Error>> {
    if let Some(ocean) = terrain.ocean_surface_at(x, z) {
        let shallow = 1.0 - normalize(ocean.water_depth_meters, 0.0, 45.0);
        let reef = terrain
            .reef_at(x, z)
            .map_or(0.0, |reef| reef.coverage_fraction);
        return Ok([
            0.03 + (reef * 0.10),
            0.26 + (shallow * 0.18) + (reef * 0.20),
            0.46 + (shallow * 0.18),
        ]);
    }
    if let Some(lake) = terrain.lake_surface_at(x, z) {
        let shallow = 1.0 - normalize(lake.water_depth_meters, 0.0, 20.0);
        return Ok([0.04, 0.28 + (shallow * 0.10), 0.50 + (shallow * 0.10)]);
    }
    let soil = Soil::new(terrain.world())
        .sample(x, z)
        .ok_or("contact-sheet soil sample unavailable")?;
    let forest = ForestDistribution::new(terrain.world())
        .sample(x, z)
        .ok_or("contact-sheet forest sample unavailable")?;
    let climate = Climate::new(terrain.world())
        .sample(x, z)
        .ok_or("contact-sheet climate sample unavailable")?;
    let snow = normalize(
        climate.maximum_snowpack_water_equivalent_millimeters,
        40.0,
        900.0,
    ) * (1.0 - normalize(soil.slope, 0.25, 1.1));
    let rock = soil.rock_exposure;
    let green = [
        0.13 + ((1.0 - soil.surface_moisture) * 0.12),
        0.29 + (soil.surface_moisture * 0.13),
        0.10,
    ];
    let earth = [
        0.28 + (soil.composition.sand_fraction * 0.10),
        0.20,
        0.12 + (soil.composition.clay_fraction * 0.08),
    ];
    let stone = [0.39, 0.38, 0.35];
    let vegetation = (forest.canopy_cover_fraction * 0.72 + 0.16) * (1.0 - rock);
    let mut color = mix_f64(earth, green, vegetation);
    color = mix_f64(color, stone, rock);
    color = mix_f64(color, [0.88, 0.91, 0.93], snow);
    let altitude_haze = normalize(elevation, 800.0, 2_200.0) * 0.08;
    Ok(mix_f64(color, [0.62, 0.70, 0.75], altitude_haze))
}

fn mix_f64(start: [f64; 3], end: [f64; 3], amount: f64) -> [f64; 3] {
    std::array::from_fn(|channel| start[channel] + ((end[channel] - start[channel]) * amount))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn float_channel(value: f64) -> u8 {
    libm::round(value.clamp(0.0, 1.0) * 255.0) as u8
}

fn blit_panel(
    destination: &mut [u8],
    destination_width: usize,
    row: usize,
    column: usize,
    panel: &RasterImage,
) {
    for y in 0..panel.height {
        for scale_y in 0..CONTACT_SCALE {
            for x in 0..panel.width {
                let source = (y * panel.width + x) * 3;
                for scale_x in 0..CONTACT_SCALE {
                    let destination_x = (column * PANEL_WIDTH + x) * CONTACT_SCALE + scale_x;
                    let destination_y = (row * PANEL_HEIGHT + y) * CONTACT_SCALE + scale_y;
                    let destination_index = (destination_y * destination_width + destination_x) * 3;
                    destination[destination_index..destination_index + 3]
                        .copy_from_slice(&panel.pixels[source..source + 3]);
                }
            }
        }
    }
}

fn write_ppm(
    path: &Path,
    width: usize,
    height: usize,
    pixels: &[u8],
) -> Result<(), Box<dyn Error>> {
    if pixels.len() != width * height * 3 {
        return Err("PPM pixel buffer has the wrong size".into());
    }
    let mut file = File::create(path)?;
    write!(file, "P6\n{width} {height}\n255\n")?;
    file.write_all(pixels)?;
    Ok(())
}

fn descriptors_csv(descriptors: &[LandscapeDescriptor]) -> String {
    let mut csv = String::from(
        "seed,region,x,z,family,mean_elevation,relief,roughness,temperature,precipitation,\
         canopy,moisture,river_fraction,lake_fraction,wetland_fraction,reef_fraction,cave_fraction\n",
    );
    for descriptor in descriptors {
        writeln!(
            csv,
            "{:016x},{},{:.0},{:.0},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5}",
            descriptor.seed,
            descriptor.region_index,
            descriptor.center[0],
            descriptor.center[1],
            descriptor.family.label(),
            descriptor.mean_elevation,
            descriptor.relief,
            descriptor.roughness,
            descriptor.mean_temperature,
            descriptor.precipitation,
            descriptor.canopy,
            descriptor.moisture,
            descriptor.river_fraction,
            descriptor.lake_fraction,
            descriptor.wetland_fraction,
            descriptor.reef_fraction,
            descriptor.cave_fraction,
        )
        .expect("writing to String cannot fail");
    }
    csv
}

fn markdown_report(
    config: &AuditConfig,
    descriptors: &[LandscapeDescriptor],
    novelty: &NoveltyReport,
    suites: &[ViewpointSuite],
) -> String {
    let mut report = format!(
        "# Treeline world-quality audit\n\nGenerator version: {}\n\n\
         Seeds: {}\n\nRegions per seed: {}\n\n## Novelty\n\n{}\n\n",
        CURRENT_GENERATOR_VERSION,
        config
            .seeds
            .iter()
            .map(|seed| format!("`{seed:016x}`"))
            .collect::<Vec<_>>()
            .join(", "),
        config.regions_per_seed,
        novelty.summary(),
    );
    if let Some((left, right, distance)) = novelty.closest_pair {
        writeln!(
            report,
            "Closest pair: descriptor {left} and {right}, normalized distance `{distance:.5}`.\n"
        )
        .expect("writing to String cannot fail");
    }
    report.push_str("| Landscape family | Regions |\n|---|---:|\n");
    for family in LandscapeFamily::ALL {
        writeln!(
            report,
            "| {} | {} |",
            family.label(),
            novelty.family_counts.get(&family).copied().unwrap_or(0)
        )
        .expect("writing to String cannot fail");
    }
    report.push_str("\n## Findings\n\n");
    if novelty.suspicious_pairs.is_empty() && novelty.outliers.is_empty() {
        report.push_str("No descriptor duplicates or configured plausibility outliers detected.\n");
    } else {
        for &(left, right, distance) in &novelty.suspicious_pairs {
            writeln!(
                report,
                "- Repetition candidate: descriptors {left} and {right} have distance `{distance:.5}`."
            )
            .expect("writing to String cannot fail");
        }
        for outlier in &novelty.outliers {
            writeln!(report, "- {outlier}").expect("writing to String cannot fail");
        }
    }
    report.push_str("\n## Viewpoint coverage\n\n");
    for suite in suites {
        writeln!(report, "Seed `{0:016x}`:", suite.seed).expect("writing to String cannot fail");
        for viewpoint in &suite.viewpoints {
            writeln!(
                report,
                "- {} at ({:.0}, {:.0}): {}",
                viewpoint.kind.label(),
                viewpoint.position[0],
                viewpoint.position[1],
                if viewpoint.qualified {
                    "feature found"
                } else {
                    "fallback frame; feature underrepresented in search area"
                }
            )
            .expect("writing to String cannot fail");
        }
        report.push('\n');
    }
    report.push_str(
        "The binary `contact-sheet.ppm` stores the deterministic hill-shaded frames in the \
         fixed viewpoint order valley, ridge, river, forest, lake shore, cave, summit. \
         Magenta-tinted panels are explicit fallback frames rather than false positives.\n",
    );
    if descriptors.is_empty() {
        report.push_str("\nNo descriptors were captured.\n");
    }
    report
}

fn audit_fingerprint(descriptors: &[u8], pixels: &[u8], generator_version: u32) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for byte in generator_version
        .to_le_bytes()
        .into_iter()
        .chain(descriptors.iter().copied())
        .chain(pixels.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn update_baseline(
    output: &Path,
    fingerprint: u64,
    accept_baseline: bool,
) -> Result<(), Box<dyn Error>> {
    let baseline_path = output.join("baseline.txt");
    let current = format!("{fingerprint:016x}\n");
    let previous = read_optional_string(&baseline_path)?;
    let (status, should_write) = match previous.as_deref().map(str::trim) {
        None => ("CREATED: no prior baseline existed.", true),
        Some(existing) if existing == current.trim() => {
            ("PASS: audit matches the baseline.", false)
        }
        Some(_) if accept_baseline => ("ACCEPTED: baseline updated by explicit --accept.", true),
        Some(_) => (
            "CHANGED: audit differs from baseline; baseline retained. Review report.md and contact-sheet.ppm, then rerun with --accept if intentional.",
            false,
        ),
    };
    if should_write {
        fs::write(baseline_path, current)?;
    }
    fs::write(output.join("regression-status.txt"), format!("{status}\n"))?;
    Ok(())
}

fn read_optional_string(path: &Path) -> Result<Option<String>, Box<dyn Error>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut value = String::new();
    file.read_to_string(&mut value)?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_arguments_accept_hexadecimal_seeds_and_preserve_baselines_by_default() {
        let arguments = [
            "--regions".to_owned(),
            "4".to_owned(),
            "--seeds".to_owned(),
            "0x5eed,42".to_owned(),
        ];
        let config = AuditConfig::parse(&arguments).expect("valid audit arguments");
        assert_eq!(config.regions_per_seed, 4);
        assert_eq!(config.seeds, [0x5eed, 42]);
        assert!(!config.accept_baseline);
    }

    #[test]
    fn descriptor_distance_detects_exact_repetition() {
        let descriptor = [0.2, 0.4, 0.6, 0.8, 1.0, 0.1, 0.3, 0.5, 0.7];
        assert!(descriptor_distance(descriptor, descriptor).abs() < f64::EPSILON);
        assert!(descriptor_distance(descriptor, [0.0; 9]) > 0.0);
    }

    #[test]
    fn audit_region_centers_are_stable_and_spatially_separated() {
        let first = audit_region_center(0x5eed, 3).map(f64::to_bits);
        let repeated = audit_region_center(0x5eed, 3).map(f64::to_bits);
        let next = audit_region_center(0x5eed, 4).map(f64::to_bits);
        assert_eq!(first, repeated);
        assert_ne!(first, next);
        assert!(
            audit_region_center(0x5eed, 3)
                .into_iter()
                .all(|coordinate| coordinate % 64_000.0 == 0.0)
        );
    }

    #[test]
    fn viewpoint_suite_has_a_fixed_regression_order() {
        assert_eq!(ViewpointKind::ALL.len(), CONTACT_COLUMNS);
        assert_eq!(ViewpointKind::ALL[0], ViewpointKind::Valley);
        assert_eq!(ViewpointKind::ALL[6], ViewpointKind::Summit);
    }

    #[test]
    fn fingerprint_changes_with_pixels_or_generator_version() {
        let first = audit_fingerprint(b"descriptor", &[1, 2, 3], 16);
        assert_ne!(first, audit_fingerprint(b"descriptor", &[1, 2, 4], 16));
        assert_ne!(first, audit_fingerprint(b"descriptor", &[1, 2, 3], 17));
    }
}
