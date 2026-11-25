//! What a creature does next: wander, notice the player, close in, circle, swing, guard,
//! dodge, keep its distance and cast, or run.
//!
//! A brain sees only what it is told each frame (`Senses`) and answers with where to go,
//! which way to face and what to do (`Intent`); the app turns that into the body's stick
//! and the fighter's commands.

use super::creatures::{CreatureDef, Style};
use super::fighter::{Action, Command, Fighter};
use super::items::Item;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Mood {
    /// Strolling about (to `to`), not yet aware of the player.
    Wander { to: [f32; 2], wait: f32 },
    /// After the player.
    Hunt,
    /// Running away (low on health).
    Flee,
}

/// What it knows this frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Senses {
    /// Where it stands and where the player stands (feet).
    pub me: [f32; 3],
    pub player: [f32; 3],
    pub player_alive: bool,
    /// The player has just started a swing (a moment to guard or roll away).
    pub player_swinging: bool,
    /// It can see the player (nothing solid between their heads).
    pub sees: bool,
    /// It has just been hit (it turns on whoever did it, wherever they are).
    pub hurt: bool,
}

/// What it wants to do: move (`move_x` right, `move_y` forward, -1..1, relative to
/// `yaw`, the way it faces; yaw 0 faces +Z) and act.
#[derive(Clone, Debug, PartialEq)]
pub struct Intent {
    pub yaw: f32,
    pub move_x: f32,
    pub move_y: f32,
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug)]
pub struct Brain {
    pub def: &'static CreatureDef,
    pub mood: Mood,
    /// Seconds before it next decides how to fight.
    think: f32,
    /// Which way it circles (+1 or -1), and for how much longer.
    circle: f32,
    circle_time: f32,
    /// Holding its guard for this much longer.
    guard_time: f32,
    seed: u32,
    yaw: f32,
    /// Whether the player was winding up last frame (it reacts once to each swing, as it
    /// starts, not on every frame of it).
    saw_swing: bool,
}

impl Brain {
    pub fn new(def: &'static CreatureDef, seed: u32, home: [f32; 3]) -> Brain {
        Brain { def, mood: Mood::Wander { to: [home[0], home[2]], wait: 1.0 }, think: 0.5, circle: 1.0, circle_time: 0.0, guard_time: 0.0, seed: seed | 1, yaw: 0.0, saw_swing: false }
    }

    fn rand(&mut self) -> f32 {
        // xorshift32
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn update(&mut self, dt: f32, s: &Senses, me: &Fighter, staff: Option<Item>) -> Intent {
        let mut intent = Intent { yaw: self.yaw, move_x: 0.0, move_y: 0.0, commands: Vec::new() };
        if !me.alive() {
            return intent;
        }
        let (dx, dz) = (s.player[0] - s.me[0], s.player[2] - s.me[2]);
        let dist = (dx * dx + dz * dz).sqrt();
        let toward = dx.atan2(dz);
        let near = dist < self.def.sight && (s.sees || dist < 6.0);
        // Notice the player, give up when they are long gone (or dead), run when hurt badly.
        match self.mood {
            Mood::Wander { .. } if s.player_alive && (near || s.hurt) => self.mood = Mood::Hunt,
            Mood::Hunt if !s.player_alive || dist > self.def.sight * 2.0 => self.mood = Mood::Wander { to: [s.me[0], s.me[2]], wait: 2.0 },
            Mood::Hunt if self.def.flees > 0.0 && me.vitals.health.frac() < self.def.flees => self.mood = Mood::Flee,
            Mood::Flee if dist > self.def.sight * 1.5 || !s.player_alive => self.mood = Mood::Wander { to: [s.me[0], s.me[2]], wait: 3.0 },
            _ => {}
        }
        let fresh_swing = s.player_swinging && !self.saw_swing;
        self.saw_swing = s.player_swinging;
        self.think -= dt;
        self.circle_time -= dt;
        self.guard_time -= dt;
        match self.mood {
            Mood::Wander { to, wait } => {
                let (wx, wz) = (to[0] - s.me[0], to[1] - s.me[2]);
                let d = (wx * wx + wz * wz).sqrt();
                if d < 1.0 {
                    let wait = wait - dt;
                    if wait <= 0.0 {
                        let a = self.rand() * std::f32::consts::TAU;
                        let r = 3.0 + self.rand() * 7.0;
                        self.mood = Mood::Wander { to: [s.me[0] + a.sin() * r, s.me[2] + a.cos() * r], wait: 2.0 + self.rand() * 5.0 };
                    } else {
                        self.mood = Mood::Wander { to, wait };
                    }
                } else {
                    self.yaw = wx.atan2(wz);
                    intent.move_y = 0.3;
                }
                if me.guarding() {
                    intent.commands.push(Command::GuardDown);
                }
            }
            Mood::Flee => {
                self.yaw = (-dx).atan2(-dz);
                intent.move_y = self.def.speed;
                if me.guarding() {
                    intent.commands.push(Command::GuardDown);
                }
            }
            Mood::Hunt => {
                self.yaw = toward;
                match self.def.style {
                    Style::Melee => self.melee(s, fresh_swing, dist, me, &mut intent),
                    Style::Caster => self.caster(s, fresh_swing, dist, me, staff, &mut intent),
                }
            }
        }
        intent.yaw = self.yaw;
        intent
    }

    fn melee(&mut self, _s: &Senses, fresh_swing: bool, dist: f32, me: &Fighter, intent: &mut Intent) {
        let reach = me.weapon.reach * self.def.scale.max(1.0);
        // Against a swing in reach (as it starts): guard or roll, as its nature has it.
        if fresh_swing && dist < 4.0 && !me.busy() && self.guard_time <= 0.0 {
            let r = self.rand();
            if r < self.def.guards {
                intent.commands.push(Command::GuardUp);
                self.guard_time = 0.6 + self.rand() * 0.6;
                return;
            } else if r < self.def.guards + self.def.dodges {
                intent.move_y = -1.0;
                intent.commands.push(Command::Dodge);
                return;
            }
        }
        if me.guarding() {
            if self.guard_time > 0.0 {
                intent.move_y = -0.2;
                return;
            }
            intent.commands.push(Command::GuardDown);
        }
        if dist > reach + 0.2 {
            // Close in (walking the last stretch, so it doesn't overrun).
            intent.move_y = if dist > reach + 4.0 { self.def.speed } else { 0.45 * self.def.speed };
            return;
        }
        if dist < reach * 0.45 {
            intent.move_y = -0.4; // too close for a swing: step back
        } else {
            // In reach: circle a little between swings.
            if self.circle_time <= 0.0 {
                self.circle = if self.rand() < 0.5 { -1.0 } else { 1.0 };
                self.circle_time = 1.0 + self.rand() * 1.5;
            }
            intent.move_x = self.circle * 0.3;
        }
        if self.think <= 0.0 && !me.busy() {
            let r = self.rand();
            if r < 0.62 {
                intent.commands.push(Command::Light);
                // Sometimes a second swing straight after.
                if self.rand() < 0.4 {
                    intent.commands.push(Command::Light);
                }
            } else if r < 0.82 {
                intent.commands.push(Command::Heavy);
            }
            self.think = 0.7 + self.rand() * 1.1;
        }
    }

    fn caster(&mut self, s: &Senses, fresh_swing: bool, dist: f32, me: &Fighter, staff: Option<Item>, intent: &mut Intent) {
        // Keep 7 to 14 metres away, casting when the way is clear.
        if dist < 6.0 {
            intent.move_y = -0.8;
        } else if dist > 14.0 || !s.sees {
            intent.move_y = 0.7;
        } else {
            if self.circle_time <= 0.0 {
                self.circle = if self.rand() < 0.5 { -1.0 } else { 1.0 };
                self.circle_time = 1.5 + self.rand() * 2.0;
            }
            intent.move_x = self.circle * 0.4;
        }
        if fresh_swing && dist < 3.5 && self.rand() < self.def.dodges {
            intent.commands.push(Command::Dodge);
            return;
        }
        if let (Some(Item::Staff(st)), true, true) = (staff, s.sees, self.think <= 0.0) {
            if !me.busy() && dist < 20.0 {
                let heavy = me.vitals.mana.value > 60.0 && self.rand() < 0.25 && me.cooldowns[st.heavy] <= 0.0;
                intent.commands.push(Command::Cast { spell: if heavy { st.heavy } else { st.light }, power: st.power * 0.6 });
                self.think = 1.6 + self.rand() * 1.4;
            }
        }
    }
}

/// Whether a fighter is about to land a blow (the moment to react to it).
pub fn winding_up(f: &Fighter) -> bool {
    matches!(f.action, Action::Attack { t, mv, struck: false, .. } if t < mv.strike)
}

#[cfg(test)]
mod tests {
    use super::super::creatures::by_key;
    use super::super::items::Weapon;
    use super::super::Vitals;
    use super::*;

    fn senses(me: [f32; 3], player: [f32; 3]) -> Senses {
        Senses { me, player, player_alive: true, player_swinging: false, sees: true, hurt: false }
    }

    fn fighter(def: &CreatureDef) -> Fighter {
        let mut w = Weapon::fists();
        w.reach = 2.0;
        Fighter::new(Vitals::new(def.health, def.stamina, def.mana), w, def.poise)
    }

    #[test]
    fn a_goblin_wanders_until_it_sees_the_player_then_closes_in_and_swings() {
        let def = by_key("goblin").unwrap();
        let mut b = Brain::new(def, 7, [0.0; 3]);
        let f = fighter(def);
        let i = b.update(0.1, &senses([0.0; 3], [100.0, 0.0, 0.0]), &f, None);
        assert!(matches!(b.mood, Mood::Wander { .. }) && i.commands.is_empty());
        // Seen: it turns toward the player (+X: yaw 90 degrees) and runs at them.
        let i = b.update(0.1, &senses([0.0; 3], [12.0, 0.0, 0.0]), &f, None);
        assert_eq!(b.mood, Mood::Hunt);
        assert!((i.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3 && i.move_y > 0.5, "{i:?}");
        // In reach, before long it swings.
        let mut swung = false;
        for _ in 0..60 {
            let i = b.update(0.1, &senses([0.0; 3], [1.5, 0.0, 0.0]), &f, None);
            swung |= i.commands.iter().any(|c| matches!(c, Command::Light | Command::Heavy));
        }
        assert!(swung);
    }

    #[test]
    fn it_reacts_to_a_swing_once_as_it_starts_not_on_every_frame_of_it() {
        let def = by_key("goblin").unwrap();
        let f = fighter(def);
        let mut dodges = 0;
        for seed in 1..200 {
            let mut b = Brain::new(def, seed, [0.0; 3]);
            b.update(0.02, &senses([0.0; 3], [1.5, 0.0, 0.0]), &f, None);
            let mut s = senses([0.0; 3], [1.5, 0.0, 0.0]);
            s.player_swinging = true;
            let mut n = 0;
            for _ in 0..15 {
                n += b.update(0.02, &s, &f, None).commands.iter().filter(|c| **c == Command::Dodge).count();
            }
            assert!(n <= 1);
            dodges += n;
        }
        // About as often as its nature says (a third of the time for a goblin).
        assert!((40..100).contains(&dodges), "{dodges}");
    }

    #[test]
    fn a_badly_hurt_goblin_runs_and_a_caster_keeps_its_distance() {
        let def = by_key("goblin").unwrap();
        let mut b = Brain::new(def, 3, [0.0; 3]);
        let mut f = fighter(def);
        b.update(0.1, &senses([0.0; 3], [5.0, 0.0, 0.0]), &f, None);
        f.vitals.health.value = 3.0;
        let i = b.update(0.1, &senses([0.0; 3], [5.0, 0.0, 0.0]), &f, None);
        assert_eq!(b.mood, Mood::Flee);
        assert!((i.yaw + std::f32::consts::FRAC_PI_2).abs() < 1e-3 && i.move_y > 0.5, "{i:?}");

        let def = by_key("goblin_shaman").unwrap();
        let mut b = Brain::new(def, 5, [0.0; 3]);
        let f = fighter(def);
        let i = b.update(0.1, &senses([0.0; 3], [3.0, 0.0, 0.0]), &f, None);
        assert!(i.move_y < 0.0, "backs away: {i:?}");
    }
}
