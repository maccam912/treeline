# Treeline contributor guide

## Purpose

Treeline is a wilderness exploration game. Its promise is that a player can
choose a distant natural landmark, travel to it through coherent geography,
survive the journey, and return with a story. Combat, loot ladders, quests, and
general-purpose engine work are not pillars.

Read `DESIGN.md` before making architectural or product changes. It is the
source of truth for direction, and the document proposals are reviewed against.
`SURVEYED_WORLD.md` is the data contract for the measured world.

## Architecture

A Rust workspace split by stable domain boundaries:

```text
crates/
  coordinates/  World identity, positions, cells, stable hashing
  terrain/      The surveyed bundle, and the density/surface contracts
  climate/      Seasonal normals for the site
  ecology/      Forest structure and tree individuals
  voxel/        Chunks, LOD levels, lattice alignment
  mesher/       Marching Cubes, Transvoxel, surface grids
  world/        Composition, streaming, and the mesh queue
  renderer/     Bevy integration: measured meshes, materials, lighting
  platform/     Platform boundaries kept out of generation
  protocol/     Versioned network contracts

apps/
  client/         The game
  server/         Headless host scaffold
  generator-lab/  Top-down inspector for every layer
```

Dependencies point downward:

```text
apps -> renderer / platform / protocol / world
     -> ecology / voxel / mesher
     -> terrain / climate
     -> coordinates
```

Avoid circular dependencies. If two crates need the same value type, move it to
the lowest crate that owns its meaning; do not create a utilities crate.

## Core invariants

- The world is measured. Terrain, lake footprints, and canopy structure come
  from the surveyed bundle, not from synthesis.
- What is not measured is absent, not invented. Admitting a new layer means
  finding an authoritative source and writing down what it means.
- Measurements bound derivations. Nothing derived may exceed what was measured —
  a tree cannot grow past its stand's canopy top.
- Generation is a pure function of world identity, coordinates, explicit
  settings, and versioned artifacts.
- The same input produces bit-for-bit stable output on every supported machine.
- Generation never depends on visitation order, job completion order, wall clock
  time, or a process-randomized hash.
- Spatially adjacent regions share deterministic boundary conditions.
- Signed density is negative inside solid terrain, zero on the surface, and
  positive in air.
- Terrain is regenerated, not persisted. Store only identity, generation
  version, summaries, and deviations.
- Distant representations may be cheaper, but must stay spatially aligned with
  near-world ones.
- Protocol changes are explicitly versioned.

## Working conventions

- Work directly on `main`; do not create feature branches unless asked.
- Use plain `git`, not `gh`, for commits and pushes.
- Build only machinery this game needs. No scene graph, scripting runtime,
  editor framework, plugin system, or reusable engine.
- Prefer functional sampling APIs — `terrain.surface_height_at(x, z)`,
  `climate.season(season, elevation)`.
- Add expensive dependencies only with the first feature that uses them end to
  end.
- Keep platform APIs out of generation crates.
- Use integer or deliberately specified floating-point operations in generation.
  Document any operation whose cross-platform behavior affects a world.
- Never use `std::collections::hash_map::DefaultHasher` for world generation.
- Treat `generator-lab` as a product, not a disposable debug executable.
- Keep public APIs small and domain-named. Avoid `common`, `helpers`, and
  catch-all modules.
- Keep files short enough to hold in your head — roughly two screens. When a
  file grows past that, it is usually doing more than one job; split it by
  concern rather than by line count.
- Order code so it reads top to bottom: a reader should not need what is below
  to understand what is above. Name a function for what it does, and let its
  body be the only place that says how.

## Testing expectations

Every generator change should add or update tests for the applicable
invariants:

- determinism, including golden fingerprints where they matter;
- generation-order and request-order independence;
- negative-coordinate and cell-boundary behavior;
- chunk seams and LOD transition alignment;
- measured bounds — nothing derived exceeds its source;
- serialization and protocol compatibility, once those exist.

Run the full local gate before submitting:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Browser-only code is behind `cfg(target_arch = "wasm32")` and the native gate
never compiles it. Check it separately:

```sh
cargo clippy -p client --target wasm32-unknown-unknown --all-targets -- -D warnings
```

When shell access includes RTK, prefix shell commands with `rtk` to keep output
compact.

## Roadmap tracking

When work materially changes the status of a roadmap item, update the tracker in
`DESIGN.md` in the same change. Mark something done only when it has landed end
to end and is usable; when it has not, say what remains. Do not leave `DESIGN.md`
claiming implemented work is missing, or claiming completion when only types and
contracts exist.

## Definition of done

A change is done when it is scoped to a design pillar, respects crate
boundaries, includes relevant invariant tests, passes formatting, lint, test,
and docs, keeps the `DESIGN.md` tracker current, and explains any intentional
generator-version or persistence impact.
