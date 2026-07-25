# Treeline

Treeline is an early-stage multiplayer wilderness exploration game built around
deterministic, effectively endless geography. The wilderness is the content:
mountains, watersheds, forests, caves, weather, camps, and the stories players
bring home.

The complete product and technical direction lives in
[`DESIGN.md`](DESIGN.md). Keep that document in the repository and consult it
before changing architecture or priorities.

## Current status

This repository is a compileable foundation, not yet a playable prototype. It
establishes the boundaries described in the design and provides small,
test-backed primitives for:

- stable world identity and coordinate hashing;
- functional terrain density sampling;
- continuous deterministic regional fields;
- hydrology and cave graph invariants;
- voxel LOD alignment;
- world-region lifecycle, protocol, simulation, and render tiers.

Heavy runtime dependencies such as `wgpu`, `winit`, and `iroh` should be added
when their first end-to-end feature is implemented. Keeping them out of the
initial scaffold makes the baseline quick to build and keeps architectural
intent separate from unused dependency weight.

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

## Repository map

- `crates/` contains narrowly scoped game libraries.
- `apps/` contains executable composition roots.
- `.github/workflows/ci.yml` runs formatting, lint, test, and documentation
  checks.
- `AGENTS.md` is the contributor and coding-agent guide.
- `DESIGN.md` is the durable product and architecture north star.

## License

Licensed under the Apache License, Version 2.0. See [`LICENSE`](LICENSE).

