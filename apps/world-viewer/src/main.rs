use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use treeline_coordinates::{WorldIdentity, stable_hash};
use treeline_ecology::{
    EcosystemDistribution, ForestDistribution, GroundVegetationDistribution, Soil,
};
use treeline_geography::{Climate, ProvincePlan, RegionalProfile};
use treeline_renderer::terrain_tier;
use treeline_terrain::{CalibratedTerrain, LandformParameters, SurfaceField, WildernessTerrain};
use treeline_voxel::LodLevel;
use treeline_world::{CURRENT_GENERATOR_VERSION, GeneratedWorldTerrain, GenerationPriority};

const DEFAULT_OUTPUT: &str = "artifacts/world-quality";
const DEFAULT_REGION_COUNT: usize = 12;
const DEFAULT_SEEDS: [u64; 3] = [0x5eed, 0xa11c_e5ed, 0xd15c_0a7e];
const AUDIT_SPACING_METERS: f64 = 4_000.0;
const AUDIT_GRID_EDGE: usize = 7;
const TERRAIN_AUDIT_SPACING_METERS: f64 = 500.0;
const TERRAIN_AUDIT_GRID_EDGE: usize = 49;
const QUIET_SLOPE_MAXIMUM: f64 = 0.035;
const ROLLING_SLOPE_MAXIMUM: f64 = 0.25;
const CLIFF_SLOPE_MINIMUM: f64 = 0.75;
const COHERENT_CLIFF_MINIMUM_CELLS: usize = 3;
const SPIKE_PROMINENCE_MINIMUM: f64 = 0.75;
const VIEW_SEARCH_SPACING_METERS: f64 = 2_000.0;
const VIEW_SEARCH_EDGE: usize = 9;
const CURATED_DISCOVERY_GRID_EDGE: usize = 9;
const CURATED_DISCOVERY_CELL_METERS: i64 = 12_000_000;
const CURATED_DISCOVERY_JITTER_METERS: i64 = 2_000_000;
const CURATED_CENTER_MINIMUM_SEPARATION_METERS: f64 = 1_024_000.0;
const PANEL_WIDTH: usize = 48;
const PANEL_HEIGHT: usize = 36;
const CONTACT_SCALE: usize = 3;
const PANEL_SPAN_METERS: f64 = 12_000.0;
const CONTACT_COLUMNS: usize = 17;
const DOMAIN_AUDIT_REGION: u64 = 0x574f_524c_445f_4155;
const DOMAIN_CURATED_VIEW_CENTER: u64 = 0x4355_5241_5445_5657;
const DOMAIN_VIEW_DIRECTION: u64 = 0x5649_4557_5f44_4952;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "heightmap-batch")
    {
        run_heightmap_batch(&arguments[1..])?;
        return Ok(());
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "terrain-parameters")
    {
        print_terrain_parameters()?;
        return Ok(());
    }
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
        "world-viewer audit [--output PATH] [--regions COUNT] [--seeds LIST] [--accept] [--require-coverage]\n\
         world-viewer heightmap-batch --request PATH --output PATH\n\
         world-viewer terrain-parameters\n\
         Seeds are comma-separated decimal or 0x-prefixed u64 values. Existing baselines are\n\
         compared but retained unless --accept is supplied. --require-coverage exits unsuccessfully\n\
         when any required visible outcome or qualified viewpoint remains missing."
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuditConfig {
    output: PathBuf,
    regions_per_seed: usize,
    seeds: Vec<u64>,
    accept_baseline: bool,
    require_coverage: bool,
}

impl AuditConfig {
    fn parse(arguments: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut config = Self {
            output: PathBuf::from(DEFAULT_OUTPUT),
            regions_per_seed: DEFAULT_REGION_COUNT,
            seeds: DEFAULT_SEEDS.to_vec(),
            accept_baseline: false,
            require_coverage: false,
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
                "--require-coverage" => config.require_coverage = true,
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

fn print_terrain_parameters() -> Result<(), Box<dyn Error>> {
    let values = LandformParameters::for_generator_version(CURRENT_GENERATOR_VERSION)
        .named_values()
        .into_iter()
        .map(|(name, value)| (name.to_owned(), serde_json::Value::from(value)))
        .collect::<serde_json::Map<_, _>>();
    println!("{}", serde_json::to_string_pretty(&values)?);
    Ok(())
}

enum HeightmapSampler {
    Landform(Box<CalibratedTerrain>),
    Composed(GeneratedWorldTerrain),
}

impl HeightmapSampler {
    fn height_at(&self, x: f64, z: f64) -> Option<f64> {
        match self {
            Self::Landform(terrain) => terrain.height_at(x, z),
            Self::Composed(terrain) => terrain.surface_height(x, z),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn run_heightmap_batch(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let mut request_path = None;
    let mut output_path = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--request" => {
                index += 1;
                request_path = Some(PathBuf::from(
                    arguments.get(index).ok_or("--request requires a path")?,
                ));
            }
            "--output" => {
                index += 1;
                output_path = Some(PathBuf::from(
                    arguments.get(index).ok_or("--output requires a path")?,
                ));
            }
            unknown => return Err(format!("unknown heightmap-batch argument: {unknown}").into()),
        }
        index += 1;
    }
    let request_path = request_path.ok_or("heightmap-batch requires --request")?;
    let output_path = output_path.ok_or("heightmap-batch requires --output")?;
    let request: serde_json::Value = serde_json::from_str(&fs::read_to_string(request_path)?)?;
    let object = request
        .as_object()
        .ok_or("heightmap request must be a JSON object")?;
    let generator_version =
        object
            .get("generator_version")
            .map_or(Ok(CURRENT_GENERATOR_VERSION), |value| {
                value
                    .as_u64()
                    .ok_or("generator_version must be an unsigned integer")
                    .and_then(|version| {
                        u32::try_from(version).map_err(|_| "generator_version exceeds u32")
                    })
            })?;
    if generator_version < treeline_geography::PROVINCE_GENERATOR_VERSION {
        return Err("heightmap calibration requires generator version 18 or newer".into());
    }
    let sampler_name = object.get("sampler").map_or(Ok("landform"), |value| {
        value.as_str().ok_or("sampler must be a string")
    })?;
    if !matches!(sampler_name, "landform" | "composed") {
        return Err(format!("unknown heightmap sampler: {sampler_name}").into());
    }

    let mut parameters = LandformParameters::for_generator_version(generator_version);
    if let Some(overrides) = object.get("parameters") {
        let overrides = overrides
            .as_object()
            .ok_or("parameters must be a JSON object")?;
        if sampler_name == "composed" && !overrides.is_empty() {
            return Err("composed heightmaps do not accept offline landform overrides".into());
        }
        for (name, value) in overrides {
            let value = value
                .as_f64()
                .ok_or("landform parameter values must be numbers")?;
            parameters
                .set_named(name, value)
                .map_err(|reason| format!("invalid parameter {name}: {reason}"))?;
        }
    }
    let parameter_words = parameters
        .named_values()
        .into_iter()
        .map(|(_, value)| value.to_bits())
        .collect::<Vec<_>>();
    let parameter_fingerprint = stable_hash(&parameter_words);
    let rasters = object
        .get("rasters")
        .and_then(serde_json::Value::as_array)
        .ok_or("heightmap request requires a rasters array")?;
    if rasters.is_empty() {
        return Err("heightmap request must contain at least one raster".into());
    }
    fs::create_dir_all(&output_path)?;

    for raster in rasters {
        let raster = raster
            .as_object()
            .ok_or("each raster request must be a JSON object")?;
        let id = raster
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("raster id must be a string")?;
        if id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(format!("unsafe raster id: {id}").into());
        }
        let seed_value = raster.get("seed").ok_or("raster seed is required")?;
        let seed = parse_json_seed(seed_value)?;
        let center_x = json_f64(raster, "center_x_meters")?;
        let center_z = json_f64(raster, "center_z_meters")?;
        let span = json_f64(raster, "span_meters")?;
        if span <= 0.0 {
            return Err("span_meters must be positive".into());
        }
        let edge_u64 = raster
            .get("edge")
            .and_then(serde_json::Value::as_u64)
            .ok_or("raster edge must be an unsigned integer")?;
        let edge = usize::try_from(edge_u64).map_err(|_| "raster edge exceeds usize")?;
        if !(2..=4_096).contains(&edge) {
            return Err("raster edge must be between 2 and 4096".into());
        }
        let spacing = span / usize_as_f64(edge);
        let minimum_x = center_x - (span * 0.5);
        let minimum_z = center_z - (span * 0.5);
        // Keep common random numbers across optimizer proposals. The parameter
        // fingerprint identifies the artifact but must not reseed its fields.
        let world = WorldIdentity::new(seed, generator_version, 0);
        let terrain = match sampler_name {
            "landform" => HeightmapSampler::Landform(Box::new(
                CalibratedTerrain::new(world, parameters)
                    .ok_or("calibrated terrain rejected the request")?,
            )),
            "composed" => HeightmapSampler::Composed(GeneratedWorldTerrain::new(world)),
            _ => unreachable!("sampler name validated above"),
        };
        let mut bytes = Vec::with_capacity(edge * edge * std::mem::size_of::<f32>());
        let mut minimum_height = f64::INFINITY;
        let mut maximum_height = f64::NEG_INFINITY;
        for row in 0..edge {
            let z = minimum_z + ((usize_as_f64(row) + 0.5) * spacing);
            for column in 0..edge {
                let x = minimum_x + ((usize_as_f64(column) + 0.5) * spacing);
                let height = terrain
                    .height_at(x, z)
                    .ok_or("terrain sample unavailable")?;
                if !height.is_finite() || height.abs() > f64::from(f32::MAX) {
                    return Err("calibrated terrain produced a height outside f32 range".into());
                }
                minimum_height = minimum_height.min(height);
                maximum_height = maximum_height.max(height);
                bytes.extend_from_slice(&heightmap_f32(height).to_le_bytes());
            }
        }
        fs::write(output_path.join(format!("{id}.f32")), bytes)?;
        let metadata = serde_json::json!({
            "id": id,
            "format": "little-endian-f32-row-major-south-to-north",
            "edge": edge,
            "span_meters": span,
            "spacing_meters": spacing,
            "center_x_meters": center_x,
            "center_z_meters": center_z,
            "seed": format!("{seed:016x}"),
            "generator_version": generator_version,
            "sampler": sampler_name,
            "parameter_fingerprint": format!("{parameter_fingerprint:016x}"),
            "minimum_height_meters": minimum_height,
            "maximum_height_meters": maximum_height,
        });
        fs::write(
            output_path.join(format!("{id}.json")),
            serde_json::to_vec_pretty(&metadata)?,
        )?;
    }
    println!(
        "Heightmap batch wrote {} raster(s) to {} with parameter fingerprint {parameter_fingerprint:016x}",
        rasters.len(),
        output_path.display()
    );
    Ok(())
}

fn parse_json_seed(value: &serde_json::Value) -> Result<u64, Box<dyn Error>> {
    if let Some(seed) = value.as_u64() {
        return Ok(seed);
    }
    let text = value
        .as_str()
        .ok_or("raster seed must be an unsigned integer or string")?;
    Ok(text.strip_prefix("0x").map_or_else(
        || text.parse::<u64>(),
        |hexadecimal| u64::from_str_radix(hexadecimal, 16),
    )?)
}

fn json_f64(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<f64, Box<dyn Error>> {
    let value = object
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("raster {key} must be a number"))?;
    if !value.is_finite() {
        return Err(format!("raster {key} must be finite").into());
    }
    Ok(value)
}

#[allow(clippy::cast_possible_truncation)]
fn heightmap_f32(value: f64) -> f32 {
    value as f32
}

fn run_audit(config: &AuditConfig) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(&config.output)?;
    let mut descriptors = Vec::with_capacity(config.seeds.len() * config.regions_per_seed);
    let mut suites = Vec::with_capacity(config.seeds.len());

    for &seed in &config.seeds {
        let world = WorldIdentity::new(seed, CURRENT_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let mut centers = Vec::with_capacity(config.regions_per_seed);
        for region_index in 0..config.regions_per_seed {
            let center = audit_region_center(seed, region_index);
            centers.push(center);
            descriptors.push(sample_descriptor(&terrain, region_index, center)?);
        }
        let curated_centers = discover_curated_view_centers(world, &centers)?;
        suites.push(capture_viewpoint_suite(
            &terrain,
            &centers,
            &curated_centers,
        )?);
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
    let missing_viewpoints = missing_viewpoints(&suites);
    let acceptance_passed = novelty.coverage_findings.is_empty()
        && novelty.outliers.is_empty()
        && novelty.suspicious_pairs.is_empty()
        && missing_viewpoints.is_empty();
    update_baseline(
        &config.output,
        fingerprint,
        config.accept_baseline && (!config.require_coverage || acceptance_passed),
    )?;

    println!(
        "World-quality audit wrote {} descriptors and {} viewpoint frames to {}",
        descriptors.len(),
        suites.len() * ViewpointKind::ALL.len(),
        config.output.display()
    );
    println!("fingerprint {fingerprint:016x}; {}", novelty.summary());
    if config.require_coverage && !acceptance_passed {
        return Err(format!(
            "landscape acceptance failed: {} coverage finding(s), {} outlier(s), {} suspicious pair(s), {} missing viewpoint family/families",
            novelty.coverage_findings.len(),
            novelty.outliers.len(),
            novelty.suspicious_pairs.len(),
            missing_viewpoints.len(),
        )
        .into());
    }
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(usize)]
enum CuratedCause {
    Mountain,
    Dune,
    Scarp,
    AlpineGlacial,
    Forest,
    Prairie,
    SteppeShrubland,
    DesertSaltBasin,
    River,
    Lake,
    Coast,
    Wetland,
    Reef,
}

impl CuratedCause {
    const ALL: [Self; 13] = [
        Self::Mountain,
        Self::Dune,
        Self::Scarp,
        Self::AlpineGlacial,
        Self::Forest,
        Self::Prairie,
        Self::SteppeShrubland,
        Self::DesertSaltBasin,
        Self::River,
        Self::Lake,
        Self::Coast,
        Self::Wetland,
        Self::Reef,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Mountain => "mountain",
            Self::Dune => "dune",
            Self::Scarp => "scarp / bluff",
            Self::AlpineGlacial => "alpine / glacial",
            Self::Forest => "forest",
            Self::Prairie => "prairie / grassland",
            Self::SteppeShrubland => "steppe / shrubland",
            Self::DesertSaltBasin => "desert / salt basin",
            Self::River => "river",
            Self::Lake => "lake",
            Self::Coast => "coast",
            Self::Wetland => "wetland",
            Self::Reef => "reef",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug)]
struct CausalCandidate {
    position: [f64; 2],
    scores: [f64; CuratedCause::ALL.len()],
}

#[derive(Clone, Copy, Debug)]
struct CuratedViewCenter {
    cause: CuratedCause,
    position: [f64; 2],
    score: f64,
}

fn discover_curated_view_centers(
    world: WorldIdentity,
    randomized_centers: &[[f64; 2]],
) -> Result<Vec<CuratedViewCenter>, Box<dyn Error>> {
    let candidates = curated_discovery_positions(world.seed)
        .into_iter()
        .filter_map(|position| sample_causal_candidate(world, position))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err("curated viewpoint discovery produced no valid causal samples".into());
    }
    Ok(select_curated_view_centers(
        &candidates,
        randomized_centers,
        CURATED_CENTER_MINIMUM_SEPARATION_METERS,
    ))
}

fn curated_discovery_positions(seed: u64) -> Vec<[f64; 2]> {
    let count = CURATED_DISCOVERY_GRID_EDGE * CURATED_DISCOVERY_GRID_EDGE;
    let half =
        i64::try_from(CURATED_DISCOVERY_GRID_EDGE / 2).expect("discovery grid edge is small");
    (0..count)
        .map(|index| {
            let index_u64 = u64::try_from(index).expect("discovery index fits u64");
            let key = stable_hash(&[seed, DOMAIN_CURATED_VIEW_CENTER, index_u64]);
            let column = i64::try_from(index % CURATED_DISCOVERY_GRID_EDGE)
                .expect("discovery column fits i64");
            let row =
                i64::try_from(index / CURATED_DISCOVERY_GRID_EDGE).expect("discovery row fits i64");
            let x = (column - half) * CURATED_DISCOVERY_CELL_METERS + curated_discovery_jitter(key);
            let z = (row - half) * CURATED_DISCOVERY_CELL_METERS
                + curated_discovery_jitter(key.rotate_left(31));
            [index_as_f64(x), index_as_f64(z)]
        })
        .collect()
}

fn curated_discovery_jitter(value: u64) -> i64 {
    let span = u64::try_from(CURATED_DISCOVERY_JITTER_METERS * 2 + 1)
        .expect("curated jitter span fits u64");
    i64::try_from(value % span).expect("bounded curated jitter fits i64")
        - CURATED_DISCOVERY_JITTER_METERS
}

#[allow(clippy::too_many_lines)]
fn sample_causal_candidate(world: WorldIdentity, position: [f64; 2]) -> Option<CausalCandidate> {
    let [x, z] = position;
    let province = ProvincePlan::sample_at(world, x, z)?;
    let ecosystem = EcosystemDistribution::new(world).sample(x, z)?;
    let erosion = WildernessTerrain::new(world).erosion_at(x, z)?;

    let height = erosion.surface_height_meters();
    let slope = normalize(erosion.slope, 0.04, 0.72);
    let flatness = 1.0 - normalize(erosion.slope, 0.02, 0.24);
    let relief = normalize(province.macro_relief_meters, 180.0, 1_800.0);
    let highland = normalize(height, 420.0, 2_500.0);
    let openness = 1.0 - ecosystem.closed_forest_potential;
    let scarp_face = province
        .scarp_geometry
        .map_or(0.0, |geometry| geometry.face_strength);
    let dune_geometry = province
        .dune_geometry
        .map_or(0.0, |geometry| geometry.strength);
    let mountain = (province.mountain * 0.46
        + relief * 0.22
        + highland * 0.12
        + slope * 0.12
        + province.uplift * 0.08)
        .clamp(0.0, 1.0);
    let dune = (province.dune * 0.48
        + dune_geometry * 0.18
        + province.aridity * 0.12
        + province.sediment * 0.08
        + flatness * 0.06
        + openness * 0.08)
        .clamp(0.0, 1.0);
    let scarp = (province.scarp * 0.46
        + scarp_face * 0.24
        + slope * 0.16
        + erosion.rock_exposure * 0.08
        + province.faulting * 0.06)
        .clamp(0.0, 1.0);
    let alpine_glacial = (province.glacial * 0.24
        + province.glaciation * 0.12
        + ecosystem.tundra_potential * 0.12
        + ecosystem.exposed_alpine_potential * 0.22
        + ecosystem.above_tree_line_fraction * 0.14
        + highland * 0.08
        + ecosystem.exposure_fraction * 0.08)
        .clamp(0.0, 1.0);
    let forest = (ecosystem.closed_forest_potential * 0.68
        + ecosystem.water_balance_fraction * 0.12
        + (1.0 - ecosystem.exposure_fraction) * 0.10
        + (1.0 - ecosystem.fire_pressure_fraction) * 0.10)
        .clamp(0.0, 1.0);
    let prairie = (ecosystem.grassland_prairie_potential * 0.68
        + province.plains * 0.12
        + openness * 0.08
        + ecosystem.fire_pressure_fraction * 0.12)
        .clamp(0.0, 1.0);
    let steppe_shrubland = (ecosystem.steppe_potential * 0.44
        + ecosystem.shrubland_potential * 0.24
        + province.aridity * 0.12
        + openness * 0.10
        + ecosystem.disturbance_fraction * 0.10)
        .clamp(0.0, 1.0);
    let desert_salt_basin = (ecosystem.desert_potential * 0.46
        + province.aridity * 0.16
        + ecosystem.salinity_fraction * 0.12
        + ecosystem.closed_basin_fraction * 0.12
        + openness * 0.08
        + flatness * 0.06)
        .clamp(0.0, 1.0);
    let river = (ecosystem.water_balance_fraction * 0.28
        + province.drainage * 0.28
        + province.moisture * 0.18
        + ecosystem.sediment_fraction * 0.14
        + (1.0 - ecosystem.closed_basin_fraction) * 0.12)
        .clamp(0.0, 1.0);
    let lake = (ecosystem.water_balance_fraction * 0.22
        + ecosystem.closed_basin_fraction * 0.26
        + ecosystem.wetland_potential * 0.14
        + ecosystem.sediment_fraction * 0.14
        + flatness * 0.16
        + province.drainage * 0.08)
        .clamp(0.0, 1.0);
    let coast = (province.coast_fraction * 0.78
        + (1.0 - (province.land_fraction - 0.5).abs() * 2.0).clamp(0.0, 1.0) * 0.22)
        .clamp(0.0, 1.0);
    let wetland = (ecosystem.wetland_potential * 0.66
        + ecosystem.water_balance_fraction * 0.14
        + ecosystem.sediment_fraction * 0.10
        + flatness * 0.10)
        .clamp(0.0, 1.0);
    let reef = (province.coast_fraction * 0.30
        + (1.0 - province.land_fraction) * 0.22
        + province.temperature * 0.18
        + province.carbonate_fraction * 0.16
        + (1.0 - province.salinity) * 0.14)
        .clamp(0.0, 1.0);

    Some(CausalCandidate {
        position,
        scores: [
            mountain,
            dune,
            scarp,
            alpine_glacial,
            forest,
            prairie,
            steppe_shrubland,
            desert_salt_basin,
            river,
            lake,
            coast,
            wetland,
            reef,
        ],
    })
}

fn select_curated_view_centers(
    candidates: &[CausalCandidate],
    randomized_centers: &[[f64; 2]],
    minimum_separation_meters: f64,
) -> Vec<CuratedViewCenter> {
    let minimum_distance_squared = minimum_separation_meters * minimum_separation_meters;
    let mut selected = Vec::with_capacity(CuratedCause::ALL.len());
    for cause in CuratedCause::ALL {
        let mut best: Option<&CausalCandidate> = None;
        for candidate in candidates {
            if !candidate.scores[cause.index()].is_finite()
                || !position_is_separated(
                    candidate.position,
                    randomized_centers,
                    &selected,
                    minimum_distance_squared,
                )
            {
                continue;
            }
            if best.is_none_or(|current| causal_candidate_is_better(candidate, current, cause)) {
                best = Some(candidate);
            }
        }
        if let Some(candidate) = best {
            selected.push(CuratedViewCenter {
                cause,
                position: candidate.position,
                score: candidate.scores[cause.index()],
            });
        }
    }
    selected
}

fn position_is_separated(
    candidate: [f64; 2],
    randomized_centers: &[[f64; 2]],
    selected: &[CuratedViewCenter],
    minimum_distance_squared: f64,
) -> bool {
    randomized_centers
        .iter()
        .copied()
        .chain(selected.iter().map(|center| center.position))
        .all(|existing| {
            squared_horizontal_distance(candidate, existing) >= minimum_distance_squared
        })
}

fn squared_horizontal_distance(left: [f64; 2], right: [f64; 2]) -> f64 {
    let x = left[0] - right[0];
    let z = left[1] - right[1];
    x * x + z * z
}

fn causal_candidate_is_better(
    candidate: &CausalCandidate,
    current: &CausalCandidate,
    cause: CuratedCause,
) -> bool {
    let score_order = candidate.scores[cause.index()].total_cmp(&current.scores[cause.index()]);
    score_order.is_gt()
        || (score_order.is_eq()
            && (candidate.position[0]
                .total_cmp(&current.position[0])
                .is_lt()
                || (candidate.position[0].to_bits() == current.position[0].to_bits()
                    && candidate.position[1]
                        .total_cmp(&current.position[1])
                        .is_lt())))
}

#[derive(Clone, Debug)]
struct LandscapeDescriptor {
    seed: u64,
    region_index: usize,
    center: [f64; 2],
    mean_elevation: f64,
    relief: f64,
    roughness: f64,
    mean_slope: f64,
    p95_slope: f64,
    maximum_slope: f64,
    mean_curvature: f64,
    p95_curvature: f64,
    quiet_terrain_fraction: f64,
    rolling_terrain_fraction: f64,
    steep_terrain_fraction: f64,
    cliff_terrain_fraction: f64,
    coherent_cliff_fraction: f64,
    largest_cliff_patch_fraction: f64,
    spike_outlier_fraction: f64,
    mean_temperature: f64,
    precipitation: f64,
    snowpack: f64,
    dryness: f64,
    salinity: f64,
    water_balance_fraction: f64,
    above_tree_line_fraction: f64,
    exposure_fraction: f64,
    fire_pressure_fraction: f64,
    disturbance_fraction: f64,
    sediment_fraction: f64,
    closed_forest_potential: f64,
    open_woodland_potential: f64,
    grassland_prairie_potential: f64,
    steppe_potential: f64,
    shrubland_potential: f64,
    desert_potential: f64,
    tundra_potential: f64,
    exposed_alpine_potential: f64,
    wetland_potential: f64,
    sand_fraction: f64,
    rock_exposure: f64,
    canopy: f64,
    closed_forest_fraction: f64,
    mean_canopy_height: f64,
    ground_cover: f64,
    graminoid_fraction: f64,
    shrub_fraction: f64,
    open_land_fraction: f64,
    largest_open_patch_fraction: f64,
    tree_identity: TreeIdentity,
    tree_identity_strength: f64,
    river_fraction: f64,
    lake_fraction: f64,
    ocean_fraction: f64,
    wetland_fraction: f64,
    reef_fraction: f64,
    cave_fraction: f64,
    karst_probability: f64,
    province_scarp_signal: f64,
    province_mountain_signal: f64,
    closed_basin_signal: f64,
    dune_plan_signal: f64,
    glacial_plan_signal: f64,
    dune_signal: f64,
    alpine_glacial_signal: f64,
    family: LandscapeFamily,
}

impl LandscapeDescriptor {
    fn vector(&self) -> [f64; 60] {
        [
            normalize(self.mean_elevation, -200.0, 2_000.0),
            normalize(self.relief, 0.0, 3_000.0),
            normalize(self.roughness, 0.0, 800.0),
            normalize(self.mean_slope, 0.0, 1.5),
            normalize(self.p95_slope, 0.0, 2.5),
            normalize(self.maximum_slope, 0.0, 4.0),
            normalize(self.mean_curvature, 0.0, 1.5),
            normalize(self.p95_curvature, 0.0, 3.0),
            self.quiet_terrain_fraction,
            self.rolling_terrain_fraction,
            self.steep_terrain_fraction,
            self.cliff_terrain_fraction,
            self.coherent_cliff_fraction,
            self.largest_cliff_patch_fraction,
            self.spike_outlier_fraction,
            normalize(self.mean_temperature, -20.0, 35.0),
            normalize(self.precipitation, 250.0, 2_500.0),
            normalize(self.snowpack, 0.0, 1_200.0),
            self.dryness,
            self.salinity,
            self.water_balance_fraction,
            self.above_tree_line_fraction,
            self.exposure_fraction,
            self.fire_pressure_fraction,
            self.disturbance_fraction,
            self.sediment_fraction,
            self.closed_forest_potential,
            self.open_woodland_potential,
            self.grassland_prairie_potential,
            self.steppe_potential,
            self.shrubland_potential,
            self.desert_potential,
            self.tundra_potential,
            self.exposed_alpine_potential,
            self.wetland_potential,
            self.sand_fraction,
            self.rock_exposure,
            self.canopy,
            self.closed_forest_fraction,
            normalize(self.mean_canopy_height, 0.0, 31.0),
            self.ground_cover,
            self.graminoid_fraction,
            self.shrub_fraction,
            self.open_land_fraction,
            self.largest_open_patch_fraction,
            self.tree_identity_strength,
            self.river_fraction,
            self.lake_fraction,
            self.ocean_fraction,
            self.wetland_fraction,
            self.reef_fraction,
            self.cave_fraction,
            self.karst_probability,
            self.province_scarp_signal,
            self.province_mountain_signal,
            self.closed_basin_signal,
            self.dune_plan_signal,
            self.glacial_plan_signal,
            self.dune_signal,
            self.alpine_glacial_signal,
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TreeIdentity {
    EvergreenNeedleleaf,
    ColdDeciduous,
    TemperateBroadleaf,
    DryWoodland,
    Mixed,
}

impl TreeIdentity {
    const ALL: [Self; 5] = [
        Self::EvergreenNeedleleaf,
        Self::ColdDeciduous,
        Self::TemperateBroadleaf,
        Self::DryWoodland,
        Self::Mixed,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::EvergreenNeedleleaf => "evergreen needleleaf",
            Self::ColdDeciduous => "cold deciduous",
            Self::TemperateBroadleaf => "temperate broadleaf",
            Self::DryWoodland => "dry woodland",
            Self::Mixed => "mixed / weak identity",
        }
    }

    const fn is_strong(self) -> bool {
        !matches!(self, Self::Mixed)
    }
}

fn regional_tree_identity(fractions: [f64; 4]) -> (TreeIdentity, f64) {
    let identities = [
        TreeIdentity::EvergreenNeedleleaf,
        TreeIdentity::ColdDeciduous,
        TreeIdentity::TemperateBroadleaf,
        TreeIdentity::DryWoodland,
    ];
    let mut strongest = 0;
    for index in 1..fractions.len() {
        if fractions[index] > fractions[strongest] {
            strongest = index;
        }
    }
    let strength = fractions[strongest];
    if strength >= 0.52 {
        (identities[strongest], strength)
    } else {
        (TreeIdentity::Mixed, strength)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum LandscapeFamily {
    ReefCoast,
    DuneField,
    DesertOrSaltBasin,
    ExposedAlpineOrGlacial,
    CliffOrBluff,
    Mountain,
    Coast,
    Wetland,
    LakeCountry,
    RiverValley,
    KarstOrCave,
    ClosedForest,
    PrairieOrGrassland,
    SteppeOrShrubland,
    RollingUpland,
    OpenPlain,
}

impl LandscapeFamily {
    const ALL: [Self; 16] = [
        Self::ReefCoast,
        Self::DuneField,
        Self::DesertOrSaltBasin,
        Self::ExposedAlpineOrGlacial,
        Self::CliffOrBluff,
        Self::Mountain,
        Self::Coast,
        Self::Wetland,
        Self::LakeCountry,
        Self::RiverValley,
        Self::KarstOrCave,
        Self::ClosedForest,
        Self::PrairieOrGrassland,
        Self::SteppeOrShrubland,
        Self::RollingUpland,
        Self::OpenPlain,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::ReefCoast => "reef coast",
            Self::DuneField => "dune field",
            Self::DesertOrSaltBasin => "desert or salt basin",
            Self::ExposedAlpineOrGlacial => "exposed alpine or glacial",
            Self::CliffOrBluff => "cliff or bluff",
            Self::Mountain => "mountain",
            Self::Coast => "coast",
            Self::Wetland => "wetland",
            Self::LakeCountry => "lake country",
            Self::RiverValley => "river valley",
            Self::KarstOrCave => "karst or cave country",
            Self::ClosedForest => "closed forest",
            Self::PrairieOrGrassland => "prairie or grassland",
            Self::SteppeOrShrubland => "steppe or shrubland",
            Self::RollingUpland => "rolling upland",
            Self::OpenPlain => "open plain",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AuditOutcome {
    Forest,
    PrairieOrGrassland,
    SteppeOrShrubland,
    DesertOrSaltBasin,
    Dune,
    ExposedAlpineOrGlacial,
    CliffOrBluff,
    Mountain,
    River,
    Lake,
    Coast,
    Wetland,
    Reef,
    Cave,
}

impl AuditOutcome {
    const ALL: [Self; 14] = [
        Self::Forest,
        Self::PrairieOrGrassland,
        Self::SteppeOrShrubland,
        Self::DesertOrSaltBasin,
        Self::Dune,
        Self::ExposedAlpineOrGlacial,
        Self::CliffOrBluff,
        Self::Mountain,
        Self::River,
        Self::Lake,
        Self::Coast,
        Self::Wetland,
        Self::Reef,
        Self::Cave,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Forest => "forest",
            Self::PrairieOrGrassland => "prairie / grassland",
            Self::SteppeOrShrubland => "steppe / shrubland",
            Self::DesertOrSaltBasin => "desert / salt basin",
            Self::Dune => "dune",
            Self::ExposedAlpineOrGlacial => "exposed alpine / glacial",
            Self::CliffOrBluff => "cliff / bluff",
            Self::Mountain => "mountain",
            Self::River => "river",
            Self::Lake => "lake",
            Self::Coast => "coast",
            Self::Wetland => "wetland",
            Self::Reef => "reef",
            Self::Cave => "cave",
        }
    }

    fn qualifies(self, descriptor: &LandscapeDescriptor) -> bool {
        match self {
            Self::Forest => {
                descriptor.closed_forest_fraction >= 0.24
                    && descriptor.closed_forest_potential >= 0.36
                    && descriptor.tree_identity_strength >= 0.45
            }
            Self::PrairieOrGrassland => {
                descriptor.largest_open_patch_fraction >= 0.30
                    && descriptor.canopy < 0.28
                    && descriptor.ground_cover >= 0.32
                    && (descriptor.graminoid_fraction >= 0.30
                        || descriptor.grassland_prairie_potential >= 0.42)
                    && descriptor.dryness < 0.72
            }
            Self::SteppeOrShrubland => {
                descriptor.largest_open_patch_fraction >= 0.30
                    && descriptor.canopy < 0.28
                    && descriptor.dryness >= 0.38
                    && (descriptor.shrub_fraction >= 0.24
                        || descriptor.steppe_potential >= 0.40
                        || descriptor.shrubland_potential >= 0.40)
            }
            Self::DesertOrSaltBasin => {
                descriptor.largest_open_patch_fraction >= 0.30
                    && (descriptor.dryness >= 0.70
                        || descriptor.salinity >= 0.30
                        || descriptor.closed_basin_signal >= 0.55
                        || descriptor.desert_potential >= 0.45)
                    && descriptor.ground_cover < 0.42
            }
            Self::Dune => descriptor.dune_signal >= 0.62,
            Self::ExposedAlpineOrGlacial => {
                descriptor.alpine_glacial_signal >= 0.42
                    && (descriptor.exposed_alpine_potential >= 0.34
                        || descriptor.tundra_potential >= 0.40
                        || descriptor.glacial_plan_signal >= 0.45)
            }
            Self::CliffOrBluff => {
                descriptor.coherent_cliff_fraction >= 0.002
                    && descriptor.largest_cliff_patch_fraction >= 0.002
            }
            Self::Mountain => descriptor.relief >= 550.0 && descriptor.p95_slope >= 0.16,
            Self::River => descriptor.river_fraction > 0.0,
            Self::Lake => descriptor.lake_fraction > 0.0,
            Self::Coast => descriptor.ocean_fraction > 0.0,
            Self::Wetland => {
                descriptor.wetland_fraction >= 0.08
                    || (descriptor.wetland_potential >= 0.45
                        && descriptor.water_balance_fraction >= 0.55)
            }
            Self::Reef => descriptor.reef_fraction >= 0.01,
            Self::Cave => descriptor.cave_fraction > 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TerrainMetrics {
    mean_elevation: f64,
    relief: f64,
    roughness: f64,
    mean_slope: f64,
    p95_slope: f64,
    maximum_slope: f64,
    mean_curvature: f64,
    p95_curvature: f64,
    quiet_fraction: f64,
    rolling_fraction: f64,
    steep_fraction: f64,
    cliff_fraction: f64,
    coherent_cliff_fraction: f64,
    largest_cliff_patch_fraction: f64,
    spike_outlier_fraction: f64,
}

fn sample_terrain_metrics(
    terrain: &GeneratedWorldTerrain,
    center: [f64; 2],
) -> Result<TerrainMetrics, Box<dyn Error>> {
    let half = usize_as_f64(TERRAIN_AUDIT_GRID_EDGE - 1) * 0.5;
    let mut heights = Vec::with_capacity(TERRAIN_AUDIT_GRID_EDGE * TERRAIN_AUDIT_GRID_EDGE);
    for grid_z in 0..TERRAIN_AUDIT_GRID_EDGE {
        let z = center[1] + ((usize_as_f64(grid_z) - half) * TERRAIN_AUDIT_SPACING_METERS);
        for grid_x in 0..TERRAIN_AUDIT_GRID_EDGE {
            let x = center[0] + ((usize_as_f64(grid_x) - half) * TERRAIN_AUDIT_SPACING_METERS);
            heights.push(
                terrain
                    .surface_height(x, z)
                    .ok_or("terrain metric sample unavailable")?,
            );
        }
    }
    analyze_height_grid(
        &heights,
        TERRAIN_AUDIT_GRID_EDGE,
        TERRAIN_AUDIT_SPACING_METERS,
    )
    .ok_or_else(|| "invalid terrain metric grid".into())
}

fn analyze_height_grid(heights: &[f64], edge: usize, spacing: f64) -> Option<TerrainMetrics> {
    if edge < 3
        || heights.len() != edge.checked_mul(edge)?
        || !spacing.is_finite()
        || spacing <= 0.0
        || !heights.iter().all(|height| height.is_finite())
    {
        return None;
    }
    let sample_count = usize_as_f64(heights.len());
    let mean_elevation = heights.iter().sum::<f64>() / sample_count;
    let minimum = heights.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = heights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let variance = heights
        .iter()
        .map(|height| {
            let delta = height - mean_elevation;
            delta * delta
        })
        .sum::<f64>()
        / sample_count;

    let metric_edge = edge - 2;
    let metric_count = metric_edge * metric_edge;
    let mut slopes = Vec::with_capacity(metric_count);
    let mut curvatures = Vec::with_capacity(metric_count);
    let mut cliff_mask = Vec::with_capacity(metric_count);
    let mut spike_mask = Vec::with_capacity(metric_count);
    for z in 1..edge - 1 {
        for x in 1..edge - 1 {
            let center = heights[z * edge + x];
            let left = heights[z * edge + x - 1];
            let right = heights[z * edge + x + 1];
            let down = heights[(z - 1) * edge + x];
            let up = heights[(z + 1) * edge + x];
            let slope = libm::hypot(
                (right - left) / (spacing * 2.0),
                (up - down) / (spacing * 2.0),
            );
            let curvature = (left + right + down + up - (center * 4.0)).abs() / spacing;
            let neighbour_mean = (left + right + down + up) * 0.25;
            let prominence = (center - neighbour_mean).abs() / spacing;
            let strict_extremum = (center > left && center > right && center > down && center > up)
                || (center < left && center < right && center < down && center < up);
            slopes.push(slope);
            curvatures.push(curvature);
            cliff_mask.push(slope >= CLIFF_SLOPE_MINIMUM);
            spike_mask.push(strict_extremum && prominence >= SPIKE_PROMINENCE_MINIMUM);
        }
    }

    let component_sizes = connected_component_sizes(&cliff_mask, metric_edge);
    let coherent_cliff_cells = component_sizes
        .iter()
        .filter(|&&size| size >= COHERENT_CLIFF_MINIMUM_CELLS)
        .sum::<usize>();
    let largest_cliff_patch = component_sizes.into_iter().max().unwrap_or(0);
    let metric_count = usize_as_f64(metric_count);
    let mean_slope = slopes.iter().sum::<f64>() / metric_count;
    let mean_curvature = curvatures.iter().sum::<f64>() / metric_count;
    Some(TerrainMetrics {
        mean_elevation,
        relief: maximum - minimum,
        roughness: libm::sqrt(variance),
        mean_slope,
        p95_slope: percentile(&slopes, 95),
        maximum_slope: slopes.iter().copied().fold(0.0, f64::max),
        mean_curvature,
        p95_curvature: percentile(&curvatures, 95),
        quiet_fraction: fraction_matching(&slopes, |slope| slope < QUIET_SLOPE_MAXIMUM),
        rolling_fraction: fraction_matching(&slopes, |slope| {
            (QUIET_SLOPE_MAXIMUM..ROLLING_SLOPE_MAXIMUM).contains(&slope)
        }),
        steep_fraction: fraction_matching(&slopes, |slope| {
            (ROLLING_SLOPE_MAXIMUM..CLIFF_SLOPE_MINIMUM).contains(&slope)
        }),
        cliff_fraction: fraction_matching(&slopes, |slope| slope >= CLIFF_SLOPE_MINIMUM),
        coherent_cliff_fraction: usize_as_f64(coherent_cliff_cells) / metric_count,
        largest_cliff_patch_fraction: usize_as_f64(largest_cliff_patch) / metric_count,
        spike_outlier_fraction: usize_as_f64(spike_mask.iter().filter(|&&spike| spike).count())
            / metric_count,
    })
}

fn percentile(values: &[f64], percentile: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let numerator = (sorted.len() - 1).saturating_mul(percentile.min(100));
    let index = numerator.div_ceil(100);
    sorted[index]
}

fn fraction_matching(values: &[f64], predicate: impl Fn(f64) -> bool) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    usize_as_f64(
        values
            .iter()
            .copied()
            .filter(|&value| predicate(value))
            .count(),
    ) / usize_as_f64(values.len())
}

fn connected_component_sizes(mask: &[bool], width: usize) -> Vec<usize> {
    if width == 0 || mask.is_empty() || !mask.len().is_multiple_of(width) {
        return Vec::new();
    }
    let height = mask.len() / width;
    let mut visited = vec![false; mask.len()];
    let mut sizes = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || visited[start] {
            continue;
        }
        visited[start] = true;
        let mut pending = VecDeque::from([start]);
        let mut size = 0;
        while let Some(slot) = pending.pop_front() {
            size += 1;
            let x = slot % width;
            let z = slot / width;
            let neighbours = [
                (x > 0).then(|| slot - 1),
                (x + 1 < width).then(|| slot + 1),
                (z > 0).then(|| slot - width),
                (z + 1 < height).then(|| slot + width),
            ];
            for neighbour in neighbours.into_iter().flatten() {
                if mask[neighbour] && !visited[neighbour] {
                    visited[neighbour] = true;
                    pending.push_back(neighbour);
                }
            }
        }
        sizes.push(size);
    }
    sizes
}

#[allow(clippy::too_many_lines)]
fn sample_descriptor(
    terrain: &GeneratedWorldTerrain,
    region_index: usize,
    center: [f64; 2],
) -> Result<LandscapeDescriptor, Box<dyn Error>> {
    let world = terrain.world();
    let terrain_metrics = sample_terrain_metrics(terrain, center)?;
    let mut temperatures = 0.0;
    let mut precipitation = 0.0;
    let mut snowpack = 0.0;
    let mut dryness = 0.0;
    let mut salinity = 0.0;
    let mut sand = 0.0;
    let mut rock = 0.0;
    let mut canopy = 0.0;
    let mut canopy_height = 0.0;
    let mut ground_cover = 0.0;
    let mut graminoid = 0.0;
    let mut shrub = 0.0;
    let mut tree_fractions = [0.0; 4];
    let mut open_mask = Vec::with_capacity(AUDIT_GRID_EDGE * AUDIT_GRID_EDGE);
    let mut closed_forest_hits = 0;
    let mut river_hits = 0;
    let mut lake_hits = 0;
    let mut ocean_hits = 0;
    let mut wetland = 0.0;
    let mut reef = 0.0;
    let mut cave_hits = 0;
    let mut province_scarp = 0.0;
    let mut province_mountain = 0.0;
    let mut closed_basin = 0.0;
    let mut province_dune = 0.0;
    let mut province_glacial = 0.0;
    let mut ecosystem_count = 0_usize;
    let mut ecosystem_controls = [0.0; 6];
    let mut ecosystem_potentials = [0.0; 9];
    let half = i32::try_from(AUDIT_GRID_EDGE / 2).expect("audit grid is small");

    for grid_z in -half..=half {
        for grid_x in -half..=half {
            let x = center[0] + (f64::from(grid_x) * AUDIT_SPACING_METERS);
            let z = center[1] + (f64::from(grid_z) * AUDIT_SPACING_METERS);
            let climate = Climate::new(world)
                .sample(x, z)
                .ok_or("climate sample unavailable")?;
            let soil = Soil::new(world)
                .sample(x, z)
                .ok_or("soil sample unavailable")?;
            let forest = ForestDistribution::new(world)
                .sample(x, z)
                .ok_or("forest sample unavailable")?;
            let ground = GroundVegetationDistribution::new(world)
                .sample(x, z)
                .ok_or("ground-vegetation sample unavailable")?;
            let lake = terrain.lake_surface_at(x, z);
            let ocean = terrain.ocean_surface_at(x, z);
            let wetland_sample = terrain
                .wetland_at(x, z)
                .ok_or("wetland sample unavailable")?;
            let reef_sample = terrain.reef_at(x, z).ok_or("reef sample unavailable")?;
            let province = ProvincePlan::sample_at(world, x, z);
            let ecosystem = EcosystemDistribution::new(world).sample(x, z);
            let soil_dryness = 1.0 - soil.surface_moisture;
            let climatic_water_deficit = (1.0 - climate.precipitation_fraction())
                * (0.35 + climate.warmth_fraction() * 0.65);
            let sampled_dryness =
                (soil_dryness * 0.68 + climatic_water_deficit * 0.32).clamp(0.0, 1.0);
            let province_dryness = province.map_or(sampled_dryness, |sample| {
                (sampled_dryness * 0.45 + sample.aridity * 0.55).clamp(0.0, 1.0)
            });
            let local_dryness = ecosystem.map_or(province_dryness, |sample| {
                (province_dryness * 0.32 + (1.0 - sample.water_balance_fraction) * 0.68)
                    .clamp(0.0, 1.0)
            });
            let flatness = 1.0 - normalize(soil.slope, 0.02, 0.25);
            let alkalinity = normalize(soil.acidity_ph, 7.1, 8.5);
            let closed_water_factor = if lake.is_some() { 1.0 } else { 0.20 };
            let inland_salt_signal = local_dryness * flatness * alkalinity * closed_water_factor;

            temperatures += climate.mean_temperature_celsius;
            precipitation += climate.annual_precipitation_millimeters;
            snowpack += climate.maximum_snowpack_water_equivalent_millimeters;
            dryness += local_dryness;
            salinity += wetland_sample
                .salinity_fraction
                .max(inland_salt_signal)
                .max(province.map_or(0.0, |sample| sample.salinity))
                .max(ecosystem.map_or(0.0, |sample| sample.salinity_fraction));
            sand += soil.composition.sand_fraction;
            rock += soil.rock_exposure;
            canopy += forest.canopy_cover_fraction;
            canopy_height += forest.mean_canopy_height_meters;
            ground_cover += ground.ground_cover_fraction;
            graminoid += ground.composition.graminoid_fraction;
            shrub += ground.composition.low_shrub_fraction;
            tree_fractions[0] += forest.composition.evergreen_needleleaf_fraction;
            tree_fractions[1] += forest.composition.cold_deciduous_fraction;
            tree_fractions[2] += forest.composition.temperate_broadleaf_fraction;
            tree_fractions[3] += forest.composition.dry_woodland_fraction;
            let is_open =
                forest.canopy_cover_fraction < 0.24 && forest.mean_canopy_height_meters < 8.0;
            open_mask.push(is_open);
            closed_forest_hits += usize::from(
                forest.canopy_cover_fraction >= 0.48 && forest.mean_canopy_height_meters >= 8.0,
            );
            river_hits += usize::from(
                terrain
                    .river_influence_at(x, z)
                    .is_some_and(|river| river.distance_meters <= river.valley_half_width_meters),
            );
            lake_hits += usize::from(lake.is_some());
            ocean_hits += usize::from(ocean.is_some());
            wetland += wetland_sample.coverage_fraction;
            reef += reef_sample.coverage_fraction;
            cave_hits += usize::from(terrain.cave_map_at(x, z).is_some());
            province_scarp += province.map_or(0.0, |sample| sample.scarp);
            province_mountain += province.map_or(0.0, |sample| sample.mountain);
            closed_basin += province.map_or(0.0, |sample| sample.closed_basin);
            province_dune += province.map_or(0.0, |sample| sample.dune);
            province_glacial += province.map_or(0.0, |sample| sample.glacial);
            if let Some(ecosystem) = ecosystem {
                ecosystem_count += 1;
                ecosystem_controls[0] += ecosystem.water_balance_fraction;
                ecosystem_controls[1] += ecosystem.above_tree_line_fraction;
                ecosystem_controls[2] += ecosystem.exposure_fraction;
                ecosystem_controls[3] += ecosystem.fire_pressure_fraction;
                ecosystem_controls[4] += ecosystem.disturbance_fraction;
                ecosystem_controls[5] += ecosystem.sediment_fraction;
                for (total, potential) in
                    ecosystem_potentials.iter_mut().zip(ecosystem.potentials())
                {
                    *total += potential;
                }
            }
        }
    }

    let sample_count = usize_as_f64(open_mask.len());
    for fraction in &mut tree_fractions {
        *fraction /= sample_count;
    }
    let (tree_identity, tree_identity_strength) = regional_tree_identity(tree_fractions);
    let open_land_fraction =
        usize_as_f64(open_mask.iter().filter(|&&open| open).count()) / sample_count;
    let largest_open_patch_fraction = connected_component_sizes(&open_mask, AUDIT_GRID_EDGE)
        .into_iter()
        .max()
        .map_or(0.0, |size| usize_as_f64(size) / sample_count);
    let mean_temperature = temperatures / sample_count;
    let precipitation = precipitation / sample_count;
    let snowpack = snowpack / sample_count;
    let dryness = dryness / sample_count;
    let salinity = salinity / sample_count;
    let sand_fraction = sand / sample_count;
    let rock_exposure = rock / sample_count;
    let canopy = canopy / sample_count;
    let closed_forest_fraction = usize_as_f64(closed_forest_hits) / sample_count;
    let mean_canopy_height = canopy_height / sample_count;
    let ground_cover = ground_cover / sample_count;
    let graminoid_fraction = graminoid / sample_count;
    let shrub_fraction = shrub / sample_count;
    let ecosystem_sample_count = usize_as_f64(ecosystem_count);
    if ecosystem_count > 0 {
        for control in &mut ecosystem_controls {
            *control /= ecosystem_sample_count;
        }
        for potential in &mut ecosystem_potentials {
            *potential /= ecosystem_sample_count;
        }
    }
    let water_balance_fraction = if ecosystem_count > 0 {
        ecosystem_controls[0]
    } else {
        1.0 - dryness
    };
    let above_tree_line_fraction = if ecosystem_count > 0 {
        ecosystem_controls[1]
    } else {
        0.0
    };
    let exposure_fraction = if ecosystem_count > 0 {
        ecosystem_controls[2]
    } else {
        rock_exposure
    };
    let fire_pressure_fraction = if ecosystem_count > 0 {
        ecosystem_controls[3]
    } else {
        dryness * normalize(mean_temperature, -4.0, 28.0)
    };
    let disturbance_fraction = if ecosystem_count > 0 {
        ecosystem_controls[4]
    } else {
        fire_pressure_fraction
    };
    let sediment_fraction = if ecosystem_count > 0 {
        ecosystem_controls[5]
    } else {
        (1.0 - rock_exposure) * (1.0 - terrain_metrics.steep_fraction)
    };
    let province_scarp_signal = province_scarp / sample_count;
    let province_mountain_signal = province_mountain / sample_count;
    let closed_basin_signal = closed_basin / sample_count;
    let dune_plan_signal = province_dune / sample_count;
    let glacial_plan_signal = province_glacial / sample_count;
    let visible_dune_signal = (dryness * 0.38
        + sand_fraction * 0.28
        + largest_open_patch_fraction * 0.22
        + terrain_metrics.rolling_fraction * 0.12
        - rock_exposure * 0.18
        - terrain_metrics.cliff_fraction * 0.20)
        .clamp(0.0, 1.0);
    let dune_signal = (visible_dune_signal * 0.62 + dune_plan_signal * 0.38).clamp(0.0, 1.0);
    let cold_fraction = 1.0 - normalize(mean_temperature, -8.0, 8.0);
    let snow_fraction = normalize(snowpack, 80.0, 1_000.0);
    let elevation_fraction = normalize(terrain_metrics.mean_elevation, 500.0, 2_400.0);
    let visible_alpine_glacial_signal = (cold_fraction
        * (0.35 + snow_fraction * 0.65)
        * (0.30 + elevation_fraction * 0.70)
        * (0.35 + rock_exposure * 0.65)
        * (0.45 + open_land_fraction * 0.55))
        .clamp(0.0, 1.0);
    let alpine_glacial_signal =
        (visible_alpine_glacial_signal * 0.68 + glacial_plan_signal * 0.32).clamp(0.0, 1.0);
    if ecosystem_count == 0 {
        ecosystem_potentials = [
            canopy,
            (1.0 - canopy) * (1.0 - open_land_fraction),
            ground_cover * graminoid_fraction * open_land_fraction,
            dryness * open_land_fraction * (1.0 - shrub_fraction * 0.35),
            shrub_fraction * open_land_fraction,
            dryness * (1.0 - ground_cover) * open_land_fraction,
            cold_fraction * open_land_fraction,
            visible_alpine_glacial_signal,
            wetland / sample_count,
        ];
    }
    let [
        closed_forest_potential,
        open_woodland_potential,
        grassland_prairie_potential,
        steppe_potential,
        shrubland_potential,
        desert_potential,
        tundra_potential,
        exposed_alpine_potential,
        wetland_potential,
    ] = ecosystem_potentials;
    let karst_probability = RegionalProfile::sample(world, center[0], center[1])
        .ok_or("regional profile unavailable")?
        .karst_probability;
    let mut descriptor = LandscapeDescriptor {
        seed: world.seed,
        region_index,
        center,
        mean_elevation: terrain_metrics.mean_elevation,
        relief: terrain_metrics.relief,
        roughness: terrain_metrics.roughness,
        mean_slope: terrain_metrics.mean_slope,
        p95_slope: terrain_metrics.p95_slope,
        maximum_slope: terrain_metrics.maximum_slope,
        mean_curvature: terrain_metrics.mean_curvature,
        p95_curvature: terrain_metrics.p95_curvature,
        quiet_terrain_fraction: terrain_metrics.quiet_fraction,
        rolling_terrain_fraction: terrain_metrics.rolling_fraction,
        steep_terrain_fraction: terrain_metrics.steep_fraction,
        cliff_terrain_fraction: terrain_metrics.cliff_fraction,
        coherent_cliff_fraction: terrain_metrics.coherent_cliff_fraction,
        largest_cliff_patch_fraction: terrain_metrics.largest_cliff_patch_fraction,
        spike_outlier_fraction: terrain_metrics.spike_outlier_fraction,
        mean_temperature,
        precipitation,
        snowpack,
        dryness,
        salinity,
        water_balance_fraction,
        above_tree_line_fraction,
        exposure_fraction,
        fire_pressure_fraction,
        disturbance_fraction,
        sediment_fraction,
        closed_forest_potential,
        open_woodland_potential,
        grassland_prairie_potential,
        steppe_potential,
        shrubland_potential,
        desert_potential,
        tundra_potential,
        exposed_alpine_potential,
        wetland_potential,
        sand_fraction,
        rock_exposure,
        canopy,
        closed_forest_fraction,
        mean_canopy_height,
        ground_cover,
        graminoid_fraction,
        shrub_fraction,
        open_land_fraction,
        largest_open_patch_fraction,
        tree_identity,
        tree_identity_strength,
        river_fraction: usize_as_f64(river_hits) / sample_count,
        lake_fraction: usize_as_f64(lake_hits) / sample_count,
        ocean_fraction: usize_as_f64(ocean_hits) / sample_count,
        wetland_fraction: wetland / sample_count,
        reef_fraction: reef / sample_count,
        cave_fraction: usize_as_f64(cave_hits) / sample_count,
        karst_probability,
        province_scarp_signal,
        province_mountain_signal,
        closed_basin_signal,
        dune_plan_signal,
        glacial_plan_signal,
        dune_signal,
        alpine_glacial_signal,
        family: LandscapeFamily::OpenPlain,
    };
    descriptor.family = classify_landscape(&descriptor);
    Ok(descriptor)
}

fn classify_landscape(descriptor: &LandscapeDescriptor) -> LandscapeFamily {
    if descriptor.reef_fraction > 0.05 {
        LandscapeFamily::ReefCoast
    } else if descriptor.dune_signal >= 0.62 {
        LandscapeFamily::DuneField
    } else if AuditOutcome::DesertOrSaltBasin.qualifies(descriptor) {
        LandscapeFamily::DesertOrSaltBasin
    } else if AuditOutcome::ExposedAlpineOrGlacial.qualifies(descriptor) {
        LandscapeFamily::ExposedAlpineOrGlacial
    } else if AuditOutcome::CliffOrBluff.qualifies(descriptor) {
        LandscapeFamily::CliffOrBluff
    } else if descriptor.ocean_fraction > 0.05 {
        LandscapeFamily::Coast
    } else if descriptor.wetland_fraction > 0.18 || descriptor.dryness < 0.28 {
        LandscapeFamily::Wetland
    } else if AuditOutcome::Mountain.qualifies(descriptor) {
        LandscapeFamily::Mountain
    } else if descriptor.lake_fraction > 0.04 {
        LandscapeFamily::LakeCountry
    } else if descriptor.river_fraction > 0.04 {
        LandscapeFamily::RiverValley
    } else if descriptor.karst_probability > 0.72 && descriptor.cave_fraction > 0.0 {
        LandscapeFamily::KarstOrCave
    } else if AuditOutcome::Forest.qualifies(descriptor) {
        LandscapeFamily::ClosedForest
    } else if AuditOutcome::PrairieOrGrassland.qualifies(descriptor) {
        LandscapeFamily::PrairieOrGrassland
    } else if AuditOutcome::SteppeOrShrubland.qualifies(descriptor) {
        LandscapeFamily::SteppeOrShrubland
    } else if descriptor.relief > 180.0 || descriptor.rolling_terrain_fraction > 0.45 {
        LandscapeFamily::RollingUpland
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
    outcome_counts: BTreeMap<AuditOutcome, usize>,
    tree_identity_counts: BTreeMap<TreeIdentity, usize>,
    coverage_findings: Vec<String>,
    outliers: Vec<String>,
}

impl NoveltyReport {
    #[allow(clippy::too_many_lines)]
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
        let mut outcome_counts = BTreeMap::new();
        let mut tree_identity_counts = BTreeMap::new();
        let mut outliers = Vec::new();
        for (index, descriptor) in descriptors.iter().enumerate() {
            *family_counts.entry(descriptor.family).or_default() += 1;
            *tree_identity_counts
                .entry(descriptor.tree_identity)
                .or_default() += 1;
            for outcome in AuditOutcome::ALL {
                if outcome.qualifies(descriptor) {
                    *outcome_counts.entry(outcome).or_default() += 1;
                }
            }
            if descriptor.relief > 4_500.0 {
                outliers.push(format!(
                    "region {index}: extreme relief {:.0} m",
                    descriptor.relief
                ));
            }
            if descriptor.mean_elevation < -1_200.0 || descriptor.mean_elevation > 4_500.0 {
                outliers.push(format!(
                    "region {index}: implausible mean elevation {:.0} m",
                    descriptor.mean_elevation
                ));
            }
            if descriptor.canopy > 0.75 && descriptor.dryness > 0.82 {
                outliers.push(format!(
                    "region {index}: dense canopy ({:.2}) under severe dryness ({:.2})",
                    descriptor.canopy, descriptor.dryness
                ));
            }
            if descriptor.spike_outlier_fraction > 0.002 {
                outliers.push(format!(
                    "region {index}: isolated spike fraction {:.4} exceeds 0.0020",
                    descriptor.spike_outlier_fraction
                ));
            }
            if descriptor.cliff_terrain_fraction > 0.01
                && descriptor.coherent_cliff_fraction < descriptor.cliff_terrain_fraction * 0.35
            {
                outliers.push(format!(
                    "region {index}: cliff-class slopes are mostly incoherent ({:.3} of {:.3})",
                    descriptor.coherent_cliff_fraction, descriptor.cliff_terrain_fraction
                ));
            }
            if descriptor.province_scarp_signal >= 0.45
                && descriptor.coherent_cliff_fraction < 0.002
            {
                outliers.push(format!(
                    "region {index}: strong province scarp signal ({:.2}) has no coherent visible cliff face",
                    descriptor.province_scarp_signal
                ));
            }
            if descriptor.province_mountain_signal >= 0.45 && descriptor.relief < 300.0 {
                outliers.push(format!(
                    "region {index}: strong province mountain signal ({:.2}) yields only {:.0} m relief",
                    descriptor.province_mountain_signal, descriptor.relief
                ));
            }
            if descriptor.dune_plan_signal >= 0.55 && descriptor.dune_signal < 0.42 {
                outliers.push(format!(
                    "region {index}: strong province dune signal ({:.2}) lacks a visible dune outcome",
                    descriptor.dune_plan_signal
                ));
            }
            if descriptor.glacial_plan_signal >= 0.55 && descriptor.alpine_glacial_signal < 0.42 {
                outliers.push(format!(
                    "region {index}: strong province glacial signal ({:.2}) lacks an exposed alpine/glacial outcome",
                    descriptor.glacial_plan_signal
                ));
            }
        }
        let mut coverage_findings = missing_outcomes(&outcome_counts)
            .into_iter()
            .map(|outcome| {
                format!(
                    "No sampled region met the {} outcome criteria.",
                    outcome.label()
                )
            })
            .collect::<Vec<_>>();
        if !descriptors
            .iter()
            .any(|descriptor| descriptor.largest_open_patch_fraction >= 0.40)
        {
            coverage_findings.push(
                "No sampled region contains a contiguous open-land patch covering 40% of its audit grid."
                    .to_owned(),
            );
        }
        if !descriptors
            .iter()
            .any(|descriptor| descriptor.tree_identity.is_strong())
        {
            coverage_findings.push(
                "No sampled region has a tree-group share of at least 52%; forest identity remains mixed."
                    .to_owned(),
            );
        }
        if !descriptors
            .iter()
            .any(|descriptor| descriptor.coherent_cliff_fraction >= 0.002)
        {
            coverage_findings.push(
                "No sampled region contains a coherent three-cell cliff-class face.".to_owned(),
            );
        }
        if !descriptors
            .iter()
            .any(|descriptor| descriptor.quiet_terrain_fraction >= 0.60)
        {
            coverage_findings
                .push("No sampled region is dominated by coherent quiet terrain.".to_owned());
        }
        if !descriptors
            .iter()
            .any(|descriptor| descriptor.rolling_terrain_fraction >= 0.25)
        {
            coverage_findings
                .push("No sampled region is substantially composed of rolling terrain.".to_owned());
        }
        if !descriptors
            .iter()
            .any(|descriptor| descriptor.steep_terrain_fraction >= 0.02)
        {
            coverage_findings
                .push("No sampled region contains a sustained steep-slope patch.".to_owned());
        }
        Self {
            closest_pair,
            suspicious_pairs,
            family_counts,
            outcome_counts,
            tree_identity_counts,
            coverage_findings,
            outliers,
        }
    }

    fn summary(&self) -> String {
        let represented_outcomes = AuditOutcome::ALL
            .into_iter()
            .filter(|outcome| self.outcome_counts.get(outcome).copied().unwrap_or(0) > 0)
            .count();
        format!(
            "{} suspicious pair(s), {} outlier(s), {} represented primary family/families, {represented_outcomes}/{} required outcomes",
            self.suspicious_pairs.len(),
            self.outliers.len(),
            self.family_counts.len(),
            AuditOutcome::ALL.len(),
        )
    }
}

fn missing_outcomes(counts: &BTreeMap<AuditOutcome, usize>) -> Vec<AuditOutcome> {
    AuditOutcome::ALL
        .into_iter()
        .filter(|outcome| counts.get(outcome).copied().unwrap_or(0) == 0)
        .collect()
}

fn descriptor_distance<const DIMENSIONS: usize>(
    left: [f64; DIMENSIONS],
    right: [f64; DIMENSIONS],
) -> f64 {
    let squared = left
        .into_iter()
        .zip(right)
        .map(|(left, right)| {
            let delta = left - right;
            delta * delta
        })
        .sum::<f64>();
    libm::sqrt(squared / usize_as_f64(DIMENSIONS))
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
    PrairieOrGrassland,
    SteppeOrShrubland,
    DesertOrSaltBasin,
    Dune,
    ExposedAlpineOrGlacial,
    CliffOrBluff,
    Mountain,
    Coast,
    Wetland,
    Reef,
}

impl ViewpointKind {
    const ALL: [Self; 17] = [
        Self::Valley,
        Self::Ridge,
        Self::River,
        Self::Forest,
        Self::LakeShore,
        Self::Cave,
        Self::Summit,
        Self::PrairieOrGrassland,
        Self::SteppeOrShrubland,
        Self::DesertOrSaltBasin,
        Self::Dune,
        Self::ExposedAlpineOrGlacial,
        Self::CliffOrBluff,
        Self::Mountain,
        Self::Coast,
        Self::Wetland,
        Self::Reef,
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
            Self::PrairieOrGrassland => "prairie / grassland",
            Self::SteppeOrShrubland => "steppe / shrubland",
            Self::DesertOrSaltBasin => "desert / salt basin",
            Self::Dune => "dune",
            Self::ExposedAlpineOrGlacial => "exposed alpine / glacial",
            Self::CliffOrBluff => "cliff / bluff",
            Self::Mountain => "mountain",
            Self::Coast => "coast",
            Self::Wetland => "wetland",
            Self::Reef => "reef",
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
    randomized_center_count: usize,
    curated_centers: Vec<CuratedViewCenter>,
    viewpoints: Vec<Viewpoint>,
}

#[derive(Clone, Copy, Debug)]
struct ViewpointSignals {
    height: f64,
    slope: f64,
    curvature: f64,
    deposition: f64,
    uplift: f64,
    canopy: f64,
    canopy_height: f64,
    ground_cover: f64,
    graminoid: f64,
    shrub: f64,
    dryness: f64,
    salinity: f64,
    sand: f64,
    rock_exposure: f64,
    temperature: f64,
    snowpack: f64,
    tree_identity_strength: f64,
    river_score: Option<f64>,
    lake_score: Option<f64>,
    cave_score: Option<f64>,
    ocean_score: Option<f64>,
    wetland_cover: f64,
    reef_cover: f64,
    province_dune: f64,
    province_glacial: f64,
    province_scarp: f64,
    province_mountain: f64,
    closed_basin: f64,
    closed_forest_potential: f64,
    grassland_prairie_potential: f64,
    steppe_potential: f64,
    shrubland_potential: f64,
    desert_potential: f64,
    tundra_potential: f64,
    exposed_alpine_potential: f64,
    wetland_potential: f64,
}

fn capture_viewpoint_suite(
    terrain: &GeneratedWorldTerrain,
    randomized_centers: &[[f64; 2]],
    curated_centers: &[CuratedViewCenter],
) -> Result<ViewpointSuite, Box<dyn Error>> {
    let world = terrain.world();
    let fallback_center = randomized_centers
        .first()
        .copied()
        .or_else(|| curated_centers.first().map(|center| center.position))
        .unwrap_or([0.0, 0.0]);
    let mut best = ViewpointKind::ALL.map(|kind| Viewpoint {
        kind,
        position: fallback_center,
        score: f64::NEG_INFINITY,
        qualified: false,
    });
    let half = i32::try_from(VIEW_SEARCH_EDGE / 2).expect("view search grid is small");
    for center in randomized_centers {
        for grid_z in -half..=half {
            for grid_x in -half..=half {
                let position = [
                    center[0] + (f64::from(grid_x) * VIEW_SEARCH_SPACING_METERS),
                    center[1] + (f64::from(grid_z) * VIEW_SEARCH_SPACING_METERS),
                ];
                let signals = sample_viewpoint_signals(terrain, position)?;
                for candidate in &mut best {
                    let (score, qualified) = viewpoint_score(candidate.kind, &signals);
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
    }
    for center in curated_centers {
        let signals = sample_viewpoint_signals(terrain, center.position)?;
        for candidate in &mut best {
            let (score, qualified) = viewpoint_score(candidate.kind, &signals);
            let replaces = (qualified && !candidate.qualified)
                || (qualified == candidate.qualified && score > candidate.score);
            if replaces {
                candidate.position = center.position;
                candidate.score = score;
                candidate.qualified = qualified;
            }
        }
    }
    Ok(ViewpointSuite {
        seed: world.seed,
        randomized_center_count: randomized_centers.len(),
        curated_centers: curated_centers.to_vec(),
        viewpoints: best.to_vec(),
    })
}

#[allow(clippy::too_many_lines)]
fn sample_viewpoint_signals(
    terrain: &GeneratedWorldTerrain,
    position: [f64; 2],
) -> Result<ViewpointSignals, Box<dyn Error>> {
    const SHAPE_RADIUS_METERS: f64 = 250.0;

    let world = terrain.world();
    let [x, z] = position;
    let height = terrain
        .surface_height(x, z)
        .ok_or("viewpoint terrain sample unavailable")?;
    let left = terrain
        .surface_height(x - SHAPE_RADIUS_METERS, z)
        .ok_or("viewpoint left terrain sample unavailable")?;
    let right = terrain
        .surface_height(x + SHAPE_RADIUS_METERS, z)
        .ok_or("viewpoint right terrain sample unavailable")?;
    let down = terrain
        .surface_height(x, z - SHAPE_RADIUS_METERS)
        .ok_or("viewpoint down terrain sample unavailable")?;
    let up = terrain
        .surface_height(x, z + SHAPE_RADIUS_METERS)
        .ok_or("viewpoint up terrain sample unavailable")?;
    let slope = libm::hypot(
        (right - left) / (SHAPE_RADIUS_METERS * 2.0),
        (up - down) / (SHAPE_RADIUS_METERS * 2.0),
    );
    let curvature = (left + right + down + up - height * 4.0).abs() / SHAPE_RADIUS_METERS;
    let base = WildernessTerrain::new(world)
        .erosion_at(x, z)
        .ok_or("viewpoint erosion sample unavailable")?;
    let forest = ForestDistribution::new(world)
        .sample(x, z)
        .ok_or("viewpoint forest sample unavailable")?;
    let ground = GroundVegetationDistribution::new(world)
        .sample(x, z)
        .ok_or("viewpoint ground-vegetation sample unavailable")?;
    let soil = Soil::new(world)
        .sample(x, z)
        .ok_or("viewpoint soil sample unavailable")?;
    let climate = Climate::new(world)
        .sample(x, z)
        .ok_or("viewpoint climate sample unavailable")?;
    let profile =
        RegionalProfile::sample(world, x, z).ok_or("viewpoint profile sample unavailable")?;
    let wetland = terrain
        .wetland_at(x, z)
        .ok_or("viewpoint wetland sample unavailable")?;
    let reef = terrain
        .reef_at(x, z)
        .ok_or("viewpoint reef sample unavailable")?;
    let river = terrain.river_influence_at(x, z);
    let lake = terrain.lake_surface_at(x, z);
    let cave = terrain.cave_map_at(x, z);
    let ocean = terrain.ocean_surface_at(x, z);
    let province = ProvincePlan::sample_at(world, x, z);
    let ecosystem = EcosystemDistribution::new(world).sample(x, z);
    let soil_dryness = 1.0 - soil.surface_moisture;
    let climatic_water_deficit =
        (1.0 - climate.precipitation_fraction()) * (0.35 + climate.warmth_fraction() * 0.65);
    let sampled_dryness = (soil_dryness * 0.68 + climatic_water_deficit * 0.32).clamp(0.0, 1.0);
    let province_dryness = province.map_or(sampled_dryness, |sample| {
        (sampled_dryness * 0.45 + sample.aridity * 0.55).clamp(0.0, 1.0)
    });
    let dryness = ecosystem.map_or(province_dryness, |sample| {
        (province_dryness * 0.32 + (1.0 - sample.water_balance_fraction) * 0.68).clamp(0.0, 1.0)
    });
    let flatness = 1.0 - normalize(soil.slope, 0.02, 0.25);
    let alkalinity = normalize(soil.acidity_ph, 7.1, 8.5);
    let inland_salt_signal =
        dryness * flatness * alkalinity * if lake.is_some() { 1.0 } else { 0.2 };
    let composition = forest.composition;
    let tree_identity_strength = [
        composition.evergreen_needleleaf_fraction,
        composition.cold_deciduous_fraction,
        composition.temperate_broadleaf_fraction,
        composition.dry_woodland_fraction,
    ]
    .into_iter()
    .fold(0.0, f64::max);
    Ok(ViewpointSignals {
        height,
        slope,
        curvature,
        deposition: base.sediment_deposition_meters,
        uplift: profile.uplift,
        canopy: forest.canopy_cover_fraction,
        canopy_height: forest.mean_canopy_height_meters,
        ground_cover: ground.ground_cover_fraction,
        graminoid: ground.composition.graminoid_fraction,
        shrub: ground.composition.low_shrub_fraction,
        dryness,
        salinity: wetland
            .salinity_fraction
            .max(inland_salt_signal)
            .max(province.map_or(0.0, |sample| sample.salinity))
            .max(ecosystem.map_or(0.0, |sample| sample.salinity_fraction)),
        sand: soil.composition.sand_fraction,
        rock_exposure: soil.rock_exposure,
        temperature: climate.mean_temperature_celsius,
        snowpack: climate.maximum_snowpack_water_equivalent_millimeters,
        tree_identity_strength,
        river_score: river
            .filter(|river| river.distance_meters <= river.valley_half_width_meters)
            .map(|river| {
                river.segment.discharge_cubic_meters_per_second
                    - (river.distance_meters / river.valley_half_width_meters.max(1.0))
            }),
        lake_score: lake.map(|lake| -lake.water_depth_meters.abs()),
        cave_score: cave.map(|cave| -cave.horizontal_distance_meters),
        ocean_score: ocean.map(|ocean| -ocean.water_depth_meters),
        wetland_cover: wetland.coverage_fraction,
        reef_cover: reef.coverage_fraction,
        province_dune: province.map_or(0.0, |sample| sample.dune),
        province_glacial: province.map_or(0.0, |sample| sample.glacial),
        province_scarp: province.map_or(0.0, |sample| sample.scarp),
        province_mountain: province.map_or(0.0, |sample| sample.mountain),
        closed_basin: ecosystem.map_or_else(
            || province.map_or(0.0, |sample| sample.closed_basin),
            |sample| sample.closed_basin_fraction,
        ),
        closed_forest_potential: ecosystem.map_or(forest.canopy_cover_fraction, |sample| {
            sample.closed_forest_potential
        }),
        grassland_prairie_potential: ecosystem.map_or(
            ground.ground_cover_fraction * ground.composition.graminoid_fraction,
            |sample| sample.grassland_prairie_potential,
        ),
        steppe_potential: ecosystem
            .map_or(dryness * (1.0 - forest.canopy_cover_fraction), |sample| {
                sample.steppe_potential
            }),
        shrubland_potential: ecosystem.map_or(
            ground.composition.low_shrub_fraction * (1.0 - forest.canopy_cover_fraction),
            |sample| sample.shrubland_potential,
        ),
        desert_potential: ecosystem
            .map_or(dryness * (1.0 - ground.ground_cover_fraction), |sample| {
                sample.desert_potential
            }),
        tundra_potential: ecosystem.map_or(0.0, |sample| sample.tundra_potential),
        exposed_alpine_potential: ecosystem.map_or(0.0, |sample| sample.exposed_alpine_potential),
        wetland_potential: ecosystem
            .map_or(wetland.coverage_fraction, |sample| sample.wetland_potential),
    })
}

#[allow(clippy::too_many_lines)]
fn viewpoint_score(kind: ViewpointKind, signals: &ViewpointSignals) -> (f64, bool) {
    let openness = 1.0 - signals.canopy;
    let rolling_suitability = (1.0 - (signals.slope - 0.14).abs() / 0.45).clamp(0.0, 1.0);
    let visible_dune_signal = (signals.dryness * 0.38
        + signals.sand * 0.28
        + openness * 0.22
        + rolling_suitability * 0.12
        - signals.rock_exposure * 0.18)
        .clamp(0.0, 1.0);
    let dune_signal = (visible_dune_signal * 0.62 + signals.province_dune * 0.38).clamp(0.0, 1.0);
    let visible_alpine_signal = ((1.0 - normalize(signals.temperature, -8.0, 8.0)) * 0.30
        + normalize(signals.snowpack, 80.0, 1_000.0) * 0.24
        + normalize(signals.height, 500.0, 2_400.0) * 0.22
        + signals.rock_exposure * 0.12
        + openness * 0.12)
        .clamp(0.0, 1.0);
    let alpine_signal = (visible_alpine_signal * 0.48
        + signals.province_glacial * 0.18
        + signals.tundra_potential * 0.14
        + signals.exposed_alpine_potential * 0.20)
        .clamp(0.0, 1.0);
    match kind {
        ViewpointKind::Valley => (
            -signals.height + signals.deposition * 10.0,
            signals.slope < 0.08,
        ),
        ViewpointKind::Ridge => (
            signals.height + signals.slope * 900.0 + signals.uplift * 500.0,
            signals.slope > 0.04,
        ),
        ViewpointKind::River => (
            signals.river_score.unwrap_or(-1_000.0),
            signals.river_score.is_some(),
        ),
        ViewpointKind::Forest => (
            signals.canopy * 800.0
                + signals.canopy_height * 8.0
                + signals.tree_identity_strength * 180.0
                + signals.closed_forest_potential * 160.0,
            signals.canopy > 0.48
                && signals.canopy_height > 8.0
                && signals.tree_identity_strength >= 0.45
                && signals.closed_forest_potential >= 0.36,
        ),
        ViewpointKind::LakeShore => (
            signals.lake_score.unwrap_or(-1_000.0),
            signals.lake_score.is_some(),
        ),
        ViewpointKind::Cave => (
            signals.cave_score.unwrap_or(-1_000.0),
            signals.cave_score.is_some(),
        ),
        ViewpointKind::Summit => (signals.height, signals.height > 600.0),
        ViewpointKind::PrairieOrGrassland => (
            openness * 420.0
                + signals.ground_cover * 320.0
                + signals.graminoid * 260.0
                + signals.grassland_prairie_potential * 220.0
                - signals.dryness * 80.0,
            signals.canopy < 0.25
                && signals.ground_cover >= 0.32
                && (signals.graminoid >= 0.30 || signals.grassland_prairie_potential >= 0.42)
                && signals.dryness < 0.72,
        ),
        ViewpointKind::SteppeOrShrubland => (
            openness * 350.0
                + signals.shrub * 320.0
                + signals.dryness * 300.0
                + signals.steppe_potential * 190.0
                + signals.shrubland_potential * 170.0,
            signals.canopy < 0.28
                && signals.dryness >= 0.38
                && (signals.shrub >= 0.24
                    || signals.steppe_potential >= 0.42
                    || signals.shrubland_potential >= 0.42),
        ),
        ViewpointKind::DesertOrSaltBasin => (
            signals.dryness * 500.0
                + signals.salinity * 420.0
                + openness * 180.0
                + signals.closed_basin * 220.0
                + signals.desert_potential * 260.0
                - signals.ground_cover * 240.0,
            signals.canopy < 0.20
                && signals.ground_cover < 0.42
                && (signals.dryness >= 0.70
                    || signals.salinity >= 0.30
                    || signals.closed_basin >= 0.55
                    || signals.desert_potential >= 0.55),
        ),
        ViewpointKind::Dune => (dune_signal * 1_000.0, dune_signal >= 0.62),
        ViewpointKind::ExposedAlpineOrGlacial => (
            alpine_signal * 1_000.0,
            alpine_signal >= 0.52
                && (signals.province_glacial >= 0.35
                    || signals.tundra_potential >= 0.44
                    || signals.exposed_alpine_potential >= 0.44),
        ),
        ViewpointKind::CliffOrBluff => (
            signals.slope * 620.0 + signals.curvature * 380.0 + signals.province_scarp * 180.0,
            signals.slope >= CLIFF_SLOPE_MINIMUM && signals.curvature >= 0.20,
        ),
        ViewpointKind::Mountain => (
            signals.height + signals.slope * 900.0 + signals.province_mountain * 250.0,
            signals.height > 500.0 && signals.slope > 0.14,
        ),
        ViewpointKind::Coast => (
            signals.ocean_score.unwrap_or(-1_000.0),
            signals.ocean_score.is_some(),
        ),
        ViewpointKind::Wetland => (
            signals.wetland_cover * 800.0 + signals.wetland_potential * 200.0,
            signals.wetland_cover >= 0.20 || signals.wetland_potential >= 0.50,
        ),
        ViewpointKind::Reef => (signals.reef_cover * 1_000.0, signals.reef_cover >= 0.05),
    }
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
    let climate = Climate::new(terrain.world())
        .sample(x, z)
        .ok_or("contact-sheet climate sample unavailable")?;
    let surface_color = terrain
        .surface_color_at(x, z)
        .ok_or("contact-sheet surface color unavailable")?;
    let snow = normalize(
        climate.maximum_snowpack_water_equivalent_millimeters,
        40.0,
        900.0,
    ) * (1.0 - normalize(soil.slope, 0.25, 1.1));
    let mut color = [
        f64::from(surface_color[0]),
        f64::from(surface_color[1]),
        f64::from(surface_color[2]),
    ];
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
    let mut csv = String::from(concat!(
        "seed,region,x,z,family,tree_identity,",
        "mean_elevation,relief,roughness,mean_slope,p95_slope,maximum_slope,",
        "mean_curvature,p95_curvature,quiet_terrain_fraction,rolling_terrain_fraction,",
        "steep_terrain_fraction,cliff_terrain_fraction,coherent_cliff_fraction,",
        "largest_cliff_patch_fraction,spike_outlier_fraction,",
        "temperature,precipitation,snowpack,dryness,salinity,water_balance_fraction,",
        "above_tree_line_fraction,exposure_fraction,fire_pressure_fraction,disturbance_fraction,",
        "sediment_fraction,closed_forest_potential,open_woodland_potential,",
        "grassland_prairie_potential,steppe_potential,shrubland_potential,desert_potential,",
        "tundra_potential,exposed_alpine_potential,wetland_potential,sand_fraction,rock_exposure,",
        "canopy,closed_forest_fraction,mean_canopy_height,ground_cover,graminoid_fraction,",
        "shrub_fraction,open_land_fraction,largest_open_patch_fraction,tree_identity_strength,",
        "river_fraction,lake_fraction,ocean_fraction,wetland_fraction,reef_fraction,cave_fraction,",
        "karst_probability,province_scarp_signal,province_mountain_signal,closed_basin_signal,",
        "dune_plan_signal,glacial_plan_signal,dune_signal,alpine_glacial_signal\n"
    ));
    for descriptor in descriptors {
        write!(
            csv,
            "{:016x},{},{:.0},{:.0},{},{}",
            descriptor.seed,
            descriptor.region_index,
            descriptor.center[0],
            descriptor.center[1],
            descriptor.family.label(),
            descriptor.tree_identity.label(),
        )
        .expect("writing to String cannot fail");
        for value in [
            descriptor.mean_elevation,
            descriptor.relief,
            descriptor.roughness,
            descriptor.mean_slope,
            descriptor.p95_slope,
            descriptor.maximum_slope,
            descriptor.mean_curvature,
            descriptor.p95_curvature,
            descriptor.quiet_terrain_fraction,
            descriptor.rolling_terrain_fraction,
            descriptor.steep_terrain_fraction,
            descriptor.cliff_terrain_fraction,
            descriptor.coherent_cliff_fraction,
            descriptor.largest_cliff_patch_fraction,
            descriptor.spike_outlier_fraction,
            descriptor.mean_temperature,
            descriptor.precipitation,
            descriptor.snowpack,
            descriptor.dryness,
            descriptor.salinity,
            descriptor.water_balance_fraction,
            descriptor.above_tree_line_fraction,
            descriptor.exposure_fraction,
            descriptor.fire_pressure_fraction,
            descriptor.disturbance_fraction,
            descriptor.sediment_fraction,
            descriptor.closed_forest_potential,
            descriptor.open_woodland_potential,
            descriptor.grassland_prairie_potential,
            descriptor.steppe_potential,
            descriptor.shrubland_potential,
            descriptor.desert_potential,
            descriptor.tundra_potential,
            descriptor.exposed_alpine_potential,
            descriptor.wetland_potential,
            descriptor.sand_fraction,
            descriptor.rock_exposure,
            descriptor.canopy,
            descriptor.closed_forest_fraction,
            descriptor.mean_canopy_height,
            descriptor.ground_cover,
            descriptor.graminoid_fraction,
            descriptor.shrub_fraction,
            descriptor.open_land_fraction,
            descriptor.largest_open_patch_fraction,
            descriptor.tree_identity_strength,
            descriptor.river_fraction,
            descriptor.lake_fraction,
            descriptor.ocean_fraction,
            descriptor.wetland_fraction,
            descriptor.reef_fraction,
            descriptor.cave_fraction,
            descriptor.karst_probability,
            descriptor.province_scarp_signal,
            descriptor.province_mountain_signal,
            descriptor.closed_basin_signal,
            descriptor.dune_plan_signal,
            descriptor.glacial_plan_signal,
            descriptor.dune_signal,
            descriptor.alpine_glacial_signal,
        ] {
            write!(csv, ",{value:.6}").expect("writing to String cannot fail");
        }
        csv.push('\n');
    }
    csv
}

#[allow(clippy::too_many_lines)]
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
    report.push_str(
        "Primary landscape labels and required outcomes below are audit interpretations of \
         continuous generated measurements. They are not biome IDs or generator inputs.\n\n\
         ### Primary landscape labels\n\n\
         | Landscape outcome | Regions |\n|---|---:|\n",
    );
    for family in LandscapeFamily::ALL {
        writeln!(
            report,
            "| {} | {} |",
            family.label(),
            novelty.family_counts.get(&family).copied().unwrap_or(0)
        )
        .expect("writing to String cannot fail");
    }

    report.push_str("\n### Required outcome coverage\n\n");
    report.push_str("| Outcome | Qualifying regions | Coverage |\n|---|---:|---|\n");
    for outcome in AuditOutcome::ALL {
        let count = novelty.outcome_counts.get(&outcome).copied().unwrap_or(0);
        writeln!(
            report,
            "| {} | {} | {} |",
            outcome.label(),
            count,
            if count > 0 {
                "represented"
            } else {
                "**MISSING**"
            }
        )
        .expect("writing to String cannot fail");
    }

    let coherent_cliff_regions = descriptors
        .iter()
        .filter(|descriptor| descriptor.coherent_cliff_fraction >= 0.002)
        .count();
    let spike_regions = descriptors
        .iter()
        .filter(|descriptor| descriptor.spike_outlier_fraction > 0.002)
        .count();
    let contiguous_open_regions = descriptors
        .iter()
        .filter(|descriptor| descriptor.largest_open_patch_fraction >= 0.40)
        .count();
    let quiet_regions = descriptors
        .iter()
        .filter(|descriptor| descriptor.quiet_terrain_fraction >= 0.60)
        .count();
    let strong_tree_regions = descriptors
        .iter()
        .filter(|descriptor| descriptor.tree_identity.is_strong())
        .count();
    report.push_str("\n## Terrain and openness\n\n");
    writeln!(
        report,
        "- Quiet terrain dominates at least 60% of the fine audit grid in {quiet_regions}/{} regions.",
        descriptors.len()
    )
    .expect("writing to String cannot fail");
    writeln!(
        report,
        "- Coherent cliff-class faces occur in {coherent_cliff_regions}/{} regions; \
         {spike_regions}/{} regions exceed the isolated-spike threshold.",
        descriptors.len(),
        descriptors.len()
    )
    .expect("writing to String cannot fail");
    writeln!(
        report,
        "- A single connected open-land patch covers at least 40% of the broad audit grid in \
         {contiguous_open_regions}/{} regions.",
        descriptors.len()
    )
    .expect("writing to String cannot fail");
    report.push_str(
        "\nSlope classes are measured on a deterministic 500 m lattice: quiet `<0.035`, \
         rolling `0.035–0.25`, steep `0.25–0.75`, and cliff-class `≥0.75`. \
         Cliff coverage counts only connected faces of at least three cells; strict isolated \
         extrema are reported separately as spike outliers.\n",
    );

    report.push_str("\n## Regional tree identity\n\n");
    writeln!(
        report,
        "{strong_tree_regions}/{} regions have a single tree functional group at or above the \
         52% regional-share threshold.\n",
        descriptors.len()
    )
    .expect("writing to String cannot fail");
    report.push_str("| Tree-group outcome | Regions |\n|---|---:|\n");
    for identity in TreeIdentity::ALL {
        writeln!(
            report,
            "| {} | {} |",
            identity.label(),
            novelty
                .tree_identity_counts
                .get(&identity)
                .copied()
                .unwrap_or(0)
        )
        .expect("writing to String cannot fail");
    }

    report.push_str("\n## Findings\n\n");
    if novelty.coverage_findings.is_empty()
        && novelty.suspicious_pairs.is_empty()
        && novelty.outliers.is_empty()
    {
        report.push_str(
            "All configured outcomes are represented, with no descriptor duplicates or \
             plausibility outliers detected.\n",
        );
    } else {
        for finding in &novelty.coverage_findings {
            writeln!(report, "- Coverage gap: {finding}").expect("writing to String cannot fail");
        }
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

    report.push_str("\n## Viewpoint search method\n\n");
    writeln!(
        report,
        "Landscape descriptors and novelty statistics use only the deterministically randomized \
         region centers. Viewpoint capture searches the local {VIEW_SEARCH_EDGE}×{VIEW_SEARCH_EDGE} \
         lattice around those centers, then directly inspects bounded, well-separated causal \
         maxima selected from a deterministic {CURATED_DISCOVERY_GRID_EDGE}×\
         {CURATED_DISCOVERY_GRID_EDGE} province/ecology/pristine-terrain scan. Curated maxima \
         affect viewpoint selection only; they do not bias descriptor coverage.\n"
    )
    .expect("writing to String cannot fail");
    report.push_str(
        "| Seed | Randomized descriptor centers | Curated causal maxima |\n|---|---:|---:|\n",
    );
    for suite in suites {
        writeln!(
            report,
            "| `{0:016x}` | {1} | {2} |",
            suite.seed,
            suite.randomized_center_count,
            suite.curated_centers.len()
        )
        .expect("writing to String cannot fail");
    }
    report.push('\n');
    for suite in suites {
        writeln!(report, "Causal maxima for seed `{0:016x}`:", suite.seed)
            .expect("writing to String cannot fail");
        for center in &suite.curated_centers {
            writeln!(
                report,
                "- {} at ({:.0}, {:.0}), normalized cause score `{:.3}`",
                center.cause.label(),
                center.position[0],
                center.position[1],
                center.score
            )
            .expect("writing to String cannot fail");
        }
        report.push('\n');
    }

    report.push_str("## Viewpoint coverage\n\n");
    let viewpoint_counts = viewpoint_coverage(suites);
    report.push_str("| Viewpoint | Seeds with a qualified frame | Coverage |\n|---|---:|---|\n");
    for kind in ViewpointKind::ALL {
        let count = viewpoint_counts.get(&kind).copied().unwrap_or(0);
        writeln!(
            report,
            "| {} | {}/{} | {} |",
            kind.label(),
            count,
            suites.len(),
            if count > 0 {
                "feature found"
            } else {
                "**fallbacks only**"
            }
        )
        .expect("writing to String cannot fail");
    }
    report.push('\n');
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
    let viewpoint_order = ViewpointKind::ALL
        .into_iter()
        .map(ViewpointKind::label)
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        report,
        "The binary `contact-sheet.ppm` stores deterministic hill-shaded frames in this fixed \
         order: {viewpoint_order}. Magenta-tinted panels are explicit fallback frames rather \
         than false positives."
    )
    .expect("writing to String cannot fail");
    if descriptors.is_empty() {
        report.push_str("\nNo descriptors were captured.\n");
    }
    report
}

fn viewpoint_coverage(suites: &[ViewpointSuite]) -> BTreeMap<ViewpointKind, usize> {
    let mut counts = BTreeMap::new();
    for suite in suites {
        for viewpoint in &suite.viewpoints {
            if viewpoint.qualified {
                *counts.entry(viewpoint.kind).or_default() += 1;
            }
        }
    }
    counts
}

fn missing_viewpoints(suites: &[ViewpointSuite]) -> Vec<ViewpointKind> {
    let counts = viewpoint_coverage(suites);
    ViewpointKind::ALL
        .into_iter()
        .filter(|kind| counts.get(kind).copied().unwrap_or(0) == 0)
        .collect()
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
            "--require-coverage".to_owned(),
        ];
        let config = AuditConfig::parse(&arguments).expect("valid audit arguments");
        assert_eq!(config.regions_per_seed, 4);
        assert_eq!(config.seeds, [0x5eed, 42]);
        assert!(!config.accept_baseline);
        assert!(config.require_coverage);
    }

    #[test]
    fn heightmap_batch_writes_deterministic_float_rasters_and_metadata() {
        let root =
            std::env::temp_dir().join(format!("treeline-heightmap-test-{}", std::process::id()));
        let request_path = root.join("request.json");
        let output_path = root.join("output");
        fs::create_dir_all(&root).expect("temporary root");
        fs::write(
            &request_path,
            serde_json::to_vec(&serde_json::json!({
                "generator_version": 18,
                "parameters": {"rolling_regional_relief_meters": 180.0},
                "rasters": [{
                    "id": "small",
                    "seed": "0x5eed",
                    "center_x_meters": -512_000.0,
                    "center_z_meters": 0.0,
                    "span_meters": 4000.0,
                    "edge": 4
                }]
            }))
            .expect("request JSON"),
        )
        .expect("request file");
        run_heightmap_batch(&[
            "--request".to_owned(),
            request_path.display().to_string(),
            "--output".to_owned(),
            output_path.display().to_string(),
        ])
        .expect("batch succeeds");
        assert_eq!(
            fs::read(output_path.join("small.f32"))
                .expect("float raster")
                .len(),
            4 * 4 * std::mem::size_of::<f32>()
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(output_path.join("small.json")).expect("metadata"))
                .expect("metadata JSON");
        assert_eq!(metadata["edge"], 4);
        assert_eq!(metadata["sampler"], "landform");
        assert_eq!(
            metadata["format"],
            "little-endian-f32-row-major-south-to-north"
        );
        fs::remove_dir_all(root).expect("remove precise temporary test root");
    }

    #[test]
    fn composed_heightmap_batch_rejects_offline_landform_overrides() {
        let root = std::env::temp_dir().join(format!(
            "treeline-composed-heightmap-test-{}",
            std::process::id()
        ));
        let request_path = root.join("request.json");
        fs::create_dir_all(&root).expect("temporary root");
        fs::write(
            &request_path,
            serde_json::to_vec(&serde_json::json!({
                "generator_version": 19,
                "sampler": "composed",
                "parameters": {"rolling_regional_relief_meters": 180.0},
                "rasters": [{
                    "id": "small",
                    "seed": "0x5eed",
                    "center_x_meters": 0.0,
                    "center_z_meters": 0.0,
                    "span_meters": 1000.0,
                    "edge": 2
                }]
            }))
            .expect("request JSON"),
        )
        .expect("request file");
        let error = run_heightmap_batch(&[
            "--request".to_owned(),
            request_path.display().to_string(),
            "--output".to_owned(),
            root.join("output").display().to_string(),
        ])
        .expect_err("overrides must be rejected");
        assert!(
            error
                .to_string()
                .contains("do not accept offline landform overrides")
        );
        fs::remove_dir_all(root).expect("remove precise temporary test root");
    }

    #[test]
    fn checked_in_parameter_schema_matches_current_rust_defaults() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tools/terrain_calibration/parameters.json"
        ))
        .expect("parameter schema JSON");
        let schema = schema.as_object().expect("parameter schema object");
        let named =
            LandformParameters::for_generator_version(CURRENT_GENERATOR_VERSION).named_values();
        assert_eq!(schema.len(), named.len());
        for (name, value) in named {
            assert_eq!(
                schema[name]["default"]
                    .as_f64()
                    .expect("numeric default")
                    .to_bits(),
                value.to_bits(),
                "schema default drifted for {name}"
            );
        }
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
    fn curated_discovery_lattice_is_seeded_stable_and_bounded() {
        let first = curated_discovery_positions(0x5eed);
        let repeated = curated_discovery_positions(0x5eed);
        let other_seed = curated_discovery_positions(0xa11c_e5ed);
        assert_eq!(first, repeated);
        assert_ne!(first, other_seed);
        assert_eq!(
            first.len(),
            CURATED_DISCOVERY_GRID_EDGE * CURATED_DISCOVERY_GRID_EDGE
        );

        let half = i64::try_from(CURATED_DISCOVERY_GRID_EDGE / 2).expect("grid edge is small");
        let coordinate_bound =
            index_as_f64(half * CURATED_DISCOVERY_CELL_METERS + CURATED_DISCOVERY_JITTER_METERS);
        assert!(
            first
                .iter()
                .flatten()
                .all(|coordinate| coordinate.abs() <= coordinate_bound)
        );
        let unique = first
            .iter()
            .map(|position| position.map(f64::to_bits))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), first.len());
    }

    #[test]
    fn curated_cause_selection_is_ranked_bounded_and_well_separated() {
        let mut candidates = Vec::new();
        let mut excluded_scores = [f64::NEG_INFINITY; CuratedCause::ALL.len()];
        excluded_scores[CuratedCause::Mountain.index()] = 1.0;
        candidates.push(CausalCandidate {
            position: [0.0, 0.0],
            scores: excluded_scores,
        });
        for (index, cause) in CuratedCause::ALL.into_iter().enumerate() {
            let mut scores = [f64::NEG_INFINITY; CuratedCause::ALL.len()];
            scores[cause.index()] = 0.9;
            candidates.push(CausalCandidate {
                position: [
                    usize_as_f64(index + 1) * CURATED_CENTER_MINIMUM_SEPARATION_METERS * 2.0,
                    0.0,
                ],
                scores,
            });
        }

        let selected = select_curated_view_centers(
            &candidates,
            &[[0.0, 0.0]],
            CURATED_CENTER_MINIMUM_SEPARATION_METERS,
        );
        assert_eq!(selected.len(), CuratedCause::ALL.len());
        assert_eq!(
            selected
                .iter()
                .map(|center| center.cause)
                .collect::<Vec<_>>(),
            CuratedCause::ALL
        );
        assert!(squared_horizontal_distance(selected[0].position, [0.0, 0.0]) > 0.0);
        for (index, left) in selected.iter().enumerate() {
            for right in &selected[index + 1..] {
                assert!(
                    squared_horizontal_distance(left.position, right.position)
                        >= CURATED_CENTER_MINIMUM_SEPARATION_METERS
                            * CURATED_CENTER_MINIMUM_SEPARATION_METERS
                );
            }
        }
    }

    #[test]
    fn viewpoint_suite_has_a_fixed_regression_order() {
        assert_eq!(ViewpointKind::ALL.len(), CONTACT_COLUMNS);
        assert_eq!(ViewpointKind::ALL[0], ViewpointKind::Valley);
        assert_eq!(ViewpointKind::ALL[6], ViewpointKind::Summit);
        assert_eq!(ViewpointKind::ALL[7], ViewpointKind::PrairieOrGrassland);
        assert_eq!(ViewpointKind::ALL[CONTACT_COLUMNS - 1], ViewpointKind::Reef);
    }

    #[test]
    fn terrain_metrics_separate_coherent_faces_from_isolated_spikes() {
        let edge = 9;
        let spacing = 100.0;
        let cliff = (0..edge)
            .flat_map(|_| (0..edge).map(|x| if x < edge / 2 { 0.0 } else { 1_000.0 }))
            .collect::<Vec<_>>();
        let cliff_metrics = analyze_height_grid(&cliff, edge, spacing).expect("cliff grid");
        assert!(cliff_metrics.cliff_fraction > 0.0);
        assert!(cliff_metrics.coherent_cliff_fraction > 0.0);
        assert!(cliff_metrics.largest_cliff_patch_fraction > 0.0);
        assert!(cliff_metrics.spike_outlier_fraction.abs() < f64::EPSILON);

        let mut spike = vec![0.0; edge * edge];
        spike[(edge / 2) * edge + edge / 2] = 1_000.0;
        let spike_metrics = analyze_height_grid(&spike, edge, spacing).expect("spike grid");
        assert!(spike_metrics.spike_outlier_fraction > 0.0);
        assert!(spike_metrics.coherent_cliff_fraction.abs() < f64::EPSILON);
    }

    #[test]
    fn terrain_metrics_recognize_a_quiet_flat() {
        let metrics = analyze_height_grid(&[42.0; 25], 5, 500.0).expect("flat grid");
        assert!((metrics.quiet_fraction - 1.0).abs() < f64::EPSILON);
        assert!(metrics.rolling_fraction.abs() < f64::EPSILON);
        assert!(metrics.steep_fraction.abs() < f64::EPSILON);
        assert!(metrics.cliff_fraction.abs() < f64::EPSILON);
        assert!(metrics.p95_curvature.abs() < f64::EPSILON);
    }

    #[test]
    fn contiguous_open_land_uses_global_grid_adjacency() {
        let mask = [
            true, true, false, false, true, false, false, true, false, false, true, true,
        ];
        let mut sizes = connected_component_sizes(&mask, 4);
        sizes.sort_unstable();
        assert_eq!(sizes, [3, 3]);
    }

    #[test]
    fn missing_outcome_order_is_complete_and_deterministic() {
        let counts = BTreeMap::from([(AuditOutcome::Forest, 2), (AuditOutcome::Cave, 1)]);
        let missing = missing_outcomes(&counts);
        assert_eq!(missing.first(), Some(&AuditOutcome::PrairieOrGrassland));
        assert_eq!(missing.last(), Some(&AuditOutcome::Reef));
        assert_eq!(missing.len(), AuditOutcome::ALL.len() - 2);
    }

    #[test]
    fn viewpoint_coverage_counts_only_qualified_frames() {
        let suites = [
            ViewpointSuite {
                seed: 1,
                randomized_center_count: 0,
                curated_centers: Vec::new(),
                viewpoints: vec![
                    Viewpoint {
                        kind: ViewpointKind::Forest,
                        position: [0.0, 0.0],
                        score: 1.0,
                        qualified: true,
                    },
                    Viewpoint {
                        kind: ViewpointKind::Dune,
                        position: [0.0, 0.0],
                        score: 0.5,
                        qualified: false,
                    },
                ],
            },
            ViewpointSuite {
                seed: 2,
                randomized_center_count: 0,
                curated_centers: Vec::new(),
                viewpoints: vec![Viewpoint {
                    kind: ViewpointKind::Forest,
                    position: [1.0, 1.0],
                    score: 2.0,
                    qualified: true,
                }],
            },
        ];
        let coverage = viewpoint_coverage(&suites);
        assert_eq!(coverage.get(&ViewpointKind::Forest), Some(&2));
        assert!(!coverage.contains_key(&ViewpointKind::Dune));
    }

    #[test]
    fn fingerprint_changes_with_pixels_or_generator_version() {
        let first = audit_fingerprint(b"descriptor", &[1, 2, 3], 16);
        assert_ne!(first, audit_fingerprint(b"descriptor", &[1, 2, 4], 16));
        assert_ne!(first, audit_fingerprint(b"descriptor", &[1, 2, 3], 17));
    }
}
