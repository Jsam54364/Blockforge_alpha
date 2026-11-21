# blockforge

A mobile-first voxel engine in Rust, in two crates:

* the **core** (this folder): std-only, zero dependencies. World data, terrain generation,
  meshing, saves and the server/client pair. Everything that decides performance and
  architecture lives here and is tested without a GPU.
* the **app** (`app/`): the Android front-end. winit + wgpu renderer, touch controls and HUD.

> **Status:** 114 unit + 9 end-to-end tests in the core and 97 in the app (plus WGSL
> validation) pass. The APK builds on GitHub Actions and has been run and measured on a phone
> (see the numbers further down).

```
cargo test                           # unit + end-to-end tests
cargo run --release --bin headless   # stream, mesh, edit; prints stats + an ASCII map
```

## Architecture

```
            ┌────────────── client thread(s) ──────────────┐        ┌──────── server thread ────────┐
 renderer ◄─┤ Client: ChunkStore replica                    │ msgs   │ Server: authoritative World   │
 (wgpu,     │   dirty set ─► PaddedChunk snapshot ─► mesh   ├───────►│  fixed 20 Hz tick             │
  future)   │   JobPool (2-3 workers, nearest-first)        │◄───────┤  gen JobPool, chunk streaming │
            │   discards stale meshes via version check     │ local  │  behaviours + scheduled ticks │
            └───────────────────────────────────────────────┘ pair   │  EditJournal (saves = deltas) │
                                                                      └───────────────────────────────┘
```

| Module | What it does |
|---|---|
| `coords`, `block`, `chunk` | 32³ **cubic** chunks (`CHUNK_BITS` is one constant), `u16` state = 12-bit id + 4-bit **shape** (slabs/layers), palette-compressed storage (uniform chunks cost ~one entry) |
| `registry` + `assets/blocks.toml` | **Data-driven blocks**; server sends the file to clients on join so ids always match |
| `worldgen` | Deterministic terrain, pure function of (seed, chunk). Heights in **quarter-blocks**, so the surface ends in **half-slabs** (not under water: slabs aren't waterlogged yet). Caves, sea level, bedrock, `Column` heightmap + ground layer |
| `terrain`, `climate`, `biome`, `noise` | Vintage Story-style world: continents, blended **landforms**, mountain chains; temperature from latitude and altitude, rainfall; biomes emerge from the climate (see "World generation") |
| `world`, `save` | Procedural base + **edit journal** (undo/redo, tiny saves, delta sync). Versioned + CRC32 formats, atomic write-then-rename |
| `mesh` | Padded 36³ snapshot (no locks; two cells of each neighbour, the second for water shores), **8-byte packed vertices**, shape-aware face culling, per-vertex AO, 3 render layers, index-free quads |
| `cull` | **Occlusion culling**: per-chunk face links from a flood fill, a walk from the camera that skips chunks hidden behind terrain |
| `lod` | **Level-of-detail terrain** to the horizon: quadtree of sections built from the seed, mesher, streaming (see below) |
| `behavior` | Event-driven blocks emitting commands (deterministic, budgeted). Example: falling sand |
| `server`, `client`, `protocol` | **Client/server split even in singleplayer**; sim distance independent of view distance |
| `jobs` | Priority worker pool, thread count capped at 3 for big.LITTLE phones |
| `tick`, `governor`, `rng`, `raycast` | Fixed timestep, adaptive render-scale/LOD governor, deterministic RNG, voxel DDA |

## Known gaps (deliberate, in rough priority order)

1. **Renderer**: leaves are drawn opaque (no alpha-tested cutout pass
   yet); textures are generated at startup (no compressed ASTC/ETC2 yet).
2. **Lighting**: sun, moon and sky light follow the day (see "Sky and day cycle"), but there
   are no shadows yet (a face turned to the sun is lit even under an overhang), and the
   per-vertex sky/block light fields are still placeholders.
3. **Greedy meshing**: implemented as a hybrid (see below) but not the default mesher yet.
4. **LOD terrain** shows edits by column top only (overhangs and caves under an edit are not
   represented), and a small edit far away only shows once a cell is small enough to land on it.
5. **Server**: no reach/anti-cheat checks, undo history is global not per-player, autosave
   blocks the tick, events outside sim distance are dropped instead of re-scanned.
6. **Client**: a new chunk now waits (up to `Client::neighbor_wait`) for the neighbours the
   server is still sending, so it is meshed once with correct borders and AO. What remains:
   chunks at the edge of the streamed area are meshed straight away, and their AO at
   edges/corners isn't refreshed when only a diagonal neighbour arrives later.
7. **Saves**: block ids are numeric and append-only; add a name→id table to the header.
8. **Determinism**: worldgen uses `f32`; fine on one platform, switch to fixed-point if you
   need bit-identical worlds across ARM/x86 for multiplayer.

## How meshing work is kept small

Measured headless at render distance 8 (1125 chunks around the player, 233 with geometry):

| | mesh jobs at startup | mesh jobs in 6 s of flying | time in `Client::update` at startup |
|---|---|---|---|
| before | 3082 | 511 | 187 ms |
| now | 262 | 84 | 40 ms |

* `mesh::has_no_faces` resolves open sky and buried rock without a snapshot or a job
  (most chunks in a cubic-chunk world).
* New chunks wait briefly for their neighbours instead of being re-meshed as each one arrives.
* `PaddedChunk::capture` fills uniform chunks and decodes the centre chunk in one pass
  (`Chunk::decode_into`); it runs on the render thread, so this is frame time.
* The renderer is only told about chunks whose mesh really changed, so fewer GPU buffers
  are created and dropped.

## Tuning knobs worth measuring on a real phone first

`CHUNK_BITS` (4 vs 5), `ServerConfig::{sim_distance_chunks, vertical_view_chunks,
max_chunk_sends_per_tick}`, `Client::mesh_budget`, worker count, cave thresholds in `worldgen`.

## Android app (`app/`)

A separate crate (its own workspace root, so the core stays dependency-free): winit + wgpu 22.x,
NativeActivity, packaged with `cargo-apk`. Landscape only (either way up: `sensorLandscape`,
not resizable, and the activity asks for it again when it starts), so there are no portrait
layouts. Controls are described under Touch controls below.

CI flow (GitHub Actions, all from a phone):
1. Upload a zip to the repo root -> `CI` unpacks it, runs the core tests.
2. When `CI` succeeds, `Android APK` runs: host type-check + WGSL validation, then `cargo apk build`.
3. The APK is published to the repo's **Releases** page as `blockforge-debug.apk`.

### Menus (`app/src/ui`, `app/src/ui.wgsl`, `assets/ui`, `tools/make_ui_atlas.py`)

Every menu is drawn in one dark fantasy style: weathered stone set in iron frames with bronze
edges, blue crystal gems at the corners and tips, long buttons with pointed ends, a title plaque
with a gem over it, the BlockForge logo in a medieval face (Almendra) with flourished rules,
serif text (Crimson Text) with icons (Material Icons), and a blue glow on the picked item and the
button under the finger. Menu buttons act when the finger lifts off them (slide off to cancel);
the in-game buttons act on the touch. The game is landscape only.

Everything comes from one 2048 x 2048 atlas (`assets/ui/atlas.png` and `atlas.json`), built by
`tools/make_ui_atlas.py` from the fonts: the frames are painted procedurally
(`tools/ui_ornate.py`: bevelled iron lit from the top left, bronze edge lines, noisy stone with
faint cracks) as nine-slice pieces, and the text and icons are signed distance fields, so they
stay sharp at any size. Panels are nearly opaque on purpose: blending happens in linear light,
where a few percent of a bright sky shows strongly through dark stone. The
UI shader (`ui.wgsl`) has three modes per vertex: atlas colour, SDF text, and the world
thumbnails' texture. The fonts are under the SIL Open Font License and the icons under Apache
2.0 (`assets/ui/CREDITS.md`, `assets/ui/licenses`).

### Title screen (`app/src/title.rs`, `app/src/title.wgsl`, `assets/title`)

The app opens on a main menu: the BlockForge logo over a column of Single Player, Multiplayer,
Settings, Mods, Cosmetics and Quit (it closes the app); Single Player glows until another button
is pressed. At the top right is the player (it opens Cosmetics), and on the right the character
himself, the real HyRig model idling on a stone pedestal, with arrows under him that turn him
45 degrees at a time (`title::Showcase`: the controller and rig on flat ground, posed every
frame, drawn with the game's character pipeline between the pedestal and the rest of the menu,
by a camera at his chest shifted in clip space so his feet land on the pedestal;
`gfx::showcase_view_proj`). The version is at the bottom left. Behind it all a panorama of a
sunset over the sea turns once every three minutes.
The panorama was photographed in the game itself: twelve views 30 degrees apart from one spot
off a west-facing coast (seed 1337, near x -1848, z 40, at 18:18), rendered with the god rays off
so they match, and stitched into one cylindrical picture (`tools/stitch_panorama.py`: 4096 x
683, 30 degrees above and below the horizon, 425 KB as JPEG). The title shader turns each screen
pixel into a compass direction and elevation and samples it.

Multiplayer, Cosmetics and Mods open pages that say what is and is not there yet (no network
play, skins or mods so far).

### Worlds (`app/src/worlds.rs`)

Every menu page has the same top bar: the small logo at the left, the page's tabs or title plaque,
and square icon buttons at the right (Back, and the settings cog or an X); under it the page's
panels fill the screen edge to edge, with the page's own buttons inside its frame rather than in a
row along the bottom. Single Player opens the world list (tabs for Single Player and Multiplayer,
Back and the cog in the top bar): on the left every saved world as a row with its picture, name, seed, when it was
last played (date and time, in the phone's time zone) and its size on disk, the most recent
first (drag the list to scroll it); on the right the picked world's big picture, its name between
gems, seed, last played, play time, size and when it was created, and under it two rows of
buttons: Re-Create and Create New World, then Edit, Delete and Play (glowing once a world is
picked). Tap a world to
pick it and again to play it, or use Play. Create New World asks for a name
and an optional seed (a number is used as it is, a word is hashed so it always gives the same
world, blank is random). Edit renames the picked world, Re-Create opens the create page filled
in with its name and seed (a fresh copy of the same terrain), Delete asks first and then removes
its folder for good. Names and seeds are typed on an on-screen keyboard (shift, digits, space and
`-`; a name starts with a capital, as on a phone; Android's own keyboard does not reach a
NativeActivity reliably); a hardware keyboard works too. Opening a world shows Loading terrain
with a progress bar until the player has ground under him (or 25 seconds have passed).

A world's picture is taken when it is closed: the renderer draws the world once more without the
HUD into a texture, reads it back, and `icon.png` (256 x 144, the middle 16:9 averaged down) is
saved in the world's folder. Worlds without one show a crop of the title panorama around the
sun. Play time is added up each time a world is closed.

Each world lives in its own folder, `worlds/<FOLDER>/`: `level.txt` (name, seed, created, last
played, play time), `state.txt` (player, camera, time of day), `icon.png` and the server's edit
journal. The folder name comes from the world's name (`NEW_WORLD`, `NEW_WORLD-2`, ...), so two
worlds may share a name. Settings are the app's (`settings.txt` beside `worlds/`), not a world's.
The single world of earlier versions moves into the list as MY WORLD (seed 1337) the first time
the app starts, edits and all.

### Settings and the game menu

Settings has four groups, as tabs on the left of one big frame: General (walk or fly, the
camera: first person, behind or in front, stats on/off, and the time of day when opened from a
world), Video (render distance 2-12 chunks, LOD distance OFF, 1K ... 32K blocks, LOD quality, fog,
clouds, god rays, VSync), Performance (culling, mesher) and Controls (the button layout editor,
below; button opacity 30-100%, look sensitivity 1-10, the stick's size, Building: Buttons or
Touch + Buttons, and Crouch, Crawl, Roll: Three Buttons or One Button). The group's settings
are in the middle, with arrows to step values and switches; on the right a note on what the group
is about over a sepia picture (a group with more than five settings uses that room for a second
column instead). Changes take effect at once (a new render distance goes to the server straight
away). The top bar's squares: Reset to Default puts the defaults back, Back goes back to where
the settings were opened from (the title, the world list or the game menu), and X closes them
(from the game menu, straight back into the game).

In a world, the pause button at the top right opens the game menu over the world (which keeps
running behind it): Back to Game, Settings, and Save & Quit to Title, which saves the world, takes
its picture, closes it and goes back to the title screen.

### HUD

Top-left overlay (Show Stats): FPS and frame time, this app's CPU use (from
`/proc/self/stat`, so it is the app's share, not the whole phone), CPU time per frame
(excluding the wait for the display), GPU time (timestamp queries; shows N/A on devices
without them) with a derived busy%, memory (VmRSS) and chunk/mesh counts. Android exposes no
public GPU-usage percentage, hence the derived figure. The overlay's LOD line shows the sections
and quads drawn. The HUD and menus are drawn by a small immediate-mode layer (`ui.rs`,
`ui/screens.rs`, `ui/controls.rs`, `ui/draw.rs`) with no UI dependencies.

### Touch controls (`app/src/ui/controls.rs`, `app/src/game.rs`, `app/src/icons.rs`)

* **Stick:** a touch on the left 45% of the screen (not on a button) puts the stick's ring
  under the thumb, and there it stays until the thumb lifts: a plain thin ring with a light
  knob that follows the thumb out to the ring's edge. How far it is pushed walks, jogs or
  sprints (or flies where you look). It fades out a quarter second after the thumb lifts, and is
  not drawn at all otherwise.
* **Look:** drag anywhere else.
* **Attack** (round, pickaxe): a tap is a punch, which breaks the block under the crosshair.
  Held past 0.3 s it becomes a heavy attack: the character chops, and each chop breaks a block
  when it lands (0.45 s into the 0.9 s swing), again and again while held. A chop on its way
  down still lands if the button is let go.
* **Place** (round, shows the hotbar's block): puts that block on the face you look at, then
  again every 0.22 s while held (after 0.4 s), never inside you or the character.
* **Hotbar:** nine blocks along the bottom; tap one to pick it (its name shows for a moment).
  The backpack button at its end opens the inventory over the game (`ui/items.rs`).
* **Inventory screen:** two windows side by side, each with its own tabs in the top bar. The
  left window's tabs (top left): **Inventory** (36 slots, four rows of nine), **Crafting** (a
  2 x 2 grid, what it makes, and the recipes) and **Equipment** (Head, Body, Legs and Feet
  slots beside the character, turned by the arrows under him). The right window's tabs (top
  right, by the X): **Gear** (things to wear; none exist yet) and **Blocks** (the creative
  palette, organised like Minecraft's: a tab each for Building, Natural, Wood and Minerals,
  each tab's blocks by material with every block's slab after it, a biome filter under the
  tabs (arrows step through All biomes and each of the 15), scrolled by dragging, with a
  scroll bar). Along the bottom: **Move Similar** (hotbar stacks of blocks the inventory already
  holds go in with them), **Move All** (the whole hotbar into the inventory), **Sort** (the
  inventory merged and ordered), the hotbar in the middle, and a line about what is picked.
  Items are stacks of up to 64. Tap a slot to pick it up (it lights), then tap where it goes:
  onto an empty slot it moves, onto the same block the stacks merge, onto another they swap.
  A palette block fills a slot with a full stack; something tapped back onto the palette is
  put away. Tapping a hotbar slot also takes it in hand. Crafting takes any arrangement of a
  recipe's blocks (four sand make sandstone, grass makes dirt...): tap the result to make it
  once, into the inventory; closing the screen puts what is left in the grid back. The world
  is creative: placing a block doesn't use it up. The hotbar, the inventory and the grid are
  saved with each world (`state.txt`: `hotbar=stone*64,...`; older saves' plain names read as
  full stacks). The block pictures are little cubes drawn on the CPU from each block's own
  textures when the renderer starts, kept in the bottom of the world-thumbnails texture; the
  character in the Equipment tab is drawn in a pass of its own over the menu.
* **Jump** (also rises in flight), **Down** (flight only: sinks), **Roll, Crouch, Crawl, Act**
  (walking only), and **Pause**.
* **Building by touch** (optional, Settings > Controls > Building: Touch + Buttons): on the
  look side of the screen, a quick tap that doesn't drag places the hotbar's block where you
  look, or uses the block if it can be used (a block whose behaviour says `usable()`; the server
  gets `UseBlock` and the behaviour's `on_used` runs; no built-in block is usable yet). Pressing
  and holding still for 0.35 s digs with the mining swing (see the character section) until the
  finger lifts; a drag is only looking. The Attack and Place buttons still work.
* **One stance button** (optional, Settings > Controls > Crouch, Crawl, Roll: One Button):
  Crouch, Crawl and Roll become one button. A tap crouches (or stands up), holding it for
  0.35 s crawls (or stands up from crawling), and two quick taps (within 0.28 s) roll. A single
  tap waits that long before crouching, so a double tap never crouches first.
* **Layout editor** (Settings > Controls > Button Layout): every control is outlined and can be
  dragged anywhere on screen; tap one to pick it, then make it smaller or bigger (50-200%) or
  hide it. Add puts hidden controls back, including extras that start hidden: Camera, Use,
  Swing, Eat, Wave, Sit and Stats. Pause can't be hidden. Reset puts everything back.
  Done saves the layout to `controls.txt` beside the settings. Positions are kept as fractions
  of the screen, so a layout moves to another phone; a control in its default spot follows the
  default layout for that screen (the hotbar shrinks to fit between the stick and Attack).

Back-face culling is on for solid terrain (water stays double-sided so the surface is visible
from underwater). Tests prove every face is wound counter-clockwise from outside, both in the
mesher and after projection (`mesh.rs`, `gfx.rs`). Flying inside solid rock therefore looks
see-through, as in any voxel game.

**Immersive mode and the safe area.** The activity is a small Java subclass of NativeActivity
(`app/java/com/blockforge/alpha/BlockforgeActivity.java`; cargo-apk does not compile Java, so
the Android workflow compiles it with `javac` and `d8` into `classes.dex` and adds it to the APK
before aligning and signing). On the UI thread, where Android requires it, it hides the status
and navigation bars (they come back for a moment when swiped in from an edge, and hide again),
lets the game draw beside a camera cut-out, and keeps the screen on. It also records how far the
bars and cut-out still reach into each edge, which the game reads through JNI twice a second
(`src/android.rs`) and keeps the HUD inside (`Ui::set_safe_area`). If that cannot be read (an
APK built without the Java part), the HUD falls back to fixed margins measured from a real
phone (`SAFE_TOP_DP`, `SAFE_RIGHT_DP` in `ui.rs`).

Culling: back faces (solid terrain only), a real six-plane frustum test per chunk bounding
box (`math::Frustum`, tested to never reject a visible box), and occlusion culling of chunks
hidden behind terrain (`cull.rs`, below). Settings > Performance > Culling cycles
All / No Caves (no occlusion) / Frustum / Back Faces / Off so each can be measured on a device; the
overlay's DRAWN line shows how many chunk meshes survived culling out of those resident, and
HIDDEN how many in range and in view were skipped as hidden behind terrain.

**Occlusion culling** (`cull.rs`), after Tommaso Checchi's "advanced cave culling" (what
Minecraft has used since 1.8): when a chunk is meshed its see-through space is flood filled,
and for each of the 15 pairs of its faces it records whether one open region touches both
(an opaque full cube blocks sight; slabs, water and leaves don't). Each frame a breadth-first
walk starts at the camera's chunk and enters a neighbour only through faces the current chunk
connects, never doubling back against a direction already taken (sight lines are straight).
Chunks it never reaches are skipped. It is conservative (a reached chunk may still be hidden,
an unknown chunk counts as open) and the walk is redone only when the camera changes chunk or
links change (at most 20 times a second while chunks stream in); it takes well under a
millisecond. Around the spawn of seed 1337 standing on the ground it keeps 280 of 674 meshed
chunks (the ground below the first layer and caves out of sight go); renders of the test
views are pixel-identical with it on and off.

Frustum culling, same camera at each distance (render distance: drawn / meshed chunks):
6: 94/458, 8: 156/602, 10: 230/706, 12: 315/985, i.e. about 21-33% of meshed chunks. GPU
milliseconds at a vsync-capped 60 fps are NOT a reliable cost measure (the GPU clocks down to
fill the frame), so use the VSync switch (Settings > Video) to read uncapped FPS when comparing settings.
The overlay's MAIN THREAD line is the render thread's own CPU use: near 100% = CPU-bound,
low with low FPS = GPU-bound.

Uncapped measurements on a mid-range phone (VSYNC off, same camera): render distance 10, GPU time
by CULL mode: OFF 26.0 ms (37 fps, 1000 chunks drawn), BACK 17.2 ms, FRUST 9.6 ms (85 fps, 318 drawn),
ALL 9.8 ms. Render distance 12: OFF 22.3 ms, BACK 22.2 ms, FRUST 9.5 ms (95 fps, 445 drawn), ALL 8.8 ms.
So frustum culling is worth about 2.5x, back-face culling adds little on top of it, GPU time tracks the
number of chunks drawn (about 20-30 microseconds each), and the main thread sits at 35-40% of a core, i.e.
the GPU is the bottleneck and geometry per chunk is what to cut.

Measured earlier on a mid-range phone, fog off, flying over terrain (GPU time of the main pass):
render distance 6 = 7.1 ms (43% busy), 8 = 11.7 ms (70%), 10 = 12.9 ms (77%), 12 = 16.7 ms
(94%, 56 fps). CPU stayed near 5% of the device; memory 100-220 MB. The renderer is GPU-bound,
so the next wins after frustum culling were occlusion culling (now done, not yet measured on a
phone) and fewer real chunks (a lower render distance, now that LOD terrain carries the view
past it).

Solid terrain is drawn nearest chunk first so the depth test can reject hidden fragments
before they are shaded (water still goes far to near for blending). This has not been measured
on a phone yet: compare GPU time with VSYNC off against the figures above.

**Pooled vertex buffers** (`app/src/pool.rs`): chunk and LOD meshes live in 4 MB GPU buffers
("pages") instead of one buffer each, so a new mesh is one `write_buffer` into space that is
already there and a mesh that goes away just returns its range (no buffer created or freed per
chunk, which is slow on some OpenGL ES drivers). Free ranges are kept sorted per page, first fit,
merged with their neighbours; a mesh bigger than a page gets its own; an empty page is given
back once the others have a page's worth of room. Each draw binds its range of a page. The
overlay's VERTS line shows MB holding meshes / MB reserved and the page count (around the spawn
in the test harness: 35 of 36 MB in 9 pages). Harness renders drawn from the pool are
pixel-identical to the old per-chunk buffers on the OpenGL backend.

### What is kept between launches

`world.bfwj` (block edits) and `state.txt` (the camera, the time of day, the hotbar) live in each world's folder, `settings.txt` and `controls.txt` (the button layout) beside them in the app's
internal data folder. Both are written when the app goes to the background, because Android
can kill it there without warning; settings are also written when they are applied. The
world file is only rewritten when a block actually changed. When the app comes back, every
chunk mesh is handed to the new GPU surface again (`Client::requeue_all_meshes`).

### Installing updates

Every APK is signed with the same debug key, `app/debug.keystore` (see the `signing.dev`
section in `app/Cargo.toml`), so a new build installs over the old one and keeps the world and
settings. Each build's summary on GitHub Actions prints the signing certificate's SHA-256; it
should never change. Builds made before this key existed were signed with a random key, so
the first update onto a fixed-key build still needs one uninstall. The key is a debug key with
the standard `android` password and is not a secret; a Play Store release would need its own
private key kept in a repository secret.

### Icon and Play Protect

The launcher icon is in `app/res` (adaptive icon with a themed-icon layer for Android 13+,
plus plain PNGs for older launchers); `app/icon-src/make_icon.py` draws it.

Play Protect warnings on this sideloaded APK, and what helps:

* "Built for an older version of Android": shown when `target_sdk_version` is more than two
  below the phone's API level. The app targets 35, which is clean up to Android 17.
* "App scan recommended" / unknown developer: expected for a rarely seen APK. Let it scan;
  keeping one signing key (above) lets Play Protect recognise later builds as the same app.
* "Harmful app blocked" would be a misclassification (the app asks for no permissions):
  appeal at https://developers.google.com/android/play-protect/warning-dev-guidance.
* Longer term, Android developer verification ties apps to a registered developer. The free
  "limited distribution" account (up to 20 devices, no ID) covers a hobby project; register
  the package name `com.blockforge.alpha` and this signing key there.

Renderer gaps: pooled vertex buffers, occlusion culling, texture array
(colours come from a palette keyed by texture name), render-scale governor hookup.

## Level-of-detail terrain (`lod.rs`)

Terrain past the real chunks, out to 32 km, for the look of an endless view. The design is
Distant Horizons' (the Minecraft LOD mod, LGPL), rewritten from scratch for this engine and for
phones; no code was copied from it or from other LOD mods.

* **Quadtree of sections in widening rings.** A section is 32 x 32 cells; a level-L cell is a
  column 2^L blocks wide. The rings start where the real chunks end: the first has 2-block
  cells, and each further ring is twice as wide with cells twice as coarse (4, 8, 16 ... 512
  blocks). LODQ sets the first ring's width (LOW 32, MED 64, HIGH 128 blocks). Far out every
  ring holds about the same number of cells, so the cost grows with the logarithm of the
  distance (4 km to 16 km costs about +12%). A balancing pass keeps neighbouring sections
  within one level of each other.
* **Stand-in terrain while chunks load.** Under the real chunks LOD terrain is kept at 2-block
  cells and shows wherever a chunk isn't drawn yet, so flying faster than chunks stream in
  shows rough terrain instead of holes.
* **Built from the seed, not from chunks.** The generator is a pure function of the seed, so a
  cell is one height sample (`WorldGen::lod_surface`, exactly the generated surface at full
  detail). Octaves of the height noise finer than a cell are faded to their average instead of
  aliasing, and heights are rounded to steps that grow with the cell (a quarter of its width),
  which roughly halves the quads.
* **Smooth far terrain at no extra cost:** from 4-block cells outward every vertex carries the
  ground's slope at its corner (the normal from the four unrounded heights around it, packed in
  the spare 16 bits of its 8-byte vertex), and the shader lights it by that slope instead of
  by the way its face points. The tops and steps of a hillside then shade as one smooth
  surface, and steps no steeper than the cell is wide take the top's texture instead of the
  striped side. Same quads, same memory. The 2-block cells next to the real chunks stay
  blocky like them.
* **Blocky meshes like Distant Horizons':** a box per column, sides only where the neighbour is
  lower, equal faces merged. Border sides reach down to the lowest height a neighbour one level
  finer or coarser can have, so there are no cracks between levels (the quadtree keeps
  neighbours within one level; tested).
* **No holes and no overlap with the real chunks.** Distant Horizons draws its LODs from a
  distance setting, so they either overlap the real terrain or leave holes while chunks stream
  in. Here the client reports, per chunk column, whether the real surface is meshed, uploaded
  and drawn (`Client::lod_coverage`); the LOD vertex shader drops quads exactly in those
  columns. Near the real chunks (only the two finest levels can touch them) faces are merged
  only within a chunk column so every corner of a quad agrees, and on every chunk-column line the
  side faces reach a block below the real ground beside them (the sea floor under water), so
  where real terrain dips below its LOD cell no slit opens at the seam. LOD water is level with
  real water and needs none. Sections whose columns are all covered aren't drawn at all.
* **Drawn after the real terrain**, nearest first, so LOD pixels behind real terrain are rejected
  by the depth test before shading. Reverse-Z depth with no far plane keeps precision at 30 km.
* **Never a gap while detail changes:** a section is replaced only when everything replacing it
  is built *and uploaded to the GPU* (a coarser parent stands in for unready children, or finer
  children for a parent). A test flies the camera with uploads throttled to two sections a
  frame and checks every frame for holes and doubles.
* A chunk column counts as covered by real terrain once its chunks are drawn down to one that
  blocks the view from above in every block column (a cave under the ground does not uncover
  it). LOD terrain also loses depth ties, so the moment both are drawn while chunks arrive
  does not flicker.
* Fog spans the far part of the LOD view, so the horizon fades into the sky.

Measured on the build machine (headless demo; render distance 6, whole view before culling,
around the origin of seed 1337, which is a mountain peak): medium quality at 16 km is 320
sections, about 360k quads (11.6 MB), built in ~0.8 s on one thread; per frame the CPU spends
~30 us on it (coverage + planning). Only the sections in view are drawn.

Limits and next steps:

* **Edits show in LOD terrain.** The server works out the top of every block column an edit
  has touched (generated terrain plus the edit journal, so it needs no loaded chunks) and sends
  them all when a client joins, then each one an edit changes (`ServerMsg::Surface`). LOD
  sections use them instead of the seed: a cell shows the highest edited column inside it
  when that rises above the terrain (a tower stays visible kilometres away), otherwise the
  edited column at its sample point (a pit shows once cells are small enough to land in it).
  Sections an edit touches are rebuilt and drawn as they were until the new mesh is ready.
  Edits from earlier sessions count too, since they come from the saved journal.
* The terrain is now made of many noise layers (continents, landforms, mountain chains, climate),
  so building LOD sections costs about 10x what it did with the old four-octave terrain: medium
  quality at 16 km is ~0.8 s on one thread on the build machine (it was ~80 ms). Sections are
  built nearest first in the background, so this shows as the far view filling in for a few
  seconds after a start or a long jump, not as stutter. If it is too slow on a phone, the
  large-scale fields can be sampled on a coarse grid and interpolated per section.
* Not yet measured on a phone: GPU time with LODs on at each quality.

## World generation (`terrain.rs`, `climate.rs`, `biome.rs`)

Modelled on Vintage Story's: about one block per metre, the shape of the land and its ground
cover decided separately.

* **Whole-block terrain, smoothed with half slabs**: heights are rounded to whole blocks, and a
  half slab is added only at the foot of a step, where the ground rises toward a higher
  neighbour. Slopes get smoothed; flat ground stays flat.
* **Continents**: a large, domain-warped noise decides land and sea (about 60% land; the
  spawn area is always land). Coasts are low, the land rises inland, the sea floor shelves
  down to about -50.
* **Landforms** (`terrain::LANDFORMS`): plains, rolling hills, high hills, plateaus (terraced),
  mountains, wetlands (ponds). Each ~900-block region picks one by weight; neighbouring
  regions blend smoothly. **Mountain chains** run for kilometres along the zero lines of
  another large noise, through whatever landforms they cross; peaks reach about 175 blocks.
* **Climate** (`climate.rs`): temperature follows latitude along z (`POLE_DISTANCE` = 25,000
  blocks from equator to pole; the spawn is temperate, -z is towards the equator, +z towards
  a pole, and the bands repeat beyond) and drops 1.5 C per 10 blocks of altitude, so peaks
  are snowy. Rainfall has Earth-like belts (wet equator, dry subtropics, wet mid-latitudes)
  plus its own map, and coasts are wetter.
* **Fast creative flight** (`player::FLY_SPEED`): 18 m/s cruising, 72 m/s with the stick
  pushed all the way, 18 m/s up and down (the kit's 6 / 11 / 4.5 are kept for its parity
  test); moves in steps of at most 0.4 m so it can't pass through a wall in one frame.
* **The 15 biomes** (`biome.rs`): chosen from the climate, the altitude, the distance to
  the coast and large region noises for the places to discover. Frostpeak Highlands (cold
  peaks, green below and snowbound up high) and Stonefang Mountains on the high ground;
  Crystal Glacier, Whitewind Tundra and Winterpine Taiga in the cold; Ancient Wildwood,
  Amberwood and Giants' Grasslands in the temperate lands by rainfall; Sunscorched Expanse
  with Emerald Oasis pockets in the hot dry lands, Emerald Oasis also where it is hot and
  wet; Murkwater Swamp in warm wet lowlands; Stormbreak Coast along temperate shores; the
  Blightlands and the Petrified Kingdom where their region noises are high. Each names
  its ground (a surface, a block showing through it in patches, a subsurface) and its two
  stones (under the ground, on cliffs, in boulders and spires: ground three blocks steeper
  than a neighbour shows stone). **To add a biome, add an entry and a rule in `choose`.**
* **Each biome shapes its own land** (`WorldGen::style`), at real-world scale and
  amplified in the manner of JJThunder's terrain, after a set of reference pictures:
  * Ancient Wildwood: a rolling forest floor with ravines, 60-85 m redwoods, ferns.
  * Amberwood: soft hills carpeted in fallen leaves under red and gold maples.
  * Frostpeak Highlands: alpine massifs with sharp ridges and peaks up to ~400 blocks
    over snowy spruce valleys.
  * Stonefang Mountains: jagged crags and ridges, and well inside the biome massive rock
    pillars 28-60 blocks across and 50-140 tall with green tops (`WorldGen::pillar`, like
    Zhangjiajie's) in green valleys. Pillars rise only where the place is mostly
    Stonefang, so none stand half-height in neighbouring biomes.
  * Sunscorched Expanse: a sea of sharp-crested dunes up to ~20 blocks, bare jagged
    mountains rising from it.
  * Emerald Oasis: red-rock tablelands ~45 blocks up, cut by canyons with stepped walls,
    pools of water and lush ledges at their feet.
  * Murkwater Swamp: just above the water, hummocks among open pools with lily pads.
  * Whitewind Tundra: wide open swelling land with frozen lakes.
  * Crystal Glacier: an ice sheet in domes, cut by deep canyons with blue ice walls.
  * Winterpine Taiga: rolling snowy hills and spruce forest, snow on the branches.
  * Blightlands: craters and cracks glowing purple from below, twisted dead trees.
  * Petrified Kingdom: badlands of layered tablelands and hoodoos, stone trees.
  * Stormbreak Coast: green headlands ending in 40-70 block sea cliffs, sea stacks.
  * Giants' Grasslands: wide rolling hills with flower meadows and tablelands ending in
    escarpments.

  High ground is snowbound above ~150 blocks (bare rock on its cliffs), snow settles on
  leaves in the cold biomes, and frozen biomes' lakes are iced over. **Biomes blend:**
  how much a place belongs to each biome (`biome::weights`) comes from the same rules as
  choosing it, each a soft step over a margin, and the land is the weighted mix of the
  styles. Rivers are cut afterwards.
* **Trees at real sizes** (a block is a metre): redwoods 60-85 m, grown oaks to ~22 m,
  ancient ones ~40 m, spruces and firs 14-28 m (giants half again), palms 11-18 m, swamp
  cypresses 18-28 m.
* **Rivers** (`Terrain::river`): along the zero lines of a large, domain-warped noise, so
  they meander for kilometres. They start as streams a few blocks wide in the hills (none
  on the mountains) and widen to ~25 blocks towards the coast, where they meet the sea.
  Their surface follows the land seen from 600 blocks up, so it falls gently towards the
  sea a block at a time, with a bed of gravel and sand (mud in the swamp), a lip of bank
  above the water and valley sides easing down to it. Each column knows how high water
  fills it (`Column::water`, the sea's or a river's); nothing grows in rivers.
* **Groves** (`WorldGen::grove`): trees grow thick where a 150-block noise is high and thin
  out into glades where it is low; forest biomes stay mostly wooded, open lands gather
  their few trees into copses. Distant terrain draws the same groves as canopy.
* **Realistic tree sizes**, per species (`Wood::size`, a percent of an oak's): a young tree
  4-6 m, a grown oak's crown ~15 m, old giants ~25-30 m, redwoods 34-50 m, conifers
  8-18 m (giant spruces up to ~29 m); birches, pines, figs and ironwoods smaller, elms a
  little bigger.
* **The Hollow Earth** (`WorldGen::is_cavern`): under kilometre-wide regions, huge caverns
  from y -50 to -120, flooded below -104 (underground lakes), with glowing cave moss on
  their floors, glow crystals on their ceilings, giant glowcaps and fungalwood mushrooms,
  glowwood trees and crystal clusters. Caves reach down to them there, and sinkholes open
  from the surface.
* **Natural features** (`features.rs`): every feature is a pure function of its anchor, so
  each chunk draws its own part of it and the parts meet. Big ones sit on a 24-block grid
  (ancient trees, redwoods, spires, arches, big boulders and fallen logs, skeletons, ice
  spikes), ordinary ones on a 6-block grid, kept out of the big ones' way. Trees grow like
  Vintage Story's (limbs lose width as they go, bend and fork, leaf clusters at their
  ends) with the procedural-tree mods' forked trunks, buttress roots and layered crowns:
  broadleaf trees in three sizes (ancient giants up to ~40 blocks with roots spreading
  ~10 blocks and sometimes a hollow trunk you can walk into), 45-70 block redwoods,
  layered or open conifers (giant spruces in the taiga), palms, flat-topped acacias,
  weeping willows, buttressed swamp cypresses with knees, bamboo clumps, twisted dead trees,
  petrified trees, giant mushrooms. Small details: bushes, stumps, fallen logs with moss,
  mossy boulders, cacti, crystal clusters, fossil boulders.
* **Ore veins** in the rock of every chunk, by depth and biome: coal, iron, copper, gold
  deep down, silver and rubies or sapphires in the mountains, rock salt in the drylands,
  amber in Amberwood, fossils in the Petrified Kingdom and the grasslands, blightshard in
  the Blightlands, frost crystals in the glacier, glow crystals near the Hollow Earth.
* **Features in distant (LOD) terrain** (`WorldGen::lod_cell`, `Feature::lod_top`): each
  feature has an outline (crowns as domes and cones, spires as tapering columns, arches,
  boulders, giant logs) from the same sizes it is drawn with, and a cell shows the highest
  outline averaged over four points across the cell (and only where it covers at least
  two, so a tree narrower than a cell never becomes a cell-wide tower). Cells up to 8
  blocks show every feature; past that trees are left to a smooth canopy over the groves
  as high as the biome's trees, and only rock features (spires, arches, boulders) show
  individually up to 32-block cells (dense in the Wildwood and the taiga, scattered on the grasslands). Features are
  bucketed per section (`FeatureGrid`), so sections cost a few milliseconds.
* `cargo run --release --example chunk_bench` times chunk generation in a few biomes
  (well under a millisecond a chunk on a desktop).
* New players start on the nearest open, gentle ground in the Giants' Grasslands, clear of trees (`find_spawn`). The stats
  overlay shows the biome, temperature, rainfall and landform under the camera.
* `cargo run --release --example worldmap -- <seed> <blocks per pixel> map.ppm` draws a
  top-down map of the biomes.
* Worlds are versioned (`WORLDGEN_VERSION`): the app keeps a world from an older generator in
  its own save file and starts the new one fresh at the spawn.

Next: trees and plants per biome, rivers, rain shadows behind mountains.

## Block textures (`assets/textures`, `tools/paint_textures.py`)

* **Original 32 x 32 textures in the manner of Vintage Story's**: fine-grained and low in
  contrast, so ground reads as a material from a distance rather than as a pattern.
  `tools/paint_textures.py` paints them all (run it again after changing it, or edit the PNGs):
  a smooth colour ramp per material with soft mottling, grain at the scale of single pixels,
  and each material's own detail (blades for grass, crumbs and grit for soil, pebbles for
  gravel, crystals for granite, strata for limestone and sandstone, pits and joints for basalt).
* **Rock types**: the world's bedrock changes by region (`WorldGen::rock_at`): basalt, granite,
  andesite (the `stone` block), limestone, sandstone, in regions a kilometre or so wide. It is
  what is underground, and what shows where a biome's ground is bare rock. Adding one is a
  painter function, a block in `assets/blocks.toml`, a name in `app/src/textures.rs` and a
  place in `rock_at`.
* **The biome block set** (`tools/blockset.py`): the blocks the 15 biomes need, about 130 of
  them, painted by the same helpers in the same manner. Run `python3 tools/blockset.py
  --sheet` to repaint them; it also writes the generated end of `assets/blocks.toml` (its first
  part is hand-written and keeps the oldest ids; the script adds group, family and label to
  those blocks once) and `app/src/texture_files.rs`, the list of textures the game embeds.
  `BLOCKS` in the script is append-only, since saved worlds store block ids.
  * Rocks: red sandstone, slate, blightstone (sickly veins), deepstone, chalk, marble, and
    for each of the eleven rocks a cobbled, a bricks and a polished block. Also mossy cobbles
    and bricks, mud, clay, ice and snow bricks, petrified-wood bricks and polished blocks.
  * Trees: redwood, maple, birch, pine, spruce, fir, palm, cypress, willow and blightwood,
    each with a log, planks and leaves; oak planks, golden oak leaves (Amberwood),
    driftwood and petrified logs. Leaves other than oak have their own colours.
  * Ground: forest floor, leaf litter, needle floor, prairie grass (tinted), snowy grass,
    tundra moss, permafrost, rich soil, mud, peat, clay, blighted soil and turf, moss and
    glowing cave moss, red sand, shelly sand, shingle, cracked earth, ice, packed and glacial
    ice.
  * Minerals: coal, iron, copper (also in sandstone), silver, gold, ruby, sapphire and
    blightshard ores, rock salt, fossil stone, amber, and frost and glow crystals that give
    light.
  * Specials: cactus, mushroom stems and caps, glowcaps, hay bales, reed thatch, bone blocks,
    weathered stone and glowing runestones.
  * Not yet: plants that need a crossed-quad model (flowers, ferns, reeds, seaweed) and
    stairs, which the mesher can't draw yet. World generation doesn't place the new blocks
    yet either: for now they come from the inventory.
* **About a hundred blocks for every biome** (`tools/biomeblocks.py`, painted by
  `blockset.py`): each of the 15 biomes has its own two stones, two woods, plaster, accent
  colour, soil and sand, and every block is one of those in a pattern. For each stone:
  the stone, mossy, cracked, weathered, rubble and layered (natural), and 24 building
  patterns (cobbles, bricks, polished, small and large bricks, herringbone, tiles, diagonal
  tiles, chiselled, pillar, cracked and mossy bricks, mossy cobbles, flagstones, pebble
  mosaic, ashlar, framed, chequered, frieze, rough-hewn, shingles, inlaid, cut stone,
  basketweave). For each wood: stripped log, vertical planks, parquet, diagonal planks,
  panel, carved panel, shingles, timber frame, crate, log wall, lattice (and log, planks and
  leaves for the 18 new species). Then plaster, timbered and cross-timbered plaster, painted
  plaster, wattle, daub, roof tiles, mosaic, coarse, stony and rooty soil, path, pebbles and
  sand. A block's `biome` field names its biome; the Blocks window filters by it.
* **Room for ~2,000 textures:** the texture array takes as many layers as the GPU allows
  (usually 2048); on a GPU with fewer, textures past its limit are drawn as the closest in
  colour under it (`gfx::texture_layers`, `BlockRegistry::remap_textures`). Distant terrain
  keeps a texture in 11 bits. Item icons are drawn as they come on screen, into 288 slots
  reused least-recently-shown first (`icons::icon_slot`), so any number of items fits.
* **Groups, families and slabs** (fields of a block in `blocks.toml`): `group` is the
  inventory tab (building, natural, wood, minerals), `family` the material it sits with,
  `label` its name there, and `slab = true` adds its slab as an item of its own. An item is a
  block state, so a slab item is the block's bottom-slab state. Placed on top of a block a
  slab lies at the bottom of the cell; under one, at the top; on a side, in the half the
  crosshair points at. A slab placed on the open half of the same slab makes the whole block.
  In saves a slab is `name:slab`.
* **Recipes for the set** (`game::recipes`): a log makes 4 planks; a rock makes its cobbles,
  4 cobbles make 4 bricks, 4 of the rock make 4 polished blocks; 2 blocks make 4 slabs and 2
  slabs make the block back. Moss on cobbles or bricks makes them mossy; 4 clay, mud, ice,
  snow or petrified logs make bricks. With blocks in the grid, the recipe list shows only
  the recipes that use them.
* **Greyscale grass, coloured by biome** (Minecraft's way): grass tops and leaves are
  greyscale; a grass side is soil plus `<name>_overlay.png`, grass hanging most of the way down
  (so grassy slopes are green, with soil only at the foot of each step). Each biome has a
  grass colour (`biome::BIOMES`). A **tint map** around the camera (`app/src/tint.rs`), one
  texel per 16 blocks out to ~2 km and one per 128 blocks out to 16 km, is built on a
  background thread from the world generator and sampled with bilinear filtering, so colours
  blend smoothly between biomes, on LOD terrain too.
* **Hiding the tiling**: every block turns and mirrors its texture its own way (tops 8 ways,
  sides mirrored only, so fringes stay on top), and the tint map carries a gentle brightness
  variation over tens of blocks.
* **Water** (`water_shade` in the shader), after open sea under a bright sky: a deep navy body,
  nearly opaque; the reflection of the sky (the real one, see below) takes over toward the
  horizon (Fresnel), with sun and moon glints. The waves follow the open procedural-ocean
  shaders (afl_ext's "very fast procedural ocean" and the sum-of-sines method of GPU Gems
  chapter 1): sixteen wave trains from 34 blocks down to under half a block, each
  `exp(sin x - 1)` (broad troughs, sharp crests), each turned by an angle that never lines up
  again, and each dragging the point the shorter ones are measured at by its own slope, so
  ripples bunch on the faces of the swell and no pattern repeats. Long waves travel faster
  than short ones, at a little under a quarter of deep-water speed (WAVE_SPEED), so the sea
  moves slowly. The surface is also warped by drifting noise, swell and chop come and go
  in patches, and gusts roughen some areas. Up close, where water reflects little, ripples
  shade the body: facets tilted toward the sun's side are a little lighter, the others a little
  darker (same colour on average). A wave train fades out before it gets shorter than a few
  pixels on screen (which also ends the loop early far away), so far water is calm instead of
  drawing moire rings.
* **Shores:** the mesher codes, per corner of each water surface, how far the nearest block
  standing in the water is (touching, one block, one diagonally, two or more), in the AO bits
  water doesn't use. Along the shore the waves calm down over two blocks, and a band of foam
  laps in and out against the blocks, ragged and broken into bubbles, with thin streaks of
  froth further out, so the water meets the land instead of sliding past it. The surface sits 1/8 block (4 of 32 pixels) below the top of its block,
  like Minecraft's, so shores show a small lip; LOD water matches.
* All textures are uploaded as one mipmapped texture array (averaged in linear light) and
  sampled with world-position coordinates, so greedy quads and LOD terrain repeat them once
  per block. Half-slab sides start the texture at their own top.

## Sky and day cycle (`app/src/sky.rs`, `assets/sky`, `tools/bake_sky.py`)

* **A photographed sky with a sun that moves.** The sky is Poly Haven's *Rustig Koppie Pure Sky*
  HDRI (CC0). The game draws its own sun, so `tools/bake_sky.py` takes the photo's out: a clear
  sky is close to "sky by elevation" times "glow by angle from the sun" (the shape of the Perez
  sky model), so the script measures that glow from the photo, fits a small formula to it per
  colour channel (`assets/sky/glow.txt`, within ~3%), divides it out and paints over the disc.
  What is left, `assets/sky/sky.png` (the upper half, 2048 x 512, RGBE in a PNG, 0.8 MB), keeps
  the clouds and the horizon haze; the game multiplies the glow back in around its own sun.
* **The day** lasts 20 real minutes (`sky::DAY_SECONDS`); a new game starts at 09:00. The sun
  follows a real path (latitude 35 degrees, spring): it rises in the east, stands 65 degrees
  high in the south at noon and sets in the west. As it gets low the glare fades and the sky
  toward it turns gold and orange; then comes the night (below). Settings > General > Time of Day (opened from
  a world's game menu) moves the clock an hour; the time is saved with the camera.
* **The night** is a second photograph, ambientCG's *Night Sky HDRI 003* (CC0): stars, the Milky
  Way and a moon with its halo. `bake_sky.py --night` takes its smooth background out (the game
  draws that by elevation in the world, in the photo's colours) and keeps the whole sphere,
  because the stars turn through the night and the ones below the horizon come up. The game's
  moon is always opposite the sun, so the shader turns the photo (`Night::rotation`, around the
  pole with the sun's hour angle) placed so that its moon is exactly there at every hour; the
  moonlight on the terrain comes from the same place. Fog and water reflections include it.
* **Light**: each of the six face directions gets one light colour per frame (`sky::face_light`):
  the whole sky as seen facing that way (summed over a small copy of the sky, every 0.01 hours),
  direct sun and moon, light bounced off the ground, and a little extra at night to keep it
  playable. Ambient occlusion and the light fields multiply it as before.
* **Fog** takes the sky's own colour in its direction, so far terrain melts into whatever is
  behind it: bright toward the sun, golden at sunset, dark at night. **Water** reflects the
  same sky.
* **Drawing it**: one full-screen triangle after the terrain, at the far depth, so it only shades
  the pixels the terrain left empty. The sky is an RGB9E5 texture with all its mip levels, and
  the shader picks the level itself (an automatic one would show a seam where the angle wraps).
  On the way to the screen, colours up to 0.8 are left alone and brighter ones roll off softly,
  keeping their hue (an orange sunset stays orange); only the very brightest, like the sun,
  wash out to white.
* **Another sky**: download an equirectangular HDRI (2K is plenty), run
  `python3 tools/bake_sky.py sky.hdr` (day) or `python3 tools/bake_sky.py --night night.exr`
  (night; it needs its moon in it), rebuild. Clear or lightly clouded day skies work best:
  clouds stay where they are, lit from where the photo's sun was.

## The character (`app/src/hyrig`, `app/src/player.rs`, `assets/hyrig`)

* **Walk mode** (the default): you are the HyRig character, seen from behind. Camera (in Settings, or Act > View) cycles like
  Minecraft's F5: behind, in front looking back at him (the camera sits 3.7 blocks out along
  where you look; blocks are still aimed from his eyes), then first person. The
  stick walks, jogs or sprints by how far it is pushed; JUMP and ROLL repeat while held;
  CROUCH and CRAWL toggle; ACT opens everything else: punch, swing, chop, hit, use with either
  hand, eat, wave, sit, die and revive. Attack punches (tap; the jabs alternate right and
  left hands) or chops (hold).
* **Fly mode** (Settings > General > Mode, or two quick taps on JUMP, as in Minecraft's
  creative mode): he takes off with a small lift and the camera goes with him. The stick flies
  along where you look (6 m/s, 11 at full stick), JUMP held rises and DOWN (a button shown only
  in flight) sinks. His arms and legs trail behind and swing with momentum from the kit's seven
  flight clips, and his body tilts into the way he is going. Flying down onto the ground lands
  him and ends the flight, as do two more quick taps on JUMP; the Mode setting follows.
* **Carrying blocks:** the block picked in the hotbar is in his hands, held out in front of
  him with both (the kit's two-handed carry pose; the block is drawn between his hands with the
  terrain's own textures and biome colours). PLACE puts it down with both hands reaching out
  (the kit's interact for the right hand and its left-hand twin for the left). Punching,
  chopping and digging put it away for a moment; an empty hotbar slot leaves his hands empty.
* **Water:** waist-deep water (0.6 to 1.3 blocks) is waded through, arms held out over it;
  deeper water is swum in at the surface (DOWN dives; underwater the stick follows where you
  look, JUMP rises and he floats back up when let go; JUMP by a ledge climbs out). Falling in
  fast goes under first.
* **Digging by touch** (building by touch on) uses the kit's mining swing: about three strikes
  a second, and a block breaks after one strike per half point of hardness (dirt 1, stone 3,
  granite 4).
* **Block rules** (Minecraft's, where the kit has none): half slabs are walked up and down
  (a step is 0.6); a full block needs a jump (1.05 high); the body is 0.6 wide and 1.8 tall, so
  a two-block-high opening is walked through; a two-block wall is climbed by jumping at it
  (the kit's ledge climb, 1.4 to 2.55 blocks). Collisions move him onto a slab at once, but the
  body and the camera ease the rest of the way (most of it in a sixth of a second), so steps up
  and down look smooth. The follow camera stops short of walls and ground.
* **The state machine** is the kit's controller ported to Rust function for function
  (`hyrig/controller.rs`): gaits blended in step, jumps from either foot chained on touchdown,
  falls and landings, linked rolls, ledge climbs, crouch, prone, upper-body actions over any
  movement, carry poses over everything, creative flight, wading, swimming and diving (each a
  blend of up to seven clips driven by a momentum spring, with a whole-body lean), held
  punching and mining, sitting, death, idle fidgets, blinking and glancing eyes and
  expressions. `tests/hyrig_parity.rs` replays the kit's recorded reference run (13 008
  frames covering every state, on foot, flying and in water) and the port matches it on every
  frame. The game adds three things, off in the kit: `snap_down` (half slabs are walked down,
  not dropped off), `fly_follow_pitch` (in flight the stick goes where you look) and the
  "interact_both" press. The block world answers the kit's water question from its water
  blocks (`BlockWorld::water_level`: the top of the water the feet are in).
* **The model** is a tree of 25 rigid parts with 77 clips (kit version 1.6: the character is
  right-handed and its bones are named for its own sides). `tools/convert_hyrig.py` turns the
  kit's glTF into `assets/hyrig/hyrig.bin` (nodes, triangles, key frames; 1.3 MB), so the game
  needs no glTF reader. Each frame `hyrig/rig.rs` blends the clips the controller asks for
  into a pose (legs from the body clips; upper body faded against the wading pose and then the
  action, the left arm against the other hand's clip when both hands interact; carry pose on
  its arm bones; head turn; face), tilts the whole body by the lean about a point at the
  hips, and hands the renderer 768 world-space vertices (36 more for a carried block); the
  shader lights them with the terrain's per-direction light.
* The model is by Powerbyte7, **CC BY-NC 4.0: credit required, no commercial use**
  (`assets/hyrig/CREDITS.md`).

## Clouds and god rays (`app/src/clouds.rs`, the shader)

* **Clouds** as Vintage Story draws them: the sky is divided into 32-block tiles, and each tile
  holding cloud is a flat-bottomed box (base at y 290) as tall as the cloud is dense there, a
  little see-through. Density is noise fixed in "cloud space", which drifts with the wind; a
  broad second noise leaves some regions clear and others overcast.
* **Cloud levels of detail:** 32-block tiles around the camera, then rings of 64, 128 and
  256-block tiles, each ring 40 cells across with the finer level in its middle half, so every
  ring costs the same. Clouds reach 4.8 km (they used to stop at 1.7 km) with about 4 000-6 000
  quads (one 32-block grid took 10 000 for 1.7 km); far away a big tile is a few pixels, and
  coarse tiles drop the fine detail noise so they don't flicker. The set is meshed on a
  background thread in about 1.5 ms (tiles of one height share faces) when the camera crosses
  a 256-block line in cloud space, and drifts with the wind in between. One draw call:
  bottoms and tops first and sides far to near, so each pixel of cloud is shaded once or
  twice; faces turned away from the camera are culled, and the sky's haze colour is only
  looked up for distant clouds. They are lit by the sky and the sun, glow toward the sun at
  sunset, and fade into the horizon haze.
* **God rays** in screen space (the method of GPU Gems 3, chapter 13): after a frame, a
  quarter-resolution pass writes a mask of open sky near the sun, read from the depth buffer
  (terrain and clouds block it); the next frame smears that mask away from the sun's place on
  screen with 12 samples per pixel and adds it on top. Both passes are skipped while the sun is
  down or out of view, and the depth buffer is only kept when they run.
* The Clouds and God Rays switches (Settings > Video) turn each off.
* The block texture array never has a multiple of six layers (`gfx::array_layers`): on OpenGL
  ES, wgpu would make it a cube map and every block would show the wrong texture.

## Greedy meshing (`greedy.rs`)

A hybrid bit-mask greedy mesher, written from the published "binary greedy meshing" idea
(cgerikj's repository, MIT, and the 0fps article). Full opaque cubes are found with one 64-bit
integer per column, visible faces with shifts and masks, and same-texture faces merge into
rectangles. Slabs, water and leaves keep using the per-face mesher, and three occupancy masks
(full / bottom-anchored / top-anchored) reproduce the slab hiding rules exactly. Faces merge only
when their per-corner ambient occlusion is identical and unchanged along the merge direction, which
is lossless: a straight cliff foot merges along the cliff, a diagonally shaded face stays a unit quad.
(The first version merged only fully lit faces and saved just ~11% of quads on terraced terrain,
because nearly every terrace face touches ground and is shaded.) Merged quads are flagged so the
shader grows them by 0.002 blocks, hiding T-junction cracks.

The tests prove the greedy and simple meshers cover exactly the same unit faces (random blocky
scenes, flat plates, slab neighbours and real generated terrain) and that every triangle is
wound outward. Settings > Performance > Mesher switches between them at runtime (default Simple until it has
been checked on a phone); the overlay's QUADS line shows the result. The headless demo prints
quad counts and meshing time for both.

The per-face path no longer looks at every cell: it first builds one 64-bit mask per row along x
(which cells hold a block, which are full height, which hide any face touching them) and visits
only blocks that are not full height or are not walled in on all six sides, which skips deep
stone and air in a few instructions per row. On the build machine that made the SIMPLE mesher
about 4x faster (685 chunks around the spawn: 517 ms before, 122 ms after; same quads, checked by
a test against a plain per-face look on random scenes).

Not yet done: one instance per quad (8 bytes instead of 32). Held back on purpose: drawing
instances of 4 vertices is slow on some phone GPUs (vertex work is shaded in batches, and tiny
instances waste most of each batch), so it should be tried on a phone first, behind a switch.
Also texture repeat across merged quads.

## Planned next


1. Flip the default mesher to Greedy once it has been checked on a phone (the setting is
   now remembered, so switching it once is enough to keep testing it); then try one instance
   per quad (8 bytes) on a phone. (The faster per-face scan is done.)
2. Measure on a phone what occlusion culling, the vertex pool and the faster scan buy (CULL
   All vs No Caves, the GPU and MAIN THREAD lines), and check immersive mode on a phone with a
   camera cut-out and one with 3-button navigation.
