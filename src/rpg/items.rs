//! What can be carried, wielded and worn: weapons, staves, armour and potions.
//!
//! They live in `assets/items.toml` and join the block registry as items (`Model::Item`),
//! so they share its ids, names, icons, saving and the inventory with the blocks. Their
//! fighting numbers are read here, by registry id.

use std::collections::HashMap;

use super::{spells, Amounts, Defense, Element, ELEMENTS};
use crate::registry::{parse_tables, BlockRegistry, Table, Value};

/// How a weapon is swung (its moves and the pose it is held in).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WeaponClass {
    Fists,
    Sword,
    Axe,
    Mace,
    Dagger,
    Spear,
    Greatsword,
    Warhammer,
    Battleaxe,
    Club,
}

impl WeaponClass {
    fn parse(s: &str) -> Option<WeaponClass> {
        Some(match s {
            "sword" => WeaponClass::Sword,
            "axe" => WeaponClass::Axe,
            "mace" => WeaponClass::Mace,
            "dagger" => WeaponClass::Dagger,
            "spear" => WeaponClass::Spear,
            "greatsword" => WeaponClass::Greatsword,
            "warhammer" => WeaponClass::Warhammer,
            "battleaxe" => WeaponClass::Battleaxe,
            "club" => WeaponClass::Club,
            _ => return None,
        })
    }
}

/// One attack: the character's animation, its length, when in it the blow lands, and how
/// much stronger than the weapon's base numbers it is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Move {
    pub anim: &'static str,
    pub length: f32,
    pub strike: f32,
    pub damage: f32,
    pub stamina: f32,
    pub poise: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Weapon {
    pub class: WeaponClass,
    pub two_handed: bool,
    pub damage: Amounts,
    /// How far it reaches from the wielder's middle, metres, and how wide it sweeps, degrees.
    pub reach: f32,
    pub arc: f32,
    /// Stamina for a light attack (a heavy one costs twice that).
    pub stamina: f32,
    pub poise: f32,
    /// The share of a blow's physical damage a guard with it stops.
    pub guard: f32,
    /// Seconds of bleeding a blow leaves (daggers, axes), at a tenth of its damage a second.
    pub bleed: f32,
}

/// Light attacks chain up to this many swings, each a little stronger.
pub const COMBO: usize = 3;
const COMBO_DAMAGE: [f32; COMBO] = [1.0, 1.1, 1.35];

impl Weapon {
    pub fn fists() -> Weapon {
        let mut damage = [0.0; ELEMENTS];
        damage[Element::Blunt as usize] = 5.0;
        Weapon { class: WeaponClass::Fists, two_handed: false, damage, reach: 1.9, arc: 70.0, stamina: 8.0, poise: 4.0, guard: 0.35, bleed: 0.0 }
    }

    /// Light attack number `step` of a combo (0, 1, 2), or the heavy attack.
    pub fn attack(&self, heavy: bool, step: usize) -> Move {
        let quick = if self.class == WeaponClass::Dagger { 0.78 } else { 1.0 };
        let mut m = match (self.class, self.two_handed, heavy) {
            (WeaponClass::Fists, _, false) => Move { anim: "punch", length: 0.42, strike: 0.17, damage: 1.0, stamina: 1.0, poise: 1.0 },
            (WeaponClass::Fists, _, true) => Move { anim: "swing", length: 0.7, strike: 0.33, damage: 2.0, stamina: 2.0, poise: 2.5 },
            (_, false, false) => Move { anim: "swing", length: 0.7 * quick, strike: 0.3 * quick, damage: 1.0, stamina: 1.0, poise: 1.0 },
            (_, false, true) => Move { anim: "chop", length: 0.9, strike: 0.5, damage: 1.8, stamina: 2.0, poise: 2.2 },
            (_, true, false) => Move { anim: "swing_2h", length: 1.0, strike: 0.45, damage: 1.0, stamina: 1.0, poise: 1.0 },
            (_, true, true) => Move { anim: "chop_2h", length: 1.1, strike: 0.6, damage: 1.9, stamina: 2.0, poise: 2.5 },
        };
        if !heavy {
            m.damage *= COMBO_DAMAGE[step.min(COMBO - 1)];
        }
        m
    }
}

/// A staff casts its `light` spell on a tap of ATTACK, its `heavy` one when ATTACK is held,
/// `power` times as strong as the spell's own numbers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Staff {
    pub light: usize,
    pub heavy: usize,
    pub power: f32,
}

/// Where armour is worn: the equipment's four slots, top to bottom.
pub const ARMOR_SLOTS: [&str; 4] = ["head", "body", "legs", "feet"];

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Armor {
    pub slot: usize,
    pub defense: Defense,
    /// Heavier armour slows the stamina refill (a share: 0.1 = a tenth slower).
    pub weight: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Potion {
    pub heal: f32,
    pub mana: f32,
    pub stamina: f32,
    /// Healing a second for `time` seconds after.
    pub regen: f32,
    pub time: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Item {
    Weapon(Weapon),
    Staff(Staff),
    Armor(Armor),
    Potion(Potion),
}

/// Every item's numbers, by registry id.
#[derive(Clone, Debug, Default)]
pub struct ItemBook {
    items: HashMap<u16, Item>,
}

fn num(t: &Table, k: &str) -> Option<f32> {
    match t.get(k) {
        Some(Value::Num(n)) => Some(*n as f32),
        _ => None,
    }
}

fn text<'a>(t: &'a Table, k: &str) -> Option<&'a str> {
    match t.get(k) {
        Some(Value::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

impl ItemBook {
    /// Reads the items the registry was given (`BlockRegistry::with_items`).
    pub fn from_registry(reg: &BlockRegistry) -> Result<ItemBook, String> {
        let mut book = ItemBook::default();
        let tables = parse_tables(reg.items_source(), "item").map_err(|e| e.0)?;
        for t in &tables {
            let name = text(t, "name").ok_or("an [[item]] has no name")?;
            let id = reg.id(name).ok_or_else(|| format!("item '{name}' is not in the registry"))?;
            let item = match text(t, "kind") {
                Some("weapon") => {
                    let class = text(t, "class").and_then(WeaponClass::parse).ok_or_else(|| format!("item '{name}': unknown weapon class"))?;
                    let mut damage = [0.0; ELEMENTS];
                    for e in Element::ALL {
                        damage[e as usize] = num(t, e.key()).unwrap_or(0.0);
                    }
                    Item::Weapon(Weapon {
                        class,
                        two_handed: num(t, "hands").unwrap_or(1.0) >= 2.0,
                        damage,
                        reach: num(t, "reach").unwrap_or(2.3),
                        arc: num(t, "arc").unwrap_or(100.0),
                        stamina: num(t, "stamina").unwrap_or(14.0),
                        poise: num(t, "poise").unwrap_or(10.0),
                        guard: num(t, "guard").unwrap_or(0.6),
                        bleed: num(t, "bleed").unwrap_or(0.0),
                    })
                }
                Some("staff") => {
                    let spell = |k: &str| text(t, k).and_then(spells::index).ok_or_else(|| format!("item '{name}': unknown {k} spell"));
                    Item::Staff(Staff { light: spell("light")?, heavy: spell("heavy")?, power: num(t, "power").unwrap_or(1.0) })
                }
                Some("armor") => {
                    let slot = text(t, "slot").and_then(|s| ARMOR_SLOTS.iter().position(|x| *x == s)).ok_or_else(|| format!("item '{name}': unknown slot"))?;
                    let mut defense = Defense::default();
                    let physical = num(t, "armor").unwrap_or(0.0);
                    for e in Element::ALL {
                        defense.armor[e as usize] = num(t, e.key()).unwrap_or(if e.physical() { physical } else { 0.0 });
                    }
                    Item::Armor(Armor { slot, defense, weight: num(t, "weight").unwrap_or(0.0) })
                }
                Some("potion") => Item::Potion(Potion {
                    heal: num(t, "heal").unwrap_or(0.0),
                    mana: num(t, "mana").unwrap_or(0.0),
                    stamina: num(t, "stamina").unwrap_or(0.0),
                    regen: num(t, "regen").unwrap_or(0.0),
                    time: num(t, "time").unwrap_or(0.0),
                }),
                other => return Err(format!("item '{name}': unknown kind {other:?}")),
            };
            book.items.insert(id, item);
        }
        Ok(book)
    }

    pub fn get(&self, id: u16) -> Option<&Item> {
        self.items.get(&id)
    }

    pub fn weapon(&self, id: u16) -> Option<Weapon> {
        match self.items.get(&id) {
            Some(Item::Weapon(w)) => Some(*w),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The defence and weight of what is worn (empty slots: 0).
    pub fn worn(&self, equipment: &[u16]) -> (Defense, f32) {
        let mut d = Defense::default();
        let mut weight = 0.0;
        for &id in equipment {
            if let Some(Item::Armor(a)) = self.items.get(&id) {
                d.add(&a.defense);
                weight += a.weight;
            }
        }
        (d, weight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_items_parse_into_weapons_staves_armour_and_potions() {
        let reg = BlockRegistry::builtin();
        let book = ItemBook::from_registry(&reg).unwrap();
        assert!(book.len() >= 25, "{}", book.len());
        let sword = book.weapon(reg.id("iron_sword").unwrap()).unwrap();
        assert_eq!(sword.class, WeaponClass::Sword);
        assert!(sword.damage[Element::Slash as usize] > 10.0 && !sword.two_handed);
        let hammer = book.weapon(reg.id("warhammer").unwrap()).unwrap();
        assert!(hammer.two_handed && hammer.damage[Element::Blunt as usize] > sword.damage[Element::Slash as usize]);
        assert!(matches!(book.get(reg.id("ember_staff").unwrap()), Some(Item::Staff(_))));
        let worn = [reg.id("plate_helm").unwrap(), reg.id("plate_cuirass").unwrap(), 0, 0];
        let (d, w) = book.worn(&worn);
        assert!(d.armor[Element::Slash as usize] > 20.0 && w > 0.0);
        assert!(matches!(book.get(reg.id("health_potion").unwrap()), Some(Item::Potion(p)) if p.heal > 0.0));
        // Items are not blocks: none is solid or placeable as one.
        for id in 1..reg.len() as u16 {
            if book.get(id).is_some() {
                assert_eq!(reg.def(id).model, crate::registry::Model::Item);
            }
        }
    }

    #[test]
    fn heavy_attacks_and_combo_finishers_hit_harder() {
        let w = Weapon::fists();
        assert!(w.attack(true, 0).damage > w.attack(false, 0).damage);
        let mut s = w;
        s.class = WeaponClass::Sword;
        assert!(s.attack(false, 2).damage > s.attack(false, 0).damage);
        for (heavy, step) in [(false, 0), (true, 0)] {
            let m = s.attack(heavy, step);
            assert!(m.strike < m.length);
        }
    }
}
