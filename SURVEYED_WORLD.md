# Surveyed-world data contract

Treeline's player-facing default is terrain reconstructed from measured Earth
data. The first checked-in bundle is a 10 km square in Michigan's Upper
Peninsula. Procedural terrain remains available for generator research, tests,
subsurface features, and an eventual explicit gap-filling policy; it is not the
default surface presented to players.

This document is the source of truth for turning real-world observations into
a Treeline terrain bundle. `tools/surveyed_tile/prepare.py` is the current
reference implementation. It prepares one complete tile offline and the client
embeds and decodes that tile at startup. A world-scale precalculation service,
tile cache, and network streaming are deliberately deferred.

## Bundle contract

A surveyed tile uses one projected coordinate reference system whose units are
meters. All layers must cover the same axis-aligned bounds and record their
source, acquisition date when known, rights, processing method, resolution,
quantization, and output hash in the neighboring metadata JSON.

Runtime coordinates preserve physical scale and geographic handedness:

- world X increases east;
- world Z increases south, because source raster rows run north to south;
- world Y is elevation in meters in the source vertical datum;
- horizontal and vertical scale are both 1:1; and
- terrain is never normalized per tile or vertically exaggerated.

The current bundle contains four authoritative layers and one supporting
appearance layer:

| Layer | Authority | Current artifact | Runtime meaning |
| --- | --- | --- | --- |
| Bare earth | USGS 3DEP DEM | `.tldem` | Terrain surface height |
| Waterbodies | USGS NHD polygons + DEM | `.tlwater` | Lake footprint, identity, and level sheet |
| Canopy | USGS 3DEP point cloud + DEM | `.tlcanopy` | Tree-cover fraction and canopy-top height |
| Natural color | USGS NAIP | `.tlrgb` | Terrain surface color |
| Provenance | Pipeline output | `.json` | CRS, bounds, sources, methods, hashes, and inventory |

The DEM, lake footprint, and canopy measurements are data products. The smooth
voxel density field, water mesh, and individual tree meshes are runtime
representations derived from those products.

## Reproducible preparation recipe

### 1. Choose a tile

Choose a projected CRS appropriate to the location and square bounds expressed
in meters. Record both horizontal and vertical datums. Pick a recognizable
spawn position in WGS84, transform it into the projected CRS, and store its
local offset from the tile's west and north edges.

Every source must cover those exact bounds. Do not combine layers that merely
look aligned; reproject them into the tile CRS and verify their geotransforms.

### 2. Create terrain

Use a bare-earth elevation model, not a first-return surface model. Warp it to
the tile bounds with area averaging. The current contract uses two-meter cells,
quantizes elevations to decimeters, and row-delta encodes signed values in
`.tldem`.

Reject missing pixels, non-finite values, incompatible CRS metadata, and values
that overflow the artifact encoding. Keep absolute elevations. At runtime,
bilinear sampling converts the height surface into the signed-density contract:
negative below ground, zero at the surface, and positive above it.

### 3. Create lakes

Use authoritative waterbody polygons rather than detecting every depression in
the DEM. Reproject the polygons into the terrain CRS, clip them to the tile,
and rasterize stable feature identifiers. The current contract uses four-meter
cells.

Assign each lake a representative level from the median bare-earth elevation
inside its polygon, quantized to decimeters. Store the feature inventory and
method in metadata. Runtime water is a horizontal sheet over that mapped
footprint. This is not bathymetry: the implementation guarantees visible water
and a stable shoreline mask but does not claim measured lake-bottom depth.

Rivers and dynamic lake simulation are not inferred for a surveyed bundle
until authoritative linework, flow direction, and elevation conformance have a
separate reviewed contract.

### 4. Create tree cover

Use a lidar point cloud aligned with the DEM. Reproject and crop the points,
retain valid above-ground returns, and subtract the bare-earth height to obtain
height above terrain. The current contract treats returns at least two meters
above ground as canopy candidates, rejects implausible heights above 60 meters,
and aggregates them into six-meter cells.

Each canopy cell stores:

- cover fraction: the fraction of its two-meter source cells containing a
  canopy return; and
- canopy-top height: the maximum terrain-normalized return, quantized to half
  meters.

Cover controls local procedural-tree counts; measured height bounds individual
tree stature. Species, branching, age, damage, and stable placement identity
remain procedural because these inputs do not identify individual trees or
species. Cells without measured canopy remain open.

### 5. Create surface color

Natural-color aerial imagery is optional for geometry but part of the default
bundle's appearance. Reproject it to the same bounds, average it to eight-meter
cells, and encode RGB565. Do not use imagery pixels to change terrain height,
lake level, or canopy height.

### 6. Package and verify

For the checked-in Michigan source files, run:

```sh
python3 tools/surveyed_tile/prepare.py \
  /path/to/USGS_1M_16_x39y512_MI_FEMA_2019_C19.tif \
  crates/terrain/assets/michigan_tile.tldem \
  --metadata crates/terrain/assets/michigan_tile.json \
  --imagery /path/to/michigan_naip.tif \
  --color-output crates/terrain/assets/michigan_tile.tlrgb \
  --waterbodies /path/to/michigan_waterbodies.geojson \
  --water-output crates/terrain/assets/michigan_tile.tlwater \
  --canopy-ept \
    https://usgs-lidar-public.s3.amazonaws.com/MI_FEMA_Dickinson_Iron_Menominee_2019/ept.json \
  --canopy-output crates/terrain/assets/michigan_tile.tlcanopy
```

Review the generated metadata and run the full Rust gate. Tests verify artifact
dimensions, spacing, orientation, elevation range, mapped lake identity and
level, canopy variation, default-world selection, and deterministic tree
placement under measured stand adjustments.

## Identity and evolution

The surveyed bundle is selected by `DEFAULT_SURVEYED_SETTINGS_HASH`, while the
generator version continues to version procedural representations derived from
the measurements. Any incompatible change to an embedded artifact, coordinate
frame, sampler, or layer meaning requires a new settings identity. Never replace
bundle bytes while retaining an identity used by saved worlds.

The current out-of-bounds DEM behavior clamps to the nearest edge only so mesh
residency can complete at the finite tile boundary. Travel and random warps
must remain inside the measured footprint. Clamping is not a valid gap-filling
or world-expansion policy.

## Deferred delivery choices

The data-product contract above is independent of delivery. A later milestone
may choose either or both of these without changing terrain meaning:

1. Precalculate many versioned bundles, distribute them with manifests and
   hashes, and cache them locally.
2. Stream the same versioned layer tiles on demand and verify their hashes
   before admitting them to the world cache.

Both paths must preserve coordinate alignment, deterministic decoding, explicit
missing-data behavior, and saved-world identity. No delivery abstraction is
required while the single embedded tile is the product surface.
