//! Event-driven block behaviour.
//!
//! Blocks never get special cases in the engine. They declare a behaviour name in
//! `blocks.toml`; behaviours react to events (placed, broken, neighbour changed,
//! scheduled tick) and answer with *commands*, not direct world mutation. Commands
//! are applied by the server in a fixed order, which keeps the simulation
//! deterministic, makes budgeting trivial, and means every change flows through the
//! same path that feeds the journal and network sync.

use std::collections::HashMap;
use std::sync::Arc;

use crate::block::{BlockState, AIR};
use crate::coords::BlockPos;
use crate::registry::BlockRegistry;
use crate::rng::Rng;
use crate::world::World;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockEvent {
    Placed(BlockState),
    /// Carries the state the block had before removal.
    Broken(BlockState),
    NeighborChanged(BlockState),
    Tick(BlockState),
}

impl BlockEvent {
    pub fn state(self) -> BlockState {
        match self {
            BlockEvent::Placed(s)
            | BlockEvent::Broken(s)
            | BlockEvent::NeighborChanged(s)
            | BlockEvent::Tick(s) => s,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Command {
    SetBlock { pos: BlockPos, state: BlockState },
    /// Run `on_tick` for the block at `pos` after `delay` server ticks (minimum 1).
    ScheduleTick { pos: BlockPos, delay: u32 },
}

/// What a behaviour can see and do. The world is read-only here on purpose.
pub struct BehaviorCtx<'a> {
    pub world: &'a World,
    pub rng: &'a mut Rng,
    pub tick: u64,
    out: &'a mut Vec<Command>,
}

impl<'a> BehaviorCtx<'a> {
    pub fn new(world: &'a World, rng: &'a mut Rng, tick: u64, out: &'a mut Vec<Command>) -> Self {
        BehaviorCtx { world, rng, tick, out }
    }

    pub fn block(&self, pos: BlockPos) -> Option<BlockState> {
        self.world.get_block(pos)
    }

    pub fn is_solid(&self, state: BlockState) -> bool {
        self.world.registry.get(state).solid
    }

    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) {
        self.out.push(Command::SetBlock { pos, state });
    }

    pub fn schedule(&mut self, pos: BlockPos, delay: u32) {
        self.out.push(Command::ScheduleTick { pos, delay });
    }
}

pub trait BlockBehavior: Send + Sync {
    fn on_placed(&self, _ctx: &mut BehaviorCtx<'_>, _pos: BlockPos, _state: BlockState) {}
    fn on_broken(&self, _ctx: &mut BehaviorCtx<'_>, _pos: BlockPos, _state: BlockState) {}
    fn on_neighbor_changed(&self, _ctx: &mut BehaviorCtx<'_>, _pos: BlockPos, _state: BlockState) {}
    fn on_tick(&self, _ctx: &mut BehaviorCtx<'_>, _pos: BlockPos, _state: BlockState) {}
}

/// Gravity block (sand, gravel): falls one cell per two ticks until it lands.
/// Each hop re-triggers the next through events, so long falls are naturally budgeted.
pub struct Falling;

impl BlockBehavior for Falling {
    fn on_placed(&self, ctx: &mut BehaviorCtx<'_>, pos: BlockPos, _state: BlockState) {
        ctx.schedule(pos, 1);
    }

    fn on_neighbor_changed(&self, ctx: &mut BehaviorCtx<'_>, pos: BlockPos, _state: BlockState) {
        ctx.schedule(pos, 1);
    }

    fn on_tick(&self, ctx: &mut BehaviorCtx<'_>, pos: BlockPos, state: BlockState) {
        let below = pos.offset(0, -1, 0);
        // Unloaded chunk below: stay put rather than falling into the void.
        if let Some(b) = ctx.block(below) {
            if b.is_air() || !ctx.is_solid(b) {
                ctx.set_block(pos, AIR);
                ctx.set_block(below, state);
            }
        }
    }
}

/// Resolved once at startup: block id -> behaviour. No string lookups at runtime.
pub struct BehaviorTable {
    by_id: Vec<Option<Arc<dyn BlockBehavior>>>,
}

impl BehaviorTable {
    pub fn build(
        reg: &BlockRegistry,
        named: &HashMap<String, Arc<dyn BlockBehavior>>,
    ) -> Result<Self, String> {
        let mut by_id = Vec::with_capacity(reg.len());
        for id in 0..reg.len() {
            let def = reg.def(id as u16);
            match &def.behavior {
                None => by_id.push(None),
                Some(name) => match named.get(name) {
                    Some(b) => by_id.push(Some(b.clone())),
                    None => {
                        return Err(format!("block '{}' uses unknown behavior '{}'", def.name, name))
                    }
                },
            }
        }
        Ok(BehaviorTable { by_id })
    }

    pub fn builtin(reg: &BlockRegistry) -> Result<Self, String> {
        let mut named: HashMap<String, Arc<dyn BlockBehavior>> = HashMap::new();
        named.insert("falling".to_string(), Arc::new(Falling));
        Self::build(reg, &named)
    }

    pub fn get(&self, id: u16) -> Option<Arc<dyn BlockBehavior>> {
        self.by_id.get(id as usize).and_then(|o| o.clone())
    }

    pub fn has(&self, id: u16) -> bool {
        matches!(self.by_id.get(id as usize), Some(Some(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::WorldGen;

    #[test]
    fn builtin_table_resolves_sand() {
        let reg = BlockRegistry::builtin();
        let t = BehaviorTable::builtin(&reg).unwrap();
        assert!(t.has(reg.id("sand").unwrap()));
        assert!(!t.has(reg.id("stone").unwrap()));
    }

    #[test]
    fn unknown_behavior_is_an_error() {
        let reg = BlockRegistry::from_toml("[[block]]\nname = \"air\"\n[[block]]\nname = \"x\"\nbehavior = \"nope\"\n").unwrap();
        assert!(BehaviorTable::builtin(&reg).is_err());
    }

    #[test]
    fn falling_emits_move_commands_over_air() {
        let reg = Arc::new(BlockRegistry::builtin());
        let g = Arc::new(WorldGen::new(1, &reg));
        let mut w = World::new(reg.clone(), g.clone());
        let cp = crate::coords::ChunkPos::new(0, 10, 0); // y 320..351: always sky (peaks stay under ~200)
        let col = w.column(0, 0);
        w.insert_generated(cp, g.generate_chunk(cp, &col));
        let pos = BlockPos::new(1, 10 * 32 + 10, 1);
        let sand = reg.state("sand").unwrap();
        let mut rng = Rng::new(0);
        let mut out = Vec::new();
        {
            let mut ctx = BehaviorCtx::new(&w, &mut rng, 0, &mut out);
            Falling.on_tick(&mut ctx, pos, sand);
        }
        assert_eq!(
            out,
            vec![
                Command::SetBlock { pos, state: AIR },
                Command::SetBlock { pos: pos.offset(0, -1, 0), state: sand }
            ]
        );
    }
}
