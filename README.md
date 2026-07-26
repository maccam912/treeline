# Treeline

Treeline is an early-stage multiplayer wilderness exploration game built around
deterministic, effectively endless geography. The wilderness is the content:
mountains, watersheds, forests, caves, weather, camps, and the stories players
bring home.

The complete product and technical direction lives in
[`DESIGN.md`](DESIGN.md). Keep that document in the repository and consult it
before changing architecture or priorities.

## Current status

This repository now includes a small playable terrain toy alongside the
test-backed foundations described in the design. The prototype provides:

- stable world identity and coordinate hashing;
- deterministic macro elevation and elongated mountain systems;
- explainable terrain samples that identify base elevation and dominant uplift;
- Marching Cubes terrain extraction;
- a native `winit` + `wgpu` client with simple lighting;
- mouse-look, walking, sprinting, and terrain-following movement;
- deterministic 32 m terrain chunks streamed around the moving player;
- distance-selected 2 m, 4 m, and 8 m terrain LODs with Transvoxel seams;
- vertically aligned near-terrain slabs that follow high mountain surfaces;
- coarse surface-only terrain extending the visible landscape beyond 20 km;
- prioritized asynchronous terrain meshing outside the window thread;
- continuous deterministic regional fields;
- deterministic 128 km watershed artifacts with depression filling, basin spill
  levels, cross-region drainage exits, and flow accumulation;
- deterministic rainfall-fed regional river networks with discharge and
  catchment area;
- cached river-driven valley and channel incision shared by near and distant
  terrain representations;
- versioned multi-scale erosion: regional mountain weathering and sediment
  plains, drainage-graph gullies, and slope/geology-driven rock, scree, soil,
  and micro-relief;
- explainable climate with latitude-like temperature structure,
  ocean-proximity and continentality effects, prevailing winds, elevation
  cooling, windward precipitation, lee-side rain shadows, explicit seasons,
  deterministic snowpack and meltwater-fed runoff;
- deterministic lakes derived from filled drainage basins, with level water
  surfaces, stable identities, spill outlets, and near/far rendering;
- an interactive Generator Lab with pan, zoom, seed regeneration, teleport,
  terrain/watershed/flow/river/lake/erosion/temperature/precipitation/snowpack
  views, selectable seasons, and parameter inspection;
- hydrology and cave graph invariants;
- voxel LOD alignment;
- world-region lifecycle, protocol, simulation, and render tiers.

The terrain toy now has an unbounded movement path through deterministic
near-to-mid terrain chunks backed by a cheaper mountain-scale vista
representation. Macro terrain was introduced with generator version 2, and
river-shaped terrain with version 3; older world identities retain their
previous terrain contracts. Filled drainage basins become rendered lakes with
generator version 4. Multi-scale erosion is enabled by generator version 5.
Orographic climate begins with generator version 6. Latitude-like climate,
continentality, seasons, snowpack, and meltwater runoff begin with generator
version 7; versions 1–6 retain their prior generation contracts. Vegetation,
survival, and networking have not been added yet.

## Getting started

Install Rust 1.97.1 or newer, then run:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Run the world-generation viewer with:

```sh
cargo run -p generator-lab
```

In Generator Lab, use WASD or the arrow keys to pan, `+`/`-` or the mouse wheel
to zoom, `R` to advance the seed, and `1` through `9` to view terrain,
watersheds, flow accumulation, rivers, lakes, erosion, seasonal temperature,
seasonal precipitation, or snowpack. Press `C` to advance the displayed
season. Left-click to inspect a location, and right-click to teleport the view.

Run the playable terrain toy with:

```sh
cargo run -p client
```

Use the mouse to look, `WASD` or the arrow keys to walk, and either Shift key
to sprint. Press Escape to release the cursor; click the window to capture it
again.

Build the browser version with [Trunk](https://trunk-rs.dev/):

```sh
rustup target add wasm32-unknown-unknown
trunk build apps/client/index.html --release
```

Pushes to `main` build this browser version and deploy it through GitHub Pages.

## Repository map

- `crates/` contains narrowly scoped game libraries.
- `apps/` contains executable composition roots.
- `.github/workflows/ci.yml` runs formatting, lint, test, and documentation
  checks.
- `.github/workflows/pages.yml` builds and deploys the browser client.
- `AGENTS.md` is the contributor and coding-agent guide.
- `DESIGN.md` is the durable product and architecture north star.

## License

Licensed under the Apache License, Version 2.0. See [`LICENSE`](LICENSE).
