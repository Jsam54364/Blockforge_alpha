//! The fighting in the world: the player's fighter, the creatures (each a HyRig body with a
//! brain and a fighter, `blockforge::rpg`), spells in flight, sparks, items drawn in hands,
//! and what the HUD shows of it all.
//!
//! Items in hands are their 32 x 32 sprite given one texel of thickness (an outline of side
//! faces around the solid pixels), at the game's 32 texels to the metre, the grip in the
//! fist. Everything here is drawn with the character's pipeline, told apart by its texture
//! coordinates (see `fs_char` in the shader):
//!   creatures  u + 1.5 x skin, and v + 2 x their hit flash (0..3)
//!   items      u + 16, v + 2 x the sprite's texture layer (alpha-tested)
//!   sparks     u + 24, their colour in the normal (unlit, glowing)

use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::Arc;

use blockforge::client::Client;
use blockforge::coords::BlockPos;
use blockforge::registry::BlockRegistry;
use blockforge::rpg::ai::{self, Brain, Senses};
use blockforge::rpg::creatures::{CreatureDef, CREATURES};
use blockforge::rpg::fighter::{Command, Event, Fighter};
use blockforge::rpg::items::{Item, ItemBook, Weapon, WeaponClass};
use blockforge::rpg::spells::{Bolt, SpellKind, SPELLS};
use blockforge::rpg::{Hit, Outcome, Status, Vitals};

use crate::hyrig::rig::Vertex;
use crate::hyrig::{BlockWorld, Config, Controller, Input, Rig, View};
use crate::player::{hands_of, ClientBlocks, Hand, Player};

/// One texel: the game's 32 to the metre.
const TEXEL: f32 = 1.0 / 32.0;
/// A body's radius and height (the player's; creatures scale them).
const BODY_R: f32 = 0.35;
const BODY_H: f32 = 1.8;
/// Creatures come no closer than this to the player when they appear, and no further than
/// the next; they go when this far away (or dead for `CORPSE_TIME`).
const SPAWN_NEAR: f32 = 24.0;
const SPAWN_FAR: f32 = 42.0;
const DESPAWN: f32 = 80.0;
const CORPSE_TIME: f32 = 7.0;
/// How long the player lies dead before getting up again at the spawn.
pub const RESPAWN_AFTER: f32 = 5.0;
/// The first creatures wait this long after the world opens.
const FIRST_SPAWN: f32 = 12.0;

/// How fierce the world is: no creatures, some, many.
pub const DIFFICULTIES: [&str; 3] = ["Peaceful", "Normal", "Hard"];

/// What the hotbar's picked slot holds, as far as fighting goes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Held {
    Nothing,
    Block,
    Weapon(Weapon),
    Staff(blockforge::rpg::items::Staff),
    Potion(blockforge::rpg::items::Potion),
    Armor,
}

/// A vertex of an item's mesh, in sprite metres (x right, y up, z out of the picture).
#[derive(Copy, Clone)]
struct LocalVert {
    p: [f32; 3],
    n: [f32; 3],
    uv: [f32; 2],
}

pub struct Creature {
    pub id: u32,
    pub def: &'static CreatureDef,
    ctl: Controller,
    brain: Brain,
    pub fighter: Fighter,
    weapon: u16,
    pub pos: [f32; 3],
    yaw: f32,
    sees: bool,
    look_wait: f32,
    hurt: bool,
    presses: Vec<String>,
    roll: f32,
    dead_for: f32,
    out: Option<crate::hyrig::Output>,
}

impl Creature {
    fn centre(&self) -> [f32; 3] {
        [self.pos[0], self.pos[1] + BODY_H * 0.55 * self.def.scale, self.pos[2]]
    }

    /// Whether a point is inside its body.
    fn contains(&self, p: [f32; 3], pad: f32) -> bool {
        let s = self.def.scale;
        let (dx, dz) = (p[0] - self.pos[0], p[2] - self.pos[2]);
        dx * dx + dz * dz < (BODY_R * s + pad).powi(2) && p[1] > self.pos[1] - pad && p[1] < self.pos[1] + BODY_H * s + pad
    }
}

/// A glowing speck: a bolt's trail, a burst, a heal's sparkle, a lightning arc.
struct Spark {
    pos: [f32; 3],
    vel: [f32; 3],
    colour: [f32; 3],
    life: f32,
    max: f32,
    size: f32,
    gravity: f32,
}

/// A number or word floating up from where a blow landed.
pub struct Floater {
    pub pos: [f32; 3],
    pub text: String,
    pub colour: [u8; 4],
    pub age: f32,
}

/// What a frame needs from the game.
pub struct Frame<'a> {
    pub client: &'a Client,
    /// Where the player aims (the camera's forward) and from where (his eyes).
    pub aim: [f32; 3],
    pub eyes: [f32; 3],
    pub night: bool,
    pub difficulty: u8,
    /// In creative flight: untouchable, and the creatures lose interest.
    pub flying: bool,
}

/// What came of a frame that the game must act on.
#[derive(Default, Debug, PartialEq)]
pub struct Report {
    /// The player should roll now (a dodge that had the stamina).
    pub roll: bool,
    /// Dead long enough: put him back at the spawn.
    pub respawn: bool,
    /// A swing of his landed on nothing (the game may break the block he aims at).
    pub missed: bool,
}

pub struct Combat {
    reg: Arc<BlockRegistry>,
    pub book: ItemBook,
    cfg: Config,
    rig: Rig,
    pub player: Fighter,
    pub creatures: Vec<Creature>,
    bolts: Vec<Bolt>,
    sparks: Vec<Spark>,
    pub floaters: Vec<Floater>,
    meshes: HashMap<u16, Vec<LocalVert>>,
    verts: Vec<Vertex>,
    next_id: u32,
    spawn_wait: f32,
    seed: u32,
    /// Seconds the player has been dead.
    pub dead_for: Option<f32>,
    /// Counts down after the player is hurt (the screen's edge reddens).
    pub hurt: f32,
    held: u16,
    /// A short line for the HUD ("Not enough stamina"), and how long it has shown.
    pub message: Option<(String, f32)>,
}

impl Combat {
    pub fn new(reg: Arc<BlockRegistry>, seed: u64) -> Combat {
        let book = ItemBook::from_registry(&reg).unwrap_or_else(|e| {
            eprintln!("blockforge: items: {e}");
            ItemBook::default()
        });
        let cfg = Config::builtin();
        let rig = Rig::builtin(&cfg);
        Combat {
            reg,
            book,
            cfg,
            rig,
            player: Fighter::player(),
            creatures: Vec::new(),
            bolts: Vec::new(),
            sparks: Vec::new(),
            floaters: Vec::new(),
            meshes: HashMap::new(),
            verts: Vec::new(),
            next_id: 1,
            spawn_wait: FIRST_SPAWN,
            seed: (seed as u32 ^ (seed >> 32) as u32) | 1,
            dead_for: None,
            hurt: 0.0,
            held: 0,
            message: None,
        }
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// What item `id` is, for fighting.
    pub fn held_kind(&self, id: u16) -> Held {
        if id == 0 {
            return Held::Nothing;
        }
        match self.book.get(id) {
            Some(Item::Weapon(w)) => Held::Weapon(*w),
            Some(Item::Staff(s)) => Held::Staff(*s),
            Some(Item::Potion(p)) => Held::Potion(*p),
            Some(Item::Armor(_)) => Held::Armor,
            None if self.reg.def(blockforge::block::BlockState(id).id()).model == blockforge::registry::Model::Item => Held::Armor,
            None => Held::Block,
        }
    }

    /// The hotbar's picked item changed (or not): the fighter wields it, and the armour worn
    /// (the equipment's four slots, head to feet) protects him.
    pub fn equip(&mut self, held: u16, worn: [u16; 4]) {
        if held != self.held {
            self.held = held;
            if self.player.guarding() {
                self.player.command(Command::GuardDown);
            }
        }
        self.player.weapon = match self.held_kind(held) {
            Held::Weapon(w) => w,
            _ => Weapon::fists(),
        };
        let mut d = blockforge::rpg::Defense::default();
        let mut weight = 0.0;
        for (slot, &id) in worn.iter().enumerate() {
            if let Some(Item::Armor(a)) = self.book.get(id) {
                if a.slot == slot {
                    d.add(&a.defense);
                    weight += a.weight;
                }
            }
        }
        self.player.worn = d;
        self.player.weight = weight;
    }

    /// The carry pose for what he holds (the guard holds a weapon up in both hands).
    pub fn hold_pose(&self, held: Held) -> &'static str {
        match held {
            _ if self.player.guarding() => "two_handed",
            Held::Weapon(w) if w.two_handed => "two_handed",
            Held::Weapon(_) | Held::Staff(_) | Held::Potion(_) | Held::Armor => "one_handed",
            Held::Nothing | Held::Block => "none",
        }
    }

    fn say(&mut self, s: &str) {
        self.message = Some((s.to_string(), 0.0));
    }

    /// A command from the player's buttons; the body acts it out.
    pub fn command(&mut self, c: Command, body: &mut Player) -> Report {
        let mut report = Report::default();
        if self.dead_for.is_some() {
            return report;
        }
        let ev = self.player.command(c);
        self.player_events(ev, body, &mut report, None);
        report
    }

    /// Whether a creature stands within `reach` in front of the player (a punch then hits
    /// it rather than the block behind).
    pub fn foe_in_front(&self, at: [f32; 3], aim: [f32; 3], reach: f32) -> bool {
        let f = flat(aim);
        self.creatures.iter().any(|c| c.fighter.alive() && in_arc(at, f, c, reach, 80.0))
    }

    fn player_events(&mut self, ev: Vec<Event>, body: &mut Player, report: &mut Report, frame: Option<&Frame>) {
        for e in ev {
            match e {
                Event::Anim(a) => body.press(a),
                Event::Roll => report.roll = true,
                Event::Strike { hit, reach, arc, .. } => {
                    let p = body.position();
                    let at = [p[0] as f32, p[1] as f32, p[2] as f32];
                    if !self.strike_creatures(at, &hit, reach, arc, body) {
                        report.missed = true;
                    }
                }
                Event::Spell { spell, power } => {
                    if let Some(fr) = frame {
                        self.cast(0, spell, power, fr.eyes, fr.aim, fr.client);
                    }
                }
                Event::Drank(_) => self.sparkle(body_centre(body), [1.0, 0.5, 0.5], 10),
                Event::TooTired => self.say("Too tired"),
                Event::NoMana => self.say("Not enough mana"),
                Event::Staggered | Event::Died => {}
            }
        }
    }

    /// The player's swing lands: everyone in its arc is hit. Returns whether it hit anyone.
    fn strike_creatures(&mut self, at: [f32; 3], hit: &Hit, reach: f32, arc: f32, body: &mut Player) -> bool {
        let f = self.player.facing;
        let mut any = false;
        for i in 0..self.creatures.len() {
            let c = &self.creatures[i];
            if !c.fighter.alive() || !in_arc(at, f, c, reach, arc) {
                continue;
            }
            any = true;
            let mut h = hit.clone();
            h.dir = flat([c.pos[0] - at[0], 0.0, c.pos[2] - at[2]]);
            let (o, ev) = self.creatures[i].fighter.take(&h);
            self.creature_hurt(i, &o, ev);
            if o.parried {
                // His swing was parried: he reels, open to a riposte.
                let ev = self.player.stagger(1.0);
                for e in ev {
                    if let Event::Anim(a) = e {
                        body.press(a);
                    }
                }
                let p = self.creatures[i].centre();
                self.float(p, "Parried!", [255, 220, 120, 255]);
            }
        }
        any
    }

    fn creature_hurt(&mut self, i: usize, o: &Outcome, ev: Vec<Event>) {
        let c = &mut self.creatures[i];
        c.hurt = true;
        for e in ev {
            if let Event::Anim(a) = e {
                c.presses.push(a.to_string());
            }
        }
        let head = [c.pos[0], c.pos[1] + BODY_H * c.def.scale + 0.2, c.pos[2]];
        if o.dodged {
            self.float(head, "Dodged", [200, 200, 200, 255]);
        } else if o.parried {
        } else if o.damage > 0.0 {
            let colour = if o.blocked { [170, 180, 200, 255] } else if o.staggered { [255, 200, 90, 255] } else { [255, 255, 255, 255] };
            self.float(head, &format!("{}", o.damage.round().max(1.0) as i32), colour);
        }
        if o.killed {
            let p = self.creatures[i].centre();
            self.burst(p, [0.9, 0.85, 0.8], 14, 3.0);
        }
    }

    /// The player takes a hit.
    fn hurt_player(&mut self, hit: &Hit, body: &mut Player) -> Outcome {
        let (o, ev) = self.player.take(hit);
        for e in ev {
            match e {
                Event::Anim(a) => body.press(a),
                Event::Died => self.dead_for = Some(0.0),
                _ => {}
            }
        }
        if o.damage > 0.0 && !o.blocked {
            self.hurt = 0.6;
            body.press("hit");
        } else if o.blocked && !o.parried {
            self.hurt = self.hurt.max(0.2);
        }
        let p = body_centre(body);
        if o.parried {
            self.float([p[0], p[1] + 0.9, p[2]], "Parry!", [255, 220, 120, 255]);
            self.burst([p[0], p[1] + 0.3, p[2]], [1.0, 0.9, 0.5], 8, 2.5);
        } else if o.guard_broken {
            self.float([p[0], p[1] + 0.9, p[2]], "Guard broken", [255, 140, 90, 255]);
        }
        o
    }

    fn float(&mut self, pos: [f32; 3], text: &str, colour: [u8; 4]) {
        self.floaters.push(Floater { pos, text: text.to_string(), colour, age: 0.0 });
    }

    fn burst(&mut self, at: [f32; 3], colour: [f32; 3], n: usize, speed: f32) {
        for _ in 0..n {
            let (a, b) = (self.rand() * 2.0 * PI, self.rand() * PI - PI * 0.5);
            let s = speed * (0.4 + self.rand() * 0.8);
            let vel = [a.cos() * b.cos() * s, b.sin().abs() * s + 1.0, a.sin() * b.cos() * s];
            let max = 0.35 + self.rand() * 0.35;
            self.sparks.push(Spark { pos: at, vel, colour, life: max, max, size: 0.07, gravity: 6.0 });
        }
    }

    fn sparkle(&mut self, at: [f32; 3], colour: [f32; 3], n: usize) {
        for _ in 0..n {
            let a = self.rand() * 2.0 * PI;
            let r = 0.3 + self.rand() * 0.3;
            let pos = [at[0] + a.cos() * r, at[1] - 0.8 + self.rand() * 1.4, at[2] + a.sin() * r];
            let max = 0.6 + self.rand() * 0.6;
            let up = 0.9 + self.rand();
            self.sparks.push(Spark { pos, vel: [0.0, up, 0.0], colour, life: max, max, size: 0.05, gravity: 0.0 });
        }
    }

    /// A spell goes off, cast by `owner` (0: the player) from `from` toward `dir`.
    fn cast(&mut self, owner: u32, spell: usize, power: f32, from: [f32; 3], dir: [f32; 3], client: &Client) {
        let s = &SPELLS[spell];
        match s.kind {
            SpellKind::Bolt { speed, .. } => {
                let pos = [from[0] + dir[0] * 0.7, from[1] + dir[1] * 0.7 - 0.15, from[2] + dir[2] * 0.7];
                self.bolts.push(Bolt { spell, pos, vel: [dir[0] * speed, dir[1] * speed + 1.0, dir[2] * speed], power, owner, age: 0.0 });
            }
            SpellKind::Arc { range, chain, .. } => {
                let Some(hit) = s.hit(power) else { return };
                let mut points = vec![from];
                if owner == 0 {
                    // The first creature along the line of sight, then the nearest others.
                    let mut best: Option<(usize, f32)> = None;
                    for (i, c) in self.creatures.iter().enumerate() {
                        if !c.fighter.alive() {
                            continue;
                        }
                        let v = sub(c.centre(), from);
                        let along = dot(v, dir);
                        let off = len(sub(v, scale(dir, along)));
                        if along > 0.0 && along < range && off < 0.9 * c.def.scale + along * 0.04 && best.map_or(true, |(_, b)| along < b) && clear(client, from, c.centre()) {
                            best = Some((i, along));
                        }
                    }
                    let mut struck: Vec<usize> = Vec::new();
                    let mut at = best.map(|b| b.0);
                    while let Some(i) = at {
                        struck.push(i);
                        points.push(self.creatures[i].centre());
                        let mut h = hit.clone();
                        h.dir = flat(sub(self.creatures[i].pos, from));
                        let (o, ev) = self.creatures[i].fighter.take(&h);
                        self.creature_hurt(i, &o, ev);
                        if struck.len() > chain as usize {
                            break;
                        }
                        let here = self.creatures[i].centre();
                        at = self
                            .creatures
                            .iter()
                            .enumerate()
                            .filter(|(j, c)| !struck.contains(j) && c.fighter.alive() && len(sub(c.centre(), here)) < 8.0)
                            .min_by(|a, b| len(sub(a.1.centre(), here)).total_cmp(&len(sub(b.1.centre(), here))))
                            .map(|(j, _)| j);
                    }
                    if struck.is_empty() {
                        points.push([from[0] + dir[0] * range * 0.5, from[1] + dir[1] * range * 0.5, from[2] + dir[2] * range * 0.5]);
                    }
                }
                for w in points.windows(2) {
                    self.lightning(w[0], w[1], s.colour);
                }
            }
            SpellKind::Nova { radius, .. } => {
                let Some(hit) = s.hit(power) else { return };
                let centre = [from[0], from[1] - 1.0, from[2]];
                for k in 0..36 {
                    let a = k as f32 / 36.0 * 2.0 * PI;
                    let sp = radius * 2.4;
                    self.sparks.push(Spark { pos: centre, vel: [a.cos() * sp, 0.6, a.sin() * sp], colour: s.colour, life: 0.42, max: 0.42, size: 0.1, gravity: 0.0 });
                }
                if owner == 0 {
                    for i in 0..self.creatures.len() {
                        let c = &self.creatures[i];
                        if c.fighter.alive() && len(sub(c.centre(), centre)) < radius + BODY_R * c.def.scale {
                            let mut h = hit.clone();
                            h.dir = flat(sub(c.pos, centre));
                            let (o, ev) = self.creatures[i].fighter.take(&h);
                            self.creature_hurt(i, &o, ev);
                        }
                    }
                }
            }
            SpellKind::Heal { .. } => self.sparkle(from, s.colour, 18),
            SpellKind::Ward { .. } => self.sparkle(from, s.colour, 18),
        }
    }

    fn lightning(&mut self, a: [f32; 3], b: [f32; 3], colour: [f32; 3]) {
        let d = sub(b, a);
        let n = (len(d) / 0.25).ceil().max(2.0) as usize;
        let mut jitter = [0.0f32; 3];
        for k in 0..=n {
            let t = k as f32 / n as f32;
            for j in &mut jitter {
                *j = (*j + (self.rand() - 0.5) * 0.18).clamp(-0.35, 0.35) * (1.0 - (2.0 * t - 1.0).powi(8));
            }
            let pos = [a[0] + d[0] * t + jitter[0], a[1] + d[1] * t + jitter[1], a[2] + d[2] * t + jitter[2]];
            self.sparks.push(Spark { pos, vel: [0.0; 3], colour: [colour[0] * 1.6, colour[1] * 1.6, colour[2] * 1.8], life: 0.16, max: 0.16, size: 0.06, gravity: 0.0 });
        }
    }

    /// One frame of the fight.
    pub fn update(&mut self, dt: f32, fr: &Frame, body: &mut Player) -> Report {
        let mut report = Report::default();
        let bp = body.position();
        let me = [bp[0] as f32, bp[1] as f32, bp[2] as f32];
        self.player.facing = flat(fr.aim);
        if let Some(t) = &mut self.dead_for {
            *t += dt;
            if *t >= RESPAWN_AFTER {
                self.dead_for = None;
                self.player.revive();
                body.press("revive");
                report.respawn = true;
            }
        }
        let ev = self.player.tick(dt);
        self.player_events(ev, body, &mut report, Some(fr));
        if !self.player.alive() && self.dead_for.is_none() {
            self.dead_for = Some(0.0);
        }
        self.hurt = (self.hurt - dt).max(0.0);
        if let Some((_, t)) = &mut self.message {
            *t += dt;
            if *t > 1.6 {
                self.message = None;
            }
        }

        self.spawn(dt, fr, me);
        self.run_creatures(dt, fr, me, body);
        self.fly_bolts(dt, fr, body);
        for s in &mut self.sparks {
            s.vel[1] -= s.gravity * dt;
            for k in 0..3 {
                s.pos[k] += s.vel[k] * dt;
            }
            s.life -= dt;
        }
        self.sparks.retain(|s| s.life > 0.0);
        for f in &mut self.floaters {
            f.age += dt;
            f.pos[1] += dt * 0.8;
        }
        self.floaters.retain(|f| f.age < 1.1);
        report
    }

    fn spawn(&mut self, dt: f32, fr: &Frame, me: [f32; 3]) {
        let cap = match fr.difficulty {
            0 => 0,
            1 => 5,
            _ => 9,
        };
        if cap == 0 {
            // Peaceful: they all go.
            self.creatures.clear();
            return;
        }
        self.spawn_wait -= dt;
        let alive = self.creatures.iter().filter(|c| c.fighter.alive()).count();
        if self.spawn_wait > 0.0 || alive >= cap || fr.flying {
            return;
        }
        self.spawn_wait = if fr.difficulty >= 2 { 3.0 } else { 6.0 } + self.rand() * 4.0;
        // Which kind: skeletons only at night.
        let weights: Vec<(usize, f32)> = CREATURES
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let w = match (c.key, fr.night) {
                    ("skeleton", true) => 4.0,
                    ("skeleton", false) => 0.0,
                    ("goblin", _) => 4.0,
                    ("bandit", n) => if n { 1.0 } else { 2.5 },
                    ("goblin_shaman", _) => 1.3,
                    ("troll", _) => 0.4,
                    _ => 1.0,
                };
                (i, w)
            })
            .collect();
        let total: f32 = weights.iter().map(|w| w.1).sum();
        let mut pick = self.rand() * total;
        let mut kind = 0;
        for (i, w) in &weights {
            if pick < *w {
                kind = *i;
                break;
            }
            pick -= w;
        }
        let def = &CREATURES[kind];
        // Goblins come in twos and threes.
        let group = if def.key == "goblin" { 2 + (self.rand() < 0.4) as usize } else { 1 };
        let blocks = ClientBlocks(fr.client);
        let world = BlockWorld { step_height: 0.6, ..BlockWorld::new(&blocks) };
        for _ in 0..8 {
            let a = self.rand() * 2.0 * PI;
            let r = SPAWN_NEAR + self.rand() * (SPAWN_FAR - SPAWN_NEAR);
            let (x, z) = (me[0] + a.sin() * r, me[2] + a.cos() * r);
            if !world.loaded(x as f64, me[1] as f64, z as f64) {
                continue;
            }
            let y = crate::hyrig::World::ground_height(&world, x as f64, z as f64, me[1] as f64 + 24.0) as f32;
            if (y - me[1]).abs() > 20.0 || fr.client.is_liquid(BlockPos { x: x.floor() as i32, y: y.floor() as i32, z: z.floor() as i32 }) {
                continue;
            }
            for k in 0..group {
                let off = [(k as f32 - 1.0) * 1.6, (k as f32 * 1.7).sin() * 1.2];
                let d = if k > 0 && def.key == "goblin" && self.rand() < 0.3 { crate::combat::creature("goblin_shaman") } else { def };
                self.add_creature(d, [x + off[0], y + 0.6, z + off[1]], &world);
            }
            break;
        }
    }

    fn add_creature<B: crate::hyrig::Blocks>(&mut self, def: &'static CreatureDef, at: [f32; 3], world: &BlockWorld<B>) {
        let mut ctl = Controller::new(self.cfg.clone(), View::Follow);
        ctl.snap_down = 0.6;
        let yaw = self.rand() * 2.0 * PI;
        ctl.teleport(world, at[0] as f64, Some(at[1] as f64), at[2] as f64, Some(yaw as f64));
        let weapon = self.reg.id(def.weapon).unwrap_or(0);
        let w = self.book.weapon(weapon).unwrap_or_else(Weapon::fists);
        let mut fighter = Fighter::new(Vitals::new(def.health, def.stamina, def.mana), w, def.poise);
        fighter.base = def.defense();
        let id = self.next_id;
        self.next_id += 1;
        let seed = (self.rand() * 1.0e6) as u32;
        self.creatures.push(Creature {
            id,
            def,
            ctl,
            brain: Brain::new(def, seed, at),
            fighter,
            weapon,
            pos: at,
            yaw,
            sees: false,
            look_wait: 0.0,
            hurt: false,
            presses: Vec::new(),
            roll: 0.0,
            dead_for: 0.0,
            out: None,
        });
    }

    /// Development runs: creatures of `keys` in a row in front of the player (`yaw`, the
    /// controller's sense), facing him.
    pub fn dev_spawn(&mut self, keys: &[String], client: &Client, at: [f32; 3], yaw: f32) {
        let blocks = ClientBlocks(client);
        let world = BlockWorld { step_height: 0.6, ..BlockWorld::new(&blocks) };
        let n = keys.len() as f32;
        for (i, k) in keys.iter().enumerate() {
            let Some(def) = blockforge::rpg::creatures::by_key(k) else { continue };
            let off = (i as f32 - (n - 1.0) * 0.5) * 0.5;
            let a = yaw + off;
            let r = 3.2 + def.scale;
            let (x, z) = (at[0] + a.sin() * r, at[2] + a.cos() * r);
            let y = crate::hyrig::World::ground_height(&world, x as f64, z as f64, at[1] as f64 + 6.0) as f32;
            self.add_creature(def, [x, y + 0.2, z], &world);
            if let Some(c) = self.creatures.last_mut() {
                c.ctl.teleport(&world, x as f64, Some(y as f64 + 0.2), z as f64, Some((yaw + PI) as f64));
                c.yaw = yaw + PI;
            }
        }
        self.spawn_wait = 1.0e6;
    }

    fn run_creatures(&mut self, dt: f32, fr: &Frame, me: [f32; 3], body: &mut Player) {
        let blocks = ClientBlocks(fr.client);
        let player_alive = self.player.alive() && !fr.flying;
        let swinging = ai::winding_up(&self.player);
        let mut strikes: Vec<(usize, Hit, f32, f32)> = Vec::new();
        let mut casts: Vec<(usize, usize, f32)> = Vec::new();
        for i in 0..self.creatures.len() {
            let c = &mut self.creatures[i];
            c.look_wait -= dt;
            if c.look_wait <= 0.0 {
                c.look_wait = 0.3;
                let eye = [c.pos[0], c.pos[1] + 1.6 * c.def.scale, c.pos[2]];
                c.sees = clear(fr.client, eye, [me[0], me[1] + 1.6, me[2]]);
            }
            let senses = Senses { me: c.pos, player: me, player_alive, player_swinging: swinging, sees: c.sees, hurt: c.hurt };
            c.hurt = false;
            let staff = self.book.get(c.weapon).copied();
            let intent = c.brain.update(dt, &senses, &c.fighter, staff);
            c.yaw = intent.yaw;
            c.fighter.facing = [intent.yaw.sin(), intent.yaw.cos()];
            let mut ev = Vec::new();
            for cmd in &intent.commands {
                ev.extend(c.fighter.command(*cmd));
            }
            ev.extend(c.fighter.tick(dt));
            for e in ev {
                match e {
                    Event::Anim(a) => c.presses.push(a.to_string()),
                    Event::Roll => c.roll = 0.25,
                    Event::Strike { hit, reach, arc, .. } => strikes.push((i, hit, reach * c.def.scale.max(1.0), arc)),
                    Event::Spell { spell, power } => casts.push((i, spell, power)),
                    _ => {}
                }
            }
            // The body: walk where the brain says, at the pace the fight allows.
            let pace = c.fighter.pace();
            let alive = c.fighter.alive();
            let hold = match self.book.get(c.weapon) {
                _ if c.fighter.guarding() => "two_handed",
                Some(Item::Weapon(w)) if w.two_handed => "two_handed",
                Some(_) => "one_handed",
                None => "none",
            };
            c.ctl.set_hold(if alive { hold } else { "none" });
            c.roll = (c.roll - dt).max(0.0);
            let input = Input {
                move_x: if alive { (intent.move_x * pace) as f64 } else { 0.0 },
                move_y: if alive { (intent.move_y * pace) as f64 } else { 0.0 },
                look_yaw: intent.yaw as f64,
                look_pitch: 0.0,
                jump: false,
                roll: c.roll > 0.0,
                down: false,
                punch: false,
                mine: false,
                actions: std::mem::take(&mut c.presses),
            };
            let world = BlockWorld { step_height: 0.6, feet: c.ctl.pos[1], ..BlockWorld::new(&blocks) };
            if world.loaded(c.ctl.pos[0], c.ctl.pos[1], c.ctl.pos[2]) {
                let out = c.ctl.update(&world, dt as f64, &input);
                c.pos = [out.position[0] as f32, out.position[1] as f32, out.position[2] as f32];
                c.out = Some(out);
            }
            if !alive {
                c.dead_for += dt;
            }
        }
        for (i, hit, reach, arc) in strikes {
            let c = &self.creatures[i];
            let f = c.fighter.facing;
            let (dx, dz) = (me[0] - c.pos[0], me[2] - c.pos[2]);
            let d = (dx * dx + dz * dz).sqrt() - BODY_R;
            let ang = if d > 0.3 { (f[0] * dx + f[1] * dz) / (dx * dx + dz * dz).sqrt().max(1e-4) } else { 1.0 };
            if self.player.alive() && !fr.flying && d <= reach && ang >= (arc.to_radians() * 0.5).cos() && (me[1] - c.pos[1]).abs() < 2.0 {
                let mut h = hit;
                h.dir = flat([dx, 0.0, dz]);
                let o = self.hurt_player(&h, body);
                if o.parried {
                    let ev = self.creatures[i].fighter.stagger(1.3);
                    for e in ev {
                        if let Event::Anim(a) = e {
                            self.creatures[i].presses.push(a.to_string());
                        }
                    }
                }
            }
        }
        for (i, spell, power) in casts {
            let c = &self.creatures[i];
            let from = [c.pos[0], c.pos[1] + 1.45 * c.def.scale, c.pos[2]];
            let to = [me[0], me[1] + 1.1, me[2]];
            let dir = normalize(sub(to, from));
            let id = c.id;
            self.cast(id, spell, power, from, dir, fr.client);
            // A nova or lightning from a creature lands on the player at once, if near.
            let s = &SPELLS[spell];
            if let (Some(h), false) = (s.hit(power), matches!(s.kind, SpellKind::Bolt { .. })) {
                let reach = match s.kind {
                    SpellKind::Nova { radius, .. } => radius,
                    SpellKind::Arc { range, .. } => range,
                    _ => 0.0,
                };
                if len(sub(to, from)) < reach && player_alive {
                    let mut h = h;
                    h.dir = flat(dir);
                    self.hurt_player(&h, body);
                }
            }
        }
        // Burning bodies smoke and flicker.
        for i in 0..self.creatures.len() {
            if self.creatures[i].fighter.effects.has(Status::Burning) && self.rand() < dt * 14.0 {
                let p = self.creatures[i].centre();
                let off = [(self.rand() - 0.5) * 0.5, (self.rand() - 0.5) * 1.2, (self.rand() - 0.5) * 0.5];
                let max = 0.4 + self.rand() * 0.3;
                self.sparks.push(Spark { pos: [p[0] + off[0], p[1] + off[1], p[2] + off[2]], vel: [0.0, 1.4, 0.0], colour: [1.0, 0.45, 0.1], life: max, max, size: 0.06, gravity: -1.0 });
            }
        }
        self.creatures.retain(|c| {
            let far = (c.pos[0] - me[0]).hypot(c.pos[2] - me[2]) > DESPAWN;
            !(far || c.dead_for > CORPSE_TIME)
        });
    }

    fn fly_bolts(&mut self, dt: f32, fr: &Frame, body: &mut Player) {
        let me = body_centre(body);
        let bp = body.position();
        let feet = [bp[0] as f32, bp[1] as f32, bp[2] as f32];
        let mut bursts: Vec<(Bolt, [f32; 3], Option<u32>)> = Vec::new();
        let mut bolts = std::mem::take(&mut self.bolts);
        for mut b in bolts.drain(..) {
            let owner = b.owner;
            let creatures = &self.creatures;
            let player_alive = self.player.alive() && !fr.flying;
            let hits = |p: [f32; 3]| -> Option<u32> {
                if owner == 0 {
                    creatures.iter().find(|c| c.fighter.alive() && c.contains(p, 0.15)).map(|c| c.id)
                } else {
                    let (dx, dz) = (p[0] - feet[0], p[2] - feet[2]);
                    (player_alive && dx * dx + dz * dz < 0.25 && p[1] > feet[1] - 0.1 && p[1] < feet[1] + 1.9).then_some(0)
                }
            };
            let blocked = |p: [f32; 3]| solid_at(fr.client, p);
            match b.fly(dt, &blocked, &hits) {
                Some((at, who)) => bursts.push((b, at, who)),
                None => {
                    // A trail of sparks.
                    let colour = SPELLS[b.spell].colour;
                    let max = 0.25;
                    self.sparks.push(Spark { pos: b.pos, vel: [0.0, 0.3, 0.0], colour, life: max, max, size: 0.07, gravity: 0.0 });
                    self.bolts.push(b);
                }
            }
        }
        for (b, at, who) in bursts {
            let s = &SPELLS[b.spell];
            let Some(hit) = s.hit(b.power) else { continue };
            let splash = match s.kind {
                SpellKind::Bolt { splash, .. } => splash,
                _ => 0.0,
            };
            self.burst(at, s.colour, if splash > 0.0 { 28 } else { 12 }, if splash > 0.0 { 6.0 } else { 3.0 });
            let dir = normalize(b.vel);
            if b.owner == 0 {
                for i in 0..self.creatures.len() {
                    let c = &self.creatures[i];
                    let direct = who == Some(c.id);
                    if !c.fighter.alive() || !(direct || (splash > 0.0 && len(sub(c.centre(), at)) < splash + BODY_R)) {
                        continue;
                    }
                    let mut h = hit.clone();
                    if !direct {
                        h = h.scaled(0.6);
                    }
                    h.dir = flat(dir);
                    let (o, ev) = self.creatures[i].fighter.take(&h);
                    self.creature_hurt(i, &o, ev);
                }
            } else if who == Some(0) || (splash > 0.0 && len(sub(me, at)) < splash + BODY_R) {
                let mut h = hit;
                h.dir = flat(dir);
                self.hurt_player(&h, body);
            }
        }
    }

    // --- drawing ---------------------------------------------------------------------------

    fn item_mesh(&mut self, id: u16) -> &Vec<LocalVert> {
        let reg = self.reg.clone();
        self.meshes.entry(id).or_insert_with(|| sprite_mesh(&reg, id))
    }

    /// Item `id` in `hand`, the body scaled by `k` about `feet`.
    fn place_item(&mut self, id: u16, hand: Hand, k: f32, feet: [f32; 3], out: &mut Vec<Vertex>) {
        let (diagonal, grip, tilt): (bool, f32, f32) = match self.book.get(id) {
            Some(Item::Weapon(w)) => (
                true,
                match (w.class, w.two_handed) {
                    (WeaponClass::Dagger, _) => 2.6,
                    (_, true) => 9.0,
                    _ => 3.4,
                },
                0.5,
            ),
            Some(Item::Staff(_)) => (true, 12.0, 0.2),
            _ => (false, 0.0, 0.0),
        };
        // The sprite's axes: along the item (s), across it (a), out of the picture (n).
        let (s, a) = if diagonal { ([0.70710677, 0.70710677], [-0.70710677, 0.70710677]) } else { ([0.0, 1.0], [-1.0, 0.0]) };
        // The grip, in sprite metres: on the diagonal from the pommel at pixel (1, 30), or
        // the middle of the picture's lower half (a bottle).
        let g = if diagonal { [TEXEL * 1.0 + s[0] * grip * TEXEL, TEXEL * 2.0 + s[1] * grip * TEXEL] } else { [0.5, 0.3] };
        let (c, ax) = hand;
        let c = [feet[0] + (c[0] - feet[0]) * k, feet[1] + (c[1] - feet[1]) * k, feet[2] + (c[2] - feet[2]) * k];
        // Through the fist: up out of the thumb side (-z), leaning forward along the forearm
        // (-y); the flat of a blade faces out to the side (x).
        let sw = normalize([
            -ax[2][0] * tilt.cos() - ax[1][0] * tilt.sin(),
            -ax[2][1] * tilt.cos() - ax[1][1] * tilt.sin(),
            -ax[2][2] * tilt.cos() - ax[1][2] * tilt.sin(),
        ]);
        let nx = ax[0];
        let nw = normalize(sub(nx, scale(sw, dot(nx, sw))));
        let aw = cross(nw, sw);
        let mesh = self.item_mesh(id).clone();
        for v in mesh {
            let rel = [v.p[0] - g[0], v.p[1] - g[1]];
            let (ps, pa, pn) = (rel[0] * s[0] + rel[1] * s[1], rel[0] * a[0] + rel[1] * a[1], v.p[2]);
            let pos = std::array::from_fn(|i| c[i] + (sw[i] * ps + aw[i] * pa + nw[i] * pn) * k);
            let (ns, na) = (v.n[0] * s[0] + v.n[1] * s[1], v.n[0] * a[0] + v.n[1] * a[1]);
            let normal = std::array::from_fn(|i| sw[i] * ns + aw[i] * na + nw[i] * v.n[2]);
            out.push(Vertex { pos, normal, uv: v.uv });
        }
    }

    /// Everything to draw with the character: his body (and the item in his hand), the
    /// creatures and theirs, bolts and sparks.
    pub fn triangles(&mut self, body: &Player, held: u16, show_held: bool) -> &[Vertex] {
        let mut out = std::mem::take(&mut self.verts);
        out.clear();
        out.extend_from_slice(body.triangles());
        if show_held && held != 0 && self.book.get(held).is_some() {
            if let Some(hand) = body.hands[0] {
                let p = body.position();
                self.place_item(held, hand, 1.0, [p[0] as f32, p[1] as f32, p[2] as f32], &mut out);
            }
        }
        let mut tmp = Vec::new();
        for i in 0..self.creatures.len() {
            let c = &self.creatures[i];
            let Some(o) = &c.out else { continue };
            let pose = self.rig.pose(o, [0.0; 3]);
            let (k, cpos, dead_for, skin, weapon, alive) = (c.def.scale, c.pos, c.dead_for, c.def.skin, c.weapon, c.fighter.alive());
            let flash = (c.fighter.flash / 0.25 * 3.0).ceil().clamp(0.0, 3.0);
            tmp.clear();
            self.rig.triangles(&pose, &mut tmp);
            // The dead sink into the ground after a while.
            let lift = if dead_for > CORPSE_TIME - 2.5 { -(dead_for - (CORPSE_TIME - 2.5)) * 0.5 } else { 0.0 };
            let du = 1.5 * skin as f32;
            for v in &tmp {
                let pos = [cpos[0] + (v.pos[0] - cpos[0]) * k, cpos[1] + (v.pos[1] - cpos[1]) * k + lift, cpos[2] + (v.pos[2] - cpos[2]) * k];
                out.push(Vertex { pos, normal: v.normal, uv: [v.uv[0] + du, v.uv[1] + 2.0 * flash] });
            }
            if weapon != 0 && alive {
                if let Some(hand) = hands_of(&self.rig, &pose)[0] {
                    self.place_item(weapon, hand, k, cpos, &mut out);
                }
            }
        }
        for b in &self.bolts {
            let col = SPELLS[b.spell].colour;
            cube(&mut out, b.pos, 0.11, [col[0] * 2.2, col[1] * 2.2, col[2] * 2.2]);
        }
        for s in &self.sparks {
            let k = (s.life / s.max).clamp(0.0, 1.0);
            cube(&mut out, s.pos, s.size * (0.4 + 0.6 * k), [s.colour[0] * (0.6 + k), s.colour[1] * (0.6 + k), s.colour[2] * (0.6 + k)]);
        }
        self.verts = out;
        &self.verts
    }

    /// The HUD's view of the fight: bars, floating numbers and the like, placed on screen
    /// through the camera (`eye`, `forward`), for a `w` x `h` screen.
    pub fn hud(&self, eye: [f32; 3], forward: [f32; 3], w: f32, h: f32, held: Held) -> crate::ui::CombatHud {
        let project = |p: [f32; 3]| -> Option<[f32; 2]> {
            let f = normalize(forward);
            let s = normalize(cross(f, [0.0, 1.0, 0.0]));
            let u = cross(s, f);
            let d = sub(p, eye);
            let z = dot(d, f);
            if z < 0.3 {
                return None;
            }
            let t = (crate::math::FOVY * 0.5).tan();
            let (x, y) = (dot(d, s) / (z * t * w / h), dot(d, u) / (z * t));
            (x.abs() < 1.2 && y.abs() < 1.2).then(|| [(x * 0.5 + 0.5) * w, (0.5 - y * 0.5) * h])
        };
        let mut bars = Vec::new();
        for c in &self.creatures {
            let d = len(sub(c.pos, eye));
            let hurt = c.fighter.vitals.health.frac() < 0.999;
            if !c.fighter.alive() || d > 28.0 || !(hurt || d < 12.0) {
                continue;
            }
            if let Some(at) = project([c.pos[0], c.pos[1] + BODY_H * c.def.scale + 0.35, c.pos[2]]) {
                bars.push(crate::ui::FoeBar { at, frac: c.fighter.vitals.health.frac(), name: c.def.name, near: (1.0 - d / 28.0).clamp(0.3, 1.0) });
            }
        }
        let floaters = self
            .floaters
            .iter()
            .filter_map(|f| {
                let a = (1.0 - (f.age - 0.6).max(0.0) / 0.5).clamp(0.0, 1.0);
                project(f.pos).map(|at| (at, f.text.clone(), [f.colour[0], f.colour[1], f.colour[2], (f.colour[3] as f32 * a) as u8]))
            })
            .collect();
        let v = &self.player.vitals;
        let mut effects = Vec::new();
        for (s, name) in [(Status::Burning, "Burning"), (Status::Chilled, "Chilled"), (Status::Bleeding, "Bleeding"), (Status::Regenerating, "Mending"), (Status::Warded, "Warded")] {
            if self.player.effects.has(s) {
                effects.push(name);
            }
        }
        crate::ui::CombatHud {
            on: true,
            health: v.health.frac(),
            stamina: v.stamina.frac(),
            mana: v.mana.frac(),
            numbers: [v.health.value, v.health.max, v.mana.value, v.mana.max],
            guard: matches!(held, Held::Weapon(_) | Held::Staff(_)),
            potion: matches!(held, Held::Potion(_)),
            bars,
            floaters,
            dead: self.dead_for.map(|t| (RESPAWN_AFTER - t).max(0.0)),
            hurt: self.hurt,
            effects,
            message: self.message.as_ref().map(|m| m.0.clone()),
        }
    }
}

/// The creature of `key` (one of `CREATURES`).
pub fn creature(key: &str) -> &'static CreatureDef {
    blockforge::rpg::creatures::by_key(key).unwrap_or(&CREATURES[0])
}

fn body_centre(body: &Player) -> [f32; 3] {
    let p = body.position();
    [p[0] as f32, p[1] as f32 + 1.0, p[2] as f32]
}

/// Whether creature `c` is within `reach` of `at` and inside `arc` degrees about `facing`.
fn in_arc(at: [f32; 3], facing: [f32; 2], c: &Creature, reach: f32, arc: f32) -> bool {
    let (dx, dz) = (c.pos[0] - at[0], c.pos[2] - at[2]);
    let dist = (dx * dx + dz * dz).sqrt();
    if dist - BODY_R * c.def.scale > reach || (c.pos[1] - at[1]).abs() > 2.0 * c.def.scale.max(1.0) {
        return false;
    }
    if dist < 0.6 {
        return true;
    }
    (facing[0] * dx + facing[1] * dz) / dist >= (arc.to_radians() * 0.5).cos()
}

fn solid_at(client: &Client, p: [f32; 3]) -> bool {
    let cell = BlockPos { x: p[0].floor() as i32, y: p[1].floor() as i32, z: p[2].floor() as i32 };
    client.solid_span(cell).flatten().is_some_and(|(lo, hi)| {
        let f = p[1] - p[1].floor();
        f >= lo && f <= hi
    })
}

/// Nothing solid on the straight line from `a` to `b`.
fn clear(client: &Client, a: [f32; 3], b: [f32; 3]) -> bool {
    let d = sub(b, a);
    let n = (len(d) / 0.5).ceil() as usize;
    (1..n).all(|k| !solid_at(client, [a[0] + d[0] * k as f32 / n as f32, a[1] + d[1] * k as f32 / n as f32, a[2] + d[2] * k as f32 / n as f32]))
}

/// An item's 32 x 32 sprite, one texel thick: the picture front and back, and a side face
/// along every edge between a solid pixel and a see-through one (or the picture's edge).
fn sprite_mesh(reg: &BlockRegistry, id: u16) -> Vec<LocalVert> {
    let layer = reg.def(id).tex[0];
    let name = reg.texture_names().get(layer as usize).cloned().unwrap_or_default();
    let px = crate::textures::load(&name);
    let n = 32i32;
    let solid = |x: i32, y: i32| x >= 0 && y >= 0 && x < n && y < n && px[((y * n + x) * 4 + 3) as usize] >= 192;
    let lv = layer as f32 * 2.0;
    let h = TEXEL * 0.5;
    let mut out = Vec::new();
    let mut quad = |c: [[f32; 3]; 4], normal: [f32; 3], uv: [[f32; 2]; 4]| {
        for k in [0, 1, 2, 0, 2, 3] {
            out.push(LocalVert { p: c[k], n: normal, uv: uv[k] });
        }
    };
    for (z, nz) in [(h, 1.0), (-h, -1.0)] {
        let uv = |x: f32, y: f32| [16.0 + x, lv + 1.0 - y];
        quad([[0.0, 0.0, z], [1.0, 0.0, z], [1.0, 1.0, z], [0.0, 1.0, z]], [0.0, 0.0, nz], [uv(0.0, 0.0), uv(1.0, 0.0), uv(1.0, 1.0), uv(0.0, 1.0)]);
    }
    for y in 0..n {
        for x in 0..n {
            if !solid(x, y) {
                continue;
            }
            let uv = [16.0 + (x as f32 + 0.5) * TEXEL, lv + (y as f32 + 0.5) * TEXEL];
            let uvs = [uv; 4];
            let (x0, x1) = (x as f32 * TEXEL, (x + 1) as f32 * TEXEL);
            let (yt, yb) = (1.0 - y as f32 * TEXEL, 1.0 - (y + 1) as f32 * TEXEL);
            if !solid(x - 1, y) {
                quad([[x0, yb, -h], [x0, yb, h], [x0, yt, h], [x0, yt, -h]], [-1.0, 0.0, 0.0], uvs);
            }
            if !solid(x + 1, y) {
                quad([[x1, yb, h], [x1, yb, -h], [x1, yt, -h], [x1, yt, h]], [1.0, 0.0, 0.0], uvs);
            }
            if !solid(x, y - 1) {
                quad([[x0, yt, h], [x1, yt, h], [x1, yt, -h], [x0, yt, -h]], [0.0, 1.0, 0.0], uvs);
            }
            if !solid(x, y + 1) {
                quad([[x0, yb, -h], [x1, yb, -h], [x1, yb, h], [x0, yb, h]], [0.0, -1.0, 0.0], uvs);
            }
        }
    }
    out
}

/// A glowing cube (a spark or a bolt): its colour in the normal, marked unlit (u + 24).
fn cube(out: &mut Vec<Vertex>, c: [f32; 3], r: f32, colour: [f32; 3]) {
    const FACES: [[[f32; 3]; 4]; 6] = [
        [[1., -1., -1.], [1., 1., -1.], [1., 1., 1.], [1., -1., 1.]],
        [[-1., -1., 1.], [-1., 1., 1.], [-1., 1., -1.], [-1., -1., -1.]],
        [[-1., 1., -1.], [-1., 1., 1.], [1., 1., 1.], [1., 1., -1.]],
        [[-1., -1., 1.], [-1., -1., -1.], [1., -1., -1.], [1., -1., 1.]],
        [[-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.]],
        [[1., -1., -1.], [-1., -1., -1.], [-1., 1., -1.], [1., 1., -1.]],
    ];
    for f in FACES {
        for k in [0, 1, 2, 0, 2, 3] {
            let p = f[k];
            out.push(Vertex { pos: [c[0] + p[0] * r, c[1] + p[1] * r, c[2] + p[2] * r], normal: colour, uv: [24.5, 0.5] });
        }
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = len(a).max(1e-6);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// The way across the ground (unit x, z).
fn flat(a: [f32; 3]) -> [f32; 2] {
    let l = (a[0] * a[0] + a[2] * a[2]).sqrt().max(1e-6);
    [a[0] / l, a[2] / l]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sword_sprite_becomes_a_closed_one_texel_thick_mesh() {
        let reg = Arc::new(BlockRegistry::builtin());
        let id = reg.id("iron_sword").unwrap();
        let m = sprite_mesh(&reg, id);
        assert!(m.len() > 12 && m.len() % 6 == 0);
        // Thin: one texel through, and no wider than the picture.
        for v in &m {
            assert!(v.p[2].abs() <= TEXEL * 0.5 + 1e-6);
            assert!(v.p[0] >= 0.0 && v.p[0] <= 1.0 && v.p[1] >= 0.0 && v.p[1] <= 1.0);
            assert!(v.uv[0] >= 16.0 && v.uv[0] <= 17.0);
        }
    }

    #[test]
    fn items_in_the_hotbar_are_told_apart_from_blocks() {
        let reg = Arc::new(BlockRegistry::builtin());
        let c = Combat::new(reg.clone(), 1);
        assert!(matches!(c.held_kind(reg.id("iron_sword").unwrap()), Held::Weapon(_)));
        assert!(matches!(c.held_kind(reg.id("ember_staff").unwrap()), Held::Staff(_)));
        assert!(matches!(c.held_kind(reg.id("health_potion").unwrap()), Held::Potion(_)));
        assert_eq!(c.held_kind(reg.id("plate_helm").unwrap()), Held::Armor);
        assert_eq!(c.held_kind(reg.id("stone").unwrap()), Held::Block);
        assert_eq!(c.held_kind(0), Held::Nothing);
    }
}
