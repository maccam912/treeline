# Wilderness Voxel Exploration Game — Design Direction

## 1. High-Level Vision

Build a multiplayer wilderness exploration game whose primary experience is:

> **Go somewhere nobody in this world has ever gone before, discover something beautiful or strange, survive the journey, and come back with a story.**

The world is effectively endless and deterministically generated from a seed, but it should not feel like an endless shuffle of interchangeable procedural pieces.

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

World generation should operate through processes and relationships:

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

Minecraft tends to generate terrain roughly like:

```text
noise
  ↓
terrain shape
  ↓
biome
  ↓
trees
  ↓
structures
```

This project should instead build a geographical hierarchy:

```text
continental region
       ↓
mountain systems / basins
       ↓
watersheds
       ↓
major rivers
       ↓
valleys / plateaus / plains
       ↓
local geology
       ↓
erosion
       ↓
forest / wetland / desert
       ↓
small terrain
       ↓
plants / rocks / debris
```

Large features must remain coherent across tens or hundreds of kilometers.

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

# 6. Terrain Should Mostly Not Be Stored

The pristine world is a function.

```text
World(seed, coordinate, generation_version) → terrain
```

Only changes require storage.

Conceptually:

```text
final terrain
    =
procedural base terrain
    +
persistent player/world modifications
```

Therefore an unexplored mountain 800 km away consumes almost no storage.

When the player approaches it:

```text
evaluate world function
        ↓
generate samples
        ↓
mesh
        ↓
render
```

If nobody alters it, it can disappear from memory later.

This is critical to making an effectively infinite high-detail world practical.

---

# 7. World Generation Philosophy

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

# 8. Hierarchical Generation

Every location should exist inside several nested geographical scales.

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

Coordinates feed deterministic hashes at each level.

Something like:

```text
region_seed =
hash(
    world_seed,
    hierarchy_level,
    region_coordinates
)
```

This allows a region to have persistent characteristics without storing them.

---

# 9. Regional Identity

Every large region receives a procedural “genotype.”

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

These parameters should be spatially correlated rather than changing at arbitrary region boundaries.

Nearby landscapes therefore belong to recognizable geographical families.

Travel far enough and those relationships gradually change.

This creates the experience:

> “We're definitely not near home anymore.”

---

# 10. Geology Before Terrain Detail

A major source of variety should be geology.

Generate regional fields for:

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

Then let these influence terrain.

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

# 11. Generate Rivers as Structure

This is one of the most important decisions in the project.

Do not generate terrain first and sprinkle rivers onto it afterward.

Make drainage structure part of terrain generation itself.

There is published procedural-terrain work using exactly this approach: first build a hierarchical drainage network, derive watersheds from it, and then construct terrain around the river system.

Our generator should conceptually produce:

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

Terrain is then constrained around the drainage graph.

This allows continent-scale rivers without calculating every voxel between their source and destination.

---

# 12. Hydrology

At regional scale, compute:

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

The river generator therefore creates terrain, rather than merely occupying terrain.

---

# 14. Lakes

A lake should be an actual filled basin.

Generate:

```text
basin geometry
       ↓
spill elevation
       ↓
water surface
       ↓
outflow river
```

Some basins should have no surface outlet.

Those can become:

```text
salt lakes
playas
salt flats
seasonal wetlands
```

depending on climate and evaporation.

This naturally produces the kinds of features requested rather than having a “salt flat biome.”

---

# 15. Waterfalls

Do not place waterfalls.

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

Then generate:

```text
lip
plunge
pool
spray zone
undercut
downstream gorge
```

Large waterfall systems should therefore be unique consequences of terrain.

---

# 16. Erosion

Terrain should undergo an approximate erosion stage.

It does not need a geological simulation accurate to millions of years.

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

Plant species each define preferred ranges.

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

Trees should derive from procedural species grammars.

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

Generate forest age/history fields.

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

Reefs should use environmental constraints:

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

The world needs extremely low-frequency phenomena.

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

Crucially:

**These are algorithms, not prefabs.**

A “natural arch generator” creates an arch based on geological conditions.

It does not paste `arch_04.vox`.

Therefore two arches can belong to the same geological family without being copies.

---

# 24. Anti-Repetition Architecture

The generator should explicitly fight recognizability.

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

Rare things should occur through correlated probability fields rather than independent dice rolls per chunk.

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

Interesting feature detection can happen when generated.

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

# 31. Generate Distant Terrain First

Traditional chunk systems often do:

```text
near chunks
then farther chunks
then farther chunks
```

This creates terrible vistas while loading.

Instead generation jobs should have separate priorities.

The player might receive:

```text
coarse 50km terrain
        ↓
coarse 10km terrain
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

This is mostly deterministic generation.

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

Do not simulate every river in the infinite world every tick.

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

# 46. Deterministic Generation Is a Networking Superpower

The server should not normally transmit terrain.

Send:

```text
world seed
generator version
region state
modifications
```

Then clients independently generate:

```text
mountains
rivers
caves
forest
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

# 47. Generation Versioning

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
}
```

A generator update may radically change mountains.

Already explored regions must not silently transform.

Persistent region metadata can retain:

```text
generated_with_version = 17
```

while newly explored regions use version 18.

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

Build a standalone world-generation viewer very early.

It should show:

```text
height
geology
rainfall
temperature
watersheds
river flow
biomass
erosion
terrain
```

Allow:

```text
random seed
teleport
zoom 1m → 1,000km
regenerate
save screenshot
inspect parameters
```

The generator needs to be explorable independently from the game.

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

An LLM can debug procedural generation dramatically more effectively when the engine can explain **why terrain exists**.

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

Where expensive preprocessing is required, generate deterministic regional artifacts:

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

Every feature generator must either:

1. be analytically evaluable anywhere, or
2. define deterministic boundary conditions between regions.

Never let region generation depend on:

```text
which region happened to generate first
```

The world must be independent of exploration order.

This should be tested aggressively.

---

# 53. Procedural Testing

The generator needs stronger testing than ordinary games.

For every seed:

```text
same input → same output
```

Always.

Maintain golden hashes:

```text
hash terrain at region A
hash river graph B
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

# 56. Novelty Testing

Build exploration bots.

Have them sample millions of square kilometers and record:

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

The generator should have quantitative anti-repetition tests.

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

Render hundreds or thousands of seeds.

Then inspect contact sheets.

Later an image model can classify obvious failures:

* terrain noise soup
* repeated shapes
* impossible rivers
* floating vegetation
* ugly LOD transitions
* boring landscapes

World generation should become an automated experimentation discipline.

---

# 58. Performance Principle

Spend computation on places players can perceive.

The entire world does not need detailed generation.

A region 1,000 km away can exist as:

```text
a handful of deterministic macro parameters
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

But they are predetermined because the generation functions are deterministic.

---

# 59. What “Exists” Means

This allows an important philosophical property:

A cave nobody has visited has not been explicitly generated.

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

## Phase 1 — Infinite Landscape

**Phase status: Complete**

- [x] Chunk streaming
- [x] Transvoxel terrain seams
- [x] Distance-selected terrain LOD
- [x] Surface-only far terrain extending beyond 20 km
- [x] Prioritized asynchronous terrain jobs with native-thread and
      message-passing Web Worker pools, direction-aware prewarming, and a
      bounded exact-mesh cache for smooth chunk crossings
- [x] Deterministic world identity and seeds

Target:

**see a mountain 20 km away and walk there.**

This is the first major success criterion.

---

## Phase 2 — Geography

**Phase status: Complete**

- [x] Macro terrain
- [x] Elongated mountain systems
- [x] Drainage basins and spill levels
- [x] Deterministic regional watersheds
- [x] Rainfall-fed river networks
- [x] Filled-basin lakes with level surfaces and outlets
- [x] Multi-scale erosion: regional mountain weathering and sediment
      deposition, drainage-graph gullies, and slope/geology-driven rock,
      scree, soil depth, and micro-relief.

No gameplay.

Spend serious time here.

The world generator is the game.

---

## Phase 3 — Ecosystems

**Phase status: In progress**

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
- [ ] **PARTIAL** — Distant forest LOD. Near terrain renders full procedural
      trees, successive individual-tree rings use simplified and silhouette
      geometry, and deterministic composition-colored canopy surfaces plus
      vertical cluster silhouettes keep forests visible from both ridges and
      walking height across far-terrain and horizon tiles. Textured cluster
      impostors and dithered transitions between representation tiers remain.
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
- [ ] **NEXT** — Snow terrain coverage and rendering. **PARTIAL:** Climate
      already generates seasonal snowfall, accumulation, permanent snowpack,
      and meltwater runoff.
- [ ] Wetlands
- [ ] Reefs

Target:

> Ten screenshots from ten far-apart regions should look like ten different places, not ten seeds of the same generator.

---

## Phase 4 — Caves

**Phase status: Not started; cave topology types have an early foundation**

- [ ] Geological cave families
- [ ] **PARTIAL** — cave graphs. Node, edge, and topology validation types
      exist, but deterministic geological graph generation does not.
- [ ] Underground rivers
- [ ] Generated entrances
- [ ] Sinkholes
- [ ] Shafts
- [ ] Cave-graph subtraction from terrain density

Connect subterranean systems to surface geology and hydrology.

---

## Phase 5 — Expedition Gameplay

**Phase status: Not started; basic traversal exists**

- [x] Basic walking and sprinting over terrain
- [ ] Climbing
- [ ] Swimming
- [ ] Temperature simulation
- [ ] Weather
- [ ] Food
- [ ] Water needs
- [ ] Sleep
- [ ] Injury
- [ ] Camping
- [ ] Inventory
- [ ] Navigation
- [ ] **PARTIAL** — independent survival-pressure settings exist, but the
      corresponding simulation systems do not.

Still minimal wildlife.

---

## Phase 6 — Multiplayer

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

## Phase 7 — Living Water

**Phase status: Not started; equilibrium hydrology exists**

Implement active-region water simulation.

Start with:

- [ ] River response to local terrain changes
- [ ] Dynamic lake filling and spill
- [ ] Player dams
- [ ] Waterfalls
- [ ] Flooding
- [ ] **PARTIAL** — frozen-water summary types and deterministic equilibrium
      lakes exist, but no active local water simulation runs.

Do not attempt general fluid mechanics first.

---

## Phase 8 — Discovery

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

None of those locations were manually placed.

None were assembled from prefab terrain chunks.

The mountain existed because of regional uplift.

The forest existed because of climate and soil.

The river existed because of its watershed.

The waterfall existed because of river gradient and geology.

The lake existed because of a drainage basin.

The cave existed because of geology and groundwater.

The storm interacted with the mountains.

And because every stage is derived deterministically from the world seed:

**that entire journey belongs to this world and no other.**

That should be the project.
