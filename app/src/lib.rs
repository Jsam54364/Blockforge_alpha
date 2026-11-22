//! Front-end for blockforge: winit window + wgpu renderer + touch controls + HUD.
//!
//! On Android the entry point is `android_main` (NativeActivity, packaged by cargo-apk).
//! The same code type-checks and its tests run on a normal PC, which is what CI does
//! before it spends time building the APK.
#![allow(dead_code)]

#[cfg(target_os = "android")]
mod android;
mod atlas;
mod clouds;
mod font;
mod game;
mod gfx;
mod glyphs;
mod gpu_timer;
pub mod hyrig;
mod icons;
mod math;
mod perf;
mod player;
mod pool;
mod shadows;
mod sky;
mod texgen;
mod textures;
mod tint;
mod title;
mod ui;
mod worlds;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use blockforge::registry::BlockRegistry;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, TouchPhase, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

pub struct App {
    window: Option<Arc<Window>>,
    gfx: Option<gfx::Gfx>,
    /// Where worlds and settings are kept (`worlds.rs`); `None` keeps everything in memory.
    save_dir: Option<PathBuf>,
    registry: Arc<BlockRegistry>,
    /// The menus while no world is open. An opened world takes it over (`Game::new`) and
    /// hands it back when it closes (`Game::close`).
    ui: ui::Ui,
    game: Option<game::Game>,
    /// When the open world started loading, and its folder (None: kept in memory).
    opened: Instant,
    open_folder: Option<String>,
    /// Set by QUIT on the title screen.
    quit: bool,
    /// The title screen's character (made the first time it shows), and when it last moved.
    showcase: Option<title::Showcase>,
    showcase_time: f32,
    /// The window's size and scale, for a world opened later.
    surface: (u32, u32, f64),
    perf: perf::Perf,
    hud: Vec<ui::HudVertex>,
    frames: u64,
    /// When the app started: the title screen's panorama turns with it.
    started: Instant,
    #[cfg(target_os = "android")]
    android_app: Option<winit::platform::android::activity::AndroidApp>,
    /// Desktop development runs (`run_desktop`): open a world, take a picture, quit.
    dev: Option<DevShot>,
}

/// A development run (desktop only, see `run_desktop`): the world's seed, the time of day,
/// the Shaders setting, how many frames to play before the picture, and where to save it.
#[derive(Clone, Debug)]
struct DevShot {
    seed: u64,
    hours: f64,
    shaders: u8,
    frames: u64,
    path: PathBuf,
    playing_frames: u64,
    /// The camera instead of the game's: (height above the player's, yaw, pitch in degrees).
    cam: Option<(f32, f32, f32)>,
    /// Where to put the player instead of the spawn (x, z).
    at: Option<(f32, f32)>,
}

impl App {
    pub fn new(save_dir: Option<PathBuf>) -> App {
        // Older versions kept one world straight in the save folder: it becomes the first
        // world of the list.
        if let Some(d) = &save_dir {
            if let Err(e) = worlds::migrate(d) {
                eprintln!("blockforge: could not move the old world into the list: {e}");
            }
        }
        let mut ui = ui::Ui::new();
        // A different splash line each launch.
        ui.splash = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as usize);
        if let Some(text) = save_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(worlds::SETTINGS_FILE)).ok()) {
            ui.settings.apply_text(&text);
        }
        if let Some(text) = save_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(worlds::CONTROLS_FILE)).ok()) {
            ui.layout = ui::Layout::from_text(&text);
        }
        let registry = Arc::new(BlockRegistry::builtin());
        ui.blocks = game::block_items(&registry);
        let mut app = App {
            window: None,
            gfx: None,
            save_dir,
            registry,
            ui,
            game: None,
            opened: Instant::now(),
            open_folder: None,
            quit: false,
            showcase: None,
            showcase_time: 0.0,
            surface: (1, 1, 1.0),
            perf: perf::Perf::new(),
            hud: Vec::new(),
            frames: 0,
            started: Instant::now(),
            #[cfg(target_os = "android")]
            android_app: None,
            dev: None,
        };
        app.refresh_worlds();
        app
    }

    /// The UI in charge: the open world's, or the menus'.
    fn ui(&mut self) -> &mut ui::Ui {
        match &mut self.game {
            Some(g) => &mut g.ui,
            None => &mut self.ui,
        }
    }

    /// The phone's time zone, in seconds from UTC (0 where it can't be asked).
    fn utc_offset(&self) -> i64 {
        #[cfg(target_os = "android")]
        if let Some(a) = &self.android_app {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
            return android::utc_offset(a.vm_as_ptr(), ms).unwrap_or(0);
        }
        0
    }

    /// Reads the saved worlds again, and hands their pictures to the renderer.
    fn refresh_worlds(&mut self) {
        let Some(d) = self.save_dir.clone() else { return };
        let offset = self.utc_offset();
        let list = worlds::list(&d);
        let mut entries = Vec::new();
        for (i, w) in list.into_iter().enumerate() {
            // Slot 0 is the picture for worlds without one; each listed world gets its own.
            let slot = i + 1;
            let thumb = (slot < gfx::THUMB_SLOTS).then(|| worlds::load_thumbnail(&d, &w.folder)).flatten().map(|px| {
                if let Some(g) = &self.gfx {
                    g.set_thumbnail(slot, &px);
                }
                slot
            });
            entries.push(ui::WorldEntry {
                last_played: worlds::date_time_label(w.last_played, offset),
                created: worlds::short_date(w.created, offset),
                play_time: worlds::play_time_label(w.play_seconds),
                size: worlds::size_label(w.size_bytes),
                seed: (w.seed as i64).to_string(),
                thumb,
                folder: w.folder,
                name: w.name,
            });
        }
        self.ui().set_worlds(entries);
    }

    fn save_settings(&self) {
        if let Some(d) = &self.save_dir {
            game::write_atomically(&d.join(worlds::SETTINGS_FILE), &self.ui.settings.to_text());
        }
    }

    /// Does what the world screens asked for: play, create, rename or delete a world.
    fn handle_requests(&mut self) {
        while let Some(r) = self.ui().take_request() {
            match r {
                ui::Request::Quit => self.quit = true,
                ui::Request::SaveControls => {
                    if let Some(d) = &self.save_dir {
                        let text = match &self.game {
                            Some(g) => g.ui.layout.to_text(),
                            None => self.ui.layout.to_text(),
                        };
                        game::write_atomically(&d.join(worlds::CONTROLS_FILE), &text);
                    }
                }
                ui::Request::Play(folder) => self.open_world(&folder),
                ui::Request::Create { name, seed } => {
                    let seed = worlds::seed_from_text(&seed);
                    match self.save_dir.clone() {
                        Some(d) => match worlds::create(&d, &name, seed) {
                            Ok(w) => self.open_world(&w.folder),
                            Err(e) => eprintln!("blockforge: could not create the world: {e}"),
                        },
                        None => self.open(game::WorldSpec { dir: None, seed, settings_path: None }),
                    }
                }
                ui::Request::Rename { folder, name } => {
                    if let Some(d) = &self.save_dir {
                        if let Err(e) = worlds::rename(d, &folder, &name) {
                            eprintln!("blockforge: could not rename the world: {e}");
                        }
                    }
                    self.refresh_worlds();
                }
                ui::Request::Delete(folder) => {
                    if let Some(d) = &self.save_dir {
                        if let Err(e) = worlds::delete(d, &folder) {
                            eprintln!("blockforge: could not delete the world: {e}");
                        }
                    }
                    self.refresh_worlds();
                }
            }
        }
    }

    fn open_world(&mut self, folder: &str) {
        let Some(d) = self.save_dir.clone() else { return };
        match worlds::touch(&d, folder) {
            Ok(w) => {
                self.open(game::WorldSpec {
                    dir: Some(worlds::dir(&d, folder)),
                    seed: w.seed,
                    settings_path: Some(d.join(worlds::SETTINGS_FILE)),
                });
                self.open_folder = Some(folder.to_string());
            }
            Err(e) => {
                eprintln!("blockforge: could not open the world: {e}");
                self.refresh_worlds();
            }
        }
    }

    fn open(&mut self, spec: game::WorldSpec) {
        self.save_settings();
        let mut ui = std::mem::take(&mut self.ui);
        ui.screen = ui::Screen::Loading;
        ui.loading_chunks = 0;
        let mut game = game::Game::new(spec, ui, self.registry.clone());
        let (w, h, scale) = self.surface;
        game.set_surface(w, h, scale);
        if let Some(g) = self.gfx.as_mut() {
            g.clear_world();
            g.set_generator(game.generator());
        }
        self.opened = Instant::now();
        self.open_folder = None;
        self.game = Some(game);
    }

    /// The options the open world is drawn with.
    fn render_options(game: &game::Game) -> gfx::RenderOptions {
        let settings = game.ui.settings;
        let (fog_start, fog_end) = game.fog_range();
        gfx::RenderOptions {
            fog: settings.fog,
            frustum_cull: settings.cull.frustum(),
            backface_cull: settings.cull.backface(),
            occlusion_cull: settings.cull.occlusion(),
            vsync: settings.vsync,
            fog_start,
            fog_end,
            time_of_day: game.clock.hours(),
            clouds: settings.clouds,
            god_rays: settings.god_rays,
            shaders: shadows::Quality::from_index(settings.shaders),
            figure: None,
        }
    }

    /// Saves and closes the open world, back to the title screen.
    fn close_world(&mut self) {
        if let Some(mut game) = self.game.take() {
            // A picture of where the player left the world, for the world list, and the
            // time spent in it.
            if let (Some(d), Some(folder)) = (self.save_dir.clone(), self.open_folder.take()) {
                if let Some(g) = self.gfx.as_mut() {
                    if game.ui.screen != ui::Screen::Loading {
                        g.set_character(game.player.triangles());
                        let cam = game.render_cam();
                        let opts = Self::render_options(&game);
                        let view = game.view_blocks();
                        if let Some((px, w, h)) = g.capture_world(&mut game.client, &cam, view, &opts) {
                            if let Err(e) = worlds::save_thumbnail(&d, &folder, &px, w, h) {
                                eprintln!("blockforge: could not save the world's picture: {e}");
                            }
                        }
                    }
                }
                let _ = worlds::add_play_time(&d, &folder, self.opened.elapsed().as_secs());
            }
            let mut ui = game.close();
            ui.screen = ui::Screen::Title;
            ui.loading_chunks = 0;
            self.ui = ui;
            if let Some(g) = self.gfx.as_mut() {
                g.clear_world();
            }
            self.refresh_worlds();
        }
    }

    /// Asks the activity how far the system bars and cut-out reach into the window, twice a
    /// second (they change when the phone turns or the bars are swiped in).
    fn update_safe_area(&mut self) {
        #[cfg(target_os = "android")]
        if self.frames % 30 == 1 {
            if let Some(a) = &self.android_app {
                let safe = android::safe_insets(a.vm_as_ptr(), a.activity_as_ptr());
                self.ui().set_safe_area(safe);
            }
        }
    }

    /// A touch while no world is open: only the menus' buttons (they act when the finger
    /// lifts off them).
    fn menu_touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32) {
        let action = match phase {
            TouchPhase::Started => self.ui.press(id, x, y),
            TouchPhase::Ended => self.ui.release(id, x, y),
            TouchPhase::Cancelled => self.ui.release(id, f32::NAN, f32::NAN),
            TouchPhase::Moved => {
                self.ui.moved(id, x, y);
                None
            }
        };
        if let Some(action) = action {
            let before = self.ui.screen;
            self.ui.apply(action);
            if before == ui::Screen::Settings && self.ui.screen != before {
                self.save_settings();
            }
        }
    }

    /// The picture shown for worlds that have none yet: the sunset from the title screen.
    fn default_thumbnail(&mut self) {
        let Some(g) = &self.gfx else { return };
        if let Ok((px, w, h)) = title::panorama(4096) {
            // Around the sun (yaw pi/2: a quarter of the way along), 16:9.
            let ch = h * 9 / 10;
            let cw = (ch * 16 / 9).min(w);
            let x0 = (w / 4).saturating_sub(cw / 2);
            let y0 = (h - ch) / 2;
            let mut crop = Vec::with_capacity((cw * ch * 4) as usize);
            for y in y0..y0 + ch {
                let r = ((y * w + x0) * 4) as usize;
                crop.extend_from_slice(&px[r..r + (cw * 4) as usize]);
            }
            g.set_thumbnail(0, &worlds::shrink(&crop, cw, ch));
            self.ui().default_thumb = Some(0);
        }
    }

    /// Development runs: hold the time of day and the Shaders setting, count frames played.
    fn dev_frame(&mut self) {
        let (Some(d), Some(game)) = (self.dev.as_mut(), self.game.as_mut()) else { return };
        game.clock.shift(d.hours - game.clock.hours());
        game.ui.settings.shaders = d.shaders;
        if game.ui.screen == ui::Screen::Playing {
            d.playing_frames += 1;
        }
    }

    fn dev_capture(&mut self) {
        let (Some(d), Some(g), Some(game)) = (self.dev.clone(), self.gfx.as_mut(), self.game.as_mut()) else { return };
        g.set_character(game.player.triangles());
        let cam = dev_cam(&self.dev, game);
        let opts = Self::render_options(game);
        let view = game.view_blocks();
        match g.capture_world(&mut game.client, &cam, view, &opts) {
            Some((px, w, h)) => match save_png(&d.path, &px, w, h) {
                Ok(()) => eprintln!("blockforge: saved {}", d.path.display()),
                Err(e) => eprintln!("blockforge: could not save {}: {e}", d.path.display()),
            },
            None => eprintln!("blockforge: nothing captured"),
        }
    }

    fn render(&mut self, event_loop: &ActiveEventLoop, frame_start: Instant) {
        let Some(g) = self.gfx.as_mut() else { return };
        if let Some(ms) = g.take_gpu_ms() {
            self.perf.add_gpu_sample(ms);
        }
        self.perf.drawn = g.drawn_chunks();
        self.perf.hidden = g.hidden_chunks();
        self.perf.pool = g.pool_usage();
        self.perf.quads = g.drawn_quads();
        (self.perf.lod_drawn, self.perf.lod_quads) = g.drawn_lod();
        self.perf.present_mode = g.present_mode_name();
        if self.perf.gpu.is_empty() {
            self.perf.gpu = g.gpu_line().to_string();
        }
        let seconds = self.started.elapsed().as_secs_f32();
        let ui = match &mut self.game {
            Some(game) => {
                self.perf.chunks = game.client.store.len();
                self.perf.meshes = game.client.meshes.len();
                self.perf.lod_pending = game.client.lod.stats.pending;
                self.perf.place = game.place();
                &mut game.ui
            }
            None => &mut self.ui,
        };
        ui.seconds = seconds;
        ui.build(&self.perf, &mut self.hud);
        // The icons the UI asked for, a few dozen a frame (the rest show over the next ones).
        for (slot, item) in crate::icons::take_pending(48) {
            g.draw_icon(&self.registry, slot, item);
        }
        let over_world = ui.over_world();
        let (under, figure) = (ui.under, ui.showcase);
        let result = match (&mut self.game, over_world) {
            (Some(game), true) => {
                let mut opts = Self::render_options(game);
                g.set_character(game.player.triangles());
                // The inventory's equipment tab shows the character (the title screen's idling
                // figure).
                if let Some(sc) = figure {
                    let dt = seconds - self.showcase_time;
                    let verts = self.showcase.get_or_insert_with(title::Showcase::new).update(dt, sc.yaw);
                    g.set_portrait(verts);
                    opts.figure = Some((sc, under));
                }
                self.showcase_time = seconds;
                let render_cam = dev_cam(&self.dev, game);
                let view_blocks = game.view_blocks();
                g.render(&mut game.client, &render_cam, view_blocks, &opts, &self.hud)
            }
            // The title screen, its pages and the loading screen: the panorama behind them,
            // and on the title screen the character on his pedestal.
            _ => {
                if let Some(sc) = figure {
                    let dt = seconds - self.showcase_time;
                    let verts = self.showcase.get_or_insert_with(title::Showcase::new).update(dt, sc.yaw);
                    g.set_character(verts);
                }
                self.showcase_time = seconds;
                g.render_title(title::yaw(seconds), &self.hud, under, figure)
            }
        };
        match result {
            Ok(display_wait) => self.perf.end_frame(frame_start.elapsed().saturating_sub(display_wait)),
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                let (w, h) = g.size();
                g.resize(w, h);
            }
            Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
            Err(_) => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Blockforge");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("blockforge: could not create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        match pollster::block_on(gfx::Gfx::new(window.clone(), &self.registry)) {
            Ok(mut g) => {
                // A GPU with room for fewer textures than the blocks have: mesh with the
                // registry that draws the extra ones as similar ones (games open later use it).
                if let Some(map) = &g.texture_remap {
                    self.registry = Arc::new(self.registry.remap_textures(map));
                }
                self.surface = (size.width, size.height, window.scale_factor());
                self.ui.set_surface(size.width, size.height, window.scale_factor());
                self.ui.vsync_supported = g.vsync_supported();
                if let Some(game) = self.game.as_mut() {
                    g.set_generator(game.generator());
                    game.set_surface(size.width, size.height, window.scale_factor());
                    game.ui.vsync_supported = g.vsync_supported();
                    // A new Gfx has no chunk buffers. After a trip to the background the
                    // client still holds every mesh, so hand them all to the renderer again;
                    // without this the world stays invisible until chunks happen to change.
                    game.client.requeue_all_meshes();
                }
                self.gfx = Some(g);
                self.window = Some(window);
                crate::icons::reset_icons();
                self.default_thumbnail();
                self.refresh_worlds();
                if let Some(d) = self.dev.clone() {
                    self.open(game::WorldSpec { dir: None, seed: d.seed, settings_path: None });
                    if let (Some((x, z)), Some(game)) = (d.at, self.game.as_mut()) {
                        let y = game.generator().land().height(x, z, 1.0).max(1.0);
                        game.player.place([x as f64 + 0.5, y as f64 + 2.0, z as f64 + 0.5]);
                        game.player.set_fly(true);
                    }
                }
            }
            Err(e) => {
                eprintln!("blockforge: graphics init failed: {e}");
                event_loop.exit();
            }
        }
    }

    // Android destroys the native window when the app goes to the background:
    // the surface must be dropped here and recreated in `resumed`.
    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Android may kill a backgrounded app without another callback: save now.
        match &self.game {
            Some(game) => game.persist(),
            None => self.save_settings(),
        }
        self.gfx = None;
        self.window = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.close_world();
                self.save_settings();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                self.surface = (size.width, size.height, scale);
                match self.game.as_mut() {
                    Some(game) => game.set_surface(size.width, size.height, scale),
                    None => self.ui.set_surface(size.width, size.height, scale),
                }
                if let Some(g) = self.gfx.as_mut() {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::Touch(t) => {
                let (x, y) = (t.location.x as f32, t.location.y as f32);
                match self.game.as_mut() {
                    Some(game) => game.touch(t.id, t.phase, x, y),
                    None => self.menu_touch(t.id, t.phase, x, y),
                }
                self.handle_requests();
                if self.quit {
                    self.save_settings();
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // A hardware keyboard can type into the world screens' fields too.
                match &event.logical_key {
                    Key::Named(NamedKey::Backspace) => {
                        self.ui().apply(ui::Action::Backspace);
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.ui().apply(ui::Action::KeysDone);
                    }
                    _ => {
                        if let Some(text) = &event.text {
                            let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                            self.ui().type_text(&printable);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let frame_start = Instant::now();
                self.frames += 1;
                self.update_safe_area();
                if let Some(game) = self.game.as_mut() {
                    game.update();
                    if game.ui.screen == ui::Screen::Loading {
                        game.ui.loading_chunks = game.client.meshes.len();
                        if game.ready(self.opened.elapsed()) {
                            game.ui.screen = ui::Screen::Playing;
                        }
                    }
                }
                // SAVE & QUIT in the game's menu: save and close the world.
                if self.game.as_ref().is_some_and(|g| !matches!(g.ui.screen, ui::Screen::Playing | ui::Screen::Loading | ui::Screen::Pause | ui::Screen::Settings | ui::Screen::Controls | ui::Screen::Inventory)) {
                    self.close_world();
                }
                self.dev_frame();
                self.render(event_loop, frame_start);
                if self.dev.as_ref().is_some_and(|d| d.playing_frames > d.frames) {
                    self.dev_capture();
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    use winit::event_loop::{ControlFlow, EventLoop};
    use winit::platform::android::EventLoopBuilderExtAndroid;

    let save_dir = app.internal_data_path();
    let handle = app.clone();
    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("could not create the event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut state = App::new(save_dir);
    state.android_app = Some(handle);
    event_loop.run_app(&mut state).expect("event loop failed");
}

/// The camera a development run draws from (`DevShot::cam`), else the game's.
fn dev_cam(dev: &Option<DevShot>, game: &game::Game) -> game::CameraView {
    let mut cam = game.render_cam();
    if let Some((dy, yaw, pitch)) = dev.as_ref().and_then(|d| d.cam) {
        let p = game.player.position();
        cam.pos = [p[0] as f32, p[1] as f32 + dy, p[2] as f32];
        cam.yaw = yaw.to_radians();
        cam.pitch = pitch.to_radians();
    }
    cam
}

fn save_png(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

/// Desktop development run of the real game: opens an in-memory world, plays `frames` frames
/// at a fixed time of day and takes a picture with the game's own renderer. Settings come
/// from the environment: BLOCKFORGE_SEED, BLOCKFORGE_HOURS, BLOCKFORGE_SHADERS (0..2),
/// BLOCKFORGE_FRAMES, BLOCKFORGE_SHOT (the picture's path), BLOCKFORGE_CAM ("height above
/// the player, yaw, pitch" in blocks and degrees), BLOCKFORGE_AT ("x,z": where the player
/// starts instead of the spawn). See `examples/devshot.rs`.
#[cfg(not(target_os = "android"))]
pub fn run_desktop() {
    use winit::event_loop::{ControlFlow, EventLoop};
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let event_loop = EventLoop::new().expect("could not create the event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut state = App::new(None);
    state.dev = Some(DevShot {
        seed: var("BLOCKFORGE_SEED", "1337").parse().unwrap_or(1337),
        hours: var("BLOCKFORGE_HOURS", "10").parse().unwrap_or(10.0),
        shaders: var("BLOCKFORGE_SHADERS", "1").parse().unwrap_or(1),
        frames: var("BLOCKFORGE_FRAMES", "120").parse().unwrap_or(120),
        path: PathBuf::from(var("BLOCKFORGE_SHOT", "devshot.png")),
        playing_frames: 0,
        cam: std::env::var("BLOCKFORGE_CAM").ok().and_then(|v| {
            let n: Vec<f32> = v.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            (n.len() == 3).then(|| (n[0], n[1], n[2]))
        }),
        at: std::env::var("BLOCKFORGE_AT").ok().and_then(|v| {
            let n: Vec<f32> = v.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            (n.len() == 2).then(|| (n[0], n[1]))
        }),
    });
    event_loop.run_app(&mut state).expect("event loop failed");
}
