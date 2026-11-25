//! The creatures that fight. They share the player's body (the HyRig character), scaled and
//! recoloured (`skin`, see `fs_char` in the app's shader), and carry real items.

use super::{Defense, Element};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Style {
    /// Closes in and swings; `guards` is how often it raises its guard against a swing (0..1),
    /// `dodges` how often it rolls away from one.
    Melee,
    /// Keeps its distance and casts, backing off when rushed.
    Caster,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CreatureDef {
    pub key: &'static str,
    pub name: &'static str,
    /// The body's colours (an index into the shader's skins, 0..4: 0 is the player's, 1
    /// goblin green, 2 bone, 3 bandit leathers, 4 troll hide).
    pub skin: u8,
    /// Its size against the player's.
    pub scale: f32,
    pub health: f32,
    pub stamina: f32,
    pub mana: f32,
    pub poise: f32,
    /// How fast it moves against the player (the stick's push it walks with when chasing).
    pub speed: f32,
    /// What it carries (an item name) and what it wears.
    pub weapon: &'static str,
    pub armor: f32,
    /// Resistances: what it takes of each kind of damage (1 = all of it).
    pub resist: [f32; 6],
    pub style: Style,
    pub guards: f32,
    pub dodges: f32,
    /// Below this share of its health it runs (0: never).
    pub flees: f32,
    /// Notices the player this far away, metres.
    pub sight: f32,
    /// Only out at night (and burns away at dawn).
    pub night: bool,
}

pub const CREATURES: &[CreatureDef] = &[
    CreatureDef {
        key: "goblin",
        name: "Goblin",
        skin: 1,
        scale: 0.74,
        health: 38.0,
        stamina: 70.0,
        mana: 0.0,
        poise: 14.0,
        speed: 1.0,
        weapon: "iron_dagger",
        armor: 4.0,
        resist: [1.0, 1.0, 1.0, 1.25, 1.0, 1.0],
        style: Style::Melee,
        guards: 0.0,
        dodges: 0.35,
        flees: 0.25,
        sight: 20.0,
        night: false,
    },
    CreatureDef {
        key: "goblin_shaman",
        name: "Goblin Shaman",
        skin: 1,
        scale: 0.78,
        health: 34.0,
        stamina: 50.0,
        mana: 120.0,
        poise: 10.0,
        speed: 0.8,
        weapon: "ember_staff",
        armor: 2.0,
        resist: [1.0, 1.0, 1.0, 0.5, 1.2, 1.0],
        style: Style::Caster,
        guards: 0.0,
        dodges: 0.2,
        flees: 0.0,
        sight: 24.0,
        night: false,
    },
    CreatureDef {
        key: "skeleton",
        name: "Skeleton",
        skin: 2,
        scale: 1.0,
        health: 55.0,
        stamina: 90.0,
        mana: 0.0,
        poise: 22.0,
        speed: 0.8,
        weapon: "iron_sword",
        armor: 8.0,
        // Bones shatter under a mace and let arrows and points through; the cold is nothing
        // to them.
        resist: [0.8, 1.6, 0.5, 1.0, 0.3, 1.2],
        style: Style::Melee,
        guards: 0.25,
        dodges: 0.0,
        flees: 0.0,
        sight: 22.0,
        night: true,
    },
    CreatureDef {
        key: "bandit",
        name: "Bandit",
        skin: 3,
        scale: 1.0,
        health: 75.0,
        stamina: 100.0,
        mana: 0.0,
        poise: 30.0,
        speed: 0.95,
        weapon: "iron_axe",
        armor: 15.0,
        resist: [1.0; 6],
        style: Style::Melee,
        guards: 0.5,
        dodges: 0.15,
        flees: 0.15,
        sight: 22.0,
        night: false,
    },
    CreatureDef {
        key: "troll",
        name: "Troll",
        skin: 4,
        scale: 1.5,
        health: 240.0,
        stamina: 160.0,
        mana: 0.0,
        poise: 90.0,
        speed: 0.75,
        weapon: "warhammer",
        armor: 20.0,
        // Thick hide, but fire bites.
        resist: [0.8, 0.9, 0.8, 1.6, 0.8, 1.0],
        style: Style::Melee,
        guards: 0.0,
        dodges: 0.0,
        flees: 0.0,
        sight: 26.0,
        night: false,
    },
];

pub fn by_key(key: &str) -> Option<&'static CreatureDef> {
    CREATURES.iter().find(|c| c.key == key)
}

impl CreatureDef {
    pub fn defense(&self) -> Defense {
        let mut d = Defense::default();
        for e in Element::ALL {
            if e.physical() {
                d.armor[e as usize] = self.armor;
            }
        }
        d.resist = self.resist;
        d
    }
}
