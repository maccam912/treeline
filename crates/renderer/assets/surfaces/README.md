# Terrain surface material sources

These embedded PBR materials are unmodified 1K JPEG downloads from Poly Haven.
Poly Haven releases all assets under CC0 1.0; attribution is not required, but
the source records are retained here for provenance and reproducibility.

## Forest Floor

- Asset: <https://polyhaven.com/a/forest_floor>
- Author: eye-candy.xyz
- License: CC0 1.0, <https://polyhaven.com/license>
- Physical source width: 2.1 m
- Use in Treeline: predominantly horizontal soil and leaf-litter surfaces
- Files: diffuse, OpenGL normal, and packed AO/roughness/metalness

| File | SHA-256 |
| --- | --- |
| `forest_floor_diff_1k.jpg` | `f12e5adea1741f9eb7a528bfc621f8267885b9530b74c3a8afdb823899bdbf0b` |
| `forest_floor_nor_gl_1k.jpg` | `681f3de8c756c4d19bcda33039f953295498f38b1425ce9d56b37d6f97f6e518` |
| `forest_floor_arm_1k.jpg` | `b9443866b317194651550d6d961413f529a17e85a87e3e0a8289ec022cc39090` |

## Rock Face

- Asset: <https://polyhaven.com/a/rock_face>
- Authors: Dario Barresi (processing), Greg Zaal (photography)
- License: CC0 1.0, <https://polyhaven.com/license>
- Physical source width: 2.4 m
- Use in Treeline: triplanar projection on terrain surfaces steeper than 45 degrees
- Files: diffuse, OpenGL normal, and packed AO/roughness/metalness

| File | SHA-256 |
| --- | --- |
| `rock_face_diff_1k.jpg` | `cce4b50517161264bdef196f5e247e328ca3739083cd6044ad3cc54d88cb82e2` |
| `rock_face_nor_gl_1k.jpg` | `e2682e1286c8b6aca7e01ba7b8fe1726cb6606042b9ee70d097aa67470d410ef` |
| `rock_face_arm_1k.jpg` | `3a0fce8658ba2bac6690f68e79630c16e004f1676c1588ef4d54906fc3907616` |

The files are stored through Git LFS. The renderer decodes them once, resizes
them to its bounded runtime resolution, creates the complete mip chain, and
uploads them beside the bark materials in one four-layer texture array shared
by the native and WebGPU paths.
