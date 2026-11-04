# blockforge

Headless core of a mobile-first voxel engine in Rust. **Std-only, zero dependencies.**
No renderer yet: this is the part that decides performance and architecture, built so a
`wgpu` client can be dropped on top.

> **Status: written but never compiled.** It was produced in a sandbox with no Rust
> toolchain and no network. Expect a handful of compile errors to fix on first build.
> Run `cargo test` first; the tests are the spec for each module.

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
