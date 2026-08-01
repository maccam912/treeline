# Terrain calibration

This offline tool compares Treeline heightfields with real terrain in physical
meters. It does not ship with the game and never loads external parameters into
a production world.

The current experiment state, baseline measurements, recovery commands, and
append-only search ledger live in [`CONTINUATION.md`](CONTINUATION.md). Update it
at every material checkpoint before committing so a calibration run can resume
without conversation history.

Reference sources:

- [NOAA ETOPO 2022](https://www.ncei.noaa.gov/products/etopo-global-relief-model)
  supplies CC0 global 15-arc-second surface elevation for 512 km macro patches.
- [NASA NASADEM](https://data.nasa.gov/dataset/nasadem-srtm-subswath-global-1-arc-second-v001)
  supplies 1-arc-second local relief between 56°S and 60°N. NASA Earthdata may
  require an account for bulk download.

Downloaded sources and derived rasters belong under `data/terrain-calibration`
and are ignored by Git. Keep a source URL, product version, checksum, vertical
datum, and access date beside every source mosaic or VRT.

## Canonical raster contract

Each patch is a little-endian `float32` `.f32` file with a neighboring JSON
metadata file. Rows run south-to-north. Heights remain meters; water and
bathymetry remain negative, sea level is zero, and no patch is min-max scaled.
Maps use one fixed 0–9,000 m land palette.

- Macro: 1,024×1,024 cells, 500 m spacing, 512×512 km.
- Local: 512×512 cells, 30 m spacing, 15.36×15.36 km.
- Intended manifests: 1,536/256/256 ETOPO train/validation/holdout patches and
  768/128/128 NASADEM patches, grouped into 30-degree geographic blocks with a
  patch-width exclusion margin so rasters from different splits cannot overlap.

## Workflow

From the repository root:

```sh
cargo build -p world-viewer --release
python3 -m tools.terrain_calibration prepare \
  --tier macro --source data/terrain-calibration/etopo-surface.vrt \
  --output data/terrain-calibration/etopo-macro --count 2048
python3 -m tools.terrain_calibration prepare \
  --tier local --source data/terrain-calibration/nasadem.vrt \
  --output data/terrain-calibration/nasadem-local --count 1024
```

`prepare` uses local azimuthal-equidistant projections and equal-area random
centers. `extract` creates a named patch at an explicit latitude/longitude. GDAL
is preferred for mosaics and arbitrary source projections; a Pillow fallback
supports north-up, floating-point geographic GeoTIFF tiles.

Create a larger coarse generated candidate pool, retain land/coast candidates
with morphology-family stratification, then export the selected positions at
full resolution:

```sh
python3 -m tools.terrain_calibration make-request \
  --output data/terrain-calibration/candidates.json --count 512 --edge 128
python3 -m tools.terrain_calibration export \
  --request data/terrain-calibration/candidates.json \
  --output data/terrain-calibration/candidate-rasters
python3 -m tools.terrain_calibration select-request \
  --request data/terrain-calibration/candidates.json \
  --rasters data/terrain-calibration/candidate-rasters \
  --output data/terrain-calibration/calibration-request.json \
  --count 64 --edge 512
```

Measure references, rank sensitivity, and optimize. The search uses common
world seeds for every proposal, bounded diagonal CMA-style updates, progressive
request manifests, and a 2× target weight for mountain, cliff/incised, and
coastal terrain while retaining a 35% quiet-landscape floor.

```sh
python3 -m tools.terrain_calibration measure \
  --input data/terrain-calibration/etopo-macro \
  --output data/terrain-calibration/etopo-descriptors.json
python3 -m tools.terrain_calibration sensitivity \
  --schema tools/terrain_calibration/parameters.json \
  --request data/terrain-calibration/calibration-request.json \
  --reference data/terrain-calibration/etopo-descriptors.json \
  --work artifacts/terrain-calibration/sensitivity
python3 -m tools.terrain_calibration optimize \
  --schema tools/terrain_calibration/parameters.json \
  --request data/terrain-calibration/calibration-request.json \
  --reference data/terrain-calibration/etopo-descriptors.json \
  --work artifacts/terrain-calibration/optimization
```

Render labeled or anonymously shuffled maps with identical physical scales:

```sh
python3 -m tools.terrain_calibration report \
  --group Real=data/terrain-calibration/etopo-macro \
  --group Treeline=artifacts/terrain-calibration/optimization/best \
  --output artifacts/terrain-calibration/comparison --blind
```

Do not create a generator version from training scores alone. Review untouched
holdout metrics, the blind gallery, hydrology and boundary invariants, the
existing world-quality audit, and rendered perspective captures first.
