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
3. **Greedy meshing**: currently one quad per visible face. Top faces merge well.
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

Measured on a mid-range phone, fog off, flying over terrain (GPU time of the main pass):
render distance 6 = 7.1 ms (43% busy), 8 = 11.7 ms (70%), 10 = 12.9 ms (77%), 12 = 16.7 ms
(94%, 56 fps). CPU stayed near 5% of the device; memory 100-220 MB. The renderer is GPU-bound,
so the next wins after frustum culling are occlusion culling, then LOD meshes.

Renderer gaps: pooled vertex buffers, occlusion culling, texture array
(colours come from a palette keyed by texture name), render-scale governor hookup, LOD meshing.
