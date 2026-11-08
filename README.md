# blockforge

Headless core of a mobile-first voxel engine in Rust. **Std-only, zero dependencies.**
No renderer yet: this is the part that decides performance and architecture, built so a
`wgpu` client can be dropped on top.

> **Status:** the core compiles and all 61 unit + 3 end-to-end tests pass on GitHub Actions.
> The `app/` front-end (renderer, touch controls, APK packaging) has been written but not yet
> built; expect to fix a few compile errors on its first CI run.

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
| `worldgen` | Deterministic terrain, pure function of (seed, chunk). Heights in **quarter-blocks**, so the surface ends in **half-slabs**. Caves, sea level, bedrock, `Column` heightmap layer |
| `world`, `save` | Procedural base + **edit journal** (undo/redo, tiny saves, delta sync). Versioned + CRC32 formats, atomic write-then-rename |
| `mesh` | Padded 34³ snapshot (no locks), **8-byte packed vertices**, shape-aware face culling, per-vertex AO, 3 render layers, index-free quads |
| `lod` | Watertight **octree LOD selection** (detail rings double in size) |
| `behavior` | Event-driven blocks emitting commands (deterministic, budgeted). Example: falling sand |
| `server`, `client`, `protocol` | **Client/server split even in singleplayer**; sim distance independent of view distance |
| `jobs` | Priority worker pool, thread count capped at 3 for big.LITTLE phones |
| `tick`, `governor`, `rng`, `raycast` | Fixed timestep, adaptive render-scale/LOD governor, deterministic RNG, voxel DDA |

## Known gaps (deliberate, in rough priority order)

1. **Renderer**: `wgpu` + `winit`, pooled vertex buffers, one shared static index buffer
   (`mesh::quad_indices`), opaque → cutout → translucent, ASTC/ETC2 texture array, fog.
2. **Lighting**: vertices carry sky/block light fields but values are placeholders.
3. **Greedy meshing**: implemented as a hybrid (see below) but not the default mesher yet.
4. **LOD meshing**: `lod::select` picks nodes; downsampled meshing + seam handling is not written.
5. **Server**: no reach/anti-cheat checks, undo history is global not per-player, autosave
   blocks the tick, events outside sim distance are dropped instead of re-scanned.
6. **Client**: AO at chunk edges/corners isn't refreshed when only a diagonal neighbour arrives;
   chunk meshes wait for nothing (could delay until the 6 face neighbours are loaded).
7. **Saves**: block ids are numeric and append-only; add a name→id table to the header.
8. **Determinism**: worldgen uses `f32`; fine on one platform, switch to fixed-point if you
   need bit-identical worlds across ARM/x86 for multiplayer.
9. `JobPool` doesn't survive a panicking job (see note in `jobs.rs`).

## Tuning knobs worth measuring on a real phone first

`CHUNK_BITS` (4 vs 5), `ServerConfig::{sim_distance_chunks, vertical_view_chunks,
max_chunk_sends_per_tick}`, `Client::mesh_budget`, worker count, cave thresholds in `worldgen`.

## Android app (`app/`)

A separate crate (its own workspace root, so the core stays dependency-free): winit + wgpu 22.x,
NativeActivity, packaged with `cargo-apk`. Controls: left 40% of the screen is a virtual stick
(fly where you look); right side drags to look, quick tap breaks the block under the crosshair,
press-and-hold places stone.

CI flow (GitHub Actions, all from a phone):
1. Upload a zip to the repo root -> `CI` unpacks it, runs the core tests.
2. When `CI` succeeds, `Android APK` runs: host type-check + WGSL validation, then `cargo apk build`.
3. The APK is published to the repo's **Releases** page as `blockforge-debug.apk`.

### HUD and settings

Top-left overlay (toggle in the menu): FPS and frame time, this app's CPU use (from
`/proc/self/stat`, so it is the app's share, not the whole phone), CPU time per frame
(excluding the wait for the display), GPU time (timestamp queries; shows N/A on devices
without them) with a derived busy%, memory (VmRSS) and chunk/mesh counts. Android exposes no
public GPU-usage percentage, hence the derived figure.

Top-right menu button: render distance (2-12 chunks, applied to the server stream too),
LOD distance (stored and shown but **not used yet**: LOD meshing isn't built), fog on/off,
stats on/off. The HUD is drawn by a tiny immediate-mode layer (`ui.rs`) with its own 5x7 bitmap
font (`font.rs`, `glyphs.rs`), so there are no UI dependencies.

Back-face culling is on for solid terrain (water stays double-sided so the surface is visible
from underwater). Tests prove every face is wound counter-clockwise from outside, both in the
mesher and after projection (`mesh.rs`, `gfx.rs`). Flying inside solid rock therefore looks
see-through, as in any voxel game.

The HUD keeps clear of Android's status bar (top) and navigation bar (right, in landscape) using
fixed insets measured from a real phone (`SAFE_TOP_DP`, `SAFE_RIGHT_DP` in `ui.rs`). Immersive
mode or real inset queries would replace these.

Culling: back faces (solid terrain only) and a real six-plane frustum test per chunk bounding
box (`math::Frustum`, tested to never reject a visible box). The menu's CULL row cycles
ALL / FRUST / BACK / OFF so each can be measured on a device; the overlay's DRAWN line shows
how many chunk meshes survived culling out of those resident.

Frustum culling, same camera at each distance (render distance: drawn / meshed chunks):
6: 94/458, 8: 156/602, 10: 230/706, 12: 315/985, i.e. about 21-33% of meshed chunks. GPU
milliseconds at a vsync-capped 60 fps are NOT a reliable cost measure (the GPU clocks down to
fill the frame), so use the menu's VSYNC toggle to read uncapped FPS when comparing settings.
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
so the next wins after frustum culling are occlusion culling, then LOD meshes.

Renderer gaps: pooled vertex buffers, occlusion culling, texture array
(colours come from a palette keyed by texture name), render-scale governor hookup, LOD meshing.

## Greedy meshing (`greedy.rs`)

A hybrid bit-mask greedy mesher, written from the published "binary greedy meshing" idea
(cgerikj's repository, MIT, and the 0fps article). Full opaque cubes are found with one 64-bit
integer per column, visible faces with shifts and masks, and same-texture faces merge into
rectangles. Slabs, water and leaves keep using the per-face mesher, and three occupancy masks
(full / bottom-anchored / top-anchored) reproduce the slab hiding rules exactly. Only fully lit
faces merge, because a merged quad can't hold per-corner ambient occlusion. Merged quads are
flagged so the shader grows them by 0.002 blocks, hiding T-junction cracks.

The tests prove the greedy and simple meshers cover exactly the same unit faces (random blocky
scenes, flat plates, slab neighbours and real generated terrain) and that every triangle is
wound outward. The menu's MESH row switches between them at runtime (default SIMPLE until it has
been checked on a phone); the overlay's QUADS line shows the result. The headless demo prints
quad counts and meshing time for both.

Not yet done: iterate only the non-eligible voxels in the per-face path (it still scans all 32^3),
draw one instance per quad (8 bytes instead of 32), texture repeat across merged quads.

## Planned next


1. Flip the default mesher to GREEDY once it has been checked on a phone; then one instance per
   quad (8 bytes) and a faster scan for the per-face path.
2. Occlusion culling (cave-style flood fill) to skip chunks hidden behind terrain, which should
   help most underground.
3. LOD meshes behind the (currently inert) LOD distance setting.
4. Pooled vertex buffers instead of one buffer per chunk.
5. Immersive mode / real window insets instead of the fixed HUD insets.
6. A fixed signing key in CI so new APKs install over old ones without uninstalling.
