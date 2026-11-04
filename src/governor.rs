//! Adaptive quality governor for phones.
//!
//! Feed it each frame's time (and a thermal flag if the OS reports one). When frames
//! run over budget it first lowers render scale, then LOD distance; when there is
//! sustained headroom it restores them in the opposite order (LOD first, since
//! pop-in is more noticeable than slight softness).

pub struct QualityGovernor {
    pub target_ms: f32,
    pub render_scale: f32,
    pub lod_distance_chunks: i32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub min_lod: i32,
    pub max_lod: i32,
    ema_ms: f32,
    over: u32,
    under: u32,
}

const SCALE_STEP: f32 = 0.05;

impl QualityGovernor {
    pub fn new(target_fps: f32, max_lod_chunks: i32) -> Self {
        QualityGovernor {
            target_ms: 1000.0 / target_fps.max(1.0),
            render_scale: 0.9,
            lod_distance_chunks: max_lod_chunks,
            min_scale: 0.6,
            max_scale: 0.9,
            min_lod: 4,
            max_lod: max_lod_chunks,
            ema_ms: 0.0,
            over: 0,
            under: 0,
        }
    }

    /// Returns true if a setting changed this frame.
    pub fn update(&mut self, frame_ms: f32, thermal_hot: bool) -> bool {
        self.ema_ms = if self.ema_ms == 0.0 { frame_ms } else { self.ema_ms * 0.9 + frame_ms * 0.1 };
        let over_budget = thermal_hot || self.ema_ms > self.target_ms * 1.10;
        let headroom = !thermal_hot && self.ema_ms < self.target_ms * 0.75;

        if over_budget {
            self.over += 1;
            self.under = 0;
        } else if headroom {
            self.under += 1;
            self.over = 0;
        } else {
            self.over = 0;
            self.under = 0;
        }

        if self.over >= 20 {
            self.over = 0;
            return self.step_down();
        }
        if self.under >= 240 {
            self.under = 0;
            return self.step_up();
        }
        false
    }

    fn step_down(&mut self) -> bool {
        if self.render_scale - SCALE_STEP >= self.min_scale - 1e-4 {
            self.render_scale = (self.render_scale - SCALE_STEP).max(self.min_scale);
            true
        } else if self.lod_distance_chunks > self.min_lod {
            self.lod_distance_chunks -= 1;
            true
        } else {
            false
        }
    }

    fn step_up(&mut self) -> bool {
        if self.lod_distance_chunks < self.max_lod {
            self.lod_distance_chunks += 1;
            true
        } else if self.render_scale < self.max_scale - 1e-4 {
            self.render_scale = (self.render_scale + SCALE_STEP).min(self.max_scale);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrades_scale_first_then_lod() {
        let mut g = QualityGovernor::new(60.0, 16);
        for _ in 0..200 {
            g.update(40.0, false);
        }
        assert!((g.render_scale - 0.6).abs() < 1e-3, "scale {}", g.render_scale);
        assert!(g.lod_distance_chunks < 16);
        for _ in 0..2000 {
            g.update(40.0, false);
        }
        assert_eq!(g.lod_distance_chunks, g.min_lod); // floors, never below
    }

    #[test]
    fn recovers_lod_before_scale() {
        let mut g = QualityGovernor::new(60.0, 16);
        for _ in 0..400 {
            g.update(40.0, false);
        }
        let (s, l) = (g.render_scale, g.lod_distance_chunks);
        for _ in 0..3000 {
            g.update(8.0, false);
        }
        assert!(g.lod_distance_chunks > l);
        assert!(g.render_scale >= s);
    }

    #[test]
    fn thermal_flag_forces_step_down_even_with_fast_frames() {
        let mut g = QualityGovernor::new(60.0, 16);
        let start = g.render_scale;
        for _ in 0..40 {
            g.update(5.0, true);
        }
        assert!(g.render_scale < start);
    }

    #[test]
    fn steady_frames_change_nothing() {
        let mut g = QualityGovernor::new(60.0, 16);
        for _ in 0..1000 {
            assert!(!g.update(16.0, false));
        }
    }
}
