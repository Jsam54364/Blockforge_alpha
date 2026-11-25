//! Spells. A staff (or tome) casts two: one on a tap of ATTACK and a stronger one when it is
//! held (see `items::Staff`). Spells cost mana, take a moment to cast and then cool down.

use super::{Effect, Element, Hit, Status};

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SpellKind {
    /// A bolt that flies straight (`speed` m/s, a slight drop) and hurts what it hits, and
    /// everything within `splash` metres of where it bursts.
    Bolt { speed: f32, element: Element, damage: f32, splash: f32, effect: Option<(Status, f32, f32)> },
    /// Strikes the first thing along the line of sight within `range` at once.
    Arc { range: f32, element: Element, damage: f32, chain: u8 },
    /// A blast all around the caster.
    Nova { radius: f32, element: Element, damage: f32, effect: Option<(Status, f32, f32)>, knockback: f32 },
    /// Heals the caster now and over the next seconds.
    Heal { now: f32, per_second: f32, time: f32 },
    /// Extra armour for a while.
    Ward { armor: f32, time: f32 },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Spell {
    pub key: &'static str,
    pub name: &'static str,
    pub mana: f32,
    /// Seconds from starting the cast to the spell going off.
    pub cast: f32,
    /// Seconds after it before it can be cast again.
    pub cooldown: f32,
    pub kind: SpellKind,
    /// Its colour (bolts, bursts, the caster's hands), linear RGB.
    pub colour: [f32; 3],
}

pub const SPELLS: &[Spell] = &[
    Spell {
        key: "firebolt",
        name: "Firebolt",
        mana: 12.0,
        cast: 0.35,
        cooldown: 0.25,
        kind: SpellKind::Bolt { speed: 30.0, element: Element::Fire, damage: 16.0, splash: 0.0, effect: Some((Status::Burning, 3.0, 4.0)) },
        colour: [1.0, 0.45, 0.1],
    },
    Spell {
        key: "fireball",
        name: "Fireball",
        mana: 30.0,
        cast: 0.8,
        cooldown: 2.5,
        kind: SpellKind::Bolt { speed: 22.0, element: Element::Fire, damage: 30.0, splash: 3.0, effect: Some((Status::Burning, 4.0, 5.0)) },
        colour: [1.0, 0.35, 0.05],
    },
    Spell {
        key: "frost_shard",
        name: "Frost Shard",
        mana: 10.0,
        cast: 0.3,
        cooldown: 0.25,
        kind: SpellKind::Bolt { speed: 36.0, element: Element::Frost, damage: 12.0, splash: 0.0, effect: Some((Status::Chilled, 4.0, 0.4)) },
        colour: [0.55, 0.85, 1.0],
    },
    Spell {
        key: "frost_nova",
        name: "Frost Nova",
        mana: 28.0,
        cast: 0.6,
        cooldown: 4.0,
        kind: SpellKind::Nova { radius: 5.0, element: Element::Frost, damage: 14.0, effect: Some((Status::Chilled, 5.0, 0.6)), knockback: 4.0 },
        colour: [0.6, 0.9, 1.0],
    },
    Spell {
        key: "lightning",
        name: "Lightning",
        mana: 16.0,
        cast: 0.25,
        cooldown: 0.9,
        kind: SpellKind::Arc { range: 20.0, element: Element::Arcane, damage: 20.0, chain: 0 },
        colour: [0.75, 0.7, 1.0],
    },
    Spell {
        key: "chain_lightning",
        name: "Chain Lightning",
        mana: 34.0,
        cast: 0.6,
        cooldown: 3.0,
        kind: SpellKind::Arc { range: 22.0, element: Element::Arcane, damage: 24.0, chain: 3 },
        colour: [0.7, 0.65, 1.0],
    },
    Spell {
        key: "heal",
        name: "Mend",
        mana: 24.0,
        cast: 0.7,
        cooldown: 1.0,
        kind: SpellKind::Heal { now: 25.0, per_second: 4.0, time: 5.0 },
        colour: [0.55, 1.0, 0.5],
    },
    Spell {
        key: "stone_ward",
        name: "Stone Ward",
        mana: 30.0,
        cast: 0.6,
        cooldown: 12.0,
        kind: SpellKind::Ward { armor: 45.0, time: 15.0 },
        colour: [0.85, 0.75, 0.5],
    },
];

pub fn by_key(key: &str) -> Option<&'static Spell> {
    SPELLS.iter().find(|s| s.key == key)
}

pub fn index(key: &str) -> Option<usize> {
    SPELLS.iter().position(|s| s.key == key)
}

impl Spell {
    /// The hit a bolt, arc or nova lands, its strength scaled by `power`.
    pub fn hit(&self, power: f32) -> Option<Hit> {
        let (element, damage, effect, knockback) = match self.kind {
            SpellKind::Bolt { element, damage, effect, .. } => (element, damage, effect, 1.5),
            SpellKind::Arc { element, damage, .. } => (element, damage, None, 0.5),
            SpellKind::Nova { element, damage, effect, knockback, .. } => (element, damage, effect, knockback),
            SpellKind::Heal { .. } | SpellKind::Ward { .. } => return None,
        };
        let mut h = Hit::of(element, damage * power);
        h.poise = damage * 0.6 * power;
        h.knockback = knockback;
        h.effect = effect.map(|(status, time, p)| Effect { status, time, power: if status == Status::Chilled { p } else { p * power } });
        Some(h)
    }
}

/// A bolt in flight.
#[derive(Clone, Debug, PartialEq)]
pub struct Bolt {
    pub spell: usize,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub power: f32,
    /// Who cast it (so it doesn't hit them): 0 the player, else a creature's id.
    pub owner: u32,
    pub age: f32,
}

/// Bolts fall a little (they arc gently over long shots) and burn out after this long.
pub const BOLT_GRAVITY: f32 = 3.0;
pub const BOLT_LIFE: f32 = 3.0;

impl Bolt {
    /// Moves it on by `dt`, in small steps, stopping where `blocked` says a point is inside
    /// the world or `hits` finds someone there. Returns where it burst (and on whom), or None
    /// while it flies on.
    pub fn fly(&mut self, dt: f32, blocked: &dyn Fn([f32; 3]) -> bool, hits: &dyn Fn([f32; 3]) -> Option<u32>) -> Option<([f32; 3], Option<u32>)> {
        self.age += dt;
        let speed = (self.vel[0] * self.vel[0] + self.vel[1] * self.vel[1] + self.vel[2] * self.vel[2]).sqrt();
        let steps = ((speed * dt) / 0.25).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            self.vel[1] -= BOLT_GRAVITY * h;
            for k in 0..3 {
                self.pos[k] += self.vel[k] * h;
            }
            if let Some(who) = hits(self.pos) {
                return Some((self.pos, Some(who)));
            }
            if blocked(self.pos) {
                return Some((self.pos, None));
            }
        }
        if self.age > BOLT_LIFE {
            return Some((self.pos, None));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spell_is_found_by_key_and_damaging_ones_make_hits() {
        for s in SPELLS {
            assert_eq!(by_key(s.key).unwrap().name, s.name);
            assert!(s.mana > 0.0 && s.cast > 0.0);
            let damaging = !matches!(s.kind, SpellKind::Heal { .. } | SpellKind::Ward { .. });
            assert_eq!(s.hit(1.0).is_some(), damaging, "{}", s.key);
        }
        let h = by_key("firebolt").unwrap().hit(1.5).unwrap();
        assert_eq!(h.amounts[Element::Fire as usize], 24.0);
        assert_eq!(h.effect.unwrap().status, Status::Burning);
    }

    #[test]
    fn a_bolt_flies_until_it_meets_a_wall_or_someone() {
        let mut b = Bolt { spell: 0, pos: [0.0, 1.0, 0.0], vel: [0.0, 0.0, 30.0], power: 1.0, owner: 0, age: 0.0 };
        let wall = |p: [f32; 3]| p[2] > 10.0;
        let nobody = |_: [f32; 3]| None;
        assert_eq!(b.fly(0.1, &wall, &nobody), None);
        let (at, who) = b.fly(0.5, &wall, &nobody).unwrap();
        assert!(who.is_none() && at[2] > 10.0 && at[2] < 10.3, "{at:?}");
        let mut b = Bolt { spell: 0, pos: [0.0, 1.0, 0.0], vel: [0.0, 0.0, 30.0], power: 1.0, owner: 0, age: 0.0 };
        let goblin = |p: [f32; 3]| (p[2] > 5.0).then_some(7);
        let (_, who) = b.fly(1.0, &wall, &goblin).unwrap();
        assert_eq!(who, Some(7));
    }
}
