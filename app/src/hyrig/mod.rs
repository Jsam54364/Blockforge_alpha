//! The HyRig character: the kit's movement and animation state machine, ported to Rust.
//!
//!   * `config`     the state machine's numbers, read from assets/hyrig/hyrig_controller.json
//!   * `controller` the state machine itself: input and three world queries in; position,
//!     facing, which clips to show (time and weight), head turn, face and camera out. A port
//!     of the kit's hyrig_controller.gd / .js, checked frame by frame against the kit's
//!     recorded reference run (tests/hyrig_parity.rs)
//!   * `world`      the three world queries: for a list of boxes (the kit's, for the parity
//!     test) and for the game's blocks
//!   * `rig`        the model and its 51 clips (assets/hyrig/hyrig.bin, made by
//!     tools/convert_hyrig.py); samples and blends clips into a pose and a triangle list
//!
//! Units are metres (= blocks), seconds, radians. Yaw 0 faces +Z.
//! The model is by Powerbyte7, CC BY-NC 4.0 (see assets/hyrig/CREDITS.md).

pub mod config;
pub mod controller;
pub mod rig;
pub mod world;

pub use config::Config;
pub use controller::{Controller, Input, Output, View};
pub use rig::Rig;
pub use world::{BlockWorld, Blocks, BoxWorld, World};
