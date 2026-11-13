//! blockforge: the headless core of a mobile-first voxel engine.
//!
//! Layout (see README.md for the architecture diagram):
//!   coords / block / registry / chunk   data model (32^3 cubic chunks, shape bits, palette storage)
//!   worldgen / world / save             deterministic terrain + edit journal + checksummed saves
//!   mesh / lod                          packed 8-byte vertices, padded-snapshot meshing, octree LOD select
//!   behavior / raycast                  event-driven blocks, voxel DDA
//!   protocol / server / client / jobs   client/server split, fixed ticks, priority worker pool
//!   tick / governor                     fixed timestep, adaptive quality for phones

pub mod behavior;
pub mod biome;
pub mod block;
pub mod chunk;
pub mod client;
pub mod climate;
pub mod coords;
pub mod governor;
pub mod greedy;
pub mod jobs;
pub mod lod;
pub mod mesh;
pub mod noise;
pub mod protocol;
pub mod raycast;
pub mod registry;
pub mod rng;
pub mod save;
pub mod server;
pub mod terrain;
pub mod tick;
pub mod world;
pub mod worldgen;
