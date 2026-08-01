# Default Michigan surveyed-world bundle

This tool prepares the embedded terrain, natural-color, mapped-water, and
canopy artifacts used by Treeline's default player world. The authoritative
layer meanings, coordinate rules, preparation recipe, versioning policy, and
future delivery boundary are defined in [`../../SURVEYED_WORLD.md`](../../SURVEYED_WORLD.md).
This implementation preserves horizontal and vertical meters without
normalization or relief exaggeration.

The source is the public-domain USGS 3DEP product `USGS 1 Meter 16 x39y512
MI_FEMA_2019_C19`, a bare-earth DEM derived from lidar. Download the source URL
recorded in `crates/terrain/assets/michigan_tile.json`, then run:

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

The fixed artifact covers a 10 km square in NAD83 / UTM zone 16N. It averages
the one-meter source onto a two-meter grid and quantizes elevation to decimeters.
Row-delta encoding keeps the checked-in bundle reasonably small. The client
decodes the complete tile once; there is deliberately no data streaming yet.
Raster rows run north to south. At runtime they map to the engine's right-handed
ground plane with X increasing east and Z increasing south; this avoids
mirroring east and west when the camera faces geographic north.

The color input is a natural-color export of the same bounds from the official
USGS NAIP ImageServer in EPSG:26916. The output averages it to eight-meter
RGB565 samples. The water input is a GeoJSON query of `FTYPE=390` features from
the USGS NHD `Waterbody - Large Scale` layer in EPSG:26916. The tool rasterizes
those polygons at four meters and assigns each lake the median bare-earth DEM
elevation within its mapped footprint. Exact source service URLs, hashes, and
the derived lake inventory are written to `michigan_tile.json`.

The canopy pass requires PDAL. It reads only the fixed footprint from
the USGS cloud-optimized EPT point cloud, reprojects it to the terrain CRS, and
subtracts the aligned bare-earth DEM from non-ground returns. Each six-meter
cell stores the fraction of its two-meter source cells with returns at least
two meters above terrain plus the local canopy-top height in half-meter units.
This project leaves above-ground returns in ASPRS class 1 rather than assigning
vegetation classes 3–5, so the height-above-terrain threshold is the explicit
vegetation proxy. The source is public-domain USGS 3DEP data.
