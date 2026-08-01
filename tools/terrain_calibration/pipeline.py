"""Prepare, measure, optimize, and render real/generated terrain datasets.

The production generator never imports this package. Reference rasters use a
small canonical format: little-endian float32, square, row-major, with rows
ordered south-to-north. A neighboring JSON file records physical dimensions.
"""

from __future__ import annotations

import argparse
from collections import deque
import hashlib
import html
import json
import math
import os
from pathlib import Path
import random
import shutil
import struct
import subprocess
import tempfile
import zlib

import numpy as np


MACRO_SPAN_METERS = 512_000.0
MACRO_EDGE = 1_024
LOCAL_SPAN_METERS = 15_360.0
LOCAL_EDGE = 512
LAND_HEIGHT_LIMIT_METERS = 9_000.0
METRIC_QUANTILES = np.asarray([0.05, 0.25, 0.5, 0.75, 0.95])


def _json_read(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def _json_write(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, sort_keys=True)
        handle.write("\n")


def read_raster(path: Path, edge: int | None = None) -> np.ndarray:
    if edge is None:
        edge = int(_json_read(path.with_suffix(".json"))["edge"])
    values = np.fromfile(path, dtype="<f4")
    if values.size != edge * edge:
        raise ValueError(f"{path} contains {values.size} cells, expected {edge * edge}")
    raster = values.reshape(edge, edge).astype(np.float64)
    if not np.all(np.isfinite(raster)):
        raise ValueError(f"{path} contains non-finite heights")
    return raster


def write_raster(path: Path, raster: np.ndarray, metadata: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    np.asarray(raster, dtype="<f4").tofile(path)
    _json_write(path.with_suffix(".json"), metadata)


def _gradient_metrics(height: np.ndarray, spacing: float):
    dz, dx = np.gradient(height, spacing, edge_order=1)
    slope = np.hypot(dx, dz)
    dxx = np.gradient(dx, spacing, axis=1, edge_order=1)
    dzz = np.gradient(dz, spacing, axis=0, edge_order=1)
    curvature = np.abs(dxx + dzz) * spacing
    return slope, curvature


def _block_relief(height: np.ndarray, cells: int) -> np.ndarray:
    cells = max(1, min(cells, min(height.shape)))
    rows = height.shape[0] // cells
    columns = height.shape[1] // cells
    if rows == 0 or columns == 0:
        return np.asarray([np.ptp(height)])
    cropped = height[: rows * cells, : columns * cells]
    blocks = cropped.reshape(rows, cells, columns, cells).transpose(0, 2, 1, 3)
    return blocks.max(axis=(2, 3)) - blocks.min(axis=(2, 3))


def _spectrum_metrics(height: np.ndarray) -> tuple[float, float]:
    centered = height - np.mean(height)
    spectrum = np.abs(np.fft.fftshift(np.fft.fft2(centered))) ** 2
    yy, xx = np.indices(spectrum.shape, dtype=np.float64)
    yy -= (spectrum.shape[0] - 1) * 0.5
    xx -= (spectrum.shape[1] - 1) * 0.5
    radius = np.hypot(xx, yy)
    valid = (radius >= 2.0) & (radius <= min(height.shape) * 0.35) & (spectrum > 0.0)
    if np.count_nonzero(valid) < 16:
        return 0.0, 0.0
    log_radius = np.log(radius[valid])
    log_power = np.log(spectrum[valid])
    spectral_slope = float(np.polyfit(log_radius, log_power, 1)[0])
    power = spectrum[valid]
    x_energy = float(np.sum(power * xx[valid] ** 2))
    z_energy = float(np.sum(power * yy[valid] ** 2))
    anisotropy = abs(x_energy - z_energy) / max(x_energy + z_energy, 1.0e-12)
    return spectral_slope, anisotropy


def _drainage_metrics(height: np.ndarray) -> tuple[float, float]:
    # D8 accumulation on a decimated grid keeps measurement cheap and stable.
    stride = max(1, min(height.shape) // 192)
    grid = height[::stride, ::stride]
    rows, columns = grid.shape
    count = rows * columns
    flat = grid.ravel()
    receiver = np.arange(count, dtype=np.int64)
    sinks = 0
    for row in range(rows):
        for column in range(columns):
            index = row * columns + column
            best = flat[index]
            target = index
            for dz in (-1, 0, 1):
                for dx in (-1, 0, 1):
                    if dx == 0 and dz == 0:
                        continue
                    zz, xx = row + dz, column + dx
                    if 0 <= zz < rows and 0 <= xx < columns:
                        candidate = zz * columns + xx
                        if flat[candidate] < best:
                            best = flat[candidate]
                            target = candidate
            receiver[index] = target
            sinks += target == index
    accumulation = np.ones(count, dtype=np.float64)
    for index in np.argsort(flat)[::-1]:
        target = receiver[index]
        if target != index:
            accumulation[target] += accumulation[index]
    threshold = max(8.0, float(np.quantile(accumulation, 0.98)))
    return float(np.mean(accumulation >= threshold)), sinks / count


def _largest_component_fraction(mask: np.ndarray) -> float:
    visited = np.zeros(mask.shape, dtype=np.bool_)
    largest = 0
    total = int(np.count_nonzero(mask))
    if total == 0:
        return 0.0
    rows, columns = mask.shape
    for row, column in zip(*np.nonzero(mask & ~visited)):
        if visited[row, column]:
            continue
        size = 0
        queue = deque([(int(row), int(column))])
        visited[row, column] = True
        while queue:
            zz, xx = queue.popleft()
            size += 1
            for dz, dx in ((-1, 0), (1, 0), (0, -1), (0, 1)):
                neighbor_z, neighbor_x = zz + dz, xx + dx
                if (
                    0 <= neighbor_z < rows
                    and 0 <= neighbor_x < columns
                    and mask[neighbor_z, neighbor_x]
                    and not visited[neighbor_z, neighbor_x]
                ):
                    visited[neighbor_z, neighbor_x] = True
                    queue.append((neighbor_z, neighbor_x))
        largest = max(largest, size)
    return largest / total


def _surface_connectivity_metrics(land: np.ndarray, cliff: np.ndarray) -> dict[str, float]:
    stride = max(1, min(land.shape) // 256)
    land = land[::stride, ::stride]
    cliff = cliff[::stride, ::stride]
    coast_edges = np.count_nonzero(land[:, 1:] != land[:, :-1]) + np.count_nonzero(
        land[1:, :] != land[:-1, :]
    )
    possible_edges = land.shape[0] * max(land.shape[1] - 1, 0) + max(
        land.shape[0] - 1, 0
    ) * land.shape[1]
    return {
        "coast_edge_fraction": coast_edges / max(possible_edges, 1),
        "largest_land_patch_fraction": _largest_component_fraction(land),
        "largest_water_patch_fraction": _largest_component_fraction(~land),
        "largest_cliff_patch_fraction": _largest_component_fraction(cliff) * float(np.mean(cliff)),
    }


def terrain_metrics(height: np.ndarray, spacing: float) -> dict[str, float]:
    slope, curvature = _gradient_metrics(height, spacing)
    land = height >= 0.0
    land_height = np.maximum(height, 0.0)
    land_values = land_height[land] if np.any(land) else np.asarray([0.0])
    slope_values = slope[land] if np.any(land) else slope.ravel()
    curvature_values = curvature[land] if np.any(land) else curvature.ravel()
    local_mean = (
        np.roll(height, 1, 0)
        + np.roll(height, -1, 0)
        + np.roll(height, 1, 1)
        + np.roll(height, -1, 1)
    ) * 0.25
    prominence = np.abs(height - local_mean) / spacing
    spectral_slope, anisotropy = _spectrum_metrics(land_height)
    drainage_density, sink_fraction = _drainage_metrics(land_height)
    connectivity = _surface_connectivity_metrics(land, slope >= 0.75)
    metrics: dict[str, float] = {
        "land_fraction": float(np.mean(land)),
        "water_fraction": float(np.mean(~land)),
        "mean_land_elevation": float(np.mean(land_values)),
        "relief": float(np.ptp(land_values)),
        "roughness": float(np.std(land_values)),
        "mean_slope": float(np.mean(slope_values)),
        "quiet_fraction": float(np.mean(slope_values < 0.035)),
        "rolling_fraction": float(np.mean((slope_values >= 0.035) & (slope_values < 0.25))),
        "steep_fraction": float(np.mean((slope_values >= 0.25) & (slope_values < 0.75))),
        "cliff_fraction": float(np.mean(slope_values >= 0.75)),
        "spike_fraction": float(np.mean(prominence[land] >= 0.75)) if np.any(land) else 0.0,
        "spectral_slope": spectral_slope,
        "spectral_anisotropy": anisotropy,
        "drainage_density": drainage_density,
        "sink_fraction": sink_fraction,
        **connectivity,
    }
    for prefix, values in (
        ("elevation", land_values),
        ("slope", slope_values),
        ("curvature", curvature_values),
    ):
        for quantile, value in zip(METRIC_QUANTILES, np.quantile(values, METRIC_QUANTILES)):
            metrics[f"{prefix}_q{int(quantile * 100):02d}"] = float(value)
    for window_meters in (500.0, 2_000.0, 8_000.0, 32_000.0, 128_000.0, 512_000.0):
        relief = _block_relief(land_height, int(round(window_meters / spacing)))
        metrics[f"relief_{int(window_meters)}m_q50"] = float(np.quantile(relief, 0.5))
        metrics[f"relief_{int(window_meters)}m_q95"] = float(np.quantile(relief, 0.95))
    if np.std(land_values) > 1.0e-9 and np.std(slope_values) > 1.0e-9:
        metrics["elevation_slope_correlation"] = float(
            np.corrcoef(land_values, slope_values)[0, 1]
        )
    else:
        metrics["elevation_slope_correlation"] = 0.0
    return metrics


def landscape_family(metrics: dict[str, float]) -> str:
    land = metrics["land_fraction"]
    if 0.15 <= land <= 0.85:
        return "coast"
    if metrics["cliff_fraction"] >= 0.002:
        return "cliff"
    if metrics["relief"] >= 1_200.0 and metrics["slope_q95"] >= 0.15:
        return "mountain"
    if metrics["drainage_density"] >= 0.015 and metrics["relief"] >= 500.0:
        return "incised"
    if metrics["quiet_fraction"] >= 0.60:
        return "plain"
    return "rolling"


def measure_directory(directory: Path) -> list[dict]:
    descriptors = []
    for raster_path in sorted(directory.glob("*.f32")):
        metadata = _json_read(raster_path.with_suffix(".json"))
        raster = read_raster(raster_path, int(metadata["edge"]))
        metrics = terrain_metrics(raster, float(metadata["spacing_meters"]))
        family = landscape_family(metrics)
        descriptors.append(
            {
                "id": metadata["id"],
                "metadata": metadata,
                "metrics": metrics,
                "family": family,
                "adventure_weight": 2.0 if family in {"coast", "cliff", "mountain", "incised"} else 1.0,
            }
        )
    if not descriptors:
        raise ValueError(f"no .f32 rasters found in {directory}")
    return descriptors


def _weighted_quantile(values: np.ndarray, quantiles: np.ndarray, weights: np.ndarray) -> np.ndarray:
    order = np.argsort(values)
    values = values[order]
    weights = weights[order]
    cumulative = np.cumsum(weights) - weights * 0.5
    cumulative /= np.sum(weights)
    return np.interp(quantiles, cumulative, values)


def distribution_distance(reference: list[dict], generated: list[dict]) -> tuple[float, dict]:
    keys = sorted(set.intersection(*(set(item["metrics"]) for item in reference + generated)))
    details = {}
    distances = []
    real_weights = np.asarray([item.get("adventure_weight", 1.0) for item in reference])
    for key in keys:
        real = np.asarray([item["metrics"][key] for item in reference], dtype=np.float64)
        fake = np.asarray([item["metrics"][key] for item in generated], dtype=np.float64)
        scale = max(float(np.std(real)), abs(float(np.mean(real))) * 0.05, 1.0e-6)
        real_q = _weighted_quantile(real, METRIC_QUANTILES, real_weights)
        fake_q = np.quantile(fake, METRIC_QUANTILES)
        distance = float(np.mean(np.abs(real_q - fake_q)) / scale)
        details[key] = distance
        distances.append(distance)
    families = sorted(set(item.get("family", "unknown") for item in reference + generated))
    real_total = float(np.sum(real_weights))
    for family in families:
        real_fraction = float(
            np.sum(
                [weight for item, weight in zip(reference, real_weights) if item.get("family") == family]
            )
            / real_total
        )
        generated_fraction = sum(item.get("family") == family for item in generated) / len(generated)
        difference = abs(real_fraction - generated_fraction)
        details[f"family.{family}"] = difference
        distances.append(difference * 4.0)
    quiet_prevalence = sum(item.get("family") == "plain" for item in generated) / len(generated)
    quiet_shortfall = max(0.0, 0.35 - quiet_prevalence)
    details["prevalence.quiet_shortfall"] = quiet_shortfall
    distances.append(quiet_shortfall * 8.0)
    return float(np.mean(distances)), details


def _spatial_block(latitude: float, longitude: float) -> tuple[int, int]:
    return (
        math.floor((latitude + 90.0) / 30.0),
        math.floor((longitude + 180.0) / 30.0),
    )


def _spatial_split(latitude: float, longitude: float) -> str:
    block = ":".join(map(str, _spatial_block(latitude, longitude)))
    bucket = hashlib.sha256(block.encode()).digest()[0] % 8
    return "validation" if bucket == 0 else "holdout" if bucket == 1 else "train"


def _has_split_margin(latitude: float, longitude: float, span: float) -> bool:
    latitude_block, longitude_block = _spatial_block(latitude, longitude)
    latitude_minimum = latitude_block * 30.0 - 90.0
    longitude_minimum = longitude_block * 30.0 - 180.0
    latitude_margin = span * 0.5 / 111_320.0
    longitude_margin = latitude_margin / max(math.cos(math.radians(latitude)), 0.20)
    latitude_offset = latitude - latitude_minimum
    longitude_offset = longitude - longitude_minimum
    return (
        latitude_margin <= latitude_offset <= 30.0 - latitude_margin
        and longitude_margin <= longitude_offset <= 30.0 - longitude_margin
    )


def _extract_gdal(
    source: Path,
    destination: Path,
    latitude: float,
    longitude: float,
    span: float,
    edge: int,
) -> np.ndarray:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="treeline-dem-") as temp:
        raw = Path(temp) / "extract.bin"
        projection = f"+proj=aeqd +lat_0={latitude:.9f} +lon_0={longitude:.9f} +datum=WGS84 +units=m"
        half = span * 0.5
        try:
            subprocess.run(
                [
                    "gdalwarp",
                    "-overwrite",
                    "-q",
                    "-t_srs",
                    projection,
                    "-te",
                    str(-half),
                    str(-half),
                    str(half),
                    str(half),
                    "-ts",
                    str(edge),
                    str(edge),
                    "-r",
                    "bilinear",
                    "-ot",
                    "Float32",
                    "-of",
                    "ENVI",
                    str(source),
                    str(raw),
                ],
                check=True,
            )
            values = np.fromfile(raw, dtype=np.float32)
        except (FileNotFoundError, subprocess.CalledProcessError):
            return _extract_geotiff_fallback(source, latitude, longitude, span, edge)
    if values.size != edge * edge:
        raise ValueError(f"GDAL produced {values.size} cells, expected {edge * edge}")
    # GDAL writes north-to-south; Treeline's canonical rows run south-to-north.
    return values.reshape(edge, edge)[::-1].copy()


def _extract_geotiff_fallback(
    source: Path,
    latitude: float,
    longitude: float,
    span: float,
    edge: int,
) -> np.ndarray:
    """Bilinear local-AEQD extraction for a north-up geographic GeoTIFF.

    This intentionally narrow fallback keeps the calibration proof usable when
    a system GDAL install is unavailable. Complex projections and mosaics still
    require GDAL.
    """
    from PIL import Image

    image = Image.open(source)
    scale = image.tag_v2.get(33550)
    tiepoint = image.tag_v2.get(33922)
    if image.mode != "F" or not scale or not tiepoint:
        raise RuntimeError("GeoTIFF fallback requires a float, north-up geographic raster")
    source_height = np.asarray(image, dtype=np.float64)
    pixel_x, pixel_y = float(scale[0]), float(scale[1])
    west, north = float(tiepoint[3]), float(tiepoint[4])
    spacing = span / edge
    local = (np.arange(edge, dtype=np.float64) + 0.5) * spacing - span * 0.5
    x, y = np.meshgrid(local, local)
    radius = 6_371_008.8
    rho = np.hypot(x, y)
    angular = rho / radius
    lat0 = math.radians(latitude)
    lon0 = math.radians(longitude)
    safe_rho = np.where(rho == 0.0, 1.0, rho)
    output_latitude = np.arcsin(
        np.cos(angular) * math.sin(lat0)
        + y * np.sin(angular) * math.cos(lat0) / safe_rho
    )
    output_longitude = lon0 + np.arctan2(
        x * np.sin(angular),
        safe_rho * math.cos(lat0) * np.cos(angular)
        - y * math.sin(lat0) * np.sin(angular),
    )
    output_latitude[rho == 0.0] = lat0
    output_longitude[rho == 0.0] = lon0
    latitude_degrees = np.degrees(output_latitude)
    longitude_degrees = np.degrees(output_longitude)
    column = (longitude_degrees - west) / pixel_x - 0.5
    row = (north - latitude_degrees) / pixel_y - 0.5
    if (
        np.min(column) < 0.0
        or np.max(column) >= source_height.shape[1] - 1
        or np.min(row) < 0.0
        or np.max(row) >= source_height.shape[0] - 1
    ):
        raise ValueError("requested patch extends outside the fallback GeoTIFF")
    column0 = np.floor(column).astype(np.int64)
    row0 = np.floor(row).astype(np.int64)
    column_blend = column - column0
    row_blend = row - row0
    southwest = source_height[row0, column0]
    southeast = source_height[row0, column0 + 1]
    northwest = source_height[row0 + 1, column0]
    northeast = source_height[row0 + 1, column0 + 1]
    top = southwest * (1.0 - column_blend) + southeast * column_blend
    bottom = northwest * (1.0 - column_blend) + northeast * column_blend
    return top * (1.0 - row_blend) + bottom * row_blend


def prepare_reference(args) -> None:
    source = Path(args.source)
    output = Path(args.output)
    rng = random.Random(args.seed)
    span = MACRO_SPAN_METERS if args.tier == "macro" else LOCAL_SPAN_METERS
    edge = MACRO_EDGE if args.tier == "macro" else LOCAL_EDGE
    latitude_limit = 72.0 if args.tier == "macro" else 55.0
    accepted = []
    attempts = 0
    while len(accepted) < args.count and attempts < args.count * 40:
        attempts += 1
        latitude_sine_limit = math.sin(math.radians(latitude_limit))
        sine = rng.uniform(-latitude_sine_limit, latitude_sine_limit)
        latitude = math.degrees(math.asin(sine))
        longitude = rng.uniform(-180.0, 180.0)
        if not _has_split_margin(latitude, longitude, span):
            continue
        identifier = f"{args.tier}_{len(accepted):05d}"
        raster = _extract_gdal(source, output / f"{identifier}.f32", latitude, longitude, span, edge)
        finite_fraction = float(np.mean(np.isfinite(raster)))
        if finite_fraction < 0.995:
            continue
        land_fraction = float(np.mean(raster >= 0.0))
        if args.tier == "macro" and land_fraction < args.minimum_land_fraction:
            continue
        metadata = {
            "id": identifier,
            "source": str(source),
            "source_kind": "ETOPO-2022" if args.tier == "macro" else "NASADEM",
            "latitude": latitude,
            "longitude": longitude,
            "projection": "local-azimuthal-equidistant",
            "vertical_units": "meters",
            "edge": edge,
            "span_meters": span,
            "spacing_meters": span / edge,
            "land_fraction": land_fraction,
            "split": _spatial_split(latitude, longitude),
            "format": "little-endian-f32-row-major-south-to-north",
        }
        write_raster(output / f"{identifier}.f32", raster, metadata)
        accepted.append(metadata)
    if len(accepted) != args.count:
        raise RuntimeError(f"accepted only {len(accepted)}/{args.count} patches in {attempts} attempts")
    _json_write(output / "manifest.json", {"tier": args.tier, "rasters": accepted})


def extract_reference(args) -> None:
    source = Path(args.source)
    output = Path(args.output)
    span = args.span or (MACRO_SPAN_METERS if args.tier == "macro" else LOCAL_SPAN_METERS)
    edge = args.edge or (MACRO_EDGE if args.tier == "macro" else LOCAL_EDGE)
    raster = _extract_gdal(source, output, args.latitude, args.longitude, span, edge)
    if not np.all(np.isfinite(raster)):
        raise ValueError("extracted reference contains non-finite cells")
    metadata = {
        "id": args.id,
        "source": str(source),
        "source_kind": "ETOPO-2022" if args.tier == "macro" else "NASADEM",
        "latitude": args.latitude,
        "longitude": args.longitude,
        "projection": "local-azimuthal-equidistant",
        "vertical_units": "meters",
        "edge": edge,
        "span_meters": span,
        "spacing_meters": span / edge,
        "land_fraction": float(np.mean(raster >= 0.0)),
        "split": _spatial_split(args.latitude, args.longitude),
        "format": "little-endian-f32-row-major-south-to-north",
    }
    write_raster(output / f"{args.id}.f32", raster, metadata)


def make_generated_request(args) -> None:
    rng = random.Random(args.seed)
    rasters = []
    for index in range(args.count):
        rasters.append(
            {
                "id": f"generated_{index:05d}",
                "seed": f"0x{rng.getrandbits(64):016x}",
                "center_x_meters": float(rng.randrange(-64_000_000, 64_000_001, 64_000)),
                "center_z_meters": float(rng.randrange(-64_000_000, 64_000_001, 64_000)),
                "span_meters": args.span,
                "edge": args.edge,
            }
        )
    _json_write(
        Path(args.output),
        {"generator_version": 18, "parameters": {}, "rasters": rasters},
    )


def select_generated_request(args) -> None:
    request = _json_read(Path(args.request))
    by_id = {raster["id"]: raster for raster in request["rasters"]}
    measured = measure_directory(Path(args.rasters))
    eligible = [
        item for item in measured if item["metrics"]["land_fraction"] >= args.minimum_land_fraction
    ]
    if len(eligible) < args.count:
        raise RuntimeError(f"only {len(eligible)} generated candidates met the land requirement")
    families: dict[str, list[dict]] = {}
    for item in eligible:
        families.setdefault(item["family"], []).append(item)
    for items in families.values():
        items.sort(
            key=lambda item: (
                item["metrics"]["relief"],
                item["metrics"]["land_fraction"],
            ),
            reverse=True,
        )
    selected = []
    family_order = ("mountain", "cliff", "incised", "coast", "rolling", "plain")
    while len(selected) < args.count:
        progressed = False
        for family in family_order:
            items = families.get(family, [])
            if items and len(selected) < args.count:
                selected.append(items.pop(0))
                progressed = True
        if not progressed:
            break
    selected_rasters = [by_id[item["id"]] for item in selected]
    if args.edge:
        for raster in selected_rasters:
            raster["edge"] = args.edge
    _json_write(
        Path(args.output),
        {
            "generator_version": request.get("generator_version", 18),
            "parameters": request.get("parameters", {}),
            "rasters": selected_rasters,
            "selection": {
                "minimum_land_fraction": args.minimum_land_fraction,
                "families": [item["family"] for item in selected],
            },
        },
    )


def run_export(binary: Path, request: Path, output: Path, parameters: dict | None = None) -> None:
    request_value = _json_read(request)
    request_value["parameters"] = parameters or request_value.get("parameters", {})
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", suffix=".json", encoding="utf-8", delete=False) as handle:
        json.dump(request_value, handle)
        temporary_request = Path(handle.name)
    try:
        subprocess.run(
            [str(binary), "heightmap-batch", "--request", str(temporary_request), "--output", str(output)],
            check=True,
        )
    finally:
        temporary_request.unlink(missing_ok=True)


def _load_parameter_schema(path: Path):
    schema = _json_read(path)
    active = [(name, spec) for name, spec in schema.items() if spec.get("active", True)]
    if not active:
        raise ValueError("parameter schema has no active parameters")
    return active


def sensitivity(args) -> None:
    active = _load_parameter_schema(Path(args.schema))
    defaults = {name: float(spec["default"]) for name, spec in active}
    work = Path(args.work)
    baseline_output = work / "baseline"
    if not (baseline_output / "descriptors.json").exists():
        run_export(Path(args.binary), Path(args.request), baseline_output, defaults)
        _json_write(
            baseline_output / "descriptors.json",
            {"descriptors": measure_directory(baseline_output)},
        )
    baseline = _json_read(baseline_output / "descriptors.json")["descriptors"]
    reference = _json_read(Path(args.reference))["descriptors"] if args.reference else None
    results = []
    for name, spec in active:
        responses = []
        real_scores = []
        span = float(spec["maximum"]) - float(spec["minimum"])
        for direction in (-1.0, 1.0):
            proposal = defaults.copy()
            proposal[name] = float(
                np.clip(
                    proposal[name] + direction * span * args.fraction,
                    spec["minimum"],
                    spec["maximum"],
                )
            )
            digest = hashlib.sha256(json.dumps(proposal, sort_keys=True).encode()).hexdigest()[:16]
            output = work / f"sensitivity-{digest}"
            descriptor_path = output / "descriptors.json"
            if not descriptor_path.exists():
                run_export(Path(args.binary), Path(args.request), output, proposal)
                _json_write(descriptor_path, {"descriptors": measure_directory(output)})
            measured = _json_read(descriptor_path)["descriptors"]
            responses.append(distribution_distance(baseline, measured)[0])
            if reference is not None:
                real_scores.append(distribution_distance(reference, measured)[0])
        results.append(
            {
                "name": name,
                "response": float(np.mean(responses)),
                "real_score_low": real_scores[0] if real_scores else None,
                "real_score_high": real_scores[1] if real_scores else None,
            }
        )
    results.sort(key=lambda item: item["response"], reverse=True)
    _json_write(work / "sensitivity.json", {"parameters": results})


def optimize(args) -> None:
    reference = _json_read(Path(args.reference))["descriptors"]
    active = _load_parameter_schema(Path(args.schema))
    names = [name for name, _ in active]
    lower = np.asarray([spec["minimum"] for _, spec in active], dtype=np.float64)
    upper = np.asarray([spec["maximum"] for _, spec in active], dtype=np.float64)
    defaults = np.asarray([spec["default"] for _, spec in active], dtype=np.float64)
    mean = np.clip((defaults - lower) / (upper - lower), 0.0, 1.0)
    variance = np.full(mean.shape, 1.0)
    sigma = args.sigma
    population = args.population or 4 + int(3 * math.log(len(active)))
    rng = np.random.default_rng(args.seed)
    work = Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    history = []
    best_score = math.inf
    best_parameters = None

    for generation in range(args.generations):
        proposals = []
        if generation == 0:
            proposals.append(mean.copy())
        while len(proposals) < population:
            proposals.append(np.clip(mean + sigma * np.sqrt(variance) * rng.standard_normal(mean.shape), 0.0, 1.0))
        scored = []
        for candidate_index, normalized in enumerate(proposals):
            values = lower + normalized * (upper - lower)
            parameters = dict(zip(names, map(float, values)))
            digest = hashlib.sha256(json.dumps(parameters, sort_keys=True).encode()).hexdigest()[:16]
            output = work / f"candidate-{digest}"
            descriptors_path = output / "descriptors.json"
            if descriptors_path.exists():
                generated = _json_read(descriptors_path)["descriptors"]
            else:
                if output.exists():
                    shutil.rmtree(output)
                run_export(Path(args.binary), Path(args.request), output, parameters)
                generated = measure_directory(output)
                _json_write(descriptors_path, {"descriptors": generated})
            score, details = distribution_distance(reference, generated)
            scored.append((score, normalized, parameters, details, digest))
            history.append({"generation": generation, "candidate": candidate_index, "score": score, "digest": digest})
        scored.sort(key=lambda item: item[0])
        elite_count = max(2, population // 2)
        elite = scored[:elite_count]
        weights = np.log(elite_count + 0.5) - np.log(np.arange(1, elite_count + 1))
        weights /= np.sum(weights)
        old_mean = mean.copy()
        mean = sum(weight * item[1] for weight, item in zip(weights, elite))
        delta = np.stack([item[1] - old_mean for item in elite])
        variance = np.maximum(0.05, 0.8 * variance + 0.2 * np.sum(weights[:, None] * delta * delta, axis=0) / max(sigma * sigma, 1.0e-9))
        improved = elite[0][0] < best_score
        sigma = float(np.clip(sigma * (1.04 if improved else 0.88), 0.015, 0.35))
        if improved:
            best_score = elite[0][0]
            best_parameters = elite[0][2]
            _json_write(work / "best-parameters.json", best_parameters)
            _json_write(work / "best-result.json", {"score": best_score, "details": elite[0][3], "digest": elite[0][4]})
        _json_write(work / "history.json", history)
        print(f"generation {generation + 1}/{args.generations}: best={best_score:.6f} sigma={sigma:.4f}")
    if best_parameters is None:
        raise RuntimeError("optimizer did not evaluate a candidate")


def _palette(height: np.ndarray) -> np.ndarray:
    water = height < 0.0
    normalized = np.clip(np.maximum(height, 0.0) / LAND_HEIGHT_LIMIT_METERS, 0.0, 1.0)
    stops = np.asarray(
        [
            [44, 108, 71],
            [112, 146, 82],
            [184, 174, 113],
            [142, 116, 91],
            [143, 137, 132],
            [238, 241, 244],
        ],
        dtype=np.float64,
    )
    position = normalized * (len(stops) - 1)
    low = np.floor(position).astype(np.int64)
    high = np.minimum(low + 1, len(stops) - 1)
    blend = (position - low)[..., None]
    color = stops[low] * (1.0 - blend) + stops[high] * blend
    depth = np.clip(-height / 6_000.0, 0.0, 1.0)
    water_color = np.stack([20 + depth * 3, 105 - depth * 65, 155 - depth * 65], axis=-1)
    color[water] = water_color[water]
    return color


def render_heightmap(height: np.ndarray, spacing: float) -> np.ndarray:
    color = _palette(height)
    dz, dx = np.gradient(height, spacing)
    normal_x = -dx
    normal_y = np.ones_like(height)
    normal_z = -dz
    length = np.sqrt(normal_x * normal_x + normal_y * normal_y + normal_z * normal_z)
    light = np.asarray([-0.45, 0.72, -0.53])
    shade = (normal_x * light[0] + normal_y * light[1] + normal_z * light[2]) / length
    shade = np.clip(0.72 + shade * 0.42, 0.35, 1.15)
    return np.clip(color * shade[..., None], 0, 255).astype(np.uint8)


def write_png(path: Path, image: np.ndarray) -> None:
    image = np.asarray(image, dtype=np.uint8)
    height, width, channels = image.shape
    if channels != 3:
        raise ValueError("PNG image must have three channels")
    raw = b"".join(b"\x00" + image[row].tobytes() for row in range(height))

    def chunk(kind: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def report(args) -> None:
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    groups = []
    for value in args.group:
        label, separator, directory = value.partition("=")
        if not separator:
            raise ValueError("--group values must be LABEL=PATH")
        paths = sorted(Path(directory).glob("*.f32"))[: args.count]
        if not paths:
            raise ValueError(f"group {label} has no .f32 rasters")
        groups.append((label, paths))
    cards = []
    for group_index, (label, paths) in enumerate(groups):
        for image_index, raster_path in enumerate(paths):
            metadata = _json_read(raster_path.with_suffix(".json"))
            raster = read_raster(raster_path, int(metadata["edge"]))
            image = render_heightmap(raster, float(metadata["spacing_meters"]))
            filename = f"g{group_index:02d}-{image_index:03d}.png"
            write_png(output / filename, image)
            cards.append({"label": label, "file": filename, "id": metadata["id"]})
    if args.blind:
        random.Random(args.seed).shuffle(cards)
    body = []
    for index, card in enumerate(cards):
        shown_label = f"Map {index + 1}" if args.blind else f"{card['label']}: {card['id']}"
        body.append(f'<figure><img src="{html.escape(card["file"])}"><figcaption>{html.escape(shown_label)}</figcaption></figure>')
    reveal = ""
    if args.blind:
        reveal = "<details><summary>Reveal sources</summary><ol>" + "".join(
            f"<li>Map {index + 1}: {html.escape(card['label'])} / {html.escape(card['id'])}</li>"
            for index, card in enumerate(cards)
        ) + "</ol></details>"
    document = f"""<!doctype html><meta charset="utf-8"><title>Treeline terrain calibration</title>
<style>body{{font:14px system-ui;background:#151918;color:#eef3ee;margin:24px}}main{{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:18px}}figure{{margin:0;background:#242a27;padding:10px;border-radius:8px}}img{{width:100%;image-rendering:auto}}figcaption{{margin-top:8px}}details{{margin:20px 0}}</style>
<h1>Treeline real/generated heightmaps</h1><p>Fixed physical elevation scale: sea level 0 m; land palette 0–9,000 m. No per-tile normalization.</p>{reveal}<main>{''.join(body)}</main>"""
    (output / "index.html").write_text(document, encoding="utf-8")


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare", help="extract deterministic ETOPO or NASADEM patches")
    prepare.add_argument("--tier", choices=("macro", "local"), required=True)
    prepare.add_argument("--source", required=True, help="local GeoTIFF, NetCDF, mosaic, or VRT")
    prepare.add_argument("--output", required=True)
    prepare.add_argument("--count", type=int, required=True)
    prepare.add_argument("--seed", type=int, default=24301)
    prepare.add_argument("--minimum-land-fraction", type=float, default=0.15)
    prepare.set_defaults(func=prepare_reference)

    extract = subparsers.add_parser("extract", help="extract one named real-terrain patch")
    extract.add_argument("--tier", choices=("macro", "local"), required=True)
    extract.add_argument("--source", required=True)
    extract.add_argument("--output", required=True)
    extract.add_argument("--id", required=True)
    extract.add_argument("--latitude", type=float, required=True)
    extract.add_argument("--longitude", type=float, required=True)
    extract.add_argument("--span", type=float)
    extract.add_argument("--edge", type=int)
    extract.set_defaults(func=extract_reference)

    request = subparsers.add_parser("make-request", help="create deterministic generated-world raster requests")
    request.add_argument("--output", required=True)
    request.add_argument("--count", type=int, required=True)
    request.add_argument("--seed", type=int, default=24301)
    request.add_argument("--span", type=float, default=MACRO_SPAN_METERS)
    request.add_argument("--edge", type=int, default=256)
    request.set_defaults(func=make_generated_request)

    select = subparsers.add_parser("select-request", help="stratify generated candidates after a coarse export")
    select.add_argument("--request", required=True)
    select.add_argument("--rasters", required=True)
    select.add_argument("--output", required=True)
    select.add_argument("--count", type=int, required=True)
    select.add_argument("--minimum-land-fraction", type=float, default=0.15)
    select.add_argument("--edge", type=int)
    select.set_defaults(func=select_generated_request)

    export = subparsers.add_parser("export", help="invoke the Rust batch sampler")
    export.add_argument("--binary", default="target/release/world-viewer")
    export.add_argument("--request", required=True)
    export.add_argument("--output", required=True)
    export.add_argument("--parameters")
    export.set_defaults(
        func=lambda args: run_export(
            Path(args.binary),
            Path(args.request),
            Path(args.output),
            _json_read(Path(args.parameters)) if args.parameters else None,
        )
    )

    measure = subparsers.add_parser("measure", help="compute multi-scale terrain descriptors")
    measure.add_argument("--input", required=True)
    measure.add_argument("--output", required=True)
    measure.set_defaults(
        func=lambda args: _json_write(
            Path(args.output), {"descriptors": measure_directory(Path(args.input))}
        )
    )

    optimize_parser = subparsers.add_parser("optimize", help="run bounded diagonal CMA-style search")
    optimize_parser.add_argument("--reference", required=True)
    optimize_parser.add_argument("--schema", required=True)
    optimize_parser.add_argument("--request", required=True)
    optimize_parser.add_argument("--binary", default="target/release/world-viewer")
    optimize_parser.add_argument("--work", required=True)
    optimize_parser.add_argument("--generations", type=int, default=20)
    optimize_parser.add_argument("--population", type=int)
    optimize_parser.add_argument("--sigma", type=float, default=0.16)
    optimize_parser.add_argument("--seed", type=int, default=24301)
    optimize_parser.set_defaults(func=optimize)

    sensitivity_parser = subparsers.add_parser("sensitivity", help="rank one-at-a-time parameter responses")
    sensitivity_parser.add_argument("--schema", required=True)
    sensitivity_parser.add_argument("--request", required=True)
    sensitivity_parser.add_argument("--reference")
    sensitivity_parser.add_argument("--binary", default="target/release/world-viewer")
    sensitivity_parser.add_argument("--work", required=True)
    sensitivity_parser.add_argument("--fraction", type=float, default=0.10)
    sensitivity_parser.set_defaults(func=sensitivity)

    report_parser = subparsers.add_parser("report", help="render fixed-scale PNG and HTML comparisons")
    report_parser.add_argument("--group", action="append", required=True, help="LABEL=PATH")
    report_parser.add_argument("--output", required=True)
    report_parser.add_argument("--count", type=int, default=12)
    report_parser.add_argument("--blind", action="store_true")
    report_parser.add_argument("--seed", type=int, default=24301)
    report_parser.set_defaults(func=report)

    args = parser.parse_args(argv)
    args.func(args)
