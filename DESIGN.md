# Treeline — Design

## 1. What Treeline Is

A wilderness exploration game. The whole promise is one sentence:

> Pick a distant natural landmark, travel to it through coherent geography,
> survive the journey, and come back with a story.

The wilderness is the content. Combat, loot ladders, quests, and general-purpose
engine work are not pillars and are not on the path.

The emotional target is closer to *The Long Dark* × *Firewatch* × real
backpacking than to any survival-crafting game. What the player should want is
to find out what is over the next ridge.

---

## 2. Design Pillars

**Exploration is the progression.** Travel reveals unfamiliar ground rather than
larger numbers. There is no skill tree behind the horizon; the horizon is the
reward.

**Nature is structured, not noisy.** Real landscape has causes — water runs
downhill, forests thin on exposed slopes, lakes sit in basins. A world that
merely varies is not the same as a world that means something. When something in
Treeline looks a certain way there should be a reason, and that reason should be
inspectable.

**Large features come before small details.** A player notices a valley before
they notice a rock. Work the scales that carry the journey first.

**Nothing is placed by hand.** No prefabs, no set-dressed viewpoints, no authored
landmarks. The world is measured or derived from measurements, and the same
identity produces the same world everywhere.

---

## 3. The World Is Measured

Treeline's world is real terrain, not synthesized landform.

The current world is a single 10 km square in Michigan's Upper Peninsula,
reconstructed from public survey data: a USGS 3DEP bare-earth elevation model,
NHD waterbody polygons, a 3DEP lidar point cloud, and NAIP aerial imagery.
`SURVEYED_WORLD.md` is the contract those layers satisfy, and
`tools/surveyed_tile/prepare.py` produces them.

This is a deliberate reversal. Treeline previously synthesized its surface from
noise fields, geological provinces, and simulated drainage. That machinery has
been removed. It could produce variety, but variety is not coherence, and no
amount of tuning made a synthesized valley mean what a real one means.

What follows from the choice:

- **Measurements bound derivations.** Lidar says how much canopy stands where and
  how tall it is; the tree generator may not exceed that. The elevation model
  says where the ground is; nothing reshapes it.
- **What is not measured is absent, not invented.** The bundle has no rivers, so
  the world has no rivers. Admitting a layer means finding an authoritative
  source and writing down what it means — not filling the gap with noise.
- **The world is finite, and honestly so.** Sampling past the tile clamps to the
  border so mesh residency can finish at the edge. That is a rendering
  concession, not a claim about what is out there.

Growing the world means adding measured tiles, not turning synthesis back on.

---

## 4. Terrain Representation

Terrain is a **signed density field**: negative inside solid ground, zero at the
surface, positive in air. Smooth voxels, not cubes — this world is about
landscape, and a landscape made of blocks is a different game.

Near terrain is meshed volumetrically with Marching Cubes and Transvoxel, which
handles level-of-detail seams. Distant terrain is a height surface, because a
mountain seen from ten kilometers has no interior worth sampling. Both sample the
same field, so the two representations describe the same surface and stay aligned
where they meet.

Voxel resolution is 0.5 m at the finest streamed level. Coarser levels double the
spacing, and every coarse sample lands on the fine lattice, so LODs nest rather
than merely approximate each other.

Dual Contouring is not on the path. Sharp features are not what this world is
made of, and its cost is real.

---

## 5. Water

Water is a horizontal sheet over mapped lake footprints, drawn as a separate
surface. It never changes terrain density, so the shoreline the player walks and
the shoreline they see come from the same measured mask.

Each lake carries a representative level: the median bare-earth elevation inside
its source polygon. That is a level, not a bathymetry. Treeline does not claim to
know how deep these lakes are, and the runtime gives a shore cell a minimum
visible film rather than pretending to a depth it has not measured.

Rivers, flow, and dynamic water are not implemented. Treeline previously
simulated all three procedurally; that went with the procedural surface. They
return when there is authoritative linework, flow direction, and elevation
conformance to build them from.

---

## 6. Forest

Trees are where measurement and synthesis genuinely meet, and the split is the
interesting part.

Lidar measures **stand structure**: canopy cover fraction and canopy-top height
per six-meter cell. It does not measure species, age, or which trees have fallen.
So:

- **Where and how much** comes from the measurements. Cover scales stem density;
  canopy height decides what a closed canopy means, since dense young regrowth
  packs many small stems into the cover a mature stand fills with a few large
  crowns.
- **What kind and what shape** stays procedural. A site-level species mixture
  plus a per-individual genotype decides crown architecture, bark, branching, and
  life stage.
- **Height is bounded, not chosen.** The measured canopy top is a ceiling. Most
  trees sit below it and only the oldest reach it, so a stand of six-meter
  regrowth produces six-meter trees whatever the genotype would grow to in the
  open.

Trees are individuals, never a canopy texture. Distant tiers shed geometry, but
never merge trees into a surface — a forest read as a green blanket is exactly
what this system exists to avoid.

**Foliage is not drawn.** Trees render as trunks and branches only, and crowns
are a blank the renderer calls and nothing fills. Every crown the renderer has
had — mats of needles painted on solid balls, then nested shells strung along
whorls, then one ray-marched cone per crown — got the outline right and the cost
wrong, each in its own way, so the approach is being taken from the top rather
than tuned again.

What is being started over is the drawing, not the forest. Where a crown sits,
how wide it is, and what shape and condition the individual grows are measured
and derived exactly as before, and they still reach the renderer; nothing on the
data side was removed to get the pixels out.

---

## 7. Climate and Season

The world is one site, so climate is a property of that site: normals for 46.16
degrees north, humid continental, recorded as constants rather than sampled from
a synthesized global field. Elevation is the only thing that varies inside the
tile, and it varies by about eighty meters.

Season is an explicit argument, never a wall clock. Snow accumulates and melts
across the four seasons and is retained by slope, so the same ground gets the
same snow at every level of detail. Snow is a render treatment; it does not
change where the player can walk.

---

## 8. Streaming and Level of Detail

The world streams around the player and never waits. Terrain is requested, and
frames keep running until it arrives.

Three rules shape the whole system:

1. **Distance first.** The horizon is queued before near detail, and the ground
   under the player before both. A complete rough neighbourhood beats a detailed
   hole.
2. **One LOD step per ring.** Detail coarsens by exactly one level per ring
   outward, so adjacent chunks never differ by more than one level — the
   condition transition meshes are built to bridge.
3. **Hysteresis.** Chunks stay resident past the load radius, so walking back and
   forth across a ring edge does not rebuild them.

Terrain is regenerated, not persisted. Nothing is saved but identity, version,
and deviations from what generation already produces.

---

## 9. Determinism

Generation is a pure function of world identity, coordinates, and versioned
artifacts. Everything else follows:

- The same inputs produce the same output on every supported machine.
- Generation never depends on visitation order, job completion order, wall clock
  time, or a process-randomized hash.
- Adjacent regions share deterministic boundary conditions, so a request's size
  and shape cannot change what is inside it.

That is what lets the mesh queue reorder, drop, cache, and parallelize freely:
completion order is not observable by the world.

Non-basic floating-point operations go through `libm` rather than platform math,
so native and WebAssembly agree bit for bit.

Two version numbers keep worlds honest. A **settings hash** selects the measured
bundle; changing an artifact's bytes, coordinate frame, or meaning requires a new
one. A **generator version** covers everything derived from those measurements.
Neither may change silently under a saved world.

---

## 10. Architecture

A thin Rust workspace. Only machinery this game needs — no scene graph, no
scripting runtime, no editor framework, no reusable engine.

```text
crates/
  coordinates/  World identity, positions, cells, stable hashing
  terrain/      The surveyed bundle, and the density/surface contracts
  climate/      Seasonal normals for the site
  ecology/      Forest structure and tree individuals
  voxel/        Chunks, LOD levels, lattice alignment
  mesher/       Marching Cubes, Transvoxel, surface grids
  world/        Composition, streaming, and the mesh queue
  renderer/     wgpu: terrain, water, trees, sky
  platform/     Platform boundaries kept out of generation
  protocol/     Versioned network contracts

apps/
  client/         The game
  server/         Headless host scaffold
  generator-lab/  Top-down inspector for every layer
```

Dependencies point downward. If two crates need the same type, it belongs in the
lowest crate that owns its meaning — never a shared utilities crate.

Platform APIs stay out of generation crates entirely. That is what makes
generation testable without a window, and identical in a browser.

---

## 11. Testing

Generators are tested on their invariants, not their output values:

- **Determinism.** The same inputs produce the same result, including across
  machines. Golden fingerprints pin the ones that matter.
- **Order independence.** Generating A then B equals generating B then A.
- **Request independence.** A large request contains exactly the individuals a
  small overlapping one produced.
- **Boundaries.** Negative coordinates, cell edges, and half-open ranges behave.
- **Seams.** Adjacent chunks meet on their shared plane, and LOD transitions hold.
- **Bounds.** No tree exceeds its measured canopy; no sample leaves its range.

The local gate:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

---

## 12. Roadmap

This is also the implementation tracker. Update it when something lands end to
end, and say what remains when it does not.

### Done

- [x] **Smooth voxel terrain.** Signed density, Marching Cubes, Transvoxel LOD
      with transition meshes, nested sample lattices.
- [x] **Streaming world.** Two terrain tiers, priority-ordered background
      generation, mesh caching, predictive prefetching, hysteresis.
- [x] **Surveyed terrain.** The Michigan bundle — bare-earth elevation, mapped
      lakes, lidar canopy, aerial imagery — decoded, versioned, and sampled at
      1:1 horizontal and vertical scale.
- [x] **Measured forest.** Tree individuals sized and placed from lidar stand
      structure, with procedural species and architecture.
- [x] **Rendering.** One vertex format and one bind group for terrain, water,
      trees, and sky, drawn by a pipeline per surface kind so that only the kind
      that cuts holes in itself pays for doing so. Scanned PBR materials,
      cascaded sun shadows, daylight states, seasonal snow, and camera-relative
      double-precision positions. Foliage is the exception: trees draw as trunks
      and branches, and crowns are unimplemented pending a rewrite.
- [x] **Browser client.** The same world in a browser, with terrain generation on
      Web Workers.
- [x] **Inspection.** Generator Lab draws any layer as a map and reports every
      layer at a clicked position.

### Next — Expedition

Make the journey a journey rather than a walk.

- [ ] **Camping.** Shelter, fire, and rest as the structure that makes multi-day
      travel readable.
- [ ] **Survival pressures.** Hunger, thirst, temperature, and injury — each
      independently adjustable, none of them a timer.
- [ ] **Navigation.** Wayfinding by landmark, sun, and terrain rather than by
      minimap arrow.
- [ ] **Weather.** Storms the player shelters from, driven by the site's climate.

### Later

- [ ] **More measured ground.** Additional surveyed tiles with manifests, hashes,
      and local caching. Delivery — precalculated bundles or streamed tiles — is
      a choice to make then, not now.
- [ ] **Rivers.** Requires authoritative linework, flow direction, and elevation
      conformance under a reviewed contract.
- [ ] **Caves.** Subterranean structure is genuinely unobserved, and is the one
      place a reviewed procedural contract clearly belongs.
- [ ] **Wildlife.** Animals with territory and routine, not spawn points.
- [ ] **Multiplayer.** Deterministic reconstruction means the wire carries
      identity and deviations, not terrain.
- [ ] **Discovery tracking.** A record of where you have been and what you found,
      because the story is the point.

---

## 13. Non-Goals

Until the exploration game is excellent, these wait:

```text
combat                skill trees           procedural cities
hostile mob variety   villages              economies
dungeons              NPC quests            base defense
loot rarity           automation            magic systems
```

They are seductive because they are obviously "game content." They would dilute
the unusual thing this project could be.

Every proposal answers one question:

> Does this make traveling through the world, understanding it, surviving it, or
> discovering it more interesting?

If not, it waits.
