# Surveyed-world data contract

Treeline's world is terrain reconstructed from measured Earth data. The only
bundle is a 10 km square in Michigan's Upper Peninsula.

There is no procedural surface to fall back on. Treeline once synthesized its
terrain, hydrology, and forest cover; that machinery has been removed. A layer
this contract does not admit is simply absent from the world, and growing the
world means admitting more measured data — not resuming synthesis. The one place
a reviewed procedural contract clearly belongs is genuinely unobserved
structure, such as caves.

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

Assign each lake a representative source level from the median bare-earth
elevation inside its polygon, quantized to decimeters. Store the feature
inventory and method in metadata. Runtime water is a horizontal sheet over that
mapped footprint. The current bundle keeps the source-derived level and applies
a versioned four-meter horizontal expansion (one water-raster cell) around the
mapped footprint so the sheet intersects the surrounding shore. Metadata
records both the zero-meter level offset and horizontal expansion separately
from the source values. This is not bathymetry: the implementation guarantees
visible water and a stable shoreline mask but does not claim measured
lake-bottom depth.

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

Cover scales local stem counts and measured height bounds individual tree
stature: no tree may exceed the canopy top measured over it. Species, branching,
age, damage, and stable placement identity stay procedural, because these
measurements identify neither individual trees nor species. Cells without
measured canopy are open ground, not sparse forest.

### 5. Create surface color

Natural-color aerial imagery is optional for geometry but part of the bundle's
appearance. Reproject it to the same bounds, average it to eight-meter
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
level, footprint dilation, canopy variation, world selection, and that generated
trees stay inside the canopy measured over them.

## Identity and evolution

Two version numbers keep worlds honest. `SURVEYED_SETTINGS_HASH` selects the
bundle: any incompatible change to an embedded artifact, coordinate frame,
sampler, or layer meaning requires a new value. The generator version covers
everything derived from those measurements — tree individuals, meshes, snow.
Never replace bundle bytes while retaining an identity a saved world could use.

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
