//! Frame statistics for the on-screen overlay.
//!
//! * FPS and CPU frame time come from our own timing.
//! * Process CPU use is read from /proc/self/stat (Android allows this for your own
//!   process; the system-wide /proc/stat is blocked for apps since Android 8).
//! * The render (main) thread's own CPU use comes from /proc/thread-self/stat: near 100% means
//!   the CPU is the bottleneck; low with low FPS means the GPU is.
//! * Memory is VmRSS from /proc/self/status.
//! * GPU time comes from `gpu_timer` (timestamp queries). Android exposes no public
//!   "GPU usage %" counter, so busy% is derived: gpu_ms * fps / 1000.

use std::time::{Duration, Instant};

/// Linux USER_HZ; /proc reports CPU time in these ticks.
const CLK_TCK: f32 = 100.0;

pub struct Perf {
    window_start: Instant,
    frames: u32,
    cpu_acc_ms: f32,
    gpu_acc_ms: f32,
    gpu_samples: u32,
    proc_last: Option<(u64, Instant)>,
    thread_last: Option<(u64, Instant)>,
    proc_checked: Instant,
    pub fps: f32,
    pub frame_ms: f32,
    pub cpu_frame_ms: f32,
    pub gpu_ms: Option<f32>,
    /// Cores' worth of CPU time this process used over the last second (e.g. 1.8).
    pub cpu_cores: Option<f32>,
    pub mem_mb: Option<f32>,
    pub n_cores: usize,
    pub chunks: usize,
    pub meshes: usize,
    /// Chunk meshes that survived culling and were drawn last frame.
    pub drawn: usize,
    /// Name of the active present mode (FIFO = vsync).
    pub present_mode: &'static str,
    /// Fraction of one core the main (render) thread used over the last second.
    pub main_thread: Option<f32>,
}

impl Default for Perf {
    fn default() -> Self {
        Self::new()
    }
}

impl Perf {
    pub fn new() -> Perf {
        let now = Instant::now();
        Perf {
            window_start: now,
            frames: 0,
            cpu_acc_ms: 0.0,
            gpu_acc_ms: 0.0,
            gpu_samples: 0,
            proc_last: None,
            thread_last: None,
            proc_checked: now,
            fps: 0.0,
            frame_ms: 0.0,
            cpu_frame_ms: 0.0,
            gpu_ms: None,
            cpu_cores: None,
            mem_mb: None,
            n_cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            chunks: 0,
            meshes: 0,
            drawn: 0,
            present_mode: "FIFO",
            main_thread: None,
        }
    }

    pub fn add_gpu_sample(&mut self, ms: f32) {
        self.gpu_acc_ms += ms;
        self.gpu_samples += 1;
    }

    /// `cpu` = time this frame spent on the CPU (excluding waiting for the display).
    pub fn end_frame(&mut self, cpu: Duration) {
        self.end_frame_at(Instant::now(), cpu);
    }

    pub fn end_frame_at(&mut self, now: Instant, cpu: Duration) {
        self.frames += 1;
        self.cpu_acc_ms += cpu.as_secs_f32() * 1000.0;

        let elapsed = now.saturating_duration_since(self.window_start).as_secs_f32();
        if elapsed >= 0.5 {
            self.fps = self.frames as f32 / elapsed;
            self.frame_ms = 1000.0 / self.fps.max(0.001);
            self.cpu_frame_ms = self.cpu_acc_ms / self.frames as f32;
            if self.gpu_samples > 0 {
                self.gpu_ms = Some(self.gpu_acc_ms / self.gpu_samples as f32);
            }
            self.frames = 0;
            self.cpu_acc_ms = 0.0;
            self.gpu_acc_ms = 0.0;
            self.gpu_samples = 0;
            self.window_start = now;
        }

        if now.saturating_duration_since(self.proc_checked) >= Duration::from_secs(1) {
            self.proc_checked = now;
            if let Some(ticks) = read_proc_ticks() {
                if let Some((t0, i0)) = self.proc_last {
                    let dt = now.saturating_duration_since(i0).as_secs_f32();
                    if dt > 0.0 && ticks >= t0 {
                        self.cpu_cores = Some((ticks - t0) as f32 / CLK_TCK / dt);
                    }
                }
                self.proc_last = Some((ticks, now));
            }
            if let Some(ticks) = read_thread_ticks() {
                if let Some((t0, i0)) = self.thread_last {
                    let dt = now.saturating_duration_since(i0).as_secs_f32();
                    if dt > 0.0 && ticks >= t0 {
                        self.main_thread = Some((ticks - t0) as f32 / CLK_TCK / dt);
                    }
                }
                self.thread_last = Some((ticks, now));
            }
            self.mem_mb = std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| parse_rss_mb(&s));
        }
    }
}

/// CPU ticks used by the calling thread (call from the render thread).
fn read_thread_ticks() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
    parse_stat_ticks(&s)
}

fn read_proc_ticks() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    parse_stat_ticks(&s)
}

/// utime + stime from a /proc/<pid>/stat line. The process name (field 2) may contain
/// spaces, so parsing starts after the last ')'.
pub fn parse_stat_ticks(s: &str) -> Option<u64> {
    let rest = &s[s.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After ')': state is index 0, so utime (field 14) is index 11 and stime index 12.
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    Some(utime + stime)
}

pub fn parse_rss_mb(status: &str) -> Option<f32> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: f32 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024.0);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_even_when_the_name_has_spaces_and_parens() {
        let line = "123 (my (odd) app) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 1 0 100";
        assert_eq!(parse_stat_ticks(line), Some(300));
        assert_eq!(parse_stat_ticks("garbage"), None);
        assert_eq!(parse_stat_ticks("1 (x) S 1 2"), None);
    }

    #[test]
    fn parses_vmrss() {
        let status = "Name:\tapp\nVmPeak:\t  900000 kB\nVmRSS:\t  204800 kB\nThreads:\t12\n";
        assert_eq!(parse_rss_mb(status), Some(200.0));
        assert_eq!(parse_rss_mb("Name: x\n"), None);
    }

    #[test]
    fn fps_and_cpu_frame_time_are_averaged_over_the_window() {
        let mut p = Perf::new();
        let start = Instant::now();
        p.window_start = start;
        p.proc_checked = start;
        for i in 1..=40u32 {
            p.end_frame_at(start + Duration::from_micros(16_667 * i as u64), Duration::from_millis(3));
        }
        assert!((p.fps - 60.0).abs() < 3.0, "fps {}", p.fps);
        assert!((p.cpu_frame_ms - 3.0).abs() < 0.05, "cpu {}", p.cpu_frame_ms);
        assert!(p.gpu_ms.is_none());
    }

    #[test]
    fn gpu_samples_are_averaged_and_survive_empty_windows() {
        let mut p = Perf::new();
        let start = Instant::now();
        p.window_start = start;
        p.proc_checked = start;
        p.add_gpu_sample(4.0);
        p.add_gpu_sample(6.0);
        p.end_frame_at(start + Duration::from_millis(600), Duration::from_millis(1));
        assert_eq!(p.gpu_ms, Some(5.0));
        // A later window with no samples keeps the last reading instead of flickering.
        p.end_frame_at(start + Duration::from_millis(1200), Duration::from_millis(1));
        assert_eq!(p.gpu_ms, Some(5.0));
    }

    #[test]
    fn real_process_stats_are_readable_on_linux() {
        if cfg!(target_os = "linux") || cfg!(target_os = "android") {
            assert!(read_proc_ticks().is_some());
            assert!(read_thread_ticks().is_some());
            let s = std::fs::read_to_string("/proc/self/status").unwrap();
            assert!(parse_rss_mb(&s).unwrap() > 0.0);
        }
    }
}
