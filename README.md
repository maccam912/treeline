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
- deterministic 512 km geographical-province artifacts with explicit shared
  boundary conditions, parent-scale controls, and fixed generation halos;
- deterministic macro elevation and elongated mountain systems;
- condition-driven plains, rolling uplands, plateaus, scarps, rugged and
  weathered mountains, glacial valleys, dune fields, and closed salt basins;
- explainable terrain samples that identify base elevation and dominant uplift;
- Marching Cubes terrain extraction;
- a native `winit` + `wgpu` client with simple lighting;
- mouse-look, walking, sprinting, and terrain-following movement;
- deterministic 32 m terrain chunks streamed around the moving player;
- distance-selected 2 m, 4 m, and 8 m terrain LODs with Transvoxel seams;
- vertically aligned near-terrain slabs that follow high mountain surfaces;
- coarse surface-only terrain extending the visible landscape beyond 20 km;
- prioritized asynchronous terrain meshing outside the window thread, using
  native threads or independent message-passing Wasm Web Workers;
- direction-aware terrain prewarming and a bounded exact-mesh cache that reuse
  completed terrain and lake meshes across chunk crossings;
- phase-aware initial-generation progress and timing reports;
- frame-budgeted terrain uploads, worker-built lake meshes, and shader-based
  near/far cutouts that avoid rebuilding 2 km far tiles at each chunk boundary;
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
- deterministic climate-conditioned lakes derived from drainage basins, with
  stable identities, topographic and active outlets, seasonal high/low water,
  saline closed basins, dry playas, and near/far rendering;
- deterministic soil profiles, continuous forest distributions, and globally
  anchored procedural tree individuals with varied architecture, age, damage,
  wind response, competition response, and life history;
- near-client tree rendering generated from continuous trunk, branch, crown,
  bark, and foliage grammars rather than a fixed tree-model library;
- deterministic surface rocks, ground vegetation, wetlands, reefs, and
  seasonal snow surface treatment;
- overlapping, cause-driven closed forest, open woodland, prairie, grassland,
  steppe, shrubland, desert, tundra, exposed alpine, wetland, and salt-playa
  potentials without mutually exclusive biome IDs;
- geology-, climate-, and drainage-driven karst, lava-tube, fault, sea, talus,
  glacial, and erosional cave systems with connected passages, chambers,
  entrances, sinkholes, shafts, sumps, and underground rivers;
- cave subtraction and deep-layer near-terrain meshing, cave-aware traversal,
  rendered subterranean water, and surface openings kept clear of unsupported
  vegetation;
- deterministic active-region water storage and routing with terrain-change
  response, lake filling and spill, flooding, surface-to-cave connections,
  generated cascades, waterfalls, plunge pools and gorges, and compact frozen
  summaries reconstructed as the player moves;
- an interactive Generator Lab with pan, zoom, seed regeneration, teleport,
  terrain/watershed/flow/river/lake/erosion/province/temperature/precipitation/snowpack
  /soil/forest/ground-vegetation/rock/wetland/reef/cave/living-water views,
  selectable seasons, controlled water-response scenarios, and explainable
  ecosystem and cave inspection;
- hydrology and generated cave topology/determinism invariants;
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
version 7. Soil profiles begin with generator version 8, forest distribution
with version 9, and procedural tree individuals with version 10. Generator
version 11 standardizes non-basic floating-point operations on pure-Rust
`libm`; version 10 and older worlds require their original executable to retain
their previous platform-math behavior. Surface rocks begin with version 12,
ground vegetation with version 13, wetlands with version 14, reefs with version
15, cave subtraction with version 16, and fast-water terrain morphology with
version 17. Expedition survival and live networking have not been added yet.
Generator version 18 is an intentional pristine-world reset: top-down
geographical provinces now coordinate geology, landforms, climate controls,
soil, hydrology, and broad overlapping ecosystem regimes. Version 17 and older
world identities retain their previous generation paths.

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
seasonal precipitation, or snowpack. Press `0` for soil and `F` for forest
distribution. Press `P` for the province/causal-landform layer and `C` to
advance the displayed season. Left-click to inspect a location, and right-click
to teleport the view. Additional ecosystem layers use `V` for ground
vegetation, `G` for rocks, `M` for wetlands, `Q` for reefs, and `K` for caves.

Run the deterministic world-quality survey with:

```sh
cargo run -p world-viewer -- audit
```

The survey samples terrain morphology, province causes, overlapping ecosystem
potentials, drainage, climate, forest identity, soil, wetlands, reefs, and caves
across far-apart regions. It writes descriptor data, a 17-view deterministic
contact sheet with explicit fallback frames, novelty and plausibility findings,
required-outcome coverage, and a stable regression fingerprint to
`artifacts/world-quality`. Existing baselines are retained when results change;
after reviewing the report and contact sheet, pass `--accept` to adopt an
intentional visual change. Use `--help` to see seed, region-count, and
output-path controls; pass `--require-coverage` when the audit should fail on
any missing required outcome or qualified viewpoint family.

Run the playable terrain toy with:

```sh
cargo run -p client
```

Use the mouse to look, `WASD` or the arrow keys to walk, and either Shift key
to sprint. Press Escape to release the cursor; click the window to capture it
again. Press `F` to toggle aerial mode, which follows the ground from 1 km up
and moves ten times faster. Press `R` to warp to random dry ground 1,000–5,000
km away, `B` to warp near water, or `C` to find and warp to the nearest
generated cave entrance; walking over the opening descends to the cave floor.
Browsers expose aerial mode and both non-cave warps as buttons.

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
