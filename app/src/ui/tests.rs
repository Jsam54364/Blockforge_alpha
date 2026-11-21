use super::controls::{self, CONTROLS};
use super::screens::TABS;
use super::*;

const SIZES: [(u32, u32); 5] = [(2400, 1080), (1920, 1080), (2800, 1260), (2340, 1080), (3200, 1440)];
const DP: f64 = 2.75;

/// In the game (the HUD tests); `title()` for the title screen.
fn ui() -> Ui {
    let mut u = Ui::new();
    u.set_surface(2400, 1080, DP);
    u.screen = Screen::Playing;
    u
}

fn title() -> Ui {
    let mut u = Ui::new();
    u.set_surface(2400, 1080, DP);
    assert_eq!(u.screen, Screen::Title, "the app opens on the title screen");
    u
}

fn build(u: &mut Ui) -> Vec<HudVertex> {
    let mut out = Vec::new();
    u.build(&Perf::new(), &mut out);
    out
}

fn find(u: &Ui, a: Action) -> &Button {
    u.buttons.iter().rev().find(|b| b.action == a).unwrap_or_else(|| panic!("no {a:?} button on {:?}", u.screen))
}

fn has(u: &Ui, a: Action) -> bool {
    u.buttons.iter().any(|b| b.action == a)
}

fn worlds(n: usize) -> Vec<WorldEntry> {
    (0..n)
        .map(|i| WorldEntry {
            folder: format!("W{i}"),
            name: format!("World {i}"),
            seed: i.to_string(),
            last_played: "November 14, 2026 - 10:45 AM".into(),
            play_time: "12h 34m".into(),
            size: "256 MB".into(),
            created: "11/03/2026".into(),
            thumb: (i % 2 == 0).then_some(i + 1),
        })
        .collect()
}

/// Taps the middle of the button doing `a` as a finger does: down, then up on the same
/// spot. Menus act on the lift; the button glows while held.
fn tap(u: &mut Ui, a: Action) {
    let b = find(u, a);
    let (x, y) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
    assert_eq!(u.hit(x, y), Some(a), "something else is on top of {a:?}");
    let acted = match u.press(7, x, y) {
        Some(now) => Some(now),
        None => {
            assert_eq!(u.pressed_action(), Some(a));
            build(u);
            u.release(7, x, y)
        }
    };
    assert_eq!(acted, Some(a));
    u.apply(a);
    build(u);
}

/// Everything drawn and every real button on screen, none on top of another (the panels
/// behind them do nothing and are meant to be under them), and text in the atlas.
fn check_layout(u: &mut Ui, w: f32, h: f32) {
    let out = build(u);
    assert_eq!(out.len() % 6, 0);
    assert!(out.len() < MAX_HUD_VERTS, "{} vertices on {:?}", out.len(), u.screen);
    for v in &out {
        // (the glow around a button reaches a little past it)
        assert!(v.pos[0] >= -60.0 && v.pos[0] <= w + 60.0 && v.pos[1] >= -60.0 && v.pos[1] <= h + 60.0, "{:?} off a {w}x{h} {:?}", v.pos, u.screen);
    }
    let real: Vec<&Button> = u.buttons.iter().filter(|b| b.action != Action::Nothing).collect();
    for b in &real {
        assert!(b.x >= -0.5 && b.y >= -0.5 && b.x + b.w <= w + 0.5 && b.y + b.h <= h + 0.5, "{:?} off a {w}x{h} {:?}", b.action, u.screen);
        assert!(b.w >= 20.0 && b.h >= 20.0, "{:?} is {}x{}", b.action, b.w, b.h);
    }
    for (i, a) in real.iter().enumerate() {
        for b in &real[i + 1..] {
            let apart = a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5;
            assert!(apart, "{:?} overlaps {:?} on {:?} ({w}x{h})", a.action, b.action, u.screen);
        }
    }
}

#[test]
fn the_title_screen_has_its_buttons_and_each_page_leads_back() {
    for (w, h) in SIZES {
        let mut u = title();
        u.set_surface(w, h, DP);
        check_layout(&mut u, w as f32, h as f32);
        let targets = [Screen::WorldList, Screen::Multiplayer, Screen::Cosmetics, Screen::Mods];
        for s in targets {
            let b = find(&u, Action::Open(s));
            assert!(b.h >= 30.0 * 2.75 - 0.5 && b.w >= 100.0 * 2.75, "{s:?}: {}x{}", b.w, b.h);
        }
        find(&u, Action::OpenSettings);
        find(&u, Action::Quit);
        for s in targets {
            let mut p = title();
            p.set_surface(w, h, DP);
            build(&mut p);
            tap(&mut p, Action::Open(s));
            assert_eq!(p.screen, s);
            check_layout(&mut p, w as f32, h as f32);
            tap(&mut p, Action::Open(Screen::Title));
            assert_eq!(p.screen, Screen::Title);
        }
    }
}

#[test]
fn menu_buttons_act_when_the_finger_lifts_on_them() {
    let mut u = title();
    build(&mut u);
    let b = find(&u, Action::Open(Screen::Mods));
    let (x, y, bh) = (b.x + b.w * 0.5, b.y + b.h * 0.5, b.h);
    // Down: nothing yet, the button glows and the touch is the menu's.
    assert_eq!(u.press(1, x, y), None);
    assert_eq!(u.pressed_action(), Some(Action::Open(Screen::Mods)));
    assert!(u.holds(1) && !u.holds(2));
    // Another finger lifting does nothing; sliding off and lifting cancels.
    assert_eq!(u.release(2, x, y), None);
    assert_eq!(u.release(1, x, y + bh * 3.0), None);
    assert_eq!(u.pressed_action(), None);
    // Down and up on it: it acts.
    assert_eq!(u.press(1, x, y), None);
    assert_eq!(u.release(1, x, y), Some(Action::Open(Screen::Mods)));
    // QUIT asks the app to close.
    let mut u = title();
    build(&mut u);
    tap(&mut u, Action::Quit);
    assert_eq!(u.take_request(), Some(Request::Quit));
}

#[test]
fn picking_playing_editing_and_deleting_worlds() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = title();
        u.set_surface(w, h, DP);
        u.set_worlds(worlds(30));
        build(&mut u);
        tap(&mut u, Action::Open(Screen::WorldList));
        check_layout(&mut u, wf, hf);
        // Nothing picked: PLAY and RE-CREATE do nothing, and there is no EDIT or DELETE.
        assert!(!has(&u, Action::PlayWorld) && !has(&u, Action::RecreateWorld));
        assert!(!has(&u, Action::EditWorld) && !has(&u, Action::AskDelete));
        // Thirty worlds don't all fit: the list scrolls.
        assert!(!has(&u, Action::SelectWorld(29)));
        let rows = u.buttons.iter().filter(|b| matches!(b.action, Action::SelectWorld(_))).count();
        assert!(rows >= 2, "{w}x{h}: {rows} rows show");
        // Dragging the list scrolls it a row at a time, and picks nothing.
        let drag = |u: &mut Ui, rows_up: f32| {
            let b = find(u, Action::SelectWorld(u.scroll));
            let (x, y) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
            assert_eq!(u.press(4, x, y), None);
            let rh = u.list_row_height();
            u.moved(4, x, y - rows_up * rh * 0.5);
            u.moved(4, x, y - rows_up * rh);
            assert_eq!(u.release(4, x, y - rows_up * rh), None);
            build(u);
        };
        drag(&mut u, 3.0);
        assert_eq!((u.scroll, u.selected), (3, None));
        drag(&mut u, 40.0);
        find(&u, Action::SelectWorld(29)); // at the end
        assert_eq!(u.scroll, 30 - rows);
        drag(&mut u, -(30.0 - rows as f32 - 7.0));
        assert_eq!(u.scroll, 7);
        find(&u, Action::SelectWorld(8));
        // Pick one: its details and EDIT and DELETE show; tap it again to play it.
        tap(&mut u, Action::SelectWorld(7));
        assert_eq!(u.selected, Some(7));
        check_layout(&mut u, wf, hf);
        find(&u, Action::EditWorld);
        find(&u, Action::AskDelete);
        assert_eq!(u.take_request(), None);
        tap(&mut u, Action::SelectWorld(7));
        assert_eq!(u.take_request(), Some(Request::Play("W7".into())));
        tap(&mut u, Action::PlayWorld);
        assert_eq!(u.take_request(), Some(Request::Play("W7".into())));
        // Rename it with the on-screen keyboard.
        tap(&mut u, Action::EditWorld);
        assert_eq!((u.screen, u.fields[0].as_str()), (Screen::EditWorld, "World 7"));
        check_layout(&mut u, wf, hf);
        tap(&mut u, Action::Focus(0));
        check_layout(&mut u, wf, hf);
        let field = find(&u, Action::Focus(0));
        assert!(field.y + field.h <= find(&u, Action::Key('q')).y, "{w}x{h}: the field is under the keyboard");
        for _ in 0..2 {
            tap(&mut u, Action::Backspace);
        }
        tap(&mut u, Action::Shift);
        tap(&mut u, Action::Key('I'));
        tap(&mut u, Action::Shift);
        for c in "sle 2".chars() {
            tap(&mut u, Action::Key(c));
        }
        tap(&mut u, Action::KeysDone);
        tap(&mut u, Action::SaveWorld);
        assert_eq!(u.take_request(), Some(Request::Rename { folder: "W7".into(), name: "WorldIsle 2".into() }));
        assert_eq!(u.screen, Screen::WorldList);
        // Delete it, after saying yes.
        tap(&mut u, Action::AskDelete);
        assert_eq!(u.screen, Screen::DeleteWorld);
        check_layout(&mut u, wf, hf);
        tap(&mut u, Action::Open(Screen::WorldList));
        assert_eq!(u.take_request(), None, "cancel deletes nothing");
        tap(&mut u, Action::AskDelete);
        tap(&mut u, Action::ConfirmDelete);
        assert_eq!(u.take_request(), Some(Request::Delete("W7".into())));
        assert_eq!((u.screen, u.selected), (Screen::WorldList, None));
        let mut left = worlds(30);
        left.remove(7);
        u.set_worlds(left);
        check_layout(&mut u, wf, hf);
        // No worlds at all.
        u.set_worlds(Vec::new());
        check_layout(&mut u, wf, hf);
    }
}

#[test]
fn creating_a_world_with_a_name_and_a_seed() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = title();
        u.set_surface(w, h, DP);
        u.set_worlds(worlds(1));
        build(&mut u);
        tap(&mut u, Action::Open(Screen::WorldList));
        tap(&mut u, Action::NewWorld);
        assert_eq!((u.screen, u.fields[0].as_str(), u.fields[1].as_str()), (Screen::CreateWorld, "New World", ""));
        check_layout(&mut u, wf, hf);
        // Typing into the name: the keyboard comes up, the fields stay visible above it.
        tap(&mut u, Action::Focus(0));
        check_layout(&mut u, wf, hf);
        let q = find(&u, Action::Key('q')).y;
        for f in 0..2 {
            let b = find(&u, Action::Focus(f));
            assert!(b.y + b.h <= q, "{w}x{h}: field {f} is under the keyboard");
        }
        for _ in 0.."New World".len() + 1 {
            tap(&mut u, Action::Backspace);
        }
        // An empty name starts with a capital, then lower case.
        tap(&mut u, Action::Focus(0));
        assert!(u.caps);
        tap(&mut u, Action::Key('G'));
        assert!(!u.caps);
        for c in "lacier".chars() {
            tap(&mut u, Action::Key(c));
        }
        tap(&mut u, Action::Focus(1));
        for c in "-42".chars() {
            tap(&mut u, Action::Key(c));
        }
        tap(&mut u, Action::KeysDone);
        assert!(u.buttons.iter().all(|b| !matches!(b.action, Action::Key(_))), "the keyboard went away");
        tap(&mut u, Action::CreateWorld);
        assert_eq!(u.take_request(), Some(Request::Create { name: "Glacier".into(), seed: "-42".into() }));
    }
    // RE-CREATE starts from the picked world's name and seed; an empty name can't be made.
    let mut u = title();
    u.set_worlds(worlds(3));
    u.apply(Action::Open(Screen::WorldList));
    u.apply(Action::SelectWorld(2));
    u.apply(Action::RecreateWorld);
    assert_eq!((u.fields[0].as_str(), u.fields[1].as_str()), ("World 2", "2"));
    u.fields[0].clear();
    build(&mut u);
    assert!(!has(&u, Action::CreateWorld));
    // Names and seeds have a length limit, and only letters, digits, space and '-'.
    u.apply(Action::Focus(0));
    for _ in 0..40 {
        u.type_text("ab.");
    }
    assert_eq!(u.fields[0].len(), crate::worlds::MAX_NAME);
    assert!(u.fields[0].chars().all(|c| c == 'a' || c == 'b'));
}

#[test]
fn the_loading_screen_shows_progress() {
    for (w, h) in SIZES {
        let mut u = title();
        u.set_surface(w, h, DP);
        u.screen = Screen::Loading;
        u.loading_chunks = 60;
        check_layout(&mut u, w as f32, h as f32);
        assert!(u.buttons.is_empty(), "nothing to press while a world loads");
    }
}

#[test]
fn settings_take_effect_at_once_and_lead_back() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = title();
        u.set_surface(w, h, DP);
        build(&mut u);
        tap(&mut u, Action::OpenSettings);
        assert_eq!((u.screen, u.tab), (Screen::Settings, 0));
        // Every group lays out, with its settings on screen.
        let expect: [&[Action]; 4] = [
            &[Action::ToggleWalk, Action::ToggleView, Action::ToggleStats],
            &[Action::RenderDist(1), Action::RenderDist(-1), Action::LodDist(1), Action::CycleLodQuality, Action::ToggleFog, Action::ToggleClouds, Action::ToggleRays, Action::ToggleVsync],
            &[Action::CycleCull, Action::ToggleMesher],
            &[Action::OpenControls, Action::HudOpacity(1), Action::LookSpeed(-1), Action::JoySize(1), Action::ToggleTouchBuild, Action::ToggleOneStance],
        ];
        for (t, actions) in expect.iter().enumerate() {
            tap(&mut u, Action::Tab(t as u8));
            check_layout(&mut u, wf, hf);
            for a in actions.iter() {
                let b = find(&u, *a);
                assert!(b.h >= 26.0 * 2.75, "{w}x{h}: {a:?} is {}px high", b.h);
            }
            // No time of day here: it belongs to each world.
            assert!(u.buttons.iter().all(|b| !matches!(b.action, Action::Time(_))));
        }
        // Changes take effect at once; a new render distance is reported.
        let before = u.settings;
        tap(&mut u, Action::Tab(1));
        tap(&mut u, Action::ToggleFog);
        assert_eq!(u.settings.fog, !before.fog);
        assert!(u.apply(Action::RenderDist(1)));
        build(&mut u);
        // RESET TO DEFAULT puts the defaults back (but not VSync, which the phone decides).
        u.settings.vsync = false;
        assert!(u.apply(Action::ResetSettings));
        assert_eq!(u.settings, Settings { vsync: false, ..Settings::default() });
        // The X and BACK both go back.
        build(&mut u);
        let backs = u.buttons.iter().filter(|b| b.action == Action::Open(Screen::Title)).count();
        assert_eq!(backs, 2);
        tap(&mut u, Action::Open(Screen::Title));
        assert_eq!(u.screen, Screen::Title);
    }
}

#[test]
fn the_game_menu_opens_from_the_hud_and_leads_to_settings_and_the_title() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = ui();
        u.set_surface(w, h, DP);
        build(&mut u);
        // The HUD's menu button acts at once.
        let b = find(&u, Action::Open(Screen::Pause));
        assert_eq!(u.press(3, b.x + b.w * 0.5, b.y + b.h * 0.5), Some(Action::Open(Screen::Pause)));
        u.apply(Action::Open(Screen::Pause));
        assert!(u.over_world(), "the world shows behind the game menu");
        check_layout(&mut u, wf, hf);
        // Settings from here come back here, with the time of day.
        tap(&mut u, Action::OpenSettings);
        assert!(u.over_world());
        find(&u, Action::Time(1));
        find(&u, Action::Time(-1));
        tap(&mut u, Action::Open(Screen::Pause));
        assert_eq!(u.screen, Screen::Pause);
        tap(&mut u, Action::Open(Screen::Playing));
        assert_eq!(u.screen, Screen::Playing);
        u.apply(Action::Open(Screen::Pause));
        build(&mut u);
        tap(&mut u, Action::Open(Screen::Title));
        assert!(!u.over_world());
    }
}

/// Every block the hotbar can hold, as the game gives them.
fn blocks() -> Vec<BlockItem> {
    crate::game::block_items(&blockforge::registry::BlockRegistry::builtin())
}

/// The game's HUD with the blocks in it and the default hotbar's first slots filled.
fn hud(w: u32, h: u32, walk: bool) -> Ui {
    let mut u = ui();
    u.set_surface(w, h, DP);
    u.blocks = blocks();
    for (slot, b) in u.hotbar.iter_mut().zip(&u.blocks) {
        *slot = Stack::of(b.id, 64);
    }
    u.recipes = crate::game::recipes(&blockforge::registry::BlockRegistry::builtin());
    u.settings.walk = walk;
    u
}

/// Every button `pick` matches is the one a touch on its middle gets (nothing on top).
fn on_top(u: &Ui, pick: impl Fn(Action) -> bool) -> usize {
    let mut n = 0;
    for b in u.buttons.iter().filter(|b| pick(b.action)) {
        assert_eq!(u.hit(b.x + b.w * 0.5, b.y + b.h * 0.5), Some(b.action), "something is over {:?}", b.action);
        n += 1;
    }
    n
}

#[test]
fn the_hud_has_its_controls_and_they_act_on_the_touch() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        // Flying: attack, place, the hotbar and pause, and JUMP and DOWN to rise and sink;
        // not the rest of the character's buttons.
        let mut u = hud(w, h, false);
        u.settings.show_stats = true;
        check_layout(&mut u, wf, hf);
        for a in [Action::Hold("attack"), Action::Hold("place"), Action::ToggleInventory, Action::Open(Screen::Pause)] {
            find(&u, a);
        }
        for n in 0..9 {
            let b = find(&u, Action::Slot(n));
            assert!(b.w >= 26.0 * 2.75 && b.h >= 26.0 * 2.75, "{w}x{h}: slot {n} is {}x{}", b.w, b.h);
        }
        find(&u, Action::Hold("jump"));
        find(&u, Action::Hold("down"));
        assert!(!has(&u, Action::ToggleActs) && !has(&u, Action::Hold("roll")) && !has(&u, Action::Press("crouch")));
        // Walking: the character's too, none on top of another; no DOWN.
        let mut u = hud(w, h, true);
        check_layout(&mut u, wf, hf);
        assert!(!has(&u, Action::Hold("down")));
        for a in [Action::Hold("attack"), Action::Hold("place"), Action::Hold("jump"), Action::Hold("roll"), Action::Press("crouch"), Action::Press("crawl"), Action::ToggleActs] {
            let b = find(&u, a);
            assert!(b.w >= 40.0 * 2.75 && b.h >= 40.0 * 2.75, "{w}x{h}: {a:?} is {}x{}", b.w, b.h);
            // HUD buttons act on the touch, not the lift.
            assert_eq!(u.press(9, b.x + b.w * 0.5, b.y + b.h * 0.5), Some(a));
        }
        // The extras stay off until the editor puts them on.
        assert!(!has(&u, Action::Press("wave")) && !has(&u, Action::ToggleView));
        // ATTACK is big and in easy reach of the right thumb; the hotbar along the bottom.
        let attack = find(&u, Action::Hold("attack"));
        assert!(attack.x > wf * 0.6 && attack.y > hf * 0.6);
        let slot = find(&u, Action::Slot(4));
        assert!(slot.y + slot.h > hf * 0.85 && slot.x + slot.w < attack.x);
    }
}

#[test]
fn the_action_grid_and_the_block_picker_open_on_top() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = hud(w, h, true);
        build(&mut u);
        u.apply(Action::ToggleActs);
        let out = build(&mut u);
        assert!(out.iter().all(|v| v.pos[0] >= -30.0 && v.pos[0] <= wf + 30.0 && v.pos[1] >= -30.0 && v.pos[1] <= hf + 30.0));
        assert_eq!(on_top(&u, |a| matches!(a, Action::Press("punch" | "wave" | "revive" | "two_handed_item")) || a == Action::ToggleView), 5);
        u.apply(Action::Slot(6));
        // The inventory closes the grid.
        u.apply(Action::ToggleInventory);
        assert!(!u.acts_open);
        assert_eq!(u.screen, Screen::Inventory);
        assert!(u.over_world(), "the game shows behind the inventory");
        check_layout(&mut u, wf, hf);
    }
}

/// The game's HUD with the inventory screen open.
/// Scrolls the Blocks window until palette slot `slot` shows.
fn reveal(u: &mut Ui, slot: Slot) {
    let Slot::Palette(k) = slot else { return };
    for n in 0..200 {
        if palette(u).contains(&k) {
            return;
        }
        u.palette_scroll = n;
        build(u);
    }
    panic!("block {k} never shows");
}

/// The palette slots shown, in order.
fn palette(u: &Ui) -> Vec<u16> {
    u.buttons.iter().filter_map(|b| if let Action::Item(Slot::Palette(k)) = b.action { Some(k) } else { None }).collect()
}

fn inventory(w: u32, h: u32) -> Ui {
    let mut u = hud(w, h, true);
    build(&mut u);
    u.apply(Action::ToggleInventory);
    build(&mut u);
    u
}

#[test]
fn the_inventory_has_two_windows_whose_tabs_change_only_their_own_window() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = inventory(w, h);
        u.inventory[0] = u.hotbar[0];
        check_layout(&mut u, wf, hf);
        // The tabs: three for the left window, two for the right, and the X.
        for a in [Action::LeftTab(0), Action::LeftTab(1), Action::LeftTab(2), Action::RightTab(0), Action::RightTab(1), Action::Open(Screen::Playing)] {
            find(&u, a);
        }
        let (l0, r1, x) = (find(&u, Action::LeftTab(0)), find(&u, Action::RightTab(1)), find(&u, Action::Open(Screen::Playing)));
        assert!(l0.x < wf * 0.2 && r1.x > wf * 0.6 && x.x > r1.x && l0.y < hf * 0.15, "tabs in the top bar's corners");
        // At first: the inventory (36 slots) on the left, the blocks on the right, the
        // hotbar along the bottom between MOVE SIMILAR, MOVE ALL, SORT and the screen's edge.
        for n in 0..36u8 {
            let b = find(&u, Action::Item(Slot::Inv(n)));
            assert!(b.w >= 30.0 * 2.75 && b.x + b.w < wf * 0.56, "{w}x{h}: inventory slot {n}: {} at {}", b.w, b.x);
        }
        // The blocks: a tab per group, each group's blocks only, in order, scrolled by
        // dragging until its last block shows.
        for g in 0..crate::icons::GROUPS.len() as u8 {
            tap(&mut u, Action::BlockGroup(g));
            let mine: Vec<u16> = (0..u.blocks.len() as u16).filter(|&k| u.blocks[k as usize].group == g).collect();
            assert!(mine.len() > 4, "group {g}");
            let shown = palette(&u);
            assert_eq!(shown[..], mine[..shown.len()], "{w}x{h}: group {g} starts at its first block");
            for &k in &shown {
                let b = find(&u, Action::Item(Slot::Palette(k)));
                assert!(b.w >= 30.0 * 2.75 && b.x > wf * 0.5 && b.y + b.h <= hf, "{w}x{h}: block {k}");
            }
            let f = find(&u, Action::Item(Slot::Palette(mine[0])));
            let (fx, fy, fw, fh) = (f.x, f.y, f.w, f.h);
            for _ in 0..40 {
                if palette(&u).contains(mine.last().unwrap()) {
                    break;
                }
                let (cx, cy) = (fx + fw * 0.5, fy + fh * 0.5);
                u.press(7, cx, cy);
                u.moved(7, cx, cy - fh * 2.2);
                assert_eq!(u.release(7, cx, cy - fh * 2.2), None, "a drag picks nothing");
                build(&mut u);
            }
            assert!(palette(&u).contains(mine.last().unwrap()), "{w}x{h}: group {g} scrolls to its end");
        }
        tap(&mut u, Action::BlockGroup(0));
        let hot: Vec<_> = (0..9u8).map(|n| find(&u, Action::Item(Slot::Hotbar(n)))).collect();
        let sort = find(&u, Action::SortItems);
        assert!(hot.iter().all(|b| b.y > hf * 0.8 && b.y == hot[0].y) && hot[0].x > sort.x + sort.w);
        assert!(((hot[0].x + hot[8].x + hot[8].w) * 0.5 - wf * 0.5).abs() < wf * 0.12, "the hotbar is in the middle");
        for a in [Action::MoveSimilar, Action::MoveAll] {
            assert!(find(&u, a).x < hot[0].x);
        }
        // Crafting on the left: the right window keeps its blocks.
        tap(&mut u, Action::LeftTab(1));
        check_layout(&mut u, wf, hf);
        assert!(!has(&u, Action::Item(Slot::Inv(0))));
        for n in 0..4u8 {
            find(&u, Action::Item(Slot::Craft(n)));
        }
        find(&u, Action::Item(Slot::Result));
        assert!(!palette(&u).is_empty());
        // Equipment: four slots and the character between them and the window's middle.
        tap(&mut u, Action::LeftTab(2));
        check_layout(&mut u, wf, hf);
        for n in 0..4u8 {
            find(&u, Action::Item(Slot::Equip(n)));
        }
        let sc = u.showcase.expect("the character shows");
        assert!(sc.x < wf * 0.56 && sc.height > 60.0 * 2.75 && sc.feet < hf * 0.85, "{sc:?}");
        assert!(u.under > 0);
        find(&u, Action::TurnShowcase(1));
        // The right window's equipment tab: the left window keeps the equipment.
        tap(&mut u, Action::RightTab(0));
        check_layout(&mut u, wf, hf);
        assert!(palette(&u).is_empty());
        find(&u, Action::Item(Slot::Equip(0)));
        // The other tabs show no character.
        tap(&mut u, Action::LeftTab(0));
        assert!(u.showcase.is_none());
    }
}

#[test]
fn slots_are_tapped_to_pick_up_and_put_down_merge_swap_and_put_away() {
    let mut u = inventory(2400, 1080);
    let id = |u: &Ui, name: &str| u.blocks.iter().find(|b| b.name == name).unwrap().id;
    let (granite, dirt) = (id(&u, "Granite"), id(&u, "Dirt"));
    let pal = |u: &Ui, name: &str| Slot::Palette(u.blocks.iter().position(|b| b.name == name).unwrap() as u16);
    tap(&mut u, Action::BlockGroup(1));
    // A block from the palette fills a slot with a full stack.
    let (pal_granite, pal_stone) = (pal(&u, "Granite"), pal(&u, "Andesite"));
    reveal(&mut u, pal_granite);
    tap(&mut u, Action::Item(pal_granite));
    assert_eq!(u.picked, Some(pal_granite));
    tap(&mut u, Action::Item(Slot::Inv(0)));
    assert_eq!((u.inventory[0], u.picked), (Stack::of(granite, 64), None));
    // Picked up and put on an empty slot: it moves.
    tap(&mut u, Action::Item(Slot::Inv(0)));
    tap(&mut u, Action::Item(Slot::Inv(5)));
    assert!(u.inventory[0].is_empty() && u.inventory[5] == Stack::of(granite, 64));
    // Onto the same block: they merge up to 64, the rest stays.
    u.inventory[1] = Stack::of(dirt, 40);
    u.inventory[2] = Stack::of(dirt, 30);
    build(&mut u);
    tap(&mut u, Action::Item(Slot::Inv(1)));
    tap(&mut u, Action::Item(Slot::Inv(2)));
    assert_eq!((u.inventory[1], u.inventory[2]), (Stack::of(dirt, 6), Stack::of(dirt, 64)));
    // Onto another block: they swap, between the inventory and the hotbar too.
    tap(&mut u, Action::Item(Slot::Inv(5)));
    tap(&mut u, Action::Item(Slot::Hotbar(4)));
    assert_eq!(u.hotbar[4], Stack::of(granite, 64));
    assert_eq!(u.inventory[5].count, 64);
    assert_ne!(u.inventory[5].id, granite);
    // Tapping the hotbar picks the slot in hand too.
    assert_eq!(u.slot, 0);
    tap(&mut u, Action::Item(Slot::Hotbar(4)));
    assert_eq!((u.slot, u.held_block()), (4, granite));
    // Back onto the palette: put away.
    reveal(&mut u, pal_stone);
    tap(&mut u, Action::Item(pal_stone));
    assert!(u.hotbar[4].is_empty() && u.picked.is_none());
    // Tapping a picked slot again puts it down where it was.
    tap(&mut u, Action::Item(Slot::Inv(2)));
    tap(&mut u, Action::Item(Slot::Inv(2)));
    assert_eq!((u.inventory[2], u.picked), (Stack::of(dirt, 64), None));
    // Blocks can't be worn (picked up on one tab, taken to another).
    tap(&mut u, Action::Item(Slot::Inv(2)));
    tap(&mut u, Action::LeftTab(2));
    assert_eq!(u.picked, Some(Slot::Inv(2)));
    tap(&mut u, Action::Item(Slot::Equip(0)));
    assert!(u.equipment[0].is_empty() && u.inventory[2] == Stack::of(dirt, 64));
    assert!(u.item_note.is_some());
    build(&mut u);
    check_layout(&mut u, 2400.0, 1080.0);
}

#[test]
fn the_crafting_grid_makes_things_and_gives_back_what_is_left() {
    let mut u = inventory(2400, 1080);
    let id = |u: &Ui, name: &str| u.blocks.iter().find(|b| b.name == name).unwrap().id;
    let (sand, sandstone) = (id(&u, "Sand"), id(&u, "Sandstone"));
    u.inventory = [Stack::EMPTY; 36];
    tap(&mut u, Action::LeftTab(1));
    // Nothing in the grid: nothing to take.
    tap(&mut u, Action::Item(Slot::Result));
    assert!(u.item_note.is_some() && u.inventory.iter().all(|s| s.is_empty()));
    // Four sand, in any cells: sandstone.
    for n in 0..4 {
        u.craft[n] = Stack::of(sand, 3);
    }
    build(&mut u);
    assert_eq!(u.stack(Slot::Result), Stack::of(sandstone, 1));
    tap(&mut u, Action::Item(Slot::Result));
    tap(&mut u, Action::Item(Slot::Result));
    assert_eq!(u.inventory[0], Stack::of(sandstone, 2));
    assert!(u.craft.iter().all(|c| *c == Stack::of(sand, 1)));
    // Three sand isn't a recipe.
    u.craft[3] = Stack::EMPTY;
    assert!(u.craft_result().is_none());
    // Closing the inventory gives back what is still in the grid.
    tap(&mut u, Action::Open(Screen::Playing));
    assert!(u.craft.iter().all(|c| c.is_empty()));
    assert_eq!(u.inventory[1], Stack::of(sand, 3));
}

#[test]
fn move_all_move_similar_and_sort() {
    let mut u = inventory(2400, 1080);
    let id = |u: &Ui, name: &str| u.blocks.iter().find(|b| b.name == name).unwrap().id;
    let (stone, dirt, sand) = (id(&u, "Andesite"), id(&u, "Dirt"), id(&u, "Sand"));
    u.inventory = [Stack::EMPTY; 36];
    u.hotbar = [Stack::EMPTY; 9];
    u.hotbar[0] = Stack::of(stone, 10);
    u.hotbar[1] = Stack::of(dirt, 64);
    u.inventory[7] = Stack::of(stone, 60);
    build(&mut u);
    // MOVE SIMILAR: only the stone (the inventory has stone), topping up its stack first.
    tap(&mut u, Action::MoveSimilar);
    assert_eq!((u.inventory[7], u.inventory[0]), (Stack::of(stone, 64), Stack::of(stone, 6)));
    assert!(u.hotbar[0].is_empty() && u.hotbar[1] == Stack::of(dirt, 64));
    // MOVE ALL: the rest of the hotbar.
    u.hotbar[2] = Stack::of(sand, 5);
    build(&mut u);
    tap(&mut u, Action::MoveAll);
    assert!(u.hotbar.iter().all(|s| s.is_empty()));
    // SORT: merged and in the palette's order (stone, dirt, ..., sand), gaps at the end.
    tap(&mut u, Action::SortItems);
    let filled: Vec<Stack> = u.inventory.iter().copied().filter(|s| !s.is_empty()).collect();
    assert_eq!(filled, vec![Stack::of(stone, 64), Stack::of(stone, 6), Stack::of(dirt, 64), Stack::of(sand, 5)]);
    assert!(u.inventory[..4].iter().all(|s| !s.is_empty()));
}

#[test]
fn the_floating_stick_is_drawn_where_the_game_puts_it() {
    let mut u = hud(2400, 1080, true);
    let base_only = build(&mut u).len();
    u.joystick = Some(Joy { base: [400.0, 700.0], knob: [470.0, 650.0], radius: 154.0, alpha: 1.0 });
    let out = build(&mut u);
    assert_eq!(out.len(), base_only + 12, "a ring and a knob");
    let ring = out.chunks(6).position(|q| q.iter().any(|v| v.pos == [400.0 - 154.0, 700.0 - 154.0]) && q.iter().any(|v| v.pos == [400.0 + 154.0, 700.0 + 154.0]));
    let ring = ring.expect("the ring around (400, 700)");
    let knob = &out[(ring + 1) * 6..(ring + 2) * 6];
    let kr = 154.0 * 0.48;
    assert!(knob.iter().any(|v| v.pos == [470.0 - kr, 650.0 - kr]), "the knob at (470, 650)");
    // It fades: alpha in the vertex colour.
    u.joystick = Some(Joy { alpha: 0.5, ..u.joystick.unwrap() });
    let out = build(&mut u);
    assert!((out[ring * 6].color[3] as i32 - 127).abs() <= 1);
}

#[test]
fn the_controls_editor_moves_resizes_hides_and_adds_controls() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = title();
        u.set_surface(w, h, DP);
        u.blocks = blocks();
        build(&mut u);
        tap(&mut u, Action::OpenSettings);
        tap(&mut u, Action::Tab(3));
        check_layout(&mut u, wf, hf);
        for a in [Action::HudOpacity(1), Action::LookSpeed(-1), Action::JoySize(1)] {
            find(&u, a);
        }
        tap(&mut u, Action::OpenControls);
        assert_eq!(u.screen, Screen::Controls);
        check_layout(&mut u, wf, hf);
        // Every control is there to grab, the walking ones too.
        let grabs = u.buttons.iter().filter(|b| matches!(b.action, Action::CtlGrab(_))).count();
        // (all but the one stance button, which stands in for three when that is set)
        assert_eq!(grabs, CONTROLS.iter().filter(|c| c.shown && c.key != "stance").count());
        // Nothing to resize or hide until one is picked.
        assert!(!has(&u, Action::CtlSize(1)) && !has(&u, Action::CtlHide));
        // Drag ATTACK to the middle of the screen.
        let i = CONTROLS.iter().position(|c| c.key == "attack").unwrap();
        let [x, y, cw, ch] = u.control_rect(i);
        let (cx, cy) = (x + cw * 0.5, y + ch * 0.5);
        assert_eq!(u.press(5, cx + 10.0, cy - 10.0), None);
        assert!(u.holds(5));
        u.moved(5, wf * 0.5 + 10.0, hf * 0.5 - 10.0);
        assert_eq!(u.release(5, wf * 0.5 + 10.0, hf * 0.5 - 10.0), None);
        let [x, y, cw, ch] = u.control_rect(i);
        assert!((x + cw * 0.5 - wf * 0.5).abs() < 1.0 && (y + ch * 0.5 - hf * 0.5).abs() < 1.0);
        build(&mut u);
        // Picked: bigger twice; never past the screen's edge when dragged there.
        tap(&mut u, Action::CtlSize(1));
        tap(&mut u, Action::CtlSize(1));
        let [_, _, cw2, _] = u.control_rect(i);
        assert!((cw2 / cw - 1.2).abs() < 0.01, "{cw} -> {cw2}");
        assert!(u.press(6, wf * 0.5, hf * 0.5).is_none());
        u.moved(6, -wf, -hf);
        u.release(6, -wf, -hf);
        let [x, y, cw, ch] = u.control_rect(i);
        let [sl, st, ..] = u.margins();
        assert!((x - sl).abs() < 0.5 && (y - st).abs() < 0.5, "{x},{y}");
        // (and back to the middle, clear of the others)
        build(&mut u);
        u.press(6, x + cw * 0.5, y + ch * 0.5);
        u.moved(6, wf * 0.5, hf * 0.5);
        u.release(6, wf * 0.5, hf * 0.5);
        // Hide it, then ADD puts it back.
        build(&mut u);
        tap(&mut u, Action::CtlHide);
        assert!(!u.layout.places[i].shown);
        tap(&mut u, Action::CtlAddList);
        // The list opens over the controls (they are grabbable again once it closes).
        let out = build(&mut u);
        assert!(out.iter().all(|v| v.pos[0] >= -60.0 && v.pos[0] <= wf + 60.0 && v.pos[1] >= -60.0 && v.pos[1] <= hf + 60.0));
        let hidden = u.layout.places.iter().filter(|p| !p.shown).count();
        assert_eq!(on_top(&u, |a| matches!(a, Action::CtlShow(_))), hidden);
        find(&u, Action::CtlShow(i as u8));
        let wave = CONTROLS.iter().position(|c| c.key == "wave").unwrap();
        tap(&mut u, Action::CtlShow(wave as u8));
        assert!(u.layout.places[wave].shown && !u.add_open);
        build(&mut u);
        tap(&mut u, Action::CtlAddList);
        tap(&mut u, Action::CtlShow(i as u8));
        check_layout(&mut u, wf, hf);
        // The pause button can't be hidden.
        let pause = CONTROLS.iter().position(|c| c.key == "pause").unwrap();
        let [x, y, cw, ch] = u.control_rect(pause);
        u.press(7, x + cw * 0.5, y + ch * 0.5);
        u.release(7, x + cw * 0.5, y + ch * 0.5);
        build(&mut u);
        assert_eq!(u.sel_ctl, Some(pause));
        assert!(!has(&u, Action::CtlHide));
        // DONE saves and goes back to the settings; the layout survives the text file.
        let layout = u.layout.clone();
        tap(&mut u, Action::CtlDone);
        assert_eq!((u.screen, u.take_request()), (Screen::Settings, Some(Request::SaveControls)));
        assert_eq!(Layout::from_text(&layout.to_text()).to_text(), layout.to_text());
        // In the game: WAVE is on screen now, ATTACK where it was put.
        u.screen = Screen::Playing;
        u.settings.walk = true;
        build(&mut u);
        find(&u, Action::Press("wave"));
        let a = find(&u, Action::Hold("attack"));
        let [x, y, ..] = u.control_rect(i);
        assert_eq!((a.x, a.y), (x, y));
        // RESET puts everything back.
        u.apply(Action::OpenControls);
        build(&mut u);
        tap(&mut u, Action::CtlReset);
        assert_eq!(u.layout, Layout::default());
    }
}

#[test]
fn one_stance_button_stands_in_for_crouch_crawl_and_roll() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = hud(w, h, true);
        build(&mut u);
        assert!(!has(&u, Action::Hold("stance")));
        u.screen = Screen::Settings;
        u.tab = 3;
        build(&mut u);
        tap(&mut u, Action::ToggleOneStance);
        assert!(u.settings.one_stance);
        u.screen = Screen::Playing;
        check_layout(&mut u, wf, hf);
        let b = find(&u, Action::Hold("stance"));
        assert!(b.w >= 40.0 * 2.75 && b.h >= 40.0 * 2.75);
        // HUD buttons act on the touch: the game times the taps and the hold.
        assert_eq!(u.press(1, b.x + b.w * 0.5, b.y + b.h * 0.5), Some(Action::Hold("stance")));
        for a in [Action::Press("crouch"), Action::Press("crawl"), Action::Hold("roll")] {
            assert!(!has(&u, a), "{a:?} still shows");
        }
        // Not while flying.
        u.settings.walk = false;
        build(&mut u);
        assert!(!has(&u, Action::Hold("stance")));
        // The editor shows the stance button, not the three; ADD doesn't offer them either.
        u.settings.walk = true;
        u.apply(Action::OpenControls);
        u.apply(Action::CtlAddList);
        check_layout(&mut u, wf, hf);
        let at = |k: &str| CONTROLS.iter().position(|c| c.key == k).unwrap() as u8;
        assert!(has(&u, Action::CtlGrab(at("stance"))));
        for k in ["roll", "crouch", "crawl"] {
            assert!(!has(&u, Action::CtlGrab(at(k))) && !has(&u, Action::CtlShow(at(k))));
        }
        // Back to three buttons.
        u.settings.one_stance = false;
        u.screen = Screen::Playing;
        build(&mut u);
        find(&u, Action::Press("crouch"));
        assert!(!has(&u, Action::Hold("stance")));
    }
}

#[test]
fn a_damaged_controls_file_still_gives_a_layout() {
    let l = Layout::from_text("attack=0.5,0.5,9.0,1\nplace=-,-,0.1,0\npause=0.2,0.2,1.0,0\nnope=1,1,1,1\njump=banana\nwave=x,y,1.0,1\n");
    let at = |k: &str| CONTROLS.iter().position(|c| c.key == k).unwrap();
    assert_eq!(l.places[at("attack")].scale, controls::SCALE_MAX);
    assert_eq!(l.places[at("place")], controls::Place { at: None, scale: controls::SCALE_MIN, shown: false });
    assert!(l.places[at("pause")].shown, "the pause button stays");
    assert_eq!(l.places[at("jump")], Layout::default().places[at("jump")]);
    assert_eq!(l.places[at("wave")], controls::Place { at: None, scale: 1.0, shown: true });
    assert_eq!(Layout::from_text(""), Layout::default());
}

#[test]
fn the_hud_keeps_clear_of_the_safe_area_the_system_reports() {
    let mut u = ui();
    u.settings.walk = true;
    u.set_safe_area(Some([0.0; 4]));
    build(&mut u);
    let corner = find(&u, Action::Open(Screen::Pause));
    let (cx, cy) = (corner.x + corner.w, corner.y);
    // A cut-out on the right and a status bar: everything moves out of the way.
    u.set_safe_area(Some([0.0, 80.0, 140.0, 60.0]));
    build(&mut u);
    let b = find(&u, Action::Open(Screen::Pause));
    assert!(b.x + b.w <= 2400.0 - 140.0 + 0.5 && b.y >= 80.0 - 0.5);
    assert!(b.x + b.w < cx && b.y > cy);
    for b in u.buttons.iter().filter(|b| b.action != Action::Nothing) {
        assert!(b.x + b.w <= 2400.0 - 140.0 + 0.5 && b.y + b.h <= 1080.0 - 60.0 + 0.5, "{:?}", b.action);
    }
    // The menus too.
    for s in [Screen::Title, Screen::WorldList, Screen::Settings, Screen::Pause] {
        u.screen = s;
        build(&mut u);
        for b in u.buttons.iter().filter(|b| b.action != Action::Nothing) {
            assert!(b.x + b.w <= 2400.0 - 140.0 + 0.5 && b.y >= 80.0 - 0.5 && b.y + b.h <= 1080.0 - 60.0 + 0.5, "{:?} on {s:?}", b.action);
        }
    }
}

#[test]
fn settings_have_a_tab_each_and_names_read_well() {
    assert_eq!(TABS.len(), 4);
    for (name, icon, _) in TABS {
        assert!(crate::atlas::atlas().has_icon(icon), "{name}: no {icon} icon");
    }
    // Every printable character can be drawn in both fonts (world names are typed).
    let a = crate::atlas::atlas();
    for f in [crate::atlas::FontId::Serif, crate::atlas::FontId::Pixel] {
        for c in ' '..='~' {
            let g = a.font(f).glyph(c);
            assert!(g.adv > 0.0 && (c == ' ' || g.rect.is_some()), "{c:?} missing");
        }
    }
}

#[test]
fn settings_survive_a_round_trip_through_text() {
    let s = Settings {
        render_chunks: 9,
        lod_level: 3,
        lod_quality: 2,
        fog: false,
        cull: CullMode::Frustum,
        vsync: false,
        greedy: true,
        show_stats: false,
        clouds: false,
        god_rays: false,
        walk: false,
        view: CamView::Front,
        hud_opacity: 50,
        look_speed: 8,
        joy_size: 2,
        touch_build: true,
        one_stance: true,
    };
    let mut back = Settings::default();
    back.apply_text(&s.to_text());
    assert_eq!(back, s);
}

#[test]
fn bad_settings_text_is_ignored_or_clamped() {
    let mut s = Settings::default();
    s.apply_text("render_chunks=200\nlod_level=banana\nlod_quality=99\nfog=maybe\ncull=SIDEWAYS\nno equals sign\nfuture_key=1\nlod_chunks=16\n");
    assert_eq!(s, Settings { render_chunks: RENDER_MAX, lod_quality: LOD_QUALITY.len() as u8 - 1, ..Settings::default() });
    s.apply_text("lod_level=200");
    assert_eq!(s.lod_level, LOD_DISTANCES.len() as u8 - 1);
    s.apply_text("render_chunks=0");
    assert_eq!(s.render_chunks, RENDER_MIN);
    s.apply_text("hud_opacity=3\nlook_speed=99\njoy_size=7");
    assert_eq!((s.hud_opacity, s.look_speed, s.joy_size), (OPACITY_MIN, 10, 2));
}
