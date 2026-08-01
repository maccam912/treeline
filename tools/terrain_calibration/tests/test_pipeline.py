import tempfile
import unittest
from pathlib import Path

import numpy as np
from PIL import Image, TiffImagePlugin

from tools.terrain_calibration import pipeline


class TerrainCalibrationTests(unittest.TestCase):
    def test_raw_raster_round_trip_and_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ramp.f32"
            ramp = np.add.outer(np.arange(16), np.arange(16)).astype(np.float64) * 10.0
            pipeline.write_raster(
                path,
                ramp,
                {"id": "ramp", "edge": 16, "spacing_meters": 100.0},
            )
            np.testing.assert_array_equal(pipeline.read_raster(path), ramp)
            metrics = pipeline.terrain_metrics(ramp, 100.0)
            self.assertAlmostEqual(metrics["land_fraction"], 1.0)
            self.assertGreater(metrics["relief"], 0.0)
            self.assertGreater(metrics["mean_slope"], 0.0)

    def test_distribution_distance_is_zero_for_identical_descriptors(self):
        item = {
            "id": "same",
            "family": "plain",
            "adventure_weight": 1.0,
            "metrics": {"quiet_fraction": 0.8, "relief": 100.0},
        }
        distance, details = pipeline.distribution_distance([item], [item])
        self.assertAlmostEqual(distance, 0.0)
        self.assertEqual(details["prevalence.quiet_shortfall"], 0.0)

    def test_png_writer_emits_a_valid_signature(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "map.png"
            pipeline.write_png(path, np.zeros((4, 5, 3), dtype=np.uint8))
            self.assertEqual(path.read_bytes()[:8], b"\x89PNG\r\n\x1a\n")

    def test_geotiff_fallback_extracts_a_local_metric_grid(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tif"
            values = np.add.outer(np.arange(32), np.arange(32)).astype(np.float32)
            tags = TiffImagePlugin.ImageFileDirectory_v2()
            tags[33550] = (0.01, 0.01, 1.0)
            tags[33922] = (0.0, 0.0, 0.0, -0.16, 0.16, 0.0)
            Image.fromarray(values, mode="F").save(source, tiffinfo=tags)
            extracted = pipeline._extract_geotiff_fallback(
                source, latitude=0.0, longitude=0.0, span=10_000.0, edge=16
            )
            self.assertEqual(extracted.shape, (16, 16))
            self.assertTrue(np.all(np.isfinite(extracted)))
            self.assertGreater(float(np.ptp(extracted)), 0.0)


if __name__ == "__main__":
    unittest.main()
