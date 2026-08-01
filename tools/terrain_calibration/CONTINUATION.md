# Terrain calibration continuation log

This file is the durable handoff for the real-terrain calibration effort. Update
it after every material dataset, search, or generator checkpoint so work can
resume after a context reset without relying on chat history. The general user
workflow remains in `README.md`; this file records the current experiment.

## Objective and acceptance gate

Tune Treeline's deterministic terrain parameters against 512×512 km real Earth
heightmaps while preserving coherent quiet country. Heights stay in physical
meters with sea level at 0 m and a fixed 0–9,000 m land display range. Destination
families (mountain, coast, cliff, and incised terrain) receive 2× sampling weight,
but generated evaluation sets retain at least 35% quiet terrain.

Do not promote a new generator version from training loss alone. A candidate must
also pass an untouched geographic holdout, a blind heightmap gallery, hydrology
and boundary invariants, perspective rendering, and the full repository gate.

## Current checkpoint (2026-07-31)

- Starting repository commit: `e553420` (`Add aerial exploration mode`).
- Rust exposes 41 bounded named landform parameters. Supplying the version-18
  defaults remains bit-for-bit identical to production generation.
- `world-viewer heightmap-batch` exports deterministic little-endian float32
  heightmaps. `world-viewer terrain-parameters` exports the defaults.
- The Python pipeline prepares real rasters, selects generated candidates,
  measures descriptors, ranks sensitivity, optimizes parameters, and builds
  labeled or blind galleries.
- The first official reference is NOAA ETOPO 2022 tile
  `ETOPO_2022_v1_15s_N45W120_surface.tif`, SHA-256
  `806d5dbc40838e1b89c6afb7836fd62c92c47485bfc9a3aa453c3efa05b01a43`.
  The extracted patch is centered at 39°N, 112.5°W, spans 512 km, and is
  1,024×1,024 (500 m cells).
- The first selected Treeline sample uses seed `930c5483d2e3d818`, center
  `(44,288,000 m, -4,096,000 m)`, and the same span/resolution.
- Baseline real versus Treeline measurements are: total relief 3,572.87 versus
  3,839.70 m; median elevation 1,765.87 versus 296.58 m; p95 slope 0.23967
  versus 0.05257; mean slope 0.07497 versus 0.01290; quiet fraction 0.40639
  versus 0.93966. The diagnosis is broad terrain that is much too quiet, with
  relief concentrated in sparse elongated features and at least one visible
  straight diagonal artifact.
- The ignored first gallery is
  `artifacts/terrain-calibration/initial-512km-comparison/index.html`.
- The initial implementation passed formatting, strict Clippy, 284 Rust tests,
  warning-free Rustdoc, Python unit tests, and `git diff --check`.

## Local toolchain state

- Homebrew `jpeg-xl` 0.12.0 and GDAL 3.13.2 are installed. GDAL was upgraded
  from `3.13.1_1` on 2026-07-31 and now links to `libjxl.0.12.dylib` rather than
  the missing `libjxl.0.11.dylib`.
- `gdalinfo --version`, opening the ETOPO proof tile, and
  `python3 -c 'from osgeo import gdal; print(gdal.VersionInfo())'` all succeed;
  the binding reports native version `3130200`.
- The Pillow fallback successfully reads simple floating-point, north-up
  geographic GeoTIFF tiles. Working GDAL now provides mosaics, VRTs, and
  arbitrary source projections.

Verify the repaired stack with:

```sh
brew upgrade jpeg-xl gdal
brew reinstall gdal
gdalinfo --version
otool -L "$(command -v gdalinfo)" | grep -E 'jxl|gdal'
python3 -c 'from osgeo import gdal; print(gdal.VersionInfo())'
```

The Python binding may be absent from Homebrew's active Python even when the GDAL
CLI works; the pipeline can use the CLI. Do not install an unrelated PyPI GDAL
version unless it exactly matches the native library.

## Resume commands

Build the sampler and recover the first proof (source downloads and derived data
are deliberately ignored by Git):

```sh
cargo build -p world-viewer --release
mkdir -p data/terrain-calibration/sources/etopo
curl -L \
  https://www.ngdc.noaa.gov/mgg/global/relief/ETOPO2022/data/15s/15s_surface_elev_gtif/ETOPO_2022_v1_15s_N45W120_surface.tif \
  -o data/terrain-calibration/sources/etopo/ETOPO_2022_v1_15s_N45W120_surface.tif
shasum -a 256 data/terrain-calibration/sources/etopo/ETOPO_2022_v1_15s_N45W120_surface.tif
python3 -m tools.terrain_calibration extract \
  --source data/terrain-calibration/sources/etopo/ETOPO_2022_v1_15s_N45W120_surface.tif \
  --output data/terrain-calibration/etopo-proof --id southwest-us \
  --latitude 39 --longitude -112.5 --span 512000 --edge 1024
```

The next reproducible experiment is:

1. Download enough adjacent ETOPO surface tiles to form a VRT covering multiple
   continents and coasts. Preserve URL, checksum, product version, datum, and
   access date in a source manifest.
2. Prepare geographically blocked train/validation/holdout macro sets. Start
   with a tractable smoke corpus (about 32/8/8 patches at edge 256), prove the
   search loop, then expand toward the documented 1,536/256/256 target.
3. Generate at least 256 coarse Treeline candidates, stratify 32–64 full-size
   candidates, measure both corpora, and run sensitivity before optimization.
4. Optimize a small influential subset first. Record every command, schema hash,
   request hash, reference-manifest hash, seed, loss, and resulting parameters
   below. Keep common generated seeds across proposals.
5. Compare defaults and the best candidate on validation, then render a blind
   gallery. Only after that consider writing candidate values into Rust.

Template commands are in `README.md`. Run short smoke searches first, for example:

```sh
python3 -m tools.terrain_calibration optimize \
  --schema tools/terrain_calibration/parameters.json \
  --request data/terrain-calibration/calibration-request.json \
  --reference data/terrain-calibration/etopo-train-descriptors.json \
  --work artifacts/terrain-calibration/optimization-smoke \
  --generations 3 --population 6 --seed 24301
```

## Experiment ledger

Append entries here rather than replacing them.

| ID | Corpus | Search | Result | Decision |
|---|---|---|---|---|
| baseline-001 | One Southwest US ETOPO macro patch; one stratified v18 sample | None | Treeline quiet fraction 0.940 vs real 0.406; p95 slope 0.053 vs 0.240 | Build a geographically diverse smoke corpus and tune broad relief before considering v19 |
| toolchain-001 | Homebrew local environment | Upgrade GDAL 3.13.1_1 → 3.13.2; keep jpeg-xl 0.12.0 | ETOPO opens; GDAL links `libjxl.0.12`; Python binding reports 3130200 | Use GDAL for the multi-tile smoke corpus |
| corpus-001 | `etopo-smoke-v1`: 12 train, 4 validation, 4 holdout patches at 256² across 10 NOAA tiles | Fixed checked-in manifest; generated pool 256×64², selected defaults 16×256² | Train target: mean relief 3,918.79 m, median elevation 2,183.78 m, p95 slope 0.13538, quiet 0.52205. Default generated: 2,891.34 m, 222.18 m, 0.04614, 0.92995; loss 1.20784 | Run sensitivity with common 8×192² generated samples, then search the 10-parameter smoke schema |
| sensitivity-001 | `etopo-smoke-v1` train; 8 common generated samples at 192² | ±15% of each active bound span, seed 24301 | Highest response: tectonic width 5.8167; next peak range 0.8707, continental relief 0.5134, rolling relief 0.3917, base elevation 0.3619, regional wavelength 0.3322. Best single directions included shorter regional wavelength and stronger tectonic/rolling/rugged relief | Optimize `corpora/parameters-smoke-v1.json`; do not treat sensitivity score as validation |
