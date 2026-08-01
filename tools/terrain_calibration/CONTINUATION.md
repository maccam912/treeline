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

Promotion is an explicit product decision: record the available quantitative and
visual evidence, then keep iterating after promotion when perspective playtests
or drainage review expose weaknesses.

## Current checkpoint (2026-07-31)

- Starting repository commit: `e553420` (`Add aerial exploration mode`).
- Rust exposes 46 bounded named landform parameters. Generator version 19 uses
  the `optimization-smoke-v2-best.json` values by default; version 18 remains
  bit-for-bit reproducible for comparisons.
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

The current active experiment can be recreated with:

```sh
python3 -m tools.terrain_calibration extract-manifest \
  --manifest tools/terrain_calibration/corpora/etopo-smoke-v2.json \
  --source-root data/terrain-calibration/sources/etopo-smoke \
  --output data/terrain-calibration/etopo-smoke-v2
python3 -m tools.terrain_calibration optimize \
  --schema tools/terrain_calibration/corpora/parameters-smoke-v2.json \
  --request data/terrain-calibration/generated-smoke-request.json \
  --reference data/terrain-calibration/etopo-smoke-v2/train-descriptors.json \
  --work artifacts/terrain-calibration/optimization-smoke-v2 \
  --generations 4 --population 8 --sigma 0.12 --seed 24302
```

The generated request comes from 256 deterministic 64² candidates made with
seed 24301, morphology-stratified down to 16 samples, and promoted to 256². The
current candidate digest is `1298090312dd94c1`; its exact portable parameter
map is `corpora/optimization-smoke-v2-best.json`. Local galleries are under
`artifacts/terrain-calibration/optimization-smoke-v2/{gallery,blind-gallery}`.

## Experiment ledger

Append entries here rather than replacing them.

| ID | Corpus | Search | Result | Decision |
|---|---|---|---|---|
| baseline-001 | One Southwest US ETOPO macro patch; one stratified v18 sample | None | Treeline quiet fraction 0.940 vs real 0.406; p95 slope 0.053 vs 0.240 | Build a geographically diverse smoke corpus and tune broad relief before considering v19 |
| toolchain-001 | Homebrew local environment | Upgrade GDAL 3.13.1_1 → 3.13.2; keep jpeg-xl 0.12.0 | ETOPO opens; GDAL links `libjxl.0.12`; Python binding reports 3130200 | Use GDAL for the multi-tile smoke corpus |
| corpus-001 | `etopo-smoke-v1`: 12 train, 4 validation, 4 holdout patches at 256² across 10 NOAA tiles | Fixed checked-in manifest; generated pool 256×64², selected defaults 16×256² | Train target: mean relief 3,918.79 m, median elevation 2,183.78 m, p95 slope 0.13538, quiet 0.52205. Default generated: 2,891.34 m, 222.18 m, 0.04614, 0.92995; loss 1.20784 | Run sensitivity with common 8×192² generated samples, then search the 10-parameter smoke schema |
| sensitivity-001 | `etopo-smoke-v1` train; 8 common generated samples at 192² | ±15% of each active bound span, seed 24301 | Highest response: tectonic width 5.8167; next peak range 0.8707, continental relief 0.5134, rolling relief 0.3917, base elevation 0.3619, regional wavelength 0.3322. Best single directions included shorter regional wavelength and stronger tectonic/rolling/rugged relief | Optimize `corpora/parameters-smoke-v1.json`; do not treat sensitivity score as validation |
| optimize-001 | `etopo-smoke-v1` train; 16 common generated samples at 256² | Diagonal CMA-style, 10 parameters, 4 generations × 8 population, sigma 0.16, seed 24301 | Train 1.20784 → 1.10050; validation 1.78777 → 1.68639; holdout 0.92524 → 0.98433. Mean p95 slope 0.04614 → 0.07312 and relief 2,891.34 → 3,904.33 m, but quiet stayed 0.89193 and median elevation only reached 245.40 m. Exact rejected values are `optimization-smoke-v1-best.json` | Reject for production: holdout regressed and gallery shows more isolated straight ridges, not nested real relief. Expand parameterization with optional domain-warped broad/mesoscale relief whose v18 amplitudes default to zero |
| parameterization-001 | Same 16 common generated samples and `etopo-smoke-v1` train | Added optional domain-warped fractal broad, mesoscale, and ridged relief; v18 amplitudes are zero. Manual steep continental mapping plus sign-constrained background trial | Train loss reached 0.66473 (v18 1.20784); mean relief 3,779.93 vs 3,918.79 m, p95 slope 0.13458 vs 0.13538, quiet 0.50101 vs 0.52205. Median elevation remains low (909.61 vs 2,183.78 m); validation/holdout are not representative enough and score 1.83004/1.24825 | Keep new controls offline-only. Expand/rebalance geographic splits and optimize sea-level contrast plus background controls; do not promote the manual values |
| corpus-002 | `etopo-smoke-v2`: 12 train, 6 validation, 10 holdout patches at 256² across 14 non-overlapping source tiles | Added Great Plains, Sahara, Amazon, and Australian interior; each source tile belongs to exactly one split | Train/validation/holdout mean relief 2,855.67/2,496.29/3,043.01 m; p95 slope 0.09527/0.08541/0.12733; quiet 0.67239/0.70036/0.62857. Default losses 0.69559/0.69529/0.76919. Aggressive v1-tuned trial is rejected on all three | Use v2 for the next staged search. `parameters-smoke-v2.json` fixes tested local settings while varying 12 macro/multiscale controls; inactive defaults are now retained in every proposal |
| optimize-002 | `etopo-smoke-v2` train; 16 common generated samples at 256² | Staged diagonal CMA-style, 12 active + 7 fixed parameters, 4 generations × 8 population, sigma 0.12, seed 24302 | Train 0.69559 → 0.57361 (-17.5%); validation 0.69529 → 0.60274 (-13.3%); holdout 0.76919 → 0.76422 (-0.6%). Candidate mean relief 3,003.07 m, p95 slope 0.09702, quiet 0.61670; exact values are `optimization-smoke-v2-best.json`, digest `1298090312dd94c1` | Promoted as the generator-version-19 default by product decision. Gallery is much richer than v18; perspective playtesting, drainage review, and longer coherent ranges remain the next iteration targets. |
