use super::screens::TABS;
use super::*;

const SIZES: [(u32, u32); 5] = [(2400, 1080), (1920, 1080), (2800, 1260), (2340, 1080), (1080, 2400)];
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
        assert!(v.pos[0] >= -30.0 && v.pos[0] <= w + 30.0 && v.pos[1] >= -30.0 && v.pos[1] <= h + 30.0, "{:?} off a {w}x{h} {:?}", v.pos, u.screen);
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
        for _ in 0..30 - rows {
            tap(&mut u, Action::ScrollWorlds(1));
        }
        find(&u, Action::SelectWorld(29));
        tap(&mut u, Action::ScrollWorlds(1));
        find(&u, Action::SelectWorld(29)); // still at the end
        for _ in 0..30 - rows - 8 + 1 {
            tap(&mut u, Action::ScrollWorlds(-1));
        }
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
        // The pencil on a row edits that row's world.
        tap(&mut u, Action::EditWorldAt(8));
        assert_eq!((u.screen, u.selected, u.fields[0].as_str()), (Screen::EditWorld, Some(8), "World 8"));
        u.selected = Some(7);
        tap(&mut u, Action::Open(Screen::WorldList));
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
fn settings_change_a_copy_that_apply_keeps_and_back_forgets() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = title();
        u.set_surface(w, h, DP);
        build(&mut u);
        tap(&mut u, Action::OpenSettings);
        assert_eq!((u.screen, u.tab), (Screen::Settings, 0));
        // Every group lays out, with its settings on screen.
        let expect: [&[Action]; 3] = [
            &[Action::ToggleWalk, Action::ToggleView, Action::ToggleStats],
            &[Action::RenderDist(1), Action::RenderDist(-1), Action::LodDist(1), Action::CycleLodQuality, Action::ToggleFog, Action::ToggleClouds, Action::ToggleRays, Action::ToggleVsync],
            &[Action::CycleCull, Action::ToggleMesher],
        ];
        for (t, actions) in expect.iter().enumerate() {
            tap(&mut u, Action::Tab(t as u8));
            check_layout(&mut u, wf, hf);
            for a in actions.iter() {
                find(&u, *a);
            }
            // No time of day here: it belongs to each world.
            assert!(u.buttons.iter().all(|b| !matches!(b.action, Action::Time(_))));
        }
        // Changes wait for APPLY ...
        let before = u.settings;
        tap(&mut u, Action::Tab(1));
        tap(&mut u, Action::ToggleFog);
        tap(&mut u, Action::RenderDist(1));
        assert_eq!(u.settings, before);
        assert_ne!(u.draft, before);
        // ... BACK forgets them ...
        tap(&mut u, Action::Open(Screen::Title));
        assert_eq!((u.screen, u.settings), (Screen::Title, before));
        // ... APPLY keeps them, and says when the render distance changed.
        u.apply(Action::OpenSettings);
        u.apply(Action::ToggleFog);
        assert!(!u.apply(Action::ApplySettings));
        assert_eq!((u.screen, u.settings.fog), (Screen::Title, !before.fog));
        u.apply(Action::OpenSettings);
        u.apply(Action::RenderDist(-1));
        assert!(u.apply(Action::ApplySettings));
        // RESET TO DEFAULT puts the copy back to the defaults.
        u.apply(Action::OpenSettings);
        u.apply(Action::ResetSettings);
        assert_eq!(u.draft, Settings { vsync: u.settings.vsync, ..Settings::default() });
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
        tap(&mut u, Action::ApplySettings);
        assert_eq!(u.screen, Screen::Pause);
        tap(&mut u, Action::Open(Screen::Playing));
        assert_eq!(u.screen, Screen::Playing);
        u.apply(Action::Open(Screen::Pause));
        build(&mut u);
        tap(&mut u, Action::Open(Screen::Title));
        assert!(!u.over_world());
    }
}

#[test]
fn the_hud_has_the_menu_button_and_the_characters_buttons() {
    for (w, h) in SIZES {
        let (wf, hf) = (w as f32, h as f32);
        let mut u = ui();
        u.set_surface(w, h, DP);
        u.settings.walk = false;
        check_layout(&mut u, wf, hf);
        assert!(!has(&u, Action::Hold("jump")));
        u.settings.walk = true;
        u.settings.show_stats = true;
        check_layout(&mut u, wf, hf);
        for a in [Action::Hold("jump"), Action::Hold("roll"), Action::Press("crouch"), Action::Press("crawl"), Action::ToggleActs] {
            let b = find(&u, a);
            assert!(b.w >= 40.0 * 2.75 && b.h >= 40.0 * 2.75, "{a:?} is {}x{}", b.w, b.h);
            // HUD buttons act on the touch, not the lift.
            assert_eq!(u.press(9, b.x + 2.0, b.y + 2.0), Some(a));
        }
        u.apply(Action::ToggleActs);
        check_layout(&mut u, wf, hf);
        for a in [Action::Press("punch"), Action::Press("wave"), Action::Press("revive"), Action::ToggleView] {
            find(&u, a);
        }
    }
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
    assert_eq!(TABS.len(), 3);
    for (name, icon) in TABS {
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
}
