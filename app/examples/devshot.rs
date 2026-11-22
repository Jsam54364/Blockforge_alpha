//! Runs the real game on a desktop and saves a picture (see `blockforge_app::run_desktop`).
//! `BLOCKFORGE_SEED=1337 BLOCKFORGE_HOURS=8 BLOCKFORGE_SHOT=out.png cargo run --example devshot`
fn main() {
    blockforge_app::run_desktop();
}
