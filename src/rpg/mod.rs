//! The rules of a fight: health, stamina and mana, damage and armour, lasting effects.
//!
//! Everything here is plain data and arithmetic with no clock of its own (callers pass the
//! frame's `dt`), so the same rules run the player and every creature, and are tested
//! without a window.
//!
//!   items     weapons, staves, armour and potions (`assets/items.toml`), by registry id
//!   spells    the spells a staff or tome casts
//!   fighter   one combatant's actions: attack combos, heavy attacks, guarding and parrying,
//!             dodging, casting, drinking, staggering and dying
//!   ai        what a creature decides to do next
//!   creatures the creatures that fight (goblins, skeletons, bandits, ..)

pub mod ai;
pub mod creatures;
pub mod fighter;
pub mod items;
pub mod spells;

/// The kinds of damage. Weapons deal the first three, spells the last three.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    Slash,
    Blunt,
    Pierce,
    Fire,
    Frost,
    Arcane,
}

pub const ELEMENTS: usize = 6;

impl Element {
    pub const ALL: [Element; ELEMENTS] = [Element::Slash, Element::Blunt, Element::Pierce, Element::Fire, Element::Frost, Element::Arcane];

    pub fn parse(s: &str) -> Option<Element> {
        Element::ALL.into_iter().find(|e| e.key() == s)
    }

    pub fn key(self) -> &'static str {
        match self {
            Element::Slash => "slash",
            Element::Blunt => "blunt",
            Element::Pierce => "pierce",
            Element::Fire => "fire",
            Element::Frost => "frost",
            Element::Arcane => "arcane",
        }
    }

    pub fn physical(self) -> bool {
        matches!(self, Element::Slash | Element::Blunt | Element::Pierce)
    }
}

/// An amount of each kind of damage (or armour, or resistance), indexed by `Element as usize`.
pub type Amounts = [f32; ELEMENTS];

pub fn total(a: &Amounts) -> f32 {
    a.iter().sum()
}

/// A pool that drains and refills: health, stamina or mana. It refills at `regen` a second,
/// starting `delay` seconds after it was last spent.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Pool {
    pub value: f32,
    pub max: f32,
    pub regen: f32,
    pub delay: f32,
    wait: f32,
}

impl Pool {
    pub fn new(max: f32, regen: f32, delay: f32) -> Pool {
        Pool { value: max, max, regen, delay, wait: 0.0 }
    }

    /// Spends `x` if there is any left at all (a last swing on an empty bar still goes, as in
    /// most action games; the bar then stays empty until it starts refilling).
    pub fn spend(&mut self, x: f32) -> bool {
        if self.value <= 0.0 && x > 0.0 {
            return false;
        }
        self.value = (self.value - x).max(0.0);
        self.wait = self.delay;
        true
    }

    /// Spends exactly `x`, or nothing if there isn't that much (mana: a spell is cast whole).
    pub fn spend_all(&mut self, x: f32) -> bool {
        if self.value + 1e-4 < x {
            return false;
        }
        self.value = (self.value - x).max(0.0);
        self.wait = self.delay;
        true
    }

    /// Takes `x` away without pausing the refill (damage over time, a blocked blow's cost).
    pub fn drain(&mut self, x: f32) {
        self.value = (self.value - x).max(0.0);
    }

    pub fn add(&mut self, x: f32) {
        self.value = (self.value + x).min(self.max);
    }

    pub fn fill(&mut self) {
        self.value = self.max;
        self.wait = 0.0;
    }

    pub fn tick(&mut self, dt: f32, rate: f32) {
        if self.wait > 0.0 {
            self.wait -= dt;
            return;
        }
        self.add(self.regen * rate * dt);
    }

    pub fn frac(&self) -> f32 {
        if self.max > 0.0 {
            (self.value / self.max).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub fn empty(&self) -> bool {
        self.value <= 0.0
    }
}

/// Health, stamina and mana.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Vitals {
    pub health: Pool,
    pub stamina: Pool,
    pub mana: Pool,
}

impl Vitals {
    pub fn new(health: f32, stamina: f32, mana: f32) -> Vitals {
        Vitals {
            // Health mends slowly once out of a fight for a while.
            health: Pool::new(health, health * 0.012, 8.0),
            // Stamina comes back quickly but only once you stop swinging or guarding.
            stamina: Pool::new(stamina, 30.0, 0.9),
            mana: Pool::new(mana, mana * 0.05, 1.5),
        }
    }

    pub fn player() -> Vitals {
        Vitals::new(100.0, 100.0, 100.0)
    }

    pub fn alive(&self) -> bool {
        self.health.value > 0.0
    }
}

/// Lasting effects.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// Fire damage a second (`power`).
    Burning,
    /// Slowed: moves and refills stamina at (1 - `power`) of the usual rate.
    Chilled,
    /// Bleeding: damage a second that armour doesn't stop.
    Bleeding,
    /// Healing a second.
    Regenerating,
    /// Extra armour against everything physical.
    Warded,
}

impl Status {
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "burning" => Status::Burning,
            "chilled" => Status::Chilled,
            "bleeding" => Status::Bleeding,
            "regenerating" => Status::Regenerating,
            "warded" => Status::Warded,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Effect {
    pub status: Status,
    /// Seconds left.
    pub time: f32,
    pub power: f32,
}

/// The effects on one combatant (one of each status: a new one refreshes it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effects(pub Vec<Effect>);

impl Effects {
    pub fn add(&mut self, e: Effect) {
        match self.0.iter_mut().find(|x| x.status == e.status) {
            Some(x) => {
                x.time = x.time.max(e.time);
                x.power = x.power.max(e.power);
            }
            None => self.0.push(e),
        }
    }

    pub fn power(&self, s: Status) -> f32 {
        self.0.iter().find(|x| x.status == s).map_or(0.0, |x| x.power)
    }

    pub fn has(&self, s: Status) -> bool {
        self.0.iter().any(|x| x.status == s)
    }

    pub fn remove(&mut self, s: Status) {
        self.0.retain(|x| x.status != s);
    }

    /// Runs the effects for `dt`: what they do to health this frame (damage, healing).
    pub fn tick(&mut self, dt: f32) -> (f32, f32) {
        let (mut hurt, mut heal) = (0.0, 0.0);
        for e in &mut self.0 {
            let d = dt.min(e.time.max(0.0));
            match e.status {
                Status::Burning | Status::Bleeding => hurt += e.power * d,
                Status::Regenerating => heal += e.power * d,
                Status::Chilled | Status::Warded => {}
            }
            e.time -= dt;
        }
        self.0.retain(|e| e.time > 0.0);
        (hurt, heal)
    }

    /// How fast someone chilled moves and recovers (1 = normally).
    pub fn pace(&self) -> f32 {
        1.0 - self.power(Status::Chilled).clamp(0.0, 0.8)
    }
}

/// What protects a combatant: armour (flat, per kind of damage) and how much of each kind
/// gets through regardless (resistance: 1 normal, below 1 resists, above 1 weak to it).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Defense {
    pub armor: Amounts,
    pub resist: Amounts,
}

impl Default for Defense {
    fn default() -> Self {
        Defense { armor: [0.0; ELEMENTS], resist: [1.0; ELEMENTS] }
    }
}

impl Defense {
    /// How much of `amount` of `e` gets through: armour takes a share that grows with it but
    /// never all (50 armour halves a blow), then resistance scales the rest.
    pub fn through(&self, e: Element, amount: f32, extra_armor: f32) -> f32 {
        let i = e as usize;
        let armor = self.armor[i] + if e.physical() { extra_armor } else { 0.0 };
        amount * 50.0 / (50.0 + armor.max(0.0)) * self.resist[i]
    }

    pub fn add(&mut self, other: &Defense) {
        for i in 0..ELEMENTS {
            self.armor[i] += other.armor[i];
            self.resist[i] *= other.resist[i];
        }
    }
}

/// A blow, a bolt or a blast as it lands on someone.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub amounts: Amounts,
    /// How much it shakes the target (staggered when their poise runs out).
    pub poise: f32,
    /// How hard it pushes them away, metres a second.
    pub knockback: f32,
    pub effect: Option<Effect>,
    /// Which way it travels, across the ground (unit x, z), to tell a blow from the front
    /// (which a guard stops) from one from behind.
    pub dir: [f32; 2],
    /// A melee blow (a guard parries those) rather than a spell.
    pub melee: bool,
}

impl Hit {
    pub fn of(e: Element, amount: f32) -> Hit {
        let mut amounts = [0.0; ELEMENTS];
        amounts[e as usize] = amount;
        Hit { amounts, poise: amount, knockback: 0.0, effect: None, dir: [0.0, 1.0], melee: false }
    }

    pub fn scaled(mut self, k: f32) -> Hit {
        for a in &mut self.amounts {
            *a *= k;
        }
        self.poise *= k;
        self
    }
}

/// What came of a hit.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// Health it took.
    pub damage: f32,
    /// Stopped by a guard (most of it), parried (all of it, and the attacker reels), or
    /// rolled through (none of it).
    pub blocked: bool,
    pub parried: bool,
    pub dodged: bool,
    /// The guard gave way (out of stamina).
    pub guard_broken: bool,
    pub staggered: bool,
    pub killed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_spend_and_come_back_after_their_delay() {
        let mut p = Pool::new(100.0, 20.0, 1.0);
        assert!(p.spend(30.0));
        assert_eq!(p.value, 70.0);
        p.tick(0.5, 1.0);
        assert_eq!(p.value, 70.0, "still waiting");
        p.tick(0.5, 1.0);
        p.tick(1.0, 1.0);
        assert!((p.value - 90.0).abs() < 1e-3);
        p.tick(5.0, 1.0);
        assert_eq!(p.value, 100.0);
        // A last swing on a nearly empty bar goes; an empty bar stops it.
        p.value = 5.0;
        assert!(p.spend(30.0));
        assert!(!p.spend(30.0));
        // Mana is spent whole or not at all.
        let mut m = Pool::new(50.0, 1.0, 0.0);
        assert!(!m.spend_all(60.0) && m.value == 50.0);
        assert!(m.spend_all(50.0) && m.value == 0.0);
    }

    #[test]
    fn armour_softens_blows_and_resistance_scales_them() {
        let mut d = Defense::default();
        assert_eq!(d.through(Element::Slash, 20.0, 0.0), 20.0);
        d.armor[Element::Slash as usize] = 50.0;
        assert!((d.through(Element::Slash, 20.0, 0.0) - 10.0).abs() < 1e-4);
        // A ward adds armour against blows, not spells.
        assert!(d.through(Element::Blunt, 20.0, 50.0) < 10.1);
        assert_eq!(d.through(Element::Fire, 20.0, 50.0), 20.0);
        d.resist[Element::Fire as usize] = 2.0;
        assert_eq!(d.through(Element::Fire, 20.0, 0.0), 40.0);
    }

    #[test]
    fn effects_refresh_burn_and_heal_and_wear_off() {
        let mut e = Effects::default();
        e.add(Effect { status: Status::Burning, time: 2.0, power: 5.0 });
        e.add(Effect { status: Status::Burning, time: 1.0, power: 8.0 });
        assert_eq!(e.0.len(), 1);
        e.add(Effect { status: Status::Regenerating, time: 1.0, power: 4.0 });
        let (hurt, heal) = e.tick(1.0);
        assert!((hurt - 8.0).abs() < 1e-4 && (heal - 4.0).abs() < 1e-4);
        assert!(e.has(Status::Burning) && !e.has(Status::Regenerating));
        let (hurt, _) = e.tick(5.0);
        assert!((hurt - 8.0).abs() < 1e-4, "only what was left of it: {hurt}");
        assert!(e.0.is_empty());
        e.add(Effect { status: Status::Chilled, time: 3.0, power: 0.4 });
        assert!((e.pace() - 0.6).abs() < 1e-5);
    }
}
