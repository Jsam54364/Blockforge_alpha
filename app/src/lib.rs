//! Front-end for blockforge: winit window + wgpu renderer + touch controls.
//!
//! On Android the entry point is `android_main` (NativeActivity, packaged by cargo-apk).
//! The same code type-checks and its tests run on a normal PC, which is what CI does
//! before it spends time building the APK.
#![allow(dead_code)]

mod game;
mod gfx;
mod math;

use std::path::PathBuf;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

pub struct App {
    window: Option<Arc<Window>>,
    gfx: Option<gfx::Gfx>,
    game: game::Game,
}

impl App {
    pub fn new(save_dir: Option<PathBuf>) -> App {
        App { window: None, gfx: None, game: game::Game::new(save_dir) }
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
            Ok(g) => {
                self.game.size = (size.width.max(1), size.height.max(1));
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
        self.gfx = None;
        self.window = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.game.size = (size.width.max(1), size.height.max(1));
                if let Some(g) = self.gfx.as_mut() {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::Touch(t) => {
                self.game.touch(t.id, t.phase, t.location.x as f32, t.location.y as f32);
            }
            WindowEvent::RedrawRequested => {
                self.game.update();
                let view_blocks = self.game.view_blocks();
                if let Some(g) = self.gfx.as_mut() {
                    match g.render(&mut self.game.client, &self.game.cam, view_blocks) {
                        Ok(()) => {}
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
