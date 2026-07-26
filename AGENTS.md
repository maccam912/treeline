# Treeline contributor guide

## Purpose

Treeline is a wilderness exploration game. Its central promise is that a player
can choose a distant natural landmark, travel to it through coherent geography,
survive the journey, and return with a story. Combat, loot ladders, quests, and
general-purpose engine work are not current pillars.

Read `DESIGN.md` before making architectural or product changes. Keep it in the
repository: it is the source of truth for the game's direction and the document
against which proposals should be reviewed.

## Architecture

The repository is a Rust workspace split by stable domain boundaries:

```text
crates/
  coordinates/  World identity, positions, hierarchical cells, stable hashing
  terrain/      Signed density fields and surface/material sampling
  geography/    Macro regions, geology, climate, drainage, and watersheds
  ecology/      Species suitability, vegetation, and forest structure
  caves/        Geological cave graphs and subterranean connectivity
  hydrology/    Rivers, basins, lakes, and local water-state primitives
  voxel/        Samples, chunks, edits, material channels, and LOD alignment
  mesher/       Marching Cubes first; Transvoxel and transition meshes later
  world/        Streaming, region lifecycle, caches, and generation scheduling
  simulation/   Player, weather, wildlife, survival, and active-region state
  protocol/     Versioned network messages and replication data
  renderer/     wgpu-facing render tiers for terrain, water, plants, atmosphere
  platform/     Window, input, storage, jobs, networking, and lifecycle adapters

apps/
  client/         Player-facing game client
  server/         Headless authoritative simulation
  world-viewer/   Streaming and LOD inspection tool
  generator-lab/  World-generation inspection and experimentation tool
```

Dependencies should generally point downward through these layers:

```text
apps
  -> renderer / platform / protocol / simulation / world
  -> ecology / caves / hydrology / voxel / mesher
  -> geography / terrain
  -> coordinates
```

Avoid circular dependencies. If two crates need the same value type, move that
type to the lowest domain crate that owns its meaning; do not create a generic
utilities crate.

## Core invariants

- Generation is a pure function of world identity, coordinates, explicit
  settings, and versioned artifacts.
- The same input must produce bit-for-bit stable output on every machine we
  support.
- Generation never depends on visitation order, job completion order, wall
  clock time, or a process-randomized hash.
- Spatially adjacent regions share deterministic boundary conditions.
- Signed density is negative inside solid terrain, zero on the surface, and
  positive in air.
- Pristine terrain is regenerated, not persisted. Store only identity,
  generation version, summaries, and deviations from the procedural base.
- Distant representations may be cheaper, but they must remain spatially
  aligned with near-world representations.
- Simulation is active only around players. Frozen regions retain compact,
  deterministic summaries.
- Protocol changes are explicitly versioned.

## Working conventions

- Work directly on `main`; do not create feature branches unless the user
  explicitly asks for one.
- Build only machinery required by this game. Do not add a generic scene graph,
  scripting runtime, editor framework, plugin system, or reusable game engine.
- Prefer functional sampling APIs such as `terrain.sample(position)` and
  `climate.sample(xz)`.
- Add expensive external dependencies only with the first feature that uses
  them end to end.
- Keep platform APIs out of deterministic generation crates.
- Use integer or deliberately specified floating-point operations in generation
  code. Document any operation whose cross-platform behavior affects a world.
- Never use `std::collections::hash_map::DefaultHasher` for world generation.
- Treat `generator-lab` as a product, not a disposable debug executable.
- Keep public APIs small and domain-named. Avoid `common`, `helpers`, and
  catch-all modules.

## Testing expectations

Every generator change should add or update tests for the applicable invariants:

- determinism and golden hashes;
- generation-order independence;
- negative-coordinate and cell-boundary behavior;
- river descent, basin spill levels, and water conservation;
- cave graph connectivity;
- LOD alignment and transition integrity;
- serialization/protocol compatibility once persistence and networking arrive.

Run the full local gate before submitting changes:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

When shell access includes RTK, prefix shell commands with `rtk` to keep command
output compact.

## Definition of done

A change is done when it is scoped to a design pillar, respects crate
boundaries, includes relevant invariant tests, passes formatting/lint/test/docs,
and explains any intentional generator-version or persistence impact.
