# Treeline

An early wilderness exploration game built on measured terrain.

The world is a real place: a 10 km square in Michigan's Upper Peninsula,
reconstructed from public survey data at 1:1 horizontal and vertical scale. The
ground you walk on is a lidar elevation model, the lakes are mapped
hydrography, and the forest is sized by lidar canopy returns. Nothing in the
landscape was placed by hand or synthesized from noise.

The full direction lives in [`DESIGN.md`](DESIGN.md); the data contract lives in
[`SURVEYED_WORLD.md`](SURVEYED_WORLD.md).

## What works today

- **The measured world.** Bare-earth elevation, mapped lakes, lidar canopy, and
  aerial imagery, decoded from an embedded versioned bundle.
- **Walking it.** Smooth voxel terrain with terrain-following movement, streamed
  around the player at three levels of detail with a coarse surface tier out to
  the horizon.
- **A real forest.** Individual trees, placed and sized by measured stand
  structure, with procedural species and architecture. Visible to the horizon
  without ever becoming a canopy texture.
- **Rendering.** Scanned PBR materials, cascaded sun shadows, three daylight
  states, seasonal snow, animated water, and climate-tinted haze.
- **In a browser.** The same world, with terrain generation on Web Workers.

Camping, survival, weather, navigation, wildlife, and multiplayer are not built
yet — see the roadmap in `DESIGN.md`.

## Running it

```bash
cargo run -p client --release
```

| Key | Action |
| --- | --- |
| `W` `A` `S` `D` | Walk |
| `Shift` | Sprint |
| Mouse | Look (click to capture, `Escape` to release) |
| `F` | Toggle the aerial survey view |
| `T` | Cycle dawn, noon, and dusk |
| `R` | Warp to a random spot in the tile |
| `B` | Warp to a lake shore |

Touch devices get on-screen sticks: the left half of the screen moves, the right
half looks.

## Inspecting the world

The Generator Lab draws any one layer as a top-down map and reports every layer
at a clicked position. When something looks wrong in the game, this is how you
find out which layer disagrees.

```bash
cargo run -p generator-lab --release
```

Number keys select the layer, `WASD` pans, `+`/`-` zooms, `C` cycles the season,
left click inspects, and right click recenters.

## Preparing a surveyed tile

`tools/surveyed_tile/prepare.py` turns source rasters into the four embedded
layers. `SURVEYED_WORLD.md` documents the contract those layers satisfy, the
exact command that produced the current bundle, and the rules for admitting a
new one.

Bundle artifacts and scanned textures are stored with Git LFS, so clone with
LFS enabled.

## Developing

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Browser-only code is behind `cfg(target_arch = "wasm32")` and needs its own
pass:

```bash
cargo clippy -p client --target wasm32-unknown-unknown --all-targets -- -D warnings
```

[`AGENTS.md`](AGENTS.md) covers the crate layout, invariants, and conventions.

## License

Apache-2.0. Terrain sources are public domain (USGS 3DEP, NHD, NAIP); scanned
surface and bark materials are CC0 from Poly Haven, credited in the `README.md`
beside each asset directory.
