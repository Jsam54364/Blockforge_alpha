//! Front-end for blockforge: winit window + wgpu renderer + touch controls + HUD.
//!
//! On Android the entry point is `android_main` (NativeActivity, packaged by cargo-apk).
//! The same code type-checks and its tests run on a normal PC, which is what CI does
//! before it spends time building the APK.
#![allow(dead_code)]

mod font;
mod game;
mod gfx;
mod glyphs;
mod gpu_timer;
mod math;
mod perf;
mod texgen;
mod textures;
mod tint;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

pub struct App {
    window: Option<Arc<Window>>,
    gfx: Option<gfx::Gfx>,
    game: game::Game,
    perf: perf::Perf,
    hud: Vec<ui::HudVertex>,
}

impl App {
    pub fn new(save_dir: Option<PathBuf>) -> App {
        App {
            window: None,
            gfx: None,
            game: game::Game::new(save_dir),
            perf: perf::Perf::new(),
            hud: Vec::new(),
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
        match pollster::block_on(gfx::Gfx::new(window.clone(), self.game.registry())) {
            Ok(mut g) => {
                g.set_generator(self.game.generator());
                self.game.set_surface(size.width, size.height, window.scale_factor());
                self.game.ui.vsync_supported = g.vsync_supported();
                // A new Gfx has no chunk buffers. After a trip to the background the client
                // still holds every mesh, so hand them all to the renderer again; without
                // this the world stays invisible until chunks happen to change.
                self.game.client.requeue_all_meshes();
                self.gfx = Some(g);
                self.window = Some(window);
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
        self.game.persist();
        self.gfx = None;
        self.window = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.game.persist();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                self.game.set_surface(size.width, size.height, scale);
                if let Some(g) = self.gfx.as_mut() {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::Touch(t) => {
                self.game.touch(t.id, t.phase, t.location.x as f32, t.location.y as f32);
            }
            WindowEvent::RedrawRequested => {
                let frame_start = Instant::now();
                self.game.update();
                let view_blocks = self.game.view_blocks();
                self.perf.chunks = self.game.client.store.len();
                self.perf.meshes = self.game.client.meshes.len();
                if let Some(g) = self.gfx.as_mut() {
                    if let Some(ms) = g.take_gpu_ms() {
                        self.perf.add_gpu_sample(ms);
                    }
                    self.perf.drawn = g.drawn_chunks();
                    self.perf.quads = g.drawn_quads();
                    (self.perf.lod_drawn, self.perf.lod_quads) = g.drawn_lod();
                    self.perf.lod_pending = self.game.client.lod.stats.pending;
                    self.perf.present_mode = g.present_mode_name();
                    self.perf.place = self.game.place();
                    self.game.ui.build(&self.perf, &mut self.hud);
                    let settings = self.game.ui.settings;
                    let (fog_start, fog_end) = self.game.fog_range();
                    let opts = gfx::RenderOptions {
                        fog: settings.fog,
                        frustum_cull: settings.cull.frustum(),
                        backface_cull: settings.cull.backface(),
                        vsync: settings.vsync,
                        fog_start,
                        fog_end,
                    };
                    match g.render(&mut self.game.client, &self.game.cam, view_blocks, &opts, &self.hud) {
                        Ok(display_wait) => {
                            self.perf.end_frame(frame_start.elapsed().saturating_sub(display_wait));
                        }
                        Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                            let (w, h) = g.size();
                            g.resize(w, h);
                        }
                        Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                        Err(_) => {}
                    }
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
    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("could not create the event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut state = App::new(save_dir);
    event_loop.run_app(&mut state).expect("event loop failed");
}
