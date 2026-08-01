# Wilderness Voxel Exploration Game — Design Direction

## 1. High-Level Vision

Build a multiplayer wilderness exploration game whose primary experience is:

> **Go somewhere nobody in this world has ever gone before, discover something beautiful or strange, survive the journey, and come back with a story.**

The player-facing world should begin with surveyed real-world terrain rather
than synthetic landforms. Its observations, derived layers, and runtime
representations are versioned so the same world identity produces the same
surface on every supported machine. A single embedded tile is sufficient now;
precalculated regional bundles and real-time tile streaming are future delivery
choices, not prerequisites for validating the experience.

Procedural generation remains valuable for caves, simulation, missing-data
policies, research tools, and places that cannot be observed directly. It is
not the default source of surface terrain, lake footprints, or tree cover.

The game should evoke:

* hiking
* backpacking
* wilderness camping
* mountaineering
* spelunking
* kayaking and river exploration
* wandering through dense forests
* discovering geological formations
* discovering unique ecosystems
* sharing discoveries with friends

The emotional target is closer to:

**The Long Dark × Firewatch × Minecraft exploration × real wilderness**

than:

**Minecraft × RPG combat game.**

Combat is not a pillar.

The wilderness itself is the content.

---

# 2. Core Design Pillars

## Exploration is the primary progression

Travel should reveal increasingly unfamiliar terrain rather than merely increasing numeric difficulty.

A player ten minutes from spawn might find a pleasant river valley.

A player ten hours away might discover a huge canyon system.

A player fifty hours into exploration might cross a dry plateau, descend into a slot canyon, find an underground river, follow it into a cavern, and emerge through a waterfall into a forested sinkhole.

The reward is:

**“I found this.”**

Not:

**“I obtained a +7 sword.”**

---

## Nature should feel structured, not noisy

The player-facing landscape gets its large-scale structure from observations,
not from a terrain synthesizer. Derived and unobserved systems should still
operate through processes and relationships:

mountains create rain shadows
rain creates drainage
drainage creates rivers
rivers carve valleys
valleys accumulate sediment
climate influences vegetation
rock influences erosion
water tables influence caves
elevation influences snow
fire influences forest succession

The player does not need these systems to be scientifically perfect.

They need to produce *causal coherence*.

A river should feel as though there is a reason it exists.

---

## Large features come before small details

The source and derivation hierarchy for the player world is:

```text
surveyed regional coverage
        ↓
aligned elevation / hydrography / canopy / imagery
        ↓
versioned terrain, water, and vegetation representations
        ↓
honest derived environmental fields
        ↓
procedural detail only where the observations do not identify individuals
        ↓
runtime simulation and persistent deviations
```

Mountains, valleys, plateaus, coasts, and lake footprints must remain the
features present in the source data. Treeline must not replace them with a
synthetic approximation merely to create more variety.

---

## Landscape diversity is core content

The wilderness cannot be mostly one forest with water interrupting it.

Long-distance travel must cross landscapes with fundamentally different
silhouettes, traversal, exposure, vegetation, and geological character.
Required landscape families include:

* closed deciduous, coniferous, and mixed forests with strong regional identity
* open woodland, savanna-like country, prairie, grassland, and steppe
* shrubland, desert pavement, sand sheets, and coherent dune fields
* playas, saline lakes, salt flats, and seasonal closed basins
* broad plains, rolling uplands, plateaus, mesas, and deeply incised valleys
* coastal bluffs, river bluffs, escarpments, cliffs, and canyon walls
* rugged young mountain ranges, rounded old mountains, and glacial landforms
* exposed alpine rock, scree, tundra, snowfields, and summits above the tree line
* wetlands, lakes, rivers, coasts, reefs, caves, and rare natural wonders

These are not cosmetic biome palettes placed over interchangeable terrain.
They are coverage requirements for the surveyed-world catalog: long-distance
expansion should deliberately add real regions that exhibit them. Where source
layers exist, geology, drainage, climate, soil, exposure, and disturbance may
explain and enrich the observed terrain, but they must not rewrite it.

The world needs quiet plains as well as dramatic relief. It needs gradual
slopes, steep traversable slopes, and genuinely abrupt terrain. Steepness must
occur as coherent ridges, scarps, bluffs, cliffs, canyon walls, and mountain
faces rather than isolated noise spikes.

Ten surveyed regions that differ only in tree mixture or material color have
failed this pillar. Interesting geographical diversity is a data-coverage and
selection goal, not a request to exaggerate or normalize individual tiles. It
remains a precondition for expedition gameplay.

---

# 3. Technology Direction

## Use a thin Rust engine

Primary stack:

```text
Rust
wgpu        rendering
winit       platform/window/input
glam        math
iroh        networking
serde       persistent structured data
tracing     instrumentation
```

The engine itself should remain small.

Avoid building a general game engine.

Deterministic generation and simulation use pure-Rust `libm` for non-basic
floating-point operations, with architecture-specific implementations disabled.
`glam` uses its `libm` backend. Generator version 11 establishes this contract;
older world versions require their original executable because their platform
math was not bit-stable across supported targets.

Do not create:

* a generic editor framework
* arbitrary scripting infrastructure
* a generalized scene graph
* a plugin marketplace
* a generic physics engine
* an engine intended for other games

Build exactly the machinery this wilderness game requires.

`wgpu` currently provides DX12 on Windows, Metal on macOS/iOS, Vulkan on Windows/Linux/Android, GLES where appropriate, and WebGPU on WebAssembly, making it a good foundation for the desktop/mobile/web ambition.

The architecture should isolate platform concerns from simulation from the beginning.

---

# 4. Terrain Representation

## Use smooth voxels, not cube blocks

The terrain should be represented primarily as a continuous signed density field:

```text
density(x, y, z)
```

with the surface occurring at approximately:

```text
density = 0
```

This gives:

* smooth hills
* rounded boulders
* cliffs
* arches
* caves
* overhangs
* erosion gullies
* undercuts
* natural tunnels

without requiring absurdly small Minecraft-style cubes.

Voxel Tools uses precisely this kind of SDF representation for smooth terrain.

### Meshing

Start with Marching Cubes for the simplest prototype.

Production terrain should use **Transvoxel**.

Transvoxel extends the Marching-Cubes approach specifically to connect voxel meshes generated at different resolutions without cracks, which is exactly what this world needs. It requires only local voxel information for LOD transitions and was designed for dynamic volumetric terrain.

Godot Voxel Tools already uses SDF + Transvoxel to provide smooth editable terrain with LOD, which is useful validation that this architecture is practical.

### Do not begin with Dual Contouring

Dual Contouring is attractive because it can preserve sharp corners and geological edges better than classic Marching Cubes.

But initially:

**Transvoxel wins.**

Reasons:

* mature algorithm
* published tables
* known LOD solution
* simpler mental model
* existing implementations to test against
* natural terrain usually benefits from smoothing anyway

Sharp cliffs can still be represented with strong SDF gradients.

Dual Contouring can be revisited only if visual evidence shows it would substantially improve the world.

---

# 5. Voxel Resolution

Do not make the world uniformly high-resolution.

That would be catastrophic.

If voxel edge length is halved, volumetric sample density increases roughly eightfold.

Instead use:

```text
LOD 0     0.5 m samples
LOD 1     1 m
LOD 2     2 m
LOD 3     4 m
LOD 4     8 m
...
```

A 0.5-meter base resolution gives sufficient geometry for:

* narrow caves
* trail cuts
* small creek beds
* rock outcrops
* ledges
* erosion
* digging

while Marching-Cubes interpolation makes surfaces appear smoother than a literal 0.5-meter block grid.

Sub-voxel appearance should come from:

* procedural materials
* normal maps
* displacement
* vegetation
* rocks
* debris
* decals

not smaller terrain voxels.

---

# 6. Surveyed Terrain Is a Versioned Data Product

The pristine player world combines a versioned surveyed bundle with derived
runtime representations.

```text
World(
    surveyed_bundle_identity,
    coordinate,
    generation_version
) → terrain / lakes / canopy
```

The bundle records bare-earth elevation, mapped lake footprints and levels,
measured canopy cover and height, supporting surface color, coordinate
reference system, processing methods, provenance, and hashes. Horizontal and
vertical meters remain at 1:1 scale. World X points east and world Z points
south. Per-tile normalization and relief exaggeration are forbidden.

Conceptually:

```text
final terrain
    =
surveyed base terrain
    +
deterministic derived representation
    +
persistent player/world modifications
```

The first product bundle is checked into the client and decoded in full at
startup. That intentionally proves the data and rendering contract without
premature storage or networking machinery. Later, the same immutable layer
tiles may be distributed as precalculated bundles, streamed and cached on
demand, or both. Delivery must not change their coordinate alignment, decoded
values, missing-data behavior, or saved-world identity.

When the player approaches measured terrain:

```text
load or locate versioned layer samples
        ↓
derive density, water, and tree instances
        ↓
mesh
        ↓
render
```

Only player/world deviations require mutable persistence. Immutable source data
may disappear from memory and be decoded or fetched again.

`SURVEYED_WORLD.md` is the authoritative ingestion, layer, alignment,
versioning, and future-delivery contract.

---

# 7. World Construction Philosophy

## Measurements before synthesis

Where authoritative observations exist, use them for the facts they can
support:

```text
bare-earth DEM → terrain surface
waterbody polygons + DEM → lake footprint and level
terrain-normalized lidar returns → canopy cover and height
```

Do not infer exact bathymetry, species, individual tree identity, rivers, or
subsurface geology from inputs that do not contain those facts. Derived
representations must state which parts are measured and which are procedural.

## Stop treating noise as world generation

Noise is useful.

Noise is not geography.

Use noise for:

* irregularity
* roughness
* small-scale modulation
* stochastic parameter fields

Do not use:

```text
mountain_noise + detail_noise + biome_noise
```

as the core geographical model.

Instead create actual features.

---

# 8. Spatial Hierarchy and Procedural Scope

Every surveyed location exists inside several nested geographical scales, but
those scales come from the source coordinate system and coverage catalog rather
than hashed procedural ownership cells.

Example:

```text
planet-scale field

regional province
~500–2,000 km

major landscape
~50–300 km

watershed
~10–100 km

local landscape
~1–20 km

terrain feature
~10 m–2 km

surface detail
<10 m
```

Coordinates and immutable artifact identities select samples at each level.
Deterministic hashes remain appropriate for individual placement, caves,
simulation, tests, and an explicit future missing-data policy.

Something like:

```text
region_seed =
hash(
    world_seed,
    hierarchy_level,
    region_coordinates
)
```

This gives derived content stable identity without pretending that a seed is
the authority for measured surface geography.

## Preserve large regions before deriving local detail

The player-facing world is bounded by admitted surveyed coverage. It must never
silently continue beyond that coverage using a clamped edge or synthetic
terrain. Expansion means admitting another aligned, versioned surveyed bundle
through a reviewed boundary and delivery policy.

For a surveyed region, preserve this source-to-runtime hierarchy:

```text
measured landmass, coast, and elevation
        ↓
authoritative hydrography and other available mapped features
        ↓
aligned canopy, imagery, geology, climate, and land-cover observations
        ↓
versioned derived fields whose evidence is explicit
        ↓
local signed density, water surfaces, materials, and vegetation instances
        ↓
simulation and persistent world modifications
```

Procedural geographical provinces remain a reproducible research path and may
later support places with no acceptable observations, but they do not fill or
reshape the default surveyed footprint. If gap filling is ever admitted, its
boundaries, provenance, visual treatment, identity, and persistence semantics
must be explicit; it cannot masquerade as measured terrain.

Derived ecological fields should describe overlapping conditions, not assign a
single primary biome ID to every cell. Names such as
“prairie,” “salt flat,” or “alpine desert” may be useful for audits, maps, and
player language, but they describe an outcome. They are not the input that
causes the terrain.

The measured surface is height-based where the source is a DEM. Near and far
representations must derive from that same surface and remain spatially
aligned. Caves, player edits, and independently evidenced overhangs or arches
may add signed-density volume; a generic procedural morphology pass must not
invent surface landforms inside surveyed coverage.

---

# 9. Regional Identity

Every large player-facing region receives a surveyed identity: source bounds,
coordinate reference system, acquisition and processing provenance, layer
inventory, and content hashes. The following procedural “genotype” remains
useful for research worlds and genuinely unobserved derived systems, not as a
replacement for the measured surface:

For example:

```text
uplift = 0.78
erosion_age = 0.61
rock_hardness = 0.82
limestone_fraction = 0.43

mean_temperature = 0.36
seasonality = 0.70
precipitation = 0.54

forest_density = 0.78
fire_frequency = 0.22

karst_probability = 0.64
volcanism = 0.04

river_meandering = 0.31
sediment_load = 0.57
```

Where such parameters are derived, they should be spatially correlated and
carry confidence/provenance rather than changing at arbitrary tile boundaries.

Nearby landscapes therefore belong to recognizable geographical families.

Travel far enough and those relationships gradually change.

This creates the experience:

> “We're definitely not near home anymore.”

---

# 10. Geology Before Derived Terrain Detail

A major source of variety should be geology.

In surveyed worlds, ingest regional fields from authoritative sources when
available, or label them as inferred/unsupported. Useful fields include:

```text
rock type
hardness
fracture direction
stratification
permeability
erodibility
uplift
faulting
volcanism
sediment
```

Let them influence materials, caves, hydrology, and ecological suitability only
to the extent supported by their resolution and meaning. Do not deform the
measured DEM to make it look more consistent with inferred geology.

Two mountain ranges should not merely use different noise seeds.

One might be:

```text
young
steep
faulted
granite
glaciated
```

while another is:

```text
old
weathered
sedimentary
karstic
heavily forested
```

That distinction should propagate downward into caves, soil, water, cliffs, vegetation and rock exposure.

---

# 11. Treat Rivers as Authoritative Structure

This is one of the most important decisions in the project.

Do not infer visible rivers from elevation alone and present them as fact.
Admit rivers when authoritative linework, flow direction, discharge semantics,
and elevation conformance have a reviewed bundle contract. Then derive their
rendering and simulation from that aligned structure.

There is published procedural-terrain work using exactly this approach: first build a hierarchical drainage network, derive watersheds from it, and then construct terrain around the river system.

The procedural research path may still conceptually produce:

```text
major drainage basin
      ↓
trunk river
      ↓
major tributaries
      ↓
minor tributaries
      ↓
streams
```

That graph must not carve or replace surveyed terrain in the player-facing
world. A future reviewed river-ingestion stage may apply bounded, documented
conformance corrections where the resolution of the DEM and hydrography
requires them.

---

# 12. Hydrology

At regional scale, derive or ingest:

```text
rainfall
snow accumulation
evaporation
drainage direction
flow accumulation
watersheds
lake basins
spill points
river discharge
```

Priority-Flood is a well-established algorithm for identifying/filling terrain depressions and can also support watershed labeling and flow-direction analysis.

That gives us natural concepts like:

```text
basin
spill elevation
catchment
outlet
downstream basin
```

instead of Minecraft's:

```text
there is water at y=63
```

---

# 13. Rivers Should Have Life Histories

A river should carry parameters downstream:

```text
discharge
sediment
gradient
temperature
seasonality
```

Those determine its form.

Small steep stream:

```text
narrow
rocky
rapid
cold
waterfall-prone
```

Large lowland river:

```text
wide
slow
deep
meandering
muddy
floodplain
```

Mountain river:

```text
incised gorge
rapids
boulders
falls
```

Dry-climate river:

```text
braided
seasonal
flash-flood channels
alluvial fan
```

In procedural research worlds, the river generator therefore creates terrain
rather than merely occupying it. In surveyed worlds, admitted river structure
must conform to the observed terrain under a bounded, documented rule; it may
not freely redesign the valley.

---

# 14. Lakes

A lake should be a mapped waterbody with an explicit, level surface. The
current contract uses authoritative polygons and a DEM-derived representative
level; it does not claim bathymetry or infer every DEM depression as a lake.

Future source layers may establish:

```text
basin geometry
       ↓
spill elevation
       ↓
water surface
       ↓
outflow river
```

Procedural research worlds may still derive closed basins and their outlets.

Those can become:

```text
salt lakes
playas
salt flats
seasonal wetlands
```

depending on climate and evaporation.

In the surveyed player world, playas, salt flats, and seasonal wetlands should
come from real coverage or a separately reviewed derivation contract rather
than a cosmetic biome assignment.

---

# 15. Waterfalls

In the surveyed player world, do not add a waterfall until admitted river data
and the measured terrain support one. Once that contract exists, do not place
waterfalls as decorations.

Detect them.

For a river:

```text
river gradient suddenly increases
             +
sufficient discharge
             +
appropriate rock resistance
```

→ waterfall or cascade.

In a procedural research world, generate:

```text
lip
plunge
pool
spray zone
undercut
downstream gorge
```

In a surveyed world, the measured terrain and admitted river data must already
support the lip, drop, and downstream form. Large waterfall systems should be
consequences of geography, not rendering decorations.

---

# 16. Erosion

The surveyed surface already records the accumulated result of erosion at the
source resolution. Do not run a synthetic erosion pass that moves its ridges,
valleys, or drainage. Erosion models remain useful for research worlds,
material classification, micro-detail that preserves the source surface, and
future simulation of player-visible change.

For the allowed procedural and derived uses, this does not need to be a
geological simulation accurate to millions of years.

The purpose is creating recognizable:

* drainage gullies
* valleys
* canyon incision
* scree
* rounded old mountains
* sharp young mountains
* sediment plains

Graph-based erosion techniques can produce drainage, gorges and terrain shaping much more efficiently than brute-force particle simulations.

The key is that erosion should operate at several scales.

```text
macro erosion
    valleys / drainage basins

meso erosion
    gullies / canyon tributaries

micro erosion
    exposed rock / scree / soil shapes
```

---

# 17. Climate

Compute climate from geography rather than random biome noise.

At minimum:

```text
latitude-like climate field
elevation
continentality
wind direction
ocean proximity
rain shadows
season
```

derive:

```text
temperature
precipitation
snowpack
humidity
vegetation potential
```

A mountain should therefore visibly alter the ecosystem across it.

```text
wet forest
   ↓
mountain
   ↓
dry valley
```

rather than two unrelated biome IDs meeting at a border.

---

# 18. Ecosystems, Not Biomes

Avoid:

```text
Biome::Forest
Biome::Desert
Biome::Tundra
```

as primary generation concepts.

Instead calculate environmental variables:

```text
temperature
rainfall
soil depth
soil acidity
rock exposure
surface moisture
sun exposure
disturbance
elevation
```

Plant species each define preferred ranges. In the current surveyed contract,
those ranges choose and vary individuals only inside the measured canopy
occupancy and height constraints.

The forest becomes an emergent distribution.

That allows:

```text
dense cedar swamp
open pine ridge
young birch burn
ancient hemlock forest
scrubby alpine krummholz
river-bottom hardwood forest
```

without manually defining each as a biome.

---

# 19. Trees Are Important Enough to Be a System

Players will spend enormous amounts of time looking at trees.

Do not have six tree models.

Measured canopy cover and height should control where trees occur and bound
their stature. Because the current observations do not identify individual
trees or species, the individuals may derive from procedural species grammars.

A tree genotype includes:

```text
height distribution
trunk taper
branching angle
branch density
crown shape
leaf density
bark style
response to slope
response to wind
response to competition
age
damage
```

Then individuals vary.

The same species should have:

```text
saplings
mature trees
ancient trees
wind-damaged trees
fallen trees
dead standing trees
leaning trees
storm-broken trees
```

Forest structure matters more than raw species count.

---

# 20. Forest Succession

Forest age/history fields are a procedural interpretation unless a supporting
disturbance or land-cover source is admitted. They may create local detail and
simulation history inside measured canopy constraints, but must not erase a
measured opening or populate a cell with no observed canopy.

Possible history:

```text
old growth
recent fire
80-year regrowth
windthrow
flood disturbance
beaver-like wetland disturbance
landslide
```

That gives landscapes visual histories.

A player walking through the forest should occasionally notice:

> “Something happened here.”

without needing a quest marker explaining it.

---

# 21. Coral Reefs

Reef location and large-scale relief require suitable bathymetry or an explicit
authoritative reef source before they enter the surveyed player world. Within
that observed footprint, procedural detail should use environmental
constraints:

```text
water depth
temperature
wave exposure
clarity
substrate
currents
```

Generate reef growth as structures growing toward favorable light/depth.

That gives:

* fringe reefs
* patch reefs
* barrier-like reefs
* channels
* lagoons

instead of random coral noise.

The same philosophy should apply underwater as above water.

---

# 22. Caves

Do not use only 3D noise worms.

Generate cave systems according to geology.

Possible cave families:

```text
karst caves
lava tubes
fault caves
sea caves
talus caves
glacial caves
erosional caverns
```

Karst generation might follow:

```text
limestone region
      +
fracture field
      +
water table
      +
drainage
      ↓
cave graph
```

Then convert the cave graph into an SDF subtraction from solid terrain.

Possible cave structures:

```text
passage
chamber
shaft
sump
underground stream
collapsed roof
entrance
sinkhole
cenote
```

This produces cave *systems*, not random holes.

---

# 23. Rare Natural Wonders

The world needs extremely uncommon destinations. In the surveyed catalog,
these are selected because the real terrain and supporting sources contain
them; they are not injected into an otherwise ordinary tile to satisfy a
frequency target.

Examples:

* giant slot canyon
* huge natural arch
* enormous cave chamber
* underground waterfall
* basalt-column valley
* volcanic caldera
* geothermal terraces
* glacial cirque
* fjord
* giant dune field
* salt basin
* meteor crater
* limestone tower landscape
* enormous sinkhole
* stone forest
* maze canyon
* isolated alpine lake system
* gigantic old-growth grove
* submerged cave system
* coral atoll
* blue-hole-like formation

For unobserved subterranean features and explicit procedural research worlds:

**Use algorithms, not prefabs.**

A “natural arch generator” creates an arch based on geological conditions.

It does not paste `arch_04.vox`.

Therefore two arches can belong to the same geological family without being copies.

---

# 24. Anti-Repetition Architecture

Real regional coverage is the primary defense against recognizability. Do not
distort measured tiles in pursuit of novelty. Procedural subsystems should
still explicitly fight recognizable repetition.

Minecraft frequently exposes its generator because players learn:

> “Oh, that's that formation again.”

We need to avoid procedural fingerprints.

Do this through **high-dimensional variation**.

A canyon is influenced by:

```text
rock
uplift
age
river discharge
gradient
fracture
climate
sediment
tributary density
erosion strength
regional structure
```

Not:

```text
CanyonPreset = 7
```

The combinatorial space becomes enormous.

---

# 25. Feature Families Instead of Prefabs

Reusable code is fine.

Reusable geometry should be rare.

Good:

```text
generate_karst_tower(parameters)
generate_glacial_valley(parameters)
generate_arch(parameters)
```

Bad:

```text
place(karts_tower_model_3)
```

The generator should specify rules, constraints and processes.

The result should emerge from the location.

---

# 26. Spatial Rarity

Observed rare features retain their real clustering. For procedural caves and
other unsupported systems, rare things should occur through correlated
probability fields rather than independent dice rolls per chunk.

Example:

```text
rare karst conditions
        ↓
one 40 km region
        ↓
unusual geology
        ↓
several related wonders
```

This produces geographic destinations.

A player might say:

> “The country 600 km northwest has these insane limestone towers.”

That is much more interesting than every 1,000th chunk having a rare feature.

---

# 27. Discovery Tracking

The game can quietly recognize discoveries.

For example:

```text
First recorded visit:
Great Hollow Cavern

Discovered by:
Matt

World day:
184
```

But only label features after discovery.

Before someone explores an area, it should not exist in a searchable database of POIs.

Interesting feature detection can happen when surveyed data is admitted or when
an explicitly procedural feature is derived.

Then the server assigns a discovery identity:

```text
feature fingerprint
+
world coordinate
+
generator metadata
```

Players could share:

* map pins
* photographs
* trail routes
* coordinates
* field notes

Discovery becomes the social layer.

---

# 28. LOD: Near World

Near the player:

```text
0.5m SDF samples
Transvoxel mesh
full collisions
vegetation
dynamic water
interactive objects
```

This is the playable simulation bubble.

---

# 29. LOD: Mid Distance

Further away:

```text
1m
2m
4m
8m
...
```

terrain resolution progressively falls.

Transvoxel handles boundaries between levels.

This allows caves and overhangs to technically persist farther out while becoming cheaper.

---

# 30. LOD: Far World

At sufficient distance, caves no longer matter visually.

Switch representations.

Use a dedicated far-terrain renderer based on:

```text
surface elevation
surface material
snow
vegetation coverage
water
```

rather than full volumetric terrain.

Conceptually:

```text
0–200m
full voxel world

200m–2km
Transvoxel LOD

2–20km
coarse terrain mesh

20–100km
horizon terrain + atmospheric rendering
```

Exact distances should be performance-driven.

The important principle is:

**far mountains do not need voxel interiors.**

That is how enormous landscapes become practical.

---

# 31. Load Distant Terrain First

Traditional chunk systems often do:

```text
near chunks
then farther chunks
then farther chunks
```

This creates terrible vistas while loading.

Instead decode, fetch, and derivation jobs should have separate priorities.

The player might receive:

```text
coarse surveyed coverage
        ↓
regional elevation and appearance
        ↓
near terrain
        ↓
vegetation
        ↓
high-detail surfaces
```

So distant mountains appear almost immediately.

Then nearby detail resolves around the player.

---

# 32. Distant Forests

Trees may ultimately be a larger rendering problem than terrain.

Use:

```text
near:
full procedural tree meshes

medium:
simplified tree meshes

far:
cluster impostors

very far:
forest canopy representation
```

At 20 km, the renderer needs:

**“dark conifer forest covers this slope.”**

It does not need 1.3 million tree meshes.

---

# 33. Water Architecture

Water should have two systems.

## System A — hydrological world state

Defines long-term equilibrium:

```text
ocean
rivers
lakes
wetlands
groundwater tendencies
```

This is mostly immutable surveyed or explicitly derived state.

## System B — local dynamic water simulation

Around active players:

```text
water volume
surface elevation
velocity
flow
```

This responds to local terrain changes.

The distinction is essential.

Do not simulate every waterbody in the admitted surveyed world every tick.

---

# 34. Dynamic Water

The local simulator should conserve water volume.

A useful representation is a surface/shallow-water model for ordinary outdoor water combined with specialized volumetric cells where necessary.

For example:

```text
river / lake
    ↓
2D water surface + depth + horizontal velocity
```

while:

```text
waterfall
cave stream
flooded cave
```

may need local 3D handling.

Do **not** attempt full Navier-Stokes simulation.

We need believable water, not computational fluid dynamics research.

---

# 35. Lakes Should Actually Fill

Suppose a player builds a dam.

Water should accumulate behind it.

Eventually:

```text
water level rises
      ↓
basin fills
      ↓
reaches spill point
      ↓
outflow begins
```

This may happen dynamically while the area is active.

When the area unloads, summarize its water state:

```text
basin water volume
water level
outflow
persistent terrain changes
```

It does not need to tick while nobody is present.

When loaded again, reconstruct the dynamic representation.

---

# 36. Frozen Simulation

World regions have states:

```text
UNGENERATED
GENERATED
ACTIVE
FROZEN
```

ACTIVE means:

```text
full simulation
```

FROZEN means:

```text
persistent summary only
```

For example a forest region may store:

```text
last simulation time
fire state
water state
major ecological disturbance
player modifications
```

Upon reload:

```text
elapsed time
+
summary
+
deterministic environmental model
```

allows coarse catch-up simulation.

---

# 37. Weather

Weather should reinforce geography.

At the large scale:

```text
regional pressure/weather field
prevailing winds
temperature
moisture
```

Terrain modifies it.

Therefore:

* storms collect against mountains
* rain shadows occur
* fog forms in valleys
* snow lasts at elevation
* wind becomes fierce on ridges
* thunderstorms can build over warm wet areas

Weather becomes part of exploration.

---

# 38. Survival Philosophy

Survival exists to make travel meaningful.

It should not become a chore simulator.

Default mode:

```text
food matters slowly
water matters moderately
temperature matters situationally
sleep matters
weather matters
injury matters
navigation matters
```

Players should not spend the game watching six status bars decay.

The primary danger should be:

**being poorly prepared far from safety.**

---

# 39. Survival Difficulty

Expose independent controls:

```text
hunger        off → demanding
thirst        off → demanding
temperature   gentle → dangerous
injuries      forgiving → serious
weather       calm → severe
wildlife      passive → dangerous
navigation    assisted → hardcore
```

This lets the same world support:

**relaxing hiking**

and:

**The Long Dark-ish expedition survival.**

Do not tie these into one difficulty slider.

---

# 40. Wildlife

Wildlife should make wilderness feel alive.

Large categories:

```text
small animals
birds
herbivores
fish
rare predators
```

Most encounters should be observational.

Potential threats:

* bears
* wolves
* mountain lions
* venomous creatures
* large territorial herbivores

but combat is not the expected interaction.

Instead:

```text
notice
avoid
back away
make noise
use deterrent
escape
```

A predator appearing should be memorable precisely because it is uncommon.

---

# 41. Camping

Camping should be a central ritual.

The journey loop:

```text
travel
      ↓
choose camp
      ↓
shelter
fire
food
water
weather
map
      ↓
sleep
      ↓
continue
```

Camping produces pauses in exploration and emotional attachment to places.

Players should remember:

> “That's where we camped during the storm before climbing the ridge.”

---

# 42. Navigation

Avoid instantly perfect maps.

The player begins with uncertainty.

Possible tools:

```text
compass
paper/topographic map
altimeter
GPS as an optional accessibility/game setting
trail markers
shared multiplayer map
```

Exploration gradually reveals maps.

High terrain becomes useful because you can actually see distant landmarks.

This makes long render distances mechanically important rather than merely decorative.

---

# 43. No Traditional RPG Quest System Initially

Do not generate:

```text
collect 8 flowers
kill 5 wolves
visit marker
```

The player's objective is self-created.

Instead support expedition goals:

```text
reach that mountain
follow this river
find the coast
explore that cave
cross this desert
map this valley
find where this river begins
```

The terrain creates objectives naturally.

---

# 44. Progression

Progression should primarily unlock capability.

Examples:

```text
better backpack
warmer equipment
lighter shelter
climbing equipment
canoe/kayak
winter gear
caving equipment
better maps
photography equipment
water purification
better boots
```

These unlock environments rather than higher-level enemies.

---

# 45. Multiplayer

Use host-authoritative multiplayer initially.

```text
Player A
  host
   │
   ├──── Player B
   ├──── Player C
   └──── Player D
```

Iroh handles connectivity, authenticated encrypted connections, NAT traversal and relay fallback. Iroh's documentation reports that roughly nine out of ten connections go direct in practice, using relays as fallback.

Later:

```text
DedicatedServer
       │
       ├─ players
       └─ persistent world
```

uses the same simulation.

---

# 46. Deterministic Reconstruction Is a Networking Superpower

The server should not normally transmit terrain meshes or pristine voxel
samples. Clients must possess or fetch the exact surveyed artifacts named by
the world identity and verify their hashes.

Send:

```text
surveyed bundle manifest and hashes
generator version
region state
modifications
```

Then clients independently reconstruct:

```text
terrain and lake representations from shared observations
procedural tree individuals bounded by measured canopy
supported derived systems
caves and other explicitly procedural layers
```

Only deviations must synchronize.

For example:

```text
player dug here
tree was cut
dam exists
camp placed
forest burned
lake state changed
```

This dramatically reduces world-transfer requirements.

---

# 47. World and Derivation Versioning

This becomes extremely important.

Never define world terrain using only:

```text
seed
```

Use:

```text
WorldIdentity {
    seed,
    generator_version,
    generation_settings,
    surveyed_bundle_identity,
    surveyed_artifact_hashes,
}
```

A surveyed-artifact or sampler update may change the reconstructed surface; a
generator update may change derived or procedural layers.

Already explored regions must not silently transform.

Persistent region metadata can retain:

```text
generated_with_version = 19
```

while newly explored regions use version 20.

Alternatively major updates can explicitly require new worlds.

But this must be deliberate.

---

# 48. Rust Architecture

Suggested workspace:

```text
crates/

  coordinates/
      world coordinates
      hierarchical cells
      deterministic hashing

  terrain/
      SDF
      geology
      surface composition

  geography/
      macro regions
      drainage
      climate
      watersheds

  ecology/
      vegetation
      species
      forest structure

  caves/
      cave graph generation
      subterranean hydrology

  hydrology/
      rivers
      lakes
      dynamic water

  voxel/
      chunks
      sampling
      edits
      material channels

  mesher/
      marching cubes
      transvoxel

  world/
      streaming
      LOD
      job scheduling

  simulation/
      player
      wildlife
      weather
      survival

  protocol/
      network messages
      replication

  renderer/
      wgpu
      terrain
      vegetation
      atmosphere
      water

  platform/
      storage
      input
      jobs
      networking
      lifecycle

apps/

  client/
  server/
  world-viewer/
  generator-lab/
```

---

# 49. Generator Lab

This should be treated as a major product.

Maintain a standalone surveyed-world and generation viewer.

It should show:

```text
source elevation and provenance
water and canopy masks
surface imagery and alignment
derived density / LOD / materials
supported geology, climate, hydrology, and ecology
procedural research layers when explicitly selected
```

Allow:

```text
bundle selection
source/derived layer toggles
teleport
zoom across admitted coverage
rebuild derived representations
save screenshot
inspect parameters
```

The data-to-world pipeline needs to be explorable independently from the game.

This will probably be one of the highest-leverage tools in the entire project.

---

# 50. Generation Debugging

Click anywhere and display:

```text
Coordinate:
(803431.2, 77.4, -59201.9)

Macro Region:
Highland #A7C2

Geology:
granite / gneiss

Uplift:
0.84

Rainfall:
1,440 mm

Watershed:
F92C18

River discharge:
3.4 m³/s

Soil:
thin acidic loam

Forest:
mature northern conifer

Terrain contributors:
mountain_chain #81
glacial_valley #443
tributary #92
erosion_pass #2
```

The inspector must distinguish source observations, processing steps, derived
values, unsupported facts, and procedural additions. An LLM can debug the
pipeline much more effectively when the engine can explain **why terrain
exists** without inventing provenance.

---

# 51. Generation Must Be Functional

Favor functions like:

```text
terrain.sample(position)
climate.sample(xz)
geology.sample(xz)
hydrology.query(xz)
```

over huge mutable procedural pipelines.

Where expensive preprocessing is required, create immutable, content-addressed
regional artifacts from explicit source inputs:

```text
WatershedRegion
RiverGraph
GeologyRegion
```

from explicit inputs.

This makes:

* caching
* testing
* reproducibility
* LOD
* multiplayer
* LLM reasoning

much easier.

---

# 52. Seamlessness

Every admitted layer and feature derivation must either:

1. share the surveyed bundle's coordinate frame and exact bounds, or
2. define deterministic, validated boundary conditions between artifacts.

Never let reconstruction depend on:

```text
which region happened to generate first
```

The world must be independent of exploration, decode, fetch, and job-completion
order.

This should be tested aggressively.

---

# 53. World Reconstruction Testing

The source-to-world pipeline needs stronger testing than ordinary games.

For every bundle identity and derivation version:

```text
same input → same output
```

Always.

Maintain golden hashes:

```text
hash decoded source layers
hash terrain representation at region A
hash admitted water graph B
hash cave system C
```

Regression tests catch accidental world changes.

---

# 54. Hydrology Invariants

Automated tests should check:

```text
river never spontaneously flows uphill
lake surface is level
lake outlet matches spill elevation
tributary connects to receiving channel
water volume is conserved locally
watershed ownership is complete
```

Millions of random generated regions can be fuzz-tested.

That is excellent work for automated coding agents.

---

# 55. Terrain LOD Invariants

Test:

```text
LOD boundaries have no holes
same world sampled at different LODs remains spatially aligned
terrain does not jump when LOD changes
materials remain consistent
modified terrain persists through LOD changes
```

Generate random terrain fields specifically to attack the mesher.

---

# 56. Coverage and Novelty Testing

Build exploration bots and surveyed-coverage audits.

Have them sample all admitted coverage and representative candidate bundles:

```text
elevation profile
terrain curvature
river topology
feature frequency
forest composition
cave topology
screenshots
```

Search for excessive repetition.

For example:

```text
“How often do two 4km regions have
nearly identical terrain descriptors?”
```

The surveyed catalog should have quantitative coverage/diversity reports.
Procedural systems should retain anti-repetition tests.

---

# 57. Automated Screenshot Exploration

Create deterministic camera viewpoints:

```text
valley
ridge
river
forest
lake shore
cave
mountain summit
```

Render fixed viewpoints across admitted bundles and, separately, procedural
research seeds.

Then inspect contact sheets.

Later an image model can classify obvious failures:

* terrain noise soup
* repeated shapes
* impossible rivers
* floating vegetation
* ugly LOD transitions
* boring landscapes

Surveyed-world reconstruction and procedural research should become automated
experimentation disciplines with clearly separated reports.

---

# 58. Performance Principle

Spend computation on places players can perceive.

The entire admitted coverage does not need detailed decoding or derivation at
once.

A distant admitted region can exist as:

```text
an immutable coarse elevation/appearance tile and manifest entry
```

until someone approaches it.

Similarly:

```text
far mountain
```

might initially require only:

```text
coarse elevation function
snow coverage
forest coverage
```

Its caves do not exist computationally yet.

But their locations are already present in the surveyed source; only their
runtime representation is deferred.

---

# 59. What “Exists” Means

This allows an important philosophical property:

A procedurally derived cave nobody has visited has not been explicitly
generated. A surface nobody has visited is already identified by immutable
surveyed artifacts, even if those bytes have not been decoded locally.

But if two machines generate it later:

```text
same seed
same coordinate
same generator version
```

they obtain the same cave.

So it simultaneously feels:

**undiscovered**

and

**already part of the world.**

That fits the game extremely well.

---

# 60. Initial Development Roadmap

## Implementation Status

This roadmap is also the implementation tracker. Update it when a feature lands
end to end or when its status materially changes.

- [x] Implemented and usable in the current prototype
- [ ] Not implemented
- [ ] **PARTIAL** — a foundation exists, but the design goal is not yet met
- [ ] **NEXT** — the next planned implementation milestone

Phases 0–4 record how the current engine and procedural research path were
built. Phase 5 is the product-direction pivot: the default surface is surveyed,
finite to admitted coverage, and expanded by adding versioned real-world data.
Unchecked procedural-surface work in the earlier phases is not a blocker for
the player world unless it is explicitly promoted into a reviewed gap-filling
contract.

## Phase 0 — Terrain Toy

**Phase status: Complete**

Goal:

> Stand on a hill and want to walk toward another hill.

- [x] `wgpu` renderer
- [x] Camera
- [x] Signed density field
- [x] Marching Cubes
- [x] Basic procedural height
- [x] Native macOS client
- [x] Web client

Nothing else.

---

## Phase 1 — Streaming Landscape Foundation

**Phase status: Complete**

- [x] Chunk streaming
- [x] Transvoxel terrain seams
- [x] Distance-selected terrain LOD
- [x] Surface-only far terrain extending beyond 20 km
- [x] Double-precision global player and mesh positions with camera-relative
      high/low GPU coordinates, preserving smooth traversal after distant warps
- [x] Prioritized asynchronous terrain jobs with native-thread and
      message-passing Web Worker pools, direction-aware prewarming, and a
      bounded exact-mesh cache for smooth chunk crossings
- [x] Deterministic world identity and seeds

Target:

**see a mountain 20 km away and walk there.**

This is the first major success criterion.

---

## Phase 2 — Geography

**Phase status: Retained procedural research path. Version 18 adds bounded
geographical provinces and a first condition-driven landform vocabulary;
remaining visual and steep/volumetric work does not gate the surveyed player
world.**

- [x] Macro terrain
- [x] Elongated mountain systems
- [x] Drainage basins and spill levels
- [x] Deterministic regional watersheds
- [x] Rainfall-fed river networks
- [x] Climate-conditioned filled basins, seasonal and saline lakes, internally
      drained playas, level surfaces, and explicit topographic versus active
      outlets
- [x] Multi-scale erosion: regional mountain weathering and sediment
      deposition, drainage-graph gullies, and slope/geology-driven rock,
      scree, soil depth, and micro-relief.
- [x] Bounded 512 km geographical-province artifacts. Parent-scale controls,
      stored shared boundary conditions, neighboring-owner generation halos,
      and continuous geological, climate, drainage, and ecological causes
      produce visitation-order-independent samples across artifact edges.
- [ ] **PARTIAL** — Condition-driven landform morphology now expresses quiet
      plains, rolling uplands, terraced plateaus, coherent scarps and bluffs,
      rugged and weathered mountain systems, glacial valleys, wind-aligned dune
      fields, river canyons, and closed salt basins. The landscape-diversity
      acceptance pass must still prove that these outcomes dominate large
      enough areas, read distinctly in the actual renderer, and provide
      sustained steep traversal without spikes or drainage failures.

No gameplay.

Spend serious time here.

The work remains useful for caves, tests, research, and a possible future
gap-filling policy. It is not the source of default surface terrain.

---

## Phase 3 — Ecosystems

**Phase status: Retained mixed research path. Version 18 adds broad overlapping
ecosystem regimes and procedural vegetation; the surveyed default now replaces
synthetic surface/ecosystem distribution with measured terrain and canopy where
available.**

- [x] Climate. Spatially correlated prevailing winds, latitude-like structure,
      continentality, elevation cooling, orographic precipitation, rain
      shadows, explicit seasons, and deterministic snowpack feed river
      discharge and erosion.
- [x] Soil. Deterministic soil texture and composition, depth, surface moisture,
      acidity, organic matter, and ecology-facing species suitability derive
      from geology, erosion, and climate.
- [x] Forest distribution. Continuous deterministic canopy cover, biomass,
      stand age, disturbance, and tree functional-group composition derive
      from climate, soil, terrain exposure, snowpack, and spatially correlated
      stand history, with a dedicated Generator Lab view and inspection data.
- [x] Procedural trees. Globally anchored individual trees derive continuous
      height, taper, branching, crown, foliage, bark, competition, slope, wind,
      age, and damage traits from forest structure. Saplings, mature and ancient
      trees, wind damage, fallen trees, standing deadwood, and storm breaks are
      deterministic; the client renders their generated trunks, branches, and
      crowns, and Generator Lab inspection explains nearby individuals.
- [x] Surface rocks. Continuous geology-, erosion-, slope-, and soil-driven
      rock distributions feed globally anchored rounded boulders, angular
      blocks, weathered slabs, and scree fragments. Individuals have
      deterministic scale, orientation, embedding, fracture, weathering, and
      moss traits; the client renders their generated low-poly forms, and
      Generator Lab maps the distribution and explains nearby individuals.
- [x] Ground vegetation. Continuous climate-, soil-, forest-, snow-, slope-, and
      disturbance-driven ground cover feeds globally anchored grasses and
      sedges, flowering forbs, ferns, low shrubs, and moss cushions. Individuals
      have deterministic height, spread, foliage, wind lean, color, and
      flowering traits; the client renders their generated low-poly forms, and
      Generator Lab maps the distribution and explains nearby individuals.
- [x] Snow terrain coverage and rendering. Seasonal climate snowpack now
      produces deterministic terrain-surface coverage. Mesh rendering
      interpolates snow depth from a bounded 3×3 climate lattice and derives
      slope retention from the already-generated surface normals at each LOD,
      avoiding per-vertex world queries during render-thread uploads;
      mesh-independent inspection retains a fixed world-space slope sample.
      The client renders the representative winter surface and Generator Lab's
      season control updates the terrain view; this does not yet simulate active
      snow deformation or accumulation from live weather.
- [x] Wetlands. Explicit equilibrium lake/ocean depth and river-floodplain
      inputs combine with climate, soil, terrain, and forest structure to
      produce continuous saturation, hydroperiod, flood frequency, open water,
      peat depth, and salinity. Emergent marsh, forested swamp, peatland,
      seasonal wetland, and salt-marsh strategies blend without biome IDs.
      Wetland ground and shallow water receive visible surface treatment, and
      Generator Lab maps and explains the distribution.
- [x] Reefs. Shallow-ocean depth, temperature, wave exposure, clarity,
      substrate, and deterministic currents constrain reef growth. Framework
      relief grows upward toward favorable light and depth while coherent
      channel fields interrupt it; coast distance and exposure yield blended
      fringing, patch, and barrier-like forms with lagoon potential. Sea-level
      ocean surfaces make reef-bearing coasts visible in the client, and
      Generator Lab maps and explains the generated structure.
- [x] Broad overlapping ecosystem regimes. Climate water balance, soil,
      elevation, tree line, exposure, fire, sediment, salinity, disturbance,
      and ecological memory produce continuous closed-forest, open-woodland,
      prairie/grassland, steppe, shrubland, desert, tundra, exposed-alpine, and
      wetland potentials without mutually exclusive biome IDs. These controls
      create contiguous open-country patches, gate terrestrial vegetation out
      of deep ocean, and strengthen deciduous, coniferous, mixed,
      successional, and dry-woodland forest identities.

Procedural research target:

> Ten screenshots from ten far-apart regions should look like ten different places, not ten seeds of the same generator.

---

## Phase 4 — Caves

**Phase status: Complete on the procedural research path; intentionally
disabled in the surveyed default until cave placement has an honest contract
with measured surface geology and hydrology.**

- [x] Geological cave families. Karst, lava-tube, fault, sea, talus, glacial,
      and erosional systems are selected from regional rock, karst,
      permeability, fracture, volcanism, coastal position, temperature,
      uplift, erosion, precipitation, and surface-drainage conditions.
- [x] Deterministic cave graphs. Region-owned connected graphs contain
      passages, chambers, shafts, sumps, entrances, and sinkholes with stable
      topology fingerprints and generation-order-independent boundaries.
- [x] Underground rivers. Permeability, recharge, inferred surface drainage,
      and a generated water table create downhill subterranean reaches; their
      water ribbons render with the near terrain.
- [x] Generated entrances. Every system has a terrain-opening surface
      connection, and the client can locate the nearest one with `C`.
- [x] Sinkholes. Every system includes a deterministic sinkhole connection.
- [x] Shafts. Vertical graph branches connect passage levels.
- [x] Cave-graph subtraction from terrain density. Analytic node spheres and
      passage capsules subtract from the signed density field; cave-aware
      vertical bounds make deep interiors part of near Transvoxel meshes while
      leaving far heightfield terrain unchanged. Cave walls have family-aware
      treatment, unsupported vegetation is removed from openings, the player
      floor query can descend into and follow cave interiors, and Generator Lab
      maps and explains cave footprints.

Connect subterranean systems to surface geology and hydrology.

Generator version 16 introduces cave subtraction for newly created worlds.
Earlier versions retain their cave-free pristine density contract.

---

From this point forward, world quality comes before player activities. Do not
resume climbing, survival, inventory, or other expedition mechanics until the
surveyed-world, presentation, and living-water phases below meet their
acceptance targets. Basic walking and warps remain sufficient for inspecting
the world while those phases are in progress.

## Phase 5 — World Quality

**Phase status: In progress; a versioned 10 km Michigan surveyed bundle is now
the player-facing default. The terrain, mapped-lake, aerial-color, and
lidar-canopy paths are integrated at 1:1 scale. Surveyed-world visual and
alignment acceptance is next. Procedural versions 18–20 remain reproducible for
research, tests, explicit future gap filling, and unobserved systems, but their
landscape-diversity pass no longer gates the default surface world.**

Goal:

> Measured terrain, lakes, and forest structure should read as one coherent,
> beautiful place at human scale, without debug overlays or visible layer
> disagreement. Expanding the traveled world must preserve that fidelity.

The procedural geography items below record retained research capability and
generator-version history. Their `PARTIAL` labels do not make synthetic
surface completion a prerequisite for the surveyed player world. Current
product work begins at **Surveyed player-world default**.

- [x] **Generation-diversity reset.** Deterministic, top-down
      geographical province artifacts that plan landmass and coast topology,
      geological structure, large landforms, drainage, erosion, climate, soil,
      and broad ecosystem regimes before local terrain is sampled. Provinces
      must coordinate through parent-scale fields, explicit boundary
      conditions, and generation halos to preserve the procedural research
      world's visitation-order-independent generation. Generator version 18
      intentionally resets pristine terrain while version 17 and older remain
      on their frozen paths.
- [ ] **PARTIAL** — Landform morphology families. Version 18 replaces the
      previous narrow family of smooth mountain ridges and low-amplitude local
      relief with
      condition-driven plains, rolling uplands, plateaus, mesas, escarpments,
      river and coastal bluffs, cliffs, canyon walls, rugged and weathered
      mountain families, glacial terrain, exposed alpine terrain, dune fields,
      playas, and salt basins. The causal algorithms are present rather than
      pasted terrain prefabs, but the acceptance audit must still establish
      regional dominance, recognizable silhouettes, and traversal-scale
      distinction for every family.
- [ ] **PARTIAL** — Steep and volumetric surface terrain. Province scarps now
      create coherent abrupt height transitions and side-aware signed-density
      undercuts, with shaped-world volume bounds and aligned far surfaces.
      Sustained steep slopes and genuinely abrupt faces still need acceptance
      tuning, and signed-density morphology for
      cliffs, undercuts, overhangs, arches, and other forms a heightfield cannot
      represent, with aligned far-surface approximations. Do not satisfy this
      item by increasing noise amplitude or creating isolated spikes.
- [x] Open-land ecological regimes. Climate water balance, soil,
      elevation, tree line, exposure, fire, sediment, salinity, and disturbance
      produce large contiguous prairie, grassland, steppe, shrubland, desert,
      tundra, exposed alpine, wetland, open woodland, and closed-forest
      landscapes. Forests should have strong deciduous, coniferous, mixed,
      successional, and structural identities rather than converging on a
      similarly dense mixture everywhere. Version 18 implements these as
      overlapping potentials and threads them through soil, forest, ground
      cover, wetlands, placement, and shared near/far terrain materials.
- [ ] **PARTIAL** — Deterministic screenshot exploration. The headless
      world-quality audit now selects 17 fixed viewpoint families covering
      valley, ridge, river, forest, lake shore, cave, summit, open-land,
      desert, dune, alpine/glacial, cliff, mountain, coast, wetland, and reef
      outcomes. It emits deterministic hill-shaded contact sheets using the
      actual shared terrain-material color path, explicitly marks
      missing-feature fallback frames, and retains reviewed regression
      fingerprints. Native GPU client frame capture and perspective baselines
      for the actual renderer remain.
- [x] Quantitative novelty testing. The configurable world-quality exploration
      audit samples far-apart regions and records 60 dimensions covering terrain
      relief, slope, curvature, coherent cliffs versus spikes, province causes,
      ecosystem potentials, open-patch connectivity, climate, soil, drainage,
      lakes, forest cover and identity, wetlands, reefs, and caves. It reports
      closest descriptor pairs, suspicious repetition, plausibility outliers,
      represented and underrepresented outcomes, stable CSV/fingerprint
      artifacts, and an optional strict mode that fails on coverage gaps.
- [x] **Surveyed player-world default.** The client selects one versioned 10 km
      Michigan bundle by default and keeps travel within its measured footprint.
      A USGS 3DEP bare-earth DEM drives voxel and LOD terrain at 1:1 horizontal
      and vertical scale; NHD polygons plus DEM-derived levels drive static lake
      sheets; aligned lidar drives six-meter canopy occupancy and height; and
      NAIP drives supporting surface color. The default identity, artifact
      decoders, local coordinate frame, provenance, reproducible preparation
      recipe, incompatibility rule, and future precalculated-versus-streamed
      delivery boundary are codified in `SURVEYED_WORLD.md`. No world-scale
      storage or streaming system is included yet.
- [x] **Procedural-terrain calibration research handoff.** A headless Rust
      batch sampler now
      exposes bounded offline parameters for province and local landform
      morphology while preserving versioned production defaults and golden
      outputs. A Python/GDAL pipeline prepares fixed-meter ETOPO and NASADEM
      patches, measures multi-scale elevation, relief, slope, curvature,
      spectra, drainage, coasts, and morphology prevalence, runs sensitivity
      and bounded CMA-style searches, and emits labeled or blind heightmap
      galleries without per-tile normalization. The first 512 km ETOPO/Treeline
      comparison confirmed that version 18 concentrates relief into sparse
      features over terrain much quieter than the real reference. A reproducible
      28-patch, 14-source-tile smoke corpus and staged search now have a first
      multiscale candidate that improves train, validation, and holdout distance,
      and those constants are now the generator-version-19 default while version
      18 remains reproducible. A separate 12-patch, 61.44 km ETOPO audit now
      resolves the 1–8 km band and samples the final composed terrain. It found
      that version 19 macro-only river and gully centerlines cut false
      cliff-class walls through calibrated terrain; version 20 fits shared
      channel nodes to the non-fluvial local surface over each drainage graph
      while preserving exact downhill ordering, bringing meso relief and slope
      distributions substantially closer to the reference. The default fixed
      Michigan 3DEP world now exercises one versioned 10 km surveyed artifact
      through the real voxel, LOD, material, vegetation, and player paths at
      1:1 horizontal and vertical scale. An aligned USGS NAIP raster now drives
      terrain color, while NHD waterbody polygons produce static lake sheets at
      DEM-derived elevations. Its local axes preserve geographic handedness,
      traversal uses human-scale meter-per-second speeds, and a matching USGS
      point-cloud pass supplies six-meter canopy occupancy and terrain-normalized
      top height. Surveyed occupancy now drives spatially varying procedural-tree
      counts, while measured height calibrates individual dimensions without
      discarding species, architecture, age, or damage variation. It deliberately
      disables procedural terrain shaping, hydrology, and caves and does not yet
      stream surveyed data. The former native 30 m corpus, larger macro
      dataset, and longer-range procedural calibration goals are retained as
      optional research; they are no longer a player-world completion gate.
- [x] Geography-aware terrain materials. Rendered terrain now blends explicit
      regional rock hardness and carbonate character, soil sand/clay and
      organic content, surface moisture, sediment deposition, rock and scree
      exposure, forest and ground cover, wetlands, reefs, cave family, and
      seasonal snow. Inputs are sampled in world space and travel with the
      shared near/far mesh path so material geography stays aligned across LODs.
- [ ] **PARTIAL** — World lighting and atmosphere. A coherent dawn/noon/dusk
      sky and sun model now drives the sky, terrain illumination, water
      reflections and glints, cool sky fill, and ground bounce. Three
      camera-centered cascaded sun-shadow maps use texel-snapped light-space
      centers, filtered comparisons, slope-aware receiver bias, and blended
      cascade transitions. Fine and coarse terrain, bounded full/simplified
      vegetation, rocks, and ground cover cast into the same maps, while
      exponential aerial perspective and locally climate-controlled lowland
      distance haze preserve landform legibility into the horizon. Native and
      Wasm use the same path, and the client can cycle dawn, noon, and dusk for
      inspection. Surveyed-world perspective baselines must still validate
      forest, valley, shoreline, cascade-transition, and long-distance
      readability without swimming, detached shadows, or exposed LOD changes.
    - [x] Coherent sky/sun lighting and stable camera-centered cascaded terrain
          and vegetation shadows.
    - [ ] When visible clouds land, add cloud shadowing that agrees with their
          position, density, wind, and sun direction.
    - [ ] After visible clouds and local fog provide participating media, add
          bounded volumetric sunlight shafts ("God rays") where sun, occluders,
          and moisture actually align. This is atmosphere polish, not a
          full-screen effect that appears in clear air.
- [ ] **PARTIAL** — vegetation across distance. Full, simplified, and
      silhouette tree meshes exist, and world-space forest composition and
      canopy cover now tint the very-far terrain representation. Cluster
      impostors and a raised very-far canopy silhouette are still missing.
- [ ] **PARTIAL** — Dedicated water presentation. Oceans and lakes now carry
      distinct hydrology colors, reef-bearing shallows and wetland water alter
      those colors, and a dedicated water shading path adds world-anchored
      long/short waves, Fresnel sky reflection, and sun glints. Cave water uses
      its own generated color. River surface ribbons, shoreline transitions,
      depth/underwater treatment, and water-specific blending remain.
- [ ] **PARTIAL** — Geography-driven visible weather. Regional precipitation,
      ocean proximity, soil moisture, temperature, and prevailing wind now
      control fog color/density and water-wave orientation in 8 km
      world-aligned atmosphere cells; seasonal climate already supplies
      persistent high-elevation surface snow. Clouds, visible precipitation,
      pressure systems, terrain-lift cloud formation, and ridge-wind cues
      remain. This phase does not include survival consequences.
- [ ] Surveyed natural-wonder coverage. Expand the versioned bundle catalog to
      real regions containing major arches and sinkholes, extreme canyon and
      karst landscapes, volcanic and glacial extremes, giant dune and salt
      basins, craters, and similarly rare destinations. Caves and other
      unobserved volume may still be procedural under an explicit contract;
      the visible surface wonder must not be injected into unrelated terrain.
- [ ] **NEXT — Surveyed-world acceptance pass.**
    - Curated ground and aerial captures must show recognizable ridges, valleys,
      open canopy, closed canopy, and lake shorelines from the default tile.
    - Near and far terrain must agree in elevation, color, orientation, and
      silhouette without normalization, mirroring, or visible LOD seams.
    - Lake masks, flat water levels, fine terrain, and shoreline vegetation must
      agree without floating sheets or trees emerging from mapped water.
    - Canopy openings and dense stands must follow lidar occupancy, while tree
      heights remain bounded by measured canopy height and individual trees keep
      enough procedural variation to avoid looking cloned.
    - Travel, random warp, and mesh residency must remain inside the measured
      footprint; the clamped DEM border must never masquerade as additional
      surveyed world.
    - The acceptance report must distinguish measured facts, derived values,
      and currently unsupported data such as bathymetry, rivers, and caves.

Do not call the world finished merely because the bundle decodes. This phase is
complete only when the aligned observations are convincing in the rendered
landscape.

The core sky/sun and cascaded-shadow pass landed ahead of the planned
surveyed-world acceptance milestone by explicit product direction. Acceptance
remains `NEXT` and must now validate those shadows against the real tile before
visible weather and cloud shadowing begin. Volumetric light shafts follow only
once fog/cloud media can make them physically legible. All precede the return
to expedition-gameplay milestones, although God rays are polish and may be cut
without blocking the core lighting pass.

The generation-diversity reset intentionally changed pristine terrain in
generator version 18, the calibrated multiscale surface changes it again in
generator version 19, and version 20 aligns river and gully centerlines with
that calibrated local surface. Existing generator versions retain their
previous terrain contract. Phase 6 below records version 17 fast-water terrain
morphology.

The surveyed bundle is selected independently by its settings identity. An
incompatible change to any bundled layer, coordinate frame, sampler, or layer
meaning requires a new settings identity; existing saved worlds must not
silently receive replacement data.

---

## Phase 6 — Living Water

**Phase status: Complete on the procedural research path; not active in the
surveyed default, whose current admitted hydrology is limited to static mapped
lakes. Re-enabling rivers and dynamic connected water requires the reviewed
surveyed river contract described above.**

Implement active-region water simulation before expedition gameplay. Start
with:

- [x] River response to local terrain changes. Bounded storage cells conserve
      displaced water when their generated bed or bank changes, then reroute it
      through deterministic, spill-controlled connections on fixed steps.
- [x] Dynamic lake filling, spill, and outflow. Sources and controlled water
      pulses fill local storage to connection sills before water enters the
      downstream store or leaves through an explicit boundary outlet.
- [x] Generated waterfalls, cascades, plunge pools, and downstream gorges.
      River drop, gradient, and discharge select fast-water morphology without
      prefabs; active flow reports visible cascade/waterfall state, and
      generator version 17 carves derived plunge pools and gorge incision.
- [x] Flooding and floodplain response. Water surfaces above generated banks
      expose stable flooded-cell state while retaining volume conservation.
- [x] Cave-stream, sump, and surface-water connections. Wet cave graph nodes
      become active stores, underground reaches preserve their directed flow,
      and entrances and sinkholes explicitly route surface water underground.
- [x] Frozen-region water summaries and deterministic reconstruction.
      Inactive regions retain millimeter-quantized depths, elapsed time, and
      boundary outflow while pure topology, terrain, sources, and connections
      regenerate from world identity. The client freezes and restores these
      summaries as the player crosses active-water footprints.
- [x] Controlled validation and inspection. Conservation, filling, spill,
      flooding, terrain displacement, fast-water morphology, cave connection,
      negative-coordinate, lifecycle, and freeze/reconstruction scenarios are
      automated. Generator Lab's `L` layer runs a controlled raised-terrain
      and water-pulse scenario and visualizes stored water, the changed strip,
      flooding, cascades, and waterfalls.

Validate terrain-change response through controlled test scenarios and
Generator Lab tools; player dam-building belongs to expedition gameplay. Do not
attempt general fluid mechanics first.

Target:

> Rivers, lakes, waterfalls, coasts, and cave water should look like connected
> parts of one hydrological system, and local water should respond believably
> when its terrain changes.

Known water bug to fix: some equilibrium lakes own a coarse basin footprint
whose composed fine terrain (including trees and surface debris) descends below
the lake surface outside the intended contained basin. The separately rendered
level water sheet then appears to float above that terrain. Fix lake footprint
and shoreline conformance so the filled fine-scale basin, water surface, and
surface-feature placement agree; do not hide it with a rendering-only skirt.

Generator version 17 introduces condition-driven plunge-pool and
downstream-gorge terrain morphology for newly created worlds. Version 16 and
earlier retain their previous pristine density contract. Active water state is
a simulation deviation and does not change pristine-terrain persistence.

---

## Phase 7 — Expedition Gameplay

**Phase status: Not started; basic traversal exists**

- [x] Basic walking and sprinting over terrain
- [ ] Climbing
- [ ] Swimming
- [ ] Temperature simulation
- [ ] Weather exposure and survival consequences
- [ ] Food
- [ ] Water needs
- [ ] Sleep
- [ ] Injury
- [ ] Camping
- [ ] Inventory
- [ ] Navigation
- [ ] Terrain modification and player-built dams
- [ ] **PARTIAL** — independent survival-pressure settings exist, but the
      corresponding simulation systems do not.

Still minimal wildlife.

---

## Phase 8 — Multiplayer

**Phase status: Not started; protocol contracts have an early foundation**

Add Iroh networking.

Because worldgen and simulation were already separated:

- [ ] Iroh transport and connection management
- [ ] Host-authoritative simulation
- [ ] Client simulation
- [ ] **PARTIAL** — versioned protocol. Initial join, movement, welcome, and
      version-rejection message contracts exist without a live transport.
- [ ] Replication
- [ ] Persistent world-modification synchronization

rather than rewriting single-player architecture.

---

## Phase 9 — Discovery

**Phase status: Not started**

- [ ] Maps
- [ ] Photography
- [ ] Field notes
- [ ] Feature discovery
- [ ] Route sharing
- [ ] First-discovery records
- [ ] Multiplayer expeditions

This becomes the long-term player motivation.

---

# 61. Explicit Non-Goals

Until the exploration game is excellent, do **not** prioritize:

```text
complex combat
dozens of hostile mobs
dungeons full of enemies
loot rarity systems
skill trees
villages
NPC quests
procedural cities
economies
base-defense mechanics
automation
factories
magic systems
```

Those features are seductive because they are obvious “game content.”

They would dilute the unusual thing this project could become.

---

# 62. The Central Rule

Every feature proposal should answer:

> **Does this make traveling through the world, understanding it, surviving it, or discovering it more interesting?**

If not, it waits.

The mountain matters.

The river matters.

The storm matters.

The forest matters.

The cave matters.

The camp matters.

The journey matters.

Everything else is secondary.

---

# 63. The Technical North Star

The engine should be capable of this scenario:

A player standing on a high ridge sees a snow-covered mountain range 30 km away.

They decide to walk there.

During the journey they:

* descend through an old forest
* cross a creek
* follow it to a larger river
* discover a waterfall
* shelter from a storm
* camp beside an alpine lake
* climb through exposed rock
* find a cave entrance
* discover that the cave contains the source of another river
* emerge the following day onto an unfamiliar plateau

None of those locations were manually placed or assembled from terrain prefabs.

The visible mountain, valley, lake footprint, and forest structure existed
because the surveyed sources observed them. Terrain density, lake surfaces,
tree individuals, LOD meshes, simulation, and unobserved subterranean structure
were derived from those measurements under explicit versioned rules. Rivers,
caves, bathymetry, and other layers appeared only where Treeline had an
authoritative source or an honest, reviewed procedural contract for them.

The storm interacted with the measured mountains.

And because every source artifact and derivation stage is tied to the world
identity:

**that entire journey belongs to this world and no other.**

That should be the project.
