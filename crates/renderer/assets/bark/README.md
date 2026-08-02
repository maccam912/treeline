# Bark material sources

These embedded PBR materials are unmodified 1K JPEG downloads from Poly Haven.
Poly Haven releases all assets under CC0 1.0; attribution is not required, but
the source records are retained here for provenance and reproducibility.

## Pine Bark

- Asset: <https://polyhaven.com/a/pine_bark>
- Author: Dimitrios Savva
- License: CC0 1.0, <https://polyhaven.com/license>
- Physical source width: 2 m
- Files: diffuse, OpenGL normal, and packed AO/roughness/metalness

| File | SHA-256 |
| --- | --- |
| `pine_bark_diff_1k.jpg` | `89ddfbff0388d30caff902ba600cdf75eb1e651ca71ece6594b5a9b27abb47ab` |
| `pine_bark_nor_gl_1k.jpg` | `666e5f175a010f12577687dd8029125bff99110e6a97958afb295b34266437db` |
| `pine_bark_arm_1k.jpg` | `1ba20ec25c9f43327d6aa0ea809917854cff553ad7f21fb548e853a5b5ec701a` |

## Bark Brown 02

- Asset: <https://polyhaven.com/a/bark_brown_02>
- Author: Rob Tuytel
- License: CC0 1.0, <https://polyhaven.com/license>
- Physical source width: 1 m
- Use in Treeline: oak-like deciduous bark
- Files: diffuse, OpenGL normal, and packed AO/roughness/metalness

| File | SHA-256 |
| --- | --- |
| `bark_brown_02_diff_1k.jpg` | `920fa0bed0c9d78c1d530e99795113afd532d7db746de440f4a85d7c83ed0f1a` |
| `bark_brown_02_nor_gl_1k.jpg` | `0d5e691e8ad8bcd093a3587887fda2a6b4a948b5708a4f256f7919584c6eb857` |
| `bark_brown_02_arm_1k.jpg` | `6bd86d37b3b9b40cc30a4b661e382ec41fa7463a8b1144ff44708b19fa675af1` |

The files are stored through Git LFS. The renderer decodes them once, resizes
them to its bounded runtime resolution, creates the complete mip chain, and
uploads two-layer texture arrays shared by the native and WebGPU paths.
