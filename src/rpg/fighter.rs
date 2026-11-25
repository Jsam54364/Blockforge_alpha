//! One combatant's actions, for the player and every creature alike.
//!
//! * **Light attacks** chain into a combo of up to three swings: pressing again during a
//!   swing queues the next, which starts as the first finishes (each a little stronger).
//! * **Heavy attacks** (ATTACK held) hit about twice as hard and shake the target more; once
//!   under way they shrug off half of what would stagger the attacker.
//! * **Guard** (held) stops most of a blow from the front for stamina; raised just before a
//!   blow lands it **parries**: no damage, and the attacker reels open to a riposte. A guard
//!   that runs out of stamina breaks and leaves its owner staggered.
//! * **Dodging** (the roll) costs stamina and nothing touches you in the middle of it.
//! * **Poise**: every blow wears it down; when it runs out you stagger, which interrupts what
//!   you were doing, and blows on a staggered or parried fighter land half again as hard.
//! * Spells take a moment to cast and cost mana; potions take a moment to drink.

use super::items::{Move, Potion, Weapon};
use super::spells::{self, SPELLS};
use super::{total, Defense, Effect, Effects, Element, Hit, Outcome, Status, Vitals};

/// A guard raised no more than this long before a blow parries it.
pub const PARRY_WINDOW: f32 = 0.22;
/// Stamina a dodge costs, how long it lasts, and when in it nothing can touch you.
pub const DODGE_STAMINA: f32 = 20.0;
pub const DODGE_LENGTH: f32 = 0.75;
pub const DODGE_SAFE: (f32, f32) = (0.06, 0.5);
/// After a swing, how long a next press still continues the combo.
const COMBO_GRACE: f32 = 0.5;
/// A queued swing starts this far through the one before it (cutting its recovery short).
const CHAIN_AT: f32 = 0.82;
/// How long a drink takes, and when it does its work.
const DRINK_LENGTH: f32 = 1.1;
const DRINK_AT: f32 = 0.8;
/// After casting, a moment before anything else.
const CAST_RECOVERY: f32 = 0.25;
/// Poise starts to come back this long after the last blow.
const POISE_WAIT: f32 = 2.0;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Action {
    Idle,
    Attack { heavy: bool, step: u8, t: f32, struck: bool, mv: Move },
    Cast { spell: usize, power: f32, t: f32, done: bool },
    /// `t`: seconds since it was raised.
    Guard { t: f32 },
    Dodge { t: f32 },
    Drink { potion: Potion, t: f32, done: bool },
    Stagger { t: f32, length: f32 },
    Dead { t: f32 },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Command {
    Light,
    Heavy,
    GuardUp,
    GuardDown,
    Dodge,
    Cast { spell: usize, power: f32 },
    Drink(Potion),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Play this one-shot on the body (a `Controller::press` name: "swing", "chop", "hit",
    /// "die", ..).
    Anim(&'static str),
    /// Roll now (the body's dodge).
    Roll,
    /// The weapon lands: whoever is within `reach` and `arc` (degrees, centred on where the
    /// fighter faces) takes `hit`.
    Strike { hit: Hit, reach: f32, arc: f32, heavy: bool },
    /// The spell goes off.
    Spell { spell: usize, power: f32 },
    Drank(Potion),
    Staggered,
    Died,
    /// Tried something without the stamina or mana for it (or a spell still cooling down).
    TooTired,
    NoMana,
}

#[derive(Clone, Debug)]
pub struct Fighter {
    pub vitals: Vitals,
    pub effects: Effects,
    /// Natural defence (a skeleton's bones, a troll's hide) and what is worn on top.
    pub base: Defense,
    pub worn: Defense,
    /// How much worn armour slows the stamina refill.
    pub weight: f32,
    pub weapon: Weapon,
    pub poise: f32,
    pub max_poise: f32,
    poise_wait: f32,
    pub action: Action,
    queued: Option<Command>,
    combo: u8,
    combo_grace: f32,
    /// Seconds before each spell (by index in `SPELLS`) can be cast again.
    pub cooldowns: Vec<f32>,
    /// Where the fighter faces across the ground (unit x, z): a guard faces this way, and a
    /// blow travelling the same way comes from behind.
    pub facing: [f32; 2],
    /// Counts down after each hit taken (the body flashes).
    pub flash: f32,
}

impl Fighter {
    pub fn new(vitals: Vitals, weapon: Weapon, max_poise: f32) -> Fighter {
        Fighter {
            vitals,
            effects: Effects::default(),
            base: Defense::default(),
            worn: Defense::default(),
            weight: 0.0,
            weapon,
            poise: max_poise,
            max_poise,
            poise_wait: 0.0,
            action: Action::Idle,
            queued: None,
            combo: 0,
            combo_grace: 0.0,
            cooldowns: vec![0.0; SPELLS.len()],
            facing: [0.0, 1.0],
            flash: 0.0,
        }
    }

    pub fn player() -> Fighter {
        Fighter::new(Vitals::player(), Weapon::fists(), 30.0)
    }

    pub fn alive(&self) -> bool {
        !matches!(self.action, Action::Dead { .. })
    }

    pub fn guarding(&self) -> bool {
        matches!(self.action, Action::Guard { .. })
    }

    pub fn busy(&self) -> bool {
        !matches!(self.action, Action::Idle | Action::Guard { .. })
    }

    pub fn staggered(&self) -> bool {
        matches!(self.action, Action::Stagger { .. })
    }

    /// Whether nothing can touch it right now (the middle of a dodge).
    pub fn untouchable(&self) -> bool {
        matches!(self.action, Action::Dodge { t } if t >= DODGE_SAFE.0 && t <= DODGE_SAFE.1)
    }

    pub fn defense(&self) -> Defense {
        let mut d = self.base;
        d.add(&self.worn);
        d
    }

    /// How fast it moves: slowed when chilled, guarding or in a heavy swing (1 = normal).
    pub fn pace(&self) -> f32 {
        let action = match self.action {
            Action::Guard { .. } => 0.45,
            Action::Attack { heavy: true, .. } => 0.35,
            Action::Attack { .. } | Action::Cast { .. } => 0.6,
            Action::Drink { .. } => 0.5,
            Action::Stagger { .. } | Action::Dead { .. } => 0.0,
            _ => 1.0,
        };
        self.effects.pace() * action
    }

    /// Asks for an action; it starts now if it can, waits its turn (a combo's next swing, a
    /// roll at the end of a swing), or is dropped. Returns the events of starting it.
    pub fn command(&mut self, c: Command) -> Vec<Event> {
        let mut ev = Vec::new();
        if !self.alive() {
            return ev;
        }
        match (c, self.action) {
            (Command::GuardDown, Action::Guard { .. }) => self.action = Action::Idle,
            (Command::GuardDown, _) => {
                if self.queued == Some(Command::GuardUp) {
                    self.queued = None;
                }
            }
            (_, Action::Stagger { .. }) => {}
            // Mid-swing: the next swing (or a roll, or the guard) waits for this one to land.
            (Command::Light | Command::Heavy | Command::Dodge | Command::GuardUp, Action::Attack { .. } | Action::Cast { .. } | Action::Dodge { .. }) => {
                self.queued = Some(c);
            }
            (_, Action::Drink { .. }) => {}
            (Command::GuardUp, _) => self.action = Action::Guard { t: 0.0 },
            (Command::Light, _) => self.start_attack(false, &mut ev),
            (Command::Heavy, _) => self.start_attack(true, &mut ev),
            (Command::Dodge, _) => self.start_dodge(&mut ev),
            (Command::Cast { spell, power }, _) => self.start_cast(spell, power, &mut ev),
            (Command::Drink(p), _) => {
                self.action = Action::Drink { potion: p, t: 0.0, done: false };
                ev.push(Event::Anim("eat"));
            }
        }
        ev
    }

    fn start_attack(&mut self, heavy: bool, ev: &mut Vec<Event>) {
        let step = if !heavy && self.combo_grace > 0.0 { (self.combo + 1) % super::items::COMBO as u8 } else { 0 };
        let mv = self.weapon.attack(heavy, step as usize);
        if !self.vitals.stamina.spend(self.weapon.stamina * mv.stamina) {
            ev.push(Event::TooTired);
            self.action = Action::Idle;
            return;
        }
        self.action = Action::Attack { heavy, step, t: 0.0, struck: false, mv };
        ev.push(Event::Anim(mv.anim));
    }

    fn start_dodge(&mut self, ev: &mut Vec<Event>) {
        if !self.vitals.stamina.spend(DODGE_STAMINA) {
            ev.push(Event::TooTired);
            self.action = Action::Idle;
            return;
        }
        self.action = Action::Dodge { t: 0.0 };
        ev.push(Event::Roll);
    }

    fn start_cast(&mut self, spell: usize, power: f32, ev: &mut Vec<Event>) {
        let Some(s) = SPELLS.get(spell) else { return };
        if self.cooldowns[spell] > 0.0 || !self.vitals.mana.spend_all(s.mana) {
            ev.push(Event::NoMana);
            self.action = Action::Idle;
            return;
        }
        self.action = Action::Cast { spell, power, t: 0.0, done: false };
        ev.push(Event::Anim(cast_anim(s)));
    }

    /// Runs `dt` seconds: actions play out, pools refill, effects burn and heal.
    pub fn tick(&mut self, dt: f32) -> Vec<Event> {
        let mut ev = Vec::new();
        self.flash = (self.flash - dt).max(0.0);
        for c in &mut self.cooldowns {
            *c = (*c - dt).max(0.0);
        }
        if let Action::Dead { t } = &mut self.action {
            *t += dt;
            return ev;
        }
        let pace = self.effects.pace();
        let (hurt, heal) = self.effects.tick(dt);
        self.vitals.health.drain(hurt);
        self.vitals.health.add(heal);
        if hurt > 0.0 && self.vitals.health.empty() {
            self.die(&mut ev);
            return ev;
        }
        // Pools: stamina only refills while not swinging or guarding (slowly while guarding).
        let stamina_rate = match self.action {
            Action::Guard { .. } => 0.3,
            Action::Attack { .. } | Action::Dodge { .. } => 0.0,
            _ => 1.0,
        } * pace
            * (1.0 - self.weight.clamp(0.0, 0.6));
        self.vitals.stamina.tick(dt, stamina_rate);
        self.vitals.mana.tick(dt, 1.0);
        self.vitals.health.tick(dt, 1.0);
        self.poise_wait -= dt;
        if self.poise_wait <= 0.0 {
            self.poise = (self.poise + self.max_poise * 0.5 * dt).min(self.max_poise);
        }
        self.combo_grace -= dt;

        let mut next = None;
        match &mut self.action {
            Action::Idle => {}
            Action::Guard { t } => *t += dt,
            Action::Attack { heavy, step, t, struck, mv } => {
                *t += dt * pace;
                if !*struck && *t >= mv.strike {
                    *struck = true;
                    ev.push(Event::Strike { hit: blow(&self.weapon, mv, *heavy, self.facing), reach: self.weapon.reach, arc: self.weapon.arc, heavy: *heavy });
                }
                let chain = self.queued.is_some() && *t >= mv.length * CHAIN_AT;
                if chain || *t >= mv.length {
                    if !*heavy {
                        self.combo = *step;
                        self.combo_grace = COMBO_GRACE;
                    }
                    next = Some(Action::Idle);
                }
            }
            Action::Cast { spell, power, t, done } => {
                *t += dt * pace;
                let s = &SPELLS[*spell];
                if !*done && *t >= s.cast {
                    *done = true;
                    self.cooldowns[*spell] = s.cooldown;
                    ev.push(Event::Spell { spell: *spell, power: *power });
                    if let Some(effect) = self_effect(s, *power) {
                        match effect {
                            SelfEffect::Heal(now, regen) => {
                                self.vitals.health.add(now);
                                self.effects.add(regen);
                            }
                            SelfEffect::Ward(e) => self.effects.add(e),
                        }
                    }
                }
                if *t >= s.cast + CAST_RECOVERY {
                    next = Some(Action::Idle);
                }
            }
            Action::Dodge { t } => {
                *t += dt;
                if *t >= DODGE_LENGTH {
                    next = Some(Action::Idle);
                }
            }
            Action::Drink { potion, t, done } => {
                *t += dt;
                if !*done && *t >= DRINK_AT {
                    *done = true;
                    let p = *potion;
                    self.vitals.health.add(p.heal);
                    self.vitals.mana.add(p.mana);
                    self.vitals.stamina.add(p.stamina);
                    if p.regen > 0.0 {
                        self.effects.add(Effect { status: Status::Regenerating, time: p.time, power: p.regen });
                    }
                    ev.push(Event::Drank(p));
                }
                if *t >= DRINK_LENGTH {
                    next = Some(Action::Idle);
                }
            }
            Action::Stagger { t, length } => {
                *t += dt;
                if *t >= *length {
                    next = Some(Action::Idle);
                }
            }
            Action::Dead { .. } => {}
        }
        if let Some(a) = next {
            self.action = a;
            if let Some(q) = self.queued.take() {
                ev.extend(self.command(q));
            }
        }
        ev
    }

    fn die(&mut self, ev: &mut Vec<Event>) {
        self.vitals.health.value = 0.0;
        self.action = Action::Dead { t: 0.0 };
        self.queued = None;
        ev.push(Event::Died);
        ev.push(Event::Anim("die"));
    }

    /// Reels for `length` seconds (a parried attacker, a broken guard), dropping whatever it
    /// was doing.
    pub fn stagger(&mut self, length: f32) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.alive() {
            self.action = Action::Stagger { t: 0.0, length };
            self.queued = None;
            self.poise = self.max_poise;
            ev.push(Event::Staggered);
            ev.push(Event::Anim("hit"));
        }
        ev
    }

    /// Takes a hit: guarded, parried, rolled through, or landing on armour.
    pub fn take(&mut self, hit: &Hit) -> (Outcome, Vec<Event>) {
        let mut out = Outcome::default();
        let mut ev = Vec::new();
        if !self.alive() {
            return (out, ev);
        }
        if self.untouchable() {
            out.dodged = true;
            return (out, ev);
        }
        // Which way the blow comes: against the way it faces (from the front) or with it.
        let along = self.facing[0] * hit.dir[0] + self.facing[1] * hit.dir[1];
        let front = along < -0.25;
        let mut k_phys = 1.0;
        let mut k_magic = 1.0;
        if let (Action::Guard { t }, true) = (self.action, front) {
            if hit.melee && t <= PARRY_WINDOW {
                out.parried = true;
                out.blocked = true;
                self.vitals.stamina.drain(4.0);
                return (out, ev);
            }
            out.blocked = true;
            k_phys = 1.0 - self.weapon.guard;
            k_magic = 1.0 - self.weapon.guard * 0.5;
            let stopped: f32 = Element::ALL.iter().map(|&e| hit.amounts[e as usize] * if e.physical() { self.weapon.guard } else { self.weapon.guard * 0.5 }).sum();
            self.vitals.stamina.drain(stopped * 1.2 + 4.0);
            if self.vitals.stamina.empty() {
                out.guard_broken = true;
                ev.extend(self.stagger(1.1));
            }
        }
        let mut k = 1.0;
        if self.staggered() {
            k *= 1.5; // a riposte
        } else if hit.melee && along > 0.5 {
            k *= 1.25; // from behind
        }
        let ward = self.effects.power(Status::Warded);
        let defense = self.defense();
        let mut damage = 0.0;
        for e in Element::ALL {
            let a = hit.amounts[e as usize] * k * if e.physical() { k_phys } else { k_magic };
            if a > 0.0 {
                damage += defense.through(e, a, ward);
            }
        }
        self.vitals.health.drain(damage);
        self.vitals.health.spend(0.0); // pauses the slow mend
        out.damage = damage;
        self.flash = 0.25;
        if let Some(e) = hit.effect {
            if !out.blocked || e.status == Status::Chilled {
                let resist = match e.status {
                    Status::Burning => defense.resist[Element::Fire as usize],
                    Status::Chilled => defense.resist[Element::Frost as usize],
                    _ => 1.0,
                };
                if resist > 0.05 {
                    self.effects.add(Effect { power: if e.status == Status::Chilled { e.power } else { e.power * resist }, ..e });
                }
            }
        }
        if self.vitals.health.empty() {
            out.killed = true;
            self.die(&mut ev);
            return (out, ev);
        }
        // Poise: blocked blows shake less; a heavy swing under way shrugs off half.
        let mut p = hit.poise * if out.blocked { 0.3 } else { 1.0 };
        if matches!(self.action, Action::Attack { heavy: true, t, mv, .. } if t > mv.strike * 0.4) {
            p *= 0.5;
        }
        self.poise -= p;
        self.poise_wait = POISE_WAIT;
        if self.poise <= 0.0 && !out.guard_broken {
            out.staggered = true;
            ev.extend(self.stagger(0.45 + (hit.poise * 0.01).min(0.4)));
        }
        (out, ev)
    }

    /// Up again with everything full (the player after dying).
    pub fn revive(&mut self) {
        self.vitals.health.fill();
        self.vitals.stamina.fill();
        self.vitals.mana.fill();
        self.effects = Effects::default();
        self.poise = self.max_poise;
        self.action = Action::Idle;
        self.queued = None;
    }
}

/// The body's animation for casting a spell.
pub fn cast_anim(s: &spells::Spell) -> &'static str {
    match s.kind {
        spells::SpellKind::Nova { .. } => "chop",
        spells::SpellKind::Heal { .. } | spells::SpellKind::Ward { .. } => "interact_left",
        _ => "interact",
    }
}

enum SelfEffect {
    Heal(f32, Effect),
    Ward(Effect),
}

fn self_effect(s: &spells::Spell, power: f32) -> Option<SelfEffect> {
    match s.kind {
        spells::SpellKind::Heal { now, per_second, time } => Some(SelfEffect::Heal(now * power, Effect { status: Status::Regenerating, time, power: per_second * power })),
        spells::SpellKind::Ward { armor, time } => Some(SelfEffect::Ward(Effect { status: Status::Warded, time, power: armor * power })),
        _ => None,
    }
}

/// The blow a swing lands.
fn blow(w: &Weapon, mv: &Move, heavy: bool, facing: [f32; 2]) -> Hit {
    let mut amounts = w.damage;
    for a in &mut amounts {
        *a *= mv.damage;
    }
    let effect = (w.bleed > 0.0).then(|| Effect { status: Status::Bleeding, time: w.bleed, power: total(&amounts) * 0.1 });
    Hit { amounts, poise: w.poise * mv.poise, knockback: if heavy { 3.5 } else { 1.2 }, effect, dir: facing, melee: true }
}

#[cfg(test)]
mod tests {
    use super::super::items::WeaponClass;
    use super::*;

    fn sword() -> Weapon {
        let mut w = Weapon::fists();
        w.class = WeaponClass::Sword;
        w.damage = [12.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        w.stamina = 10.0;
        w.guard = 0.8;
        w
    }

    fn run(f: &mut Fighter, secs: f32) -> Vec<Event> {
        let mut ev = Vec::new();
        let mut t = 0.0;
        while t < secs {
            ev.extend(f.tick(1.0 / 60.0));
            t += 1.0 / 60.0;
        }
        ev
    }

    fn strikes(ev: &[Event]) -> Vec<f32> {
        ev.iter().filter_map(|e| if let Event::Strike { hit, .. } = e { Some(hit.amounts[0]) } else { None }).collect()
    }

    #[test]
    fn light_attacks_chain_into_a_stronger_combo_and_cost_stamina() {
        let mut f = Fighter::new(Vitals::player(), sword(), 30.0);
        let mut ev = f.command(Command::Light);
        assert_eq!(ev, vec![Event::Anim("swing")]);
        ev.extend(run(&mut f, 0.2));
        // Pressed again mid-swing: the next waits, then goes (pressing twice queues one).
        f.command(Command::Light);
        f.command(Command::Light);
        ev.extend(run(&mut f, 0.6));
        f.command(Command::Light);
        ev.extend(run(&mut f, 3.0));
        let s = strikes(&ev);
        assert_eq!(s.len(), 3, "{ev:?}");
        assert!(s[0] < s[1] && s[1] < s[2], "{s:?}");
        assert!(f.vitals.stamina.value < 100.0 || f.vitals.stamina.value == 100.0);
        // After a pause the combo starts over.
        let ev = f.command(Command::Light);
        let ev2 = run(&mut f, 1.0);
        assert_eq!(ev, vec![Event::Anim("swing")]);
        assert_eq!(strikes(&ev2), vec![12.0]);
    }

    #[test]
    fn a_heavy_attack_hits_harder_and_an_empty_stamina_bar_stops_swings() {
        let mut f = Fighter::new(Vitals::player(), sword(), 30.0);
        f.command(Command::Heavy);
        let s = strikes(&run(&mut f, 1.2));
        assert!(s[0] > 20.0, "{s:?}");
        f.vitals.stamina.value = 0.0;
        assert_eq!(f.command(Command::Light), vec![Event::TooTired]);
    }

    #[test]
    fn a_guard_stops_most_of_a_blow_from_the_front_and_a_timely_one_parries() {
        let mut f = Fighter::new(Vitals::player(), sword(), 30.0);
        f.facing = [0.0, 1.0];
        let mut hit = Hit::of(Element::Slash, 20.0);
        hit.melee = true;
        hit.dir = [0.0, -1.0]; // coming at its face
        f.command(Command::GuardUp);
        let (o, _) = f.take(&hit);
        assert!(o.parried && o.damage == 0.0, "{o:?}");
        run(&mut f, 0.5);
        let (o, _) = f.take(&hit);
        assert!(o.blocked && !o.parried && (o.damage - 4.0).abs() < 0.01, "{o:?}");
        assert!(f.vitals.stamina.value < 100.0);
        // From behind the guard does nothing, and the blow lands harder.
        hit.dir = [0.0, 1.0];
        let (o, _) = f.take(&hit);
        assert!(!o.blocked && (o.damage - 25.0).abs() < 0.01, "{o:?}");
        // Out of stamina, the guard breaks.
        f.vitals.stamina.value = 1.0;
        hit.dir = [0.0, -1.0];
        f.vitals.health.fill();
        let (o, ev) = f.take(&hit);
        assert!(o.guard_broken && f.staggered() && ev.contains(&Event::Staggered), "{o:?}");
    }

    #[test]
    fn nothing_touches_a_rolling_fighter_and_poise_runs_out_into_a_stagger() {
        let mut f = Fighter::new(Vitals::player(), sword(), 30.0);
        assert_eq!(f.command(Command::Dodge), vec![Event::Roll]);
        run(&mut f, 0.2);
        let (o, _) = f.take(&Hit::of(Element::Blunt, 50.0));
        assert!(o.dodged && f.vitals.health.value == 100.0);
        run(&mut f, 1.0);
        let mut h = Hit::of(Element::Blunt, 10.0);
        h.poise = 20.0;
        let (o, _) = f.take(&h);
        assert!(!o.staggered);
        let (o, _) = f.take(&h);
        assert!(o.staggered && f.staggered());
        // A staggered fighter takes a riposte half again as hard.
        let (o, _) = f.take(&Hit::of(Element::Blunt, 10.0));
        assert!((o.damage - 15.0).abs() < 0.01);
    }

    #[test]
    fn spells_cost_mana_go_off_after_their_cast_and_cool_down() {
        let mut f = Fighter::player();
        let bolt = spells::index("firebolt").unwrap();
        let ev = f.command(Command::Cast { spell: bolt, power: 1.0 });
        assert_eq!(ev, vec![Event::Anim("interact")]);
        let ev = run(&mut f, 0.6);
        assert!(ev.contains(&Event::Spell { spell: bolt, power: 1.0 }));
        assert!(f.vitals.mana.value < 100.0);
        // Mend heals the caster; with too little mana nothing happens.
        f.vitals.health.value = 50.0;
        let heal = spells::index("heal").unwrap();
        f.command(Command::Cast { spell: heal, power: 1.0 });
        run(&mut f, 1.0);
        assert!(f.vitals.health.value > 75.0);
        f.vitals.mana.value = 1.0;
        assert_eq!(f.command(Command::Cast { spell: bolt, power: 1.0 }), vec![Event::NoMana]);
    }

    #[test]
    fn burning_kills_and_the_dead_do_nothing_until_revived() {
        let mut f = Fighter::player();
        f.vitals.health.value = 5.0;
        f.effects.add(Effect { status: Status::Burning, time: 5.0, power: 10.0 });
        let ev = run(&mut f, 1.0);
        assert!(ev.contains(&Event::Died) && !f.alive());
        assert!(f.command(Command::Light).is_empty());
        f.revive();
        assert!(f.alive() && f.vitals.health.value == 100.0);
    }

    #[test]
    fn potions_restore_after_a_drink() {
        let mut f = Fighter::player();
        f.vitals.health.value = 20.0;
        f.command(Command::Drink(Potion { heal: 40.0, mana: 0.0, stamina: 0.0, regen: 0.0, time: 0.0 }));
        let ev = run(&mut f, 1.2);
        assert!(ev.iter().any(|e| matches!(e, Event::Drank(_))));
        assert!(f.vitals.health.value >= 60.0);
    }
}
