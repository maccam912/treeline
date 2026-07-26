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
- a deterministic rolling-hill signed-density field;
- Marching Cubes terrain extraction;
- a native `winit` + `wgpu` client with simple lighting;
- mouse-look, walking, sprinting, and terrain-following movement;
- deterministic 32 m terrain chunks streamed around the moving player;
- distance-selected 2 m, 4 m, and 8 m terrain LODs with Transvoxel seams;
- surface-only far-terrain tiles extending the visible landscape beyond 2 km;
- prioritized asynchronous terrain meshing outside the window thread;
- continuous deterministic regional fields;
- hydrology and cave graph invariants;
- voxel LOD alignment;
- world-region lifecycle, protocol, simulation, and render tiers.

The terrain toy now has an unbounded movement path through deterministic
near-to-mid terrain chunks backed by a cheaper coarse vista representation.
Vegetation, survival, and networking have not been added yet.

## Getting started

Install Rust 1.85 or newer, then run:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Run one of the scaffold applications with:

```sh
cargo run -p generator-lab
```

Run the playable terrain toy with:

```sh
cargo run -p client
```

Use the mouse to look, `WASD` or the arrow keys to walk, and either Shift key
to sprint. Press Escape to release the cursor; click the window to capture it
again.

## Repository map

- `crates/` contains narrowly scoped game libraries.
- `apps/` contains executable composition roots.
- `.github/workflows/ci.yml` runs formatting, lint, test, and documentation
  checks.
- `AGENTS.md` is the contributor and coding-agent guide.
- `DESIGN.md` is the durable product and architecture north star.

## License

Licensed under the Apache License, Version 2.0. See [`LICENSE`](LICENSE).
