#!/usr/bin/env python3
"""Build Treeline's default surveyed-world bundle from fixed Michigan sources."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import struct
import tempfile
from pathlib import Path

import numpy as np
from osgeo import gdal, osr
from osgeo import ogr


MAGIC = b"TLDEM01\0"
COLOR_MAGIC = b"TLRGB01\0"
WATER_MAGIC = b"TLWTR01\0"
CANOPY_MAGIC = b"TLCAN01\0"
TARGET_EPSG = 26916
TARGET_BOUNDS = (390_000.0, 5_110_000.0, 400_000.0, 5_120_000.0)
TARGET_SPACING_METERS = 2.0
COLOR_SPACING_METERS = 8.0
WATER_SPACING_METERS = 4.0
CANOPY_SPACING_METERS = 6.0
CANOPY_SOURCE_SPACING_METERS = 2.0
MINIMUM_TREE_HEIGHT_METERS = 2.0
QUANTIZATION_METERS = 0.1
SPAWN_UTM = (396_737.563_408_352, 5_112_788.298_230_720)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path, help="USGS 1-meter GeoTIFF")
    parser.add_argument("output", type=Path, help="output .tldem artifact")
    parser.add_argument(
        "--metadata", type=Path, help="optional neighboring provenance JSON"
    )
    parser.add_argument("--imagery", type=Path, help="natural-color aerial GeoTIFF")
    parser.add_argument("--color-output", type=Path, help="output .tlrgb artifact")
    parser.add_argument("--waterbodies", type=Path, help="USGS waterbody GeoJSON")
    parser.add_argument("--water-output", type=Path, help="output .tlwater artifact")
    parser.add_argument(
        "--canopy-ept",
        help="PDAL-readable EPT source used to derive local canopy cover and height",
    )
    parser.add_argument("--canopy-output", type=Path, help="output .tlcanopy artifact")
    return parser.parse_args()


def open_source(path: Path) -> gdal.Dataset:
    source = gdal.Open(str(path), gdal.GA_ReadOnly)
    if source is None:
        raise RuntimeError(f"could not open {path}")
    spatial_reference = osr.SpatialReference(wkt=source.GetProjection())
    authority = spatial_reference.GetAuthorityCode(None)
    if authority != str(TARGET_EPSG):
        raise ValueError(f"expected EPSG:{TARGET_EPSG}, found {authority!r}")
    return source


def resample(source: gdal.Dataset) -> np.ndarray:
    west, south, east, north = TARGET_BOUNDS
    width = round((east - west) / TARGET_SPACING_METERS)
    height = round((north - south) / TARGET_SPACING_METERS)
    options = gdal.WarpOptions(
        format="MEM",
        outputBounds=TARGET_BOUNDS,
        width=width,
        height=height,
        dstSRS=f"EPSG:{TARGET_EPSG}",
        resampleAlg=gdal.GRA_Average,
        outputType=gdal.GDT_Float32,
    )
    output = gdal.Warp("", source, options=options)
    if output is None:
        raise RuntimeError("GDAL failed to resample the source DEM")
    elevations = output.GetRasterBand(1).ReadAsArray()
    if elevations is None:
        raise RuntimeError("GDAL failed to read the resampled DEM")
    elevations = np.asarray(elevations, dtype=np.float64)
    if not np.isfinite(elevations).all() or np.any(elevations <= -100_000.0):
        raise ValueError("resampled DEM contains missing or non-finite elevations")
    return elevations


def encode(elevations: np.ndarray) -> bytes:
    quantized = np.rint(elevations / QUANTIZATION_METERS).astype(np.int64)
    if quantized.min() < np.iinfo(np.int16).min or quantized.max() > np.iinfo(np.int16).max:
        raise ValueError("decimeter elevations do not fit in signed 16-bit storage")

    height, width = quantized.shape
    west, _south, _east, north = TARGET_BOUNDS
    west_pixel_center = west + TARGET_SPACING_METERS * 0.5
    north_pixel_center = north - TARGET_SPACING_METERS * 0.5
    encoded = bytearray(
        struct.pack(
            "<8sIIddd",
            MAGIC,
            width,
            height,
            west_pixel_center - TARGET_BOUNDS[0],
            north_pixel_center - TARGET_BOUNDS[1],
            TARGET_SPACING_METERS,
        )
    )
    for row in quantized:
        previous = int(row[0])
        encoded.extend(struct.pack("<h", previous))
        for value in row[1:]:
            current = int(value)
            delta = current - previous
            if -127 <= delta <= 127:
                encoded.extend(struct.pack("<b", delta))
            else:
                if not np.iinfo(np.int16).min <= delta <= np.iinfo(np.int16).max:
                    raise ValueError("adjacent elevation delta exceeds signed 16-bit storage")
                encoded.extend(struct.pack("<bh", -128, delta))
            previous = current
    return bytes(encoded)


def resample_rgb(path: Path) -> np.ndarray:
    source = gdal.Open(str(path), gdal.GA_ReadOnly)
    if source is None or source.RasterCount < 3:
        raise RuntimeError(f"could not open three-band imagery at {path}")
    west, south, east, north = TARGET_BOUNDS
    width = round((east - west) / COLOR_SPACING_METERS)
    height = round((north - south) / COLOR_SPACING_METERS)
    output = gdal.Warp(
        "",
        source,
        options=gdal.WarpOptions(
            format="MEM",
            outputBounds=TARGET_BOUNDS,
            width=width,
            height=height,
            dstSRS=f"EPSG:{TARGET_EPSG}",
            resampleAlg=gdal.GRA_Average,
            outputType=gdal.GDT_Byte,
        ),
    )
    if output is None:
        raise RuntimeError("GDAL failed to resample the aerial imagery")
    bands = [output.GetRasterBand(index).ReadAsArray() for index in range(1, 4)]
    if any(band is None for band in bands):
        raise RuntimeError("GDAL failed to read the aerial imagery")
    return np.stack(bands, axis=-1).astype(np.uint8)


def encode_rgb(rgb: np.ndarray) -> bytes:
    height, width, channels = rgb.shape
    if channels != 3:
        raise ValueError("natural-color imagery must have three bands")
    west, _south, _east, north = TARGET_BOUNDS
    header = struct.pack(
        "<8sIIddd",
        COLOR_MAGIC,
        width,
        height,
        west + (COLOR_SPACING_METERS * 0.5) - TARGET_BOUNDS[0],
        north - (COLOR_SPACING_METERS * 0.5) - TARGET_BOUNDS[1],
        COLOR_SPACING_METERS,
    )
    red = np.right_shift(rgb[:, :, 0], 3).astype("<u2")
    green = np.right_shift(rgb[:, :, 1], 2).astype("<u2")
    blue = np.right_shift(rgb[:, :, 2], 3).astype("<u2")
    packed = np.left_shift(red, 11) | np.left_shift(green, 5) | blue
    return header + packed.astype("<u2", copy=False).tobytes()


def ept_bounds() -> str:
    source = osr.SpatialReference()
    source.ImportFromEPSG(TARGET_EPSG)
    target = osr.SpatialReference()
    target.ImportFromEPSG(3857)
    source.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    target.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    transform = osr.CoordinateTransformation(source, target)
    west, south, east, north = TARGET_BOUNDS
    corners = [
        transform.TransformPoint(x, y)
        for x, y in ((west, south), (west, north), (east, south), (east, north))
    ]
    xs = [point[0] for point in corners]
    ys = [point[1] for point in corners]
    margin = 10.0
    return (
        f"([{min(xs) - margin},{max(xs) + margin}],"
        f"[{min(ys) - margin},{max(ys) + margin}])"
    )


def derive_canopy(ept: str, elevations: np.ndarray) -> tuple[bytes, dict]:
    """Derive six-meter canopy occupancy and top height from non-ground returns."""
    west, south, east, north = TARGET_BOUNDS
    with tempfile.TemporaryDirectory(prefix="treeline-canopy-") as directory:
        surface_path = Path(directory) / "surface.tif"
        center_bounds = (
            f"([{west + 1.0},{east - 1.0}],"
            f"[{south + 1.0},{north - 1.0}])"
        )
        pipeline = {
            "pipeline": [
                {
                    "type": "readers.ept",
                    "filename": ept,
                    "bounds": ept_bounds(),
                },
                {"type": "filters.reprojection", "out_srs": f"EPSG:{TARGET_EPSG}"},
                {
                    "type": "filters.crop",
                    "bounds": f"([{west},{east}],[{south},{north}])",
                },
                # This project classifies ground, water, noise, and overlap but
                # leaves valid above-ground returns in ASPRS class 1.
                {"type": "filters.range", "limits": "Classification[1:1]"},
                {
                    "type": "writers.gdal",
                    "filename": str(surface_path),
                    "resolution": CANOPY_SOURCE_SPACING_METERS,
                    "output_type": "max",
                    "bounds": center_bounds,
                    "data_type": "float",
                    "nodata": -9999,
                },
            ]
        }
        subprocess.run(
            ["pdal", "pipeline", "--stdin"],
            input=json.dumps(pipeline),
            text=True,
            check=True,
        )
        surface_source = gdal.Open(str(surface_path), gdal.GA_ReadOnly)
        if surface_source is None:
            raise RuntimeError("PDAL did not create the canopy surface")
        surface = surface_source.GetRasterBand(1).ReadAsArray()
        if surface is None:
            raise RuntimeError("GDAL could not read the canopy surface")
        surface = np.asarray(surface, dtype=np.float64)

    if surface.shape != elevations.shape:
        raise ValueError(
            f"canopy surface shape {surface.shape} does not match DEM {elevations.shape}"
        )
    heights = surface - elevations
    occupied = (
        np.isfinite(surface)
        & (surface > -9000.0)
        & (heights >= MINIMUM_TREE_HEIGHT_METERS)
        & (heights <= 60.0)
    )
    heights = np.where(occupied, heights, 0.0)

    source_cells_per_canopy_cell = round(
        CANOPY_SPACING_METERS / CANOPY_SOURCE_SPACING_METERS
    )
    source_height, source_width = heights.shape
    canopy_height = (
        source_height + source_cells_per_canopy_cell - 1
    ) // source_cells_per_canopy_cell
    canopy_width = (
        source_width + source_cells_per_canopy_cell - 1
    ) // source_cells_per_canopy_cell
    padded_shape = (
        canopy_height * source_cells_per_canopy_cell,
        canopy_width * source_cells_per_canopy_cell,
    )
    padded_heights = np.zeros(padded_shape, dtype=np.float64)
    padded_occupied = np.zeros(padded_shape, dtype=np.uint8)
    padded_heights[:source_height, :source_width] = heights
    padded_occupied[:source_height, :source_width] = occupied

    block_shape = (
        canopy_height,
        source_cells_per_canopy_cell,
        canopy_width,
        source_cells_per_canopy_cell,
    )
    block_heights = padded_heights.reshape(block_shape)
    block_occupied = padded_occupied.reshape(block_shape)
    top_height = block_heights.max(axis=(1, 3))
    occupied_count = block_occupied.sum(axis=(1, 3))
    samples_per_cell = source_cells_per_canopy_cell**2
    cover = np.rint(occupied_count * (255.0 / samples_per_cell)).astype(np.uint8)
    height_half_meters = np.rint(top_height * 2.0).clip(0, 255).astype(np.uint8)

    west_pixel_center = west + (CANOPY_SPACING_METERS * 0.5)
    north_pixel_center = north - (CANOPY_SPACING_METERS * 0.5)
    header = struct.pack(
        "<8sIIddd",
        CANOPY_MAGIC,
        canopy_width,
        canopy_height,
        west_pixel_center - west,
        north_pixel_center - south,
        CANOPY_SPACING_METERS,
    )
    interleaved = np.stack((cover, height_half_meters), axis=-1)
    metadata = {
        "artifact_size_bytes": len(header) + interleaved.size,
        "spacing_meters": CANOPY_SPACING_METERS,
        "width": canopy_width,
        "height": canopy_height,
        "minimum_tree_height_meters": MINIMUM_TREE_HEIGHT_METERS,
        "height_quantization_meters": 0.5,
        "cover_method": "fraction of 2 m cells with non-ground returns at least 2 m above the bare-earth DEM",
        "height_method": "maximum terrain-normalized non-ground return in each 6 m cell",
        "source": ept,
        "source_rights": "Public domain (USGS 3DEP)",
    }
    return header + interleaved.tobytes(), metadata


def rasterize_waterbodies(path: Path, elevations: np.ndarray) -> tuple[bytes, list[dict]]:
    source = ogr.Open(str(path), 0)
    if source is None:
        raise RuntimeError(f"could not open waterbodies at {path}")
    layer = source.GetLayer(0)
    west, south, east, north = TARGET_BOUNDS
    width = round((east - west) / WATER_SPACING_METERS)
    height = round((north - south) / WATER_SPACING_METERS)
    driver = gdal.GetDriverByName("MEM")
    lake_ids = driver.Create("", width, height, 1, gdal.GDT_Byte)
    lake_ids.SetGeoTransform((west, WATER_SPACING_METERS, 0.0, north, 0.0, -WATER_SPACING_METERS))
    spatial_reference = osr.SpatialReference()
    spatial_reference.ImportFromEPSG(TARGET_EPSG)
    lake_ids.SetProjection(spatial_reference.ExportToWkt())
    band = lake_ids.GetRasterBand(1)
    band.Fill(0)

    features: list[dict] = []
    layer.ResetReading()
    for feature in layer:
        geometry = feature.GetGeometryRef()
        if geometry is None or geometry.IsEmpty():
            continue
        lake_id = len(features) + 1
        if lake_id > 255:
            raise ValueError("fixed water artifact supports at most 255 lakes")
        memory_source = ogr.GetDriverByName("MEM").CreateDataSource("")
        single_feature_layer = memory_source.CreateLayer(
            "waterbody", spatial_reference, ogr.wkbPolygon
        )
        copied_feature = ogr.Feature(single_feature_layer.GetLayerDefn())
        copied_feature.SetGeometry(geometry.Clone())
        single_feature_layer.CreateFeature(copied_feature)
        gdal.RasterizeLayer(lake_ids, [1], single_feature_layer, burn_values=[lake_id])
        centroid = geometry.Centroid()
        name = feature.GetField("GNIS_NAME") or f"Unnamed lake {lake_id}"
        features.append(
            {
                "id": lake_id,
                "permanent_identifier": feature.GetField("PERMANENT_IDENTIFIER"),
                "name": name,
                "area_square_kilometers": feature.GetField("AREASQKM"),
                "centroid_local_meters": [
                    centroid.GetX() - west,
                    centroid.GetY() - south,
                ],
            }
        )

    identifiers = band.ReadAsArray().astype(np.uint8)
    terrain_per_water_cell = 2
    surfaces_decimeters: list[int] = []
    for feature in features:
        mask = identifiers == feature["id"]
        terrain_mask = np.repeat(np.repeat(mask, terrain_per_water_cell, axis=0),
                                 terrain_per_water_cell, axis=1)
        samples = elevations[terrain_mask]
        if samples.size == 0:
            raise ValueError(f"waterbody {feature['name']} has no in-tile samples")
        surface = float(np.median(samples))
        surface_decimeters = round(surface / QUANTIZATION_METERS)
        if not np.iinfo(np.int16).min <= surface_decimeters <= np.iinfo(np.int16).max:
            raise ValueError("lake surface does not fit in signed 16-bit storage")
        surfaces_decimeters.append(surface_decimeters)
        feature["surface_elevation_meters"] = surface_decimeters * QUANTIZATION_METERS
        center_x, center_z = feature["centroid_local_meters"]
        feature["distance_from_spawn_meters"] = float(
            np.hypot(center_x - (SPAWN_UTM[0] - west), center_z - (SPAWN_UTM[1] - south))
        )

    header = struct.pack(
        "<8sIIdddH",
        WATER_MAGIC,
        width,
        height,
        west + (WATER_SPACING_METERS * 0.5) - west,
        north - (WATER_SPACING_METERS * 0.5) - south,
        WATER_SPACING_METERS,
        len(features),
    )
    encoded = bytearray(header)
    for elevation in surfaces_decimeters:
        encoded.extend(struct.pack("<h", elevation))
    for row in identifiers:
        start = 0
        while start < width:
            value = int(row[start])
            end = start + 1
            while end < width and row[end] == value and end - start < 65_535:
                end += 1
            encoded.extend(struct.pack("<BH", value, end - start))
            start = end
    return bytes(encoded), features


def main() -> None:
    args = parse_args()
    gdal.UseExceptions()
    source = open_source(args.source)
    elevations = resample(source)
    encoded = encode(elevations)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(encoded)

    color_encoded = None
    if args.imagery or args.color_output:
        if not args.imagery or not args.color_output:
            raise ValueError("--imagery and --color-output must be provided together")
        color_encoded = encode_rgb(resample_rgb(args.imagery))
        args.color_output.parent.mkdir(parents=True, exist_ok=True)
        args.color_output.write_bytes(color_encoded)

    water_encoded = None
    water_features: list[dict] = []
    if args.waterbodies or args.water_output:
        if not args.waterbodies or not args.water_output:
            raise ValueError("--waterbodies and --water-output must be provided together")
        water_encoded, water_features = rasterize_waterbodies(args.waterbodies, elevations)
        args.water_output.parent.mkdir(parents=True, exist_ok=True)
        args.water_output.write_bytes(water_encoded)

    canopy_encoded = None
    canopy_metadata = None
    if args.canopy_ept or args.canopy_output:
        if not args.canopy_ept or not args.canopy_output:
            raise ValueError("--canopy-ept and --canopy-output must be provided together")
        canopy_encoded, canopy_metadata = derive_canopy(args.canopy_ept, elevations)
        args.canopy_output.parent.mkdir(parents=True, exist_ok=True)
        args.canopy_output.write_bytes(canopy_encoded)

    if args.metadata:
        metadata = {
            "schema_version": 1,
            "settings_identity": "0x5355525645590002",
            "artifact_sha256": hashlib.sha256(encoded).hexdigest(),
            "artifact_size_bytes": len(encoded),
            "center_wgs84": [46.16084629042455, -88.3374704874157],
            "horizontal_crs": "EPSG:26916 (NAD83 / UTM zone 16N)",
            "vertical_datum": "NAVD88",
            "bounds_utm_meters": list(TARGET_BOUNDS),
            "spacing_meters": TARGET_SPACING_METERS,
            "quantization_meters": QUANTIZATION_METERS,
            "width": int(elevations.shape[1]),
            "height": int(elevations.shape[0]),
            "minimum_elevation_meters": float(elevations.min()),
            "maximum_elevation_meters": float(elevations.max()),
            "source_title": "USGS 1 Meter 16 x39y512 MI_FEMA_2019_C19",
            "source_url": "https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/1m/Projects/MI_FEMA_2019_C19/TIFF/USGS_1M_16_x39y512_MI_FEMA_2019_C19.tif",
            "source_publication_date": "2023-01-14",
            "source_last_updated": "2024-02-07",
            "source_size_bytes": 311082711,
            "source_rights": "Public domain (USGS 3DEP)",
            "resampling": "2 meter average from the 1 meter bare-earth DEM",
        }
        if color_encoded is not None:
            metadata["color"] = {
                "artifact_sha256": hashlib.sha256(color_encoded).hexdigest(),
                "artifact_size_bytes": len(color_encoded),
                "spacing_meters": COLOR_SPACING_METERS,
                "encoding": "RGB565",
                "source": "USGS NAIP Imagery ImageServer, NaturalColor",
                "source_url": "https://imagery.nationalmap.gov/arcgis/rest/services/USGSNAIPImagery/ImageServer",
                "source_rights": "Public domain (USGS/USDA)",
            }
        if water_encoded is not None:
            metadata["water"] = {
                "artifact_sha256": hashlib.sha256(water_encoded).hexdigest(),
                "artifact_size_bytes": len(water_encoded),
                "spacing_meters": WATER_SPACING_METERS,
                "runtime_level_offset_meters": 1.0,
                "source": "USGS National Hydrography Dataset, Waterbody - Large Scale",
                "source_url": "https://hydro.nationalmap.gov/arcgis/rest/services/nhd/MapServer/12",
                "source_rights": "Open and non-proprietary (USGS)",
                "surface_method": "median bare-earth DEM elevation inside each polygon",
                "features": sorted(
                    water_features,
                    key=lambda feature: feature["distance_from_spawn_meters"],
                ),
            }
        if canopy_encoded is not None and canopy_metadata is not None:
            canopy_metadata["artifact_sha256"] = hashlib.sha256(canopy_encoded).hexdigest()
            metadata["canopy"] = canopy_metadata
        args.metadata.parent.mkdir(parents=True, exist_ok=True)
        args.metadata.write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
