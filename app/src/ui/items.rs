//! Items: stacks of a block and the containers the inventory screen moves them between (the
//! hotbar, the character's inventory, his equipment and the crafting grid), the creative
//! palettes they come from, and crafting.
//!
//! Moving things is tap and tap (no dragging on a phone): tap a slot to pick up what is in it
//! (it lights up), tap another to put it there: onto an empty slot it moves, onto the same
//! block the two stacks merge (up to `STACK_MAX`), onto another block they swap. A block from
//! the creative palette fills the slot with a full stack; something taken back onto the
//! palette is put away (gone). The world is creative: placing a block doesn't use it up.

use super::{Action, Ui};

/// Up to this many of one block in a slot.
pub const STACK_MAX: u16 = 64;
/// The character's inventory: rows of nine (the hotbar's width).
pub const INV_COLS: usize = 9;
pub const INV_ROWS: usize = 4;
pub const INV_SLOTS: usize = INV_COLS * INV_ROWS;
/// What he wears, top to bottom.
pub const EQUIP_SLOTS: [&str; 4] = ["Head", "Body", "Legs", "Feet"];
/// The crafting grid (2 x 2).
pub const CRAFT_SLOTS: usize = 4;

/// Some of one block (count 0: an empty slot).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stack {
    pub id: u16,
    pub count: u16,
}

impl Stack {
    pub const EMPTY: Stack = Stack { id: 0, count: 0 };

    pub fn of(id: u16, count: u16) -> Stack {
        if id == 0 || count == 0 {
            Stack::EMPTY
        } else {
            Stack { id, count: count.min(STACK_MAX) }
        }
    }

    pub fn is_empty(self) -> bool {
        self.id == 0 || self.count == 0
    }
}

/// A slot on the inventory screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Hotbar(u8),
    Inv(u8),
    Equip(u8),
    Craft(u8),
    /// What the crafting grid makes.
    Result,
    /// A block of the creative palette (by its index in `Ui::blocks`).
    Palette(u16),
}

/// A crafting recipe: which blocks fill the grid (any arrangement; block id and how many
/// cells of it) and what it makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub inputs: Vec<(u16, u8)>,
    pub output: Stack,
}

/// The game's recipes by block name: (inputs, (output, count)). `Recipe`s are made from them
/// with the registry (`game::recipes`).
#[allow(clippy::type_complexity)]
pub const RECIPES: &[(&[(&str, u8)], (&str, u16))] = &[
    (&[("sand", 4)], ("sandstone", 1)),
    (&[("sandstone", 1)], ("sand", 4)),
    (&[("grass", 1)], ("dirt", 1)),
    (&[("dry_grass", 1)], ("dirt", 1)),
    (&[("lush_grass", 1)], ("dirt", 1)),
    (&[("cold_grass", 1)], ("dirt", 1)),
    (&[("gravel", 2), ("sand", 2)], ("dirt", 4)),
];

/// The tabs of the inventory screen's two windows.
pub const LEFT_TABS: [(&str, &str); 3] = [("Inventory", "backpack"), ("Crafting", "handyman"), ("Equipment", "shield")];
pub const RIGHT_TABS: [(&str, &str); 2] = [("Gear", "checkroom"), ("Blocks", "view_in_ar")];
/// Which tab each window opens on: the inventory, and the blocks.
pub const LEFT_DEFAULT: u8 = 0;
pub const RIGHT_DEFAULT: u8 = 1;

impl Ui {
    /// What is in a slot (a palette block: a full stack of it).
    pub fn stack(&self, s: Slot) -> Stack {
        match s {
            Slot::Hotbar(n) => self.hotbar.get(n as usize).copied().unwrap_or_default(),
            Slot::Inv(n) => self.inventory.get(n as usize).copied().unwrap_or_default(),
            Slot::Equip(n) => self.equipment.get(n as usize).copied().unwrap_or_default(),
            Slot::Craft(n) => self.craft.get(n as usize).copied().unwrap_or_default(),
            Slot::Result => self.craft_result().map_or(Stack::EMPTY, |r| r.output),
            Slot::Palette(k) => self.blocks.get(k as usize).map_or(Stack::EMPTY, |b| Stack::of(b.id, STACK_MAX)),
        }
    }

    fn set_stack(&mut self, s: Slot, v: Stack) {
        let v = if v.is_empty() { Stack::EMPTY } else { v };
        match s {
            Slot::Hotbar(n) => self.hotbar[n as usize % 9] = v,
            Slot::Inv(n) => self.inventory[n as usize % INV_SLOTS] = v,
            Slot::Equip(n) => self.equipment[n as usize % EQUIP_SLOTS.len()] = v,
            Slot::Craft(n) => self.craft[n as usize % CRAFT_SLOTS] = v,
            Slot::Result | Slot::Palette(_) => {}
        }
    }

    /// Whether `id` may go in slot `s`. Equipment slots take only things to wear, and there
    /// are none yet: blocks stay out.
    fn accepts(&self, s: Slot, id: u16) -> bool {
        match s {
            Slot::Equip(_) => id == 0,
            Slot::Result | Slot::Palette(_) => false,
            _ => true,
        }
    }

    /// A slot was tapped on the inventory screen: pick up, put down, merge, swap or put away
    /// (see the module notes).
    pub fn tap_slot(&mut self, s: Slot) {
        self.item_note = None;
        if s == Slot::Result {
            self.craft_once();
            self.picked = None;
            return;
        }
        let Some(p) = self.picked else {
            if let Slot::Hotbar(n) = s {
                self.slot = n as usize % 9; // the hotbar slot tapped is also the one in hand
            }
            if matches!(s, Slot::Palette(_)) || !self.stack(s).is_empty() {
                self.picked = Some(s);
            } else if matches!(s, Slot::Equip(_)) {
                self.item_note = Some(format!("Armour for the {} goes here: none is made yet.", EQUIP_SLOTS[match s {
                    Slot::Equip(n) => n as usize % 4,
                    _ => 0,
                }]
                .to_lowercase()));
            }
            return;
        };
        self.picked = None;
        if p == s {
            return;
        }
        match (p, s) {
            (Slot::Palette(_), Slot::Palette(_)) => self.picked = Some(s),
            (Slot::Palette(_), _) => {
                let v = self.stack(p);
                if self.accepts(s, v.id) {
                    self.set_stack(s, v);
                } else {
                    self.item_note = Some("Only things to wear go there.".into());
                }
            }
            // Back onto the palette: put away.
            (_, Slot::Palette(_)) => self.set_stack(p, Stack::EMPTY),
            _ => self.move_stack(p, s),
        }
    }

    /// Moves what is in `from` onto `to`: merges onto the same block, swaps with another.
    fn move_stack(&mut self, from: Slot, to: Slot) {
        let (a, b) = (self.stack(from), self.stack(to));
        if !self.accepts(to, a.id) || (!b.is_empty() && !self.accepts(from, b.id)) {
            self.item_note = Some("Only things to wear go there.".into());
            return;
        }
        if !b.is_empty() && b.id == a.id {
            let total = a.count + b.count;
            let kept = total.min(STACK_MAX);
            self.set_stack(to, Stack::of(a.id, kept));
            self.set_stack(from, Stack::of(a.id, total - kept));
        } else {
            self.set_stack(to, a);
            self.set_stack(from, b);
        }
    }

    /// Puts `v` into the inventory: onto stacks of the same block first, then into empty
    /// slots. Returns what didn't fit.
    pub fn store(&mut self, mut v: Stack) -> Stack {
        for pass in 0..2 {
            for i in 0..INV_SLOTS {
                if v.is_empty() {
                    return Stack::EMPTY;
                }
                let s = self.inventory[i];
                if pass == 0 && !s.is_empty() && s.id == v.id && s.count < STACK_MAX {
                    let n = (STACK_MAX - s.count).min(v.count);
                    self.inventory[i].count += n;
                    v.count -= n;
                } else if pass == 1 && s.is_empty() {
                    self.inventory[i] = v;
                    return Stack::EMPTY;
                }
            }
        }
        if v.count == 0 {
            Stack::EMPTY
        } else {
            v
        }
    }

    /// MOVE ALL: everything in the hotbar goes into the inventory (as much as fits).
    pub fn move_all(&mut self) {
        for n in 0..9 {
            let left = self.store(self.hotbar[n]);
            self.hotbar[n] = left;
        }
        self.picked = None;
    }

    /// MOVE SIMILAR: hotbar stacks of blocks the inventory already holds go in with them.
    pub fn move_similar(&mut self) {
        for n in 0..9 {
            let v = self.hotbar[n];
            if !v.is_empty() && self.inventory.iter().any(|s| !s.is_empty() && s.id == v.id) {
                self.hotbar[n] = self.store(v);
            }
        }
        self.picked = None;
    }

    /// SORT: the inventory's stacks merged and put in the palette's order, gaps at the end.
    pub fn sort_inventory(&mut self) {
        let order = |id: u16| self.blocks.iter().position(|b| b.id == id).unwrap_or(usize::MAX);
        let mut all: Vec<Stack> = self.inventory.iter().copied().filter(|s| !s.is_empty()).collect();
        all.sort_by_key(|s| (order(s.id), s.id));
        self.inventory = [Stack::EMPTY; INV_SLOTS];
        for s in all {
            let _ = self.store(s);
        }
        self.picked = None;
    }

    /// The recipe the crafting grid holds, if any (any arrangement of the right blocks).
    pub fn craft_result(&self) -> Option<&Recipe> {
        let mut have: Vec<(u16, u8)> = Vec::new();
        for s in self.craft.iter().filter(|s| !s.is_empty()) {
            match have.iter_mut().find(|h| h.0 == s.id) {
                Some(h) => h.1 += 1,
                None => have.push((s.id, 1)),
            }
        }
        if have.is_empty() {
            return None;
        }
        have.sort_unstable();
        self.recipes.iter().find(|r| {
            let mut want = r.inputs.clone();
            want.sort_unstable();
            want == have
        })
    }

    /// Takes what the grid makes: one of each block in it is used up, and the result goes
    /// into the inventory (or the hotbar if it is full).
    fn craft_once(&mut self) {
        let Some(out) = self.craft_result().map(|r| r.output) else {
            self.item_note = Some(if self.craft.iter().all(|s| s.is_empty()) {
                "Put blocks in the grid to make something.".into()
            } else {
                "Nothing is made from that.".into()
            });
            return;
        };
        let mut left = self.store(out);
        for n in 0..9 {
            if left.is_empty() {
                break;
            }
            let h = self.hotbar[n];
            if h.is_empty() {
                self.hotbar[n] = left;
                left = Stack::EMPTY;
            } else if h.id == left.id && h.count < STACK_MAX {
                let k = (STACK_MAX - h.count).min(left.count);
                self.hotbar[n].count += k;
                left.count -= k;
            }
        }
        if left.count == out.count {
            self.item_note = Some("No room for it.".into());
            return;
        }
        for c in self.craft.iter_mut().filter(|s| !s.is_empty()) {
            c.count -= 1;
            if c.count == 0 {
                *c = Stack::EMPTY;
            }
        }
    }

    /// Leaving the inventory screen: what is left in the crafting grid goes back into the
    /// inventory (as in Minecraft), and nothing stays picked up.
    pub fn close_inventory(&mut self) {
        for n in 0..CRAFT_SLOTS {
            let v = std::mem::take(&mut self.craft[n]);
            self.craft[n] = self.store(v);
        }
        self.picked = None;
        self.item_note = None;
    }

    /// The inventory screen's own actions.
    pub(super) fn apply_items(&mut self, a: Action) {
        match a {
            Action::Item(s) => self.tap_slot(s),
            // (what is picked up stays picked up: it can be taken to another tab)
            Action::LeftTab(t) => self.left_tab = t.min(LEFT_TABS.len() as u8 - 1),
            Action::RightTab(t) => self.right_tab = t.min(RIGHT_TABS.len() as u8 - 1),
            Action::BlockGroup(g) => {
                self.block_group = g.min(crate::icons::GROUPS.len() as u8 - 1);
                self.palette_scroll = 0;
            }
            Action::MoveAll => self.move_all(),
            Action::MoveSimilar => self.move_similar(),
            Action::SortItems => self.sort_inventory(),
            _ => {}
        }
    }
}

/// Text form for the save file: `name*count` per slot, empty slots empty, comma separated.
pub fn to_text(stacks: &[Stack], name: impl Fn(u16) -> String) -> String {
    stacks.iter().map(|s| if s.is_empty() { String::new() } else { format!("{}*{}", name(s.id), s.count) }).collect::<Vec<_>>().join(",")
}

/// Reads `to_text`'s form into `slots` (a name alone, as older saves have it: a full stack).
/// Unknown names leave the slot empty.
pub fn from_text(text: &str, slots: &mut [Stack], id: impl Fn(&str) -> Option<u16>) {
    for (slot, part) in slots.iter_mut().zip(text.split(',')) {
        let (name, count) = match part.trim().split_once('*') {
            Some((n, c)) => (n, c.trim().parse().unwrap_or(STACK_MAX)),
            None => (part.trim(), STACK_MAX),
        };
        *slot = id(name).map_or(Stack::EMPTY, |i| Stack::of(i, count));
    }
}
