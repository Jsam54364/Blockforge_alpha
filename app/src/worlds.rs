//! Saved worlds, as Minecraft keeps them: each in its own folder under `worlds/`, with a
//! name, a seed and when it was created and last played (`level.txt`), the player's place
//! and the time of day (`state.txt`) and the edits (the server's journal). Settings are the
//! app's, not a world's (`settings.txt` beside `worlds/`).
//!
//! Folder names come from world names (letters and digits, the rest `_`); two worlds may
//! share a name, never a folder (`NEW_WORLD`, `NEW_WORLD-2`, ...).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const WORLDS_DIR: &str = "worlds";
pub const LEVEL_FILE: &str = "level.txt";
pub const SETTINGS_FILE: &str = "settings.txt";
/// Where the game's buttons are (the controls editor's layout), beside the settings.
pub const CONTROLS_FILE: &str = "controls.txt";
/// Where a world's player, camera and clock are kept (in its folder).
pub const STATE_FILE: &str = "state.txt";
/// The seed of the world everyone had before there were several.
pub const FIRST_SEED: u64 = 1337;
pub const MAX_NAME: usize = 24;
pub const MAX_SEED: usize = 20;

/// One saved world, as the world list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldInfo {
    /// Its folder under `worlds/` (also how it is told apart).
    pub folder: String,
    pub name: String,
    pub seed: u64,
    /// Seconds since 1970.
    pub created: u64,
    pub last_played: u64,
    /// Seconds spent in it.
    pub play_seconds: u64,
    /// Bytes its folder takes (not saved: measured by `list`).
    pub size_bytes: u64,
}

impl WorldInfo {
    fn to_text(&self) -> String {
        format!(
            "name={}\nseed={}\ncreated={}\nlast_played={}\nplay_seconds={}\n",
            self.name, self.seed, self.created, self.last_played, self.play_seconds
        )
    }

    fn from_text(folder: &str, text: &str) -> Option<WorldInfo> {
        let get = |k: &str| text.lines().find_map(|l| l.trim_end().strip_prefix(k)?.strip_prefix('=').map(str::to_string));
        Some(WorldInfo {
            folder: folder.to_string(),
            name: get("name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| folder.to_string()),
            seed: get("seed")?.trim().parse().ok()?,
            created: get("created").and_then(|v| v.trim().parse().ok()).unwrap_or(0),
            last_played: get("last_played").and_then(|v| v.trim().parse().ok()).unwrap_or(0),
            play_seconds: get("play_seconds").and_then(|v| v.trim().parse().ok()).unwrap_or(0),
            size_bytes: 0,
        })
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A world's folder.
pub fn dir(save_dir: &Path, folder: &str) -> PathBuf {
    save_dir.join(WORLDS_DIR).join(folder)
}

/// The seed a typed seed stands for: a number is itself, other text is hashed (so a word
/// always gives the same world), nothing at all is random.
pub fn seed_from_text(text: &str) -> u64 {
    let t = text.trim();
    if t.is_empty() {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        return splitmix(n);
    }
    if let Ok(n) = t.parse::<i64>() {
        return n as u64;
    }
    // FNV-1a.
    t.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// A folder name for a world called `name` that no world in `save_dir` uses yet.
pub fn free_folder(save_dir: &Path, name: &str) -> String {
    let mut base: String = name.trim().chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    if base.trim_matches('_').is_empty() {
        base = "WORLD".to_string();
    }
    base.truncate(MAX_NAME);
    let taken = |f: &str| dir(save_dir, f).exists();
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|f| !taken(f)).expect("a free name")
}

/// The saved worlds, most recently played first.
pub fn list(save_dir: &Path) -> Vec<WorldInfo> {
    let Ok(entries) = fs::read_dir(save_dir.join(WORLDS_DIR)) else { return Vec::new() };
    let mut out: Vec<WorldInfo> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let folder = e.file_name().to_string_lossy().to_string();
            let text = fs::read_to_string(e.path().join(LEVEL_FILE)).ok()?;
            let mut w = WorldInfo::from_text(&folder, &text)?;
            w.size_bytes = folder_size(&e.path());
            Some(w)
        })
        .collect();
    out.sort_by(|a, b| b.last_played.cmp(&a.last_played).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Makes a new world (an empty folder with its `level.txt`): generated when first played.
pub fn create(save_dir: &Path, name: &str, seed: u64) -> io::Result<WorldInfo> {
    let name = clean_name(name);
    let folder = free_folder(save_dir, &name);
    fs::create_dir_all(dir(save_dir, &folder))?;
    let t = now();
    let w = WorldInfo { folder, name, seed, created: t, last_played: t, play_seconds: 0, size_bytes: 0 };
    write_level(save_dir, &w)?;
    Ok(w)
}

/// Renames a world (its folder stays).
pub fn rename(save_dir: &Path, folder: &str, name: &str) -> io::Result<WorldInfo> {
    let mut w = read(save_dir, folder)?;
    w.name = clean_name(name);
    write_level(save_dir, &w)?;
    Ok(w)
}

/// Notes that a world is being played now (it moves to the top of the list).
pub fn touch(save_dir: &Path, folder: &str) -> io::Result<WorldInfo> {
    let mut w = read(save_dir, folder)?;
    w.last_played = now();
    write_level(save_dir, &w)?;
    Ok(w)
}

/// Adds time spent playing a world.
pub fn add_play_time(save_dir: &Path, folder: &str, secs: u64) -> io::Result<()> {
    let mut w = read(save_dir, folder)?;
    w.play_seconds += secs;
    write_level(save_dir, &w)
}

fn folder_size(p: &Path) -> u64 {
    fs::read_dir(p).map_or(0, |entries| {
        entries.filter_map(|e| e.ok()).map(|e| e.metadata().map_or(0, |m| if m.is_dir() { folder_size(&e.path()) } else { m.len() })).sum()
    })
}

/// Thumbnails (`icon.png` in a world's folder), as the world list shows them.
pub const THUMB_FILE: &str = "icon.png";
pub const THUMB_W: u32 = 256;
pub const THUMB_H: u32 = 144;

/// Shrinks a picture (RGBA, `w` x `h`) to a thumbnail: the middle 16:9 of it, averaged
/// down to `THUMB_W` x `THUMB_H`.
pub fn shrink(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (cw, ch) = if w * 9 > h * 16 { (h * 16 / 9, h) } else { (w, w * 9 / 16) };
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let mut out = vec![0u8; (THUMB_W * THUMB_H * 4) as usize];
    for ty in 0..THUMB_H {
        let (sy0, sy1) = (y0 + ty * ch / THUMB_H, (y0 + (ty + 1) * ch / THUMB_H).max(y0 + ty * ch / THUMB_H + 1));
        for tx in 0..THUMB_W {
            let (sx0, sx1) = (x0 + tx * cw / THUMB_W, (x0 + (tx + 1) * cw / THUMB_W).max(x0 + tx * cw / THUMB_W + 1));
            let mut acc = [0u32; 4];
            for sy in sy0..sy1.min(h) {
                for sx in sx0..sx1.min(w) {
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += rgba[i + c] as u32;
                    }
                }
            }
            let n = ((sy1.min(h) - sy0) * (sx1.min(w) - sx0)).max(1);
            let o = ((ty * THUMB_W + tx) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    out
}

/// Saves a world's thumbnail (from a picture of any size).
pub fn save_thumbnail(save_dir: &Path, folder: &str, rgba: &[u8], w: u32, h: u32) -> io::Result<()> {
    let small = shrink(rgba, w, h);
    let path = dir(save_dir, folder).join(THUMB_FILE);
    let f = fs::File::create(&path)?;
    let mut e = png::Encoder::new(io::BufWriter::new(f), THUMB_W, THUMB_H);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().and_then(|mut wr| wr.write_image_data(&small)).map_err(io::Error::other)
}

/// A world's thumbnail (`THUMB_W` x `THUMB_H` RGBA), if it has one.
pub fn load_thumbnail(save_dir: &Path, folder: &str) -> Option<Vec<u8>> {
    let f = fs::File::open(dir(save_dir, folder).join(THUMB_FILE)).ok()?;
    let mut r = png::Decoder::new(io::BufReader::new(f)).read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    (info.width == THUMB_W && info.height == THUMB_H && info.color_type == png::ColorType::Rgba).then(|| {
        buf.truncate((THUMB_W * THUMB_H * 4) as usize);
        buf
    })
}

/// "11/14/2026 9:42 PM" for seconds since 1970, shifted by `offset` seconds (the phone's
/// time zone).
pub fn date_time_label(secs: u64, offset: i64) -> String {
    let t = secs as i64 + offset;
    let mins = t.rem_euclid(86400) / 60;
    let (hh, mm) = (mins / 60, mins % 60);
    let (h12, ampm) = match hh {
        0 => (12, "AM"),
        1..=11 => (hh, "AM"),
        12 => (12, "PM"),
        _ => (hh - 12, "PM"),
    };
    format!("{} {h12}:{mm:02} {ampm}", short_date(secs, offset))
}

/// "11/03/2026" for seconds since 1970, shifted by `offset` seconds.
pub fn short_date(secs: u64, offset: i64) -> String {
    let (y, m, d) = civil((secs as i64 + offset).div_euclid(86400));
    format!("{m:02}/{d:02}/{y}")
}

/// "12h 34m", "5m", "Under a minute".
pub fn play_time_label(secs: u64) -> String {
    let m = secs / 60;
    match (m / 60, m % 60) {
        (0, 0) => "Under a minute".to_string(),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// "256 MB", "1.2 MB", "48 KB".
pub fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 10.0 {
        format!("{:.0} MB", mb)
    } else if mb >= 1.0 {
        format!("{:.1} MB", mb)
    } else {
        format!("{} KB", (bytes as f64 / 1024.0).ceil() as u64)
    }
}

/// Days since 1970 to (year, month, day): Howard Hinnant's days-to-civil.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// Deletes a world and everything in its folder, for good.
pub fn delete(save_dir: &Path, folder: &str) -> io::Result<()> {
    // Only ever a direct child of worlds/, never anything above it.
    if folder.is_empty() || folder.contains(['/', '\\']) || folder == "." || folder == ".." {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a world folder"));
    }
    fs::remove_dir_all(dir(save_dir, folder))
}

pub fn read(save_dir: &Path, folder: &str) -> io::Result<WorldInfo> {
    let text = fs::read_to_string(dir(save_dir, folder).join(LEVEL_FILE))?;
    WorldInfo::from_text(folder, &text).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad level.txt"))
}

fn write_level(save_dir: &Path, w: &WorldInfo) -> io::Result<()> {
    let path = dir(save_dir, &w.folder).join(LEVEL_FILE);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, w.to_text())?;
    fs::rename(tmp, path)
}

fn clean_name(name: &str) -> String {
    let n: String = name.trim().chars().take(MAX_NAME).collect();
    if n.is_empty() {
        "NEW WORLD".to_string()
    } else {
        n
    }
}

/// Moves the one world of older versions (its journal and `state.txt`, straight in the
/// save folder) into `worlds/MY_WORLD`, and copies its settings to `settings.txt`. Does
/// nothing once `worlds/` exists. Returns the moved world, if any.
pub fn migrate(save_dir: &Path) -> io::Result<Option<WorldInfo>> {
    if save_dir.join(WORLDS_DIR).exists() {
        return Ok(None);
    }
    let old: Vec<PathBuf> = fs::read_dir(save_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            n == STATE_FILE || (n.starts_with("world") && n.contains(".bfwj"))
        })
        .collect();
    fs::create_dir_all(save_dir.join(WORLDS_DIR))?;
    if old.is_empty() {
        return Ok(None);
    }
    let state = save_dir.join(STATE_FILE);
    if state.exists() && !save_dir.join(SETTINGS_FILE).exists() {
        // The old state file held the settings too; the settings parser skips the rest.
        fs::copy(&state, save_dir.join(SETTINGS_FILE))?;
    }
    let mut w = create(save_dir, "MY WORLD", FIRST_SEED)?;
    let into = dir(save_dir, &w.folder);
    for p in old {
        let name = p.file_name().expect("a file").to_owned();
        fs::rename(&p, into.join(name))?;
    }
    w.created = 0;
    write_level(save_dir, &w)?;
    Ok(Some(w))
}

/// "14 NOV 2026" for seconds since 1970 (UTC).
pub fn date_label(secs: u64) -> String {
    const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bf_worlds_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn worlds_are_created_listed_renamed_and_deleted() {
        let d = temp("cycle");
        assert!(list(&d).is_empty());
        let a = create(&d, "New World", 42).unwrap();
        let b = create(&d, "New World", 7).unwrap();
        let c = create(&d, "  ", 9).unwrap();
        assert_eq!(a.folder, "NEW_WORLD");
        assert_eq!(b.folder, "NEW_WORLD-2", "same name, another folder");
        assert_eq!((c.name.as_str(), c.folder.as_str()), ("NEW WORLD", "NEW_WORLD-3"));
        assert_eq!(list(&d).len(), 3);
        // Playing one moves it to the top.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        touch(&d, &a.folder).unwrap();
        assert_eq!(list(&d)[0].folder, a.folder);
        let r = rename(&d, &a.folder, "ISLAND").unwrap();
        assert_eq!((r.name.as_str(), r.seed, r.folder.as_str()), ("ISLAND", 42, "NEW_WORLD"));
        assert_eq!(read(&d, "NEW_WORLD").unwrap().name, "ISLAND");
        // Deleting takes the whole folder, and nothing else.
        fs::write(dir(&d, &b.folder).join("world_v4.bfwj"), b"x").unwrap();
        delete(&d, &b.folder).unwrap();
        assert!(!dir(&d, &b.folder).exists());
        assert_eq!(list(&d).len(), 2);
        assert!(delete(&d, "..").is_err() && delete(&d, "").is_err() && delete(&d, "a/b").is_err());
        assert!(d.join(WORLDS_DIR).exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_old_single_world_moves_into_the_list() {
        let d = temp("migrate");
        fs::write(d.join(STATE_FILE), "fog=true\nplayer=1,2,3\ngen=4\n").unwrap();
        fs::write(d.join("world_v4.bfwj"), b"journal").unwrap();
        fs::write(d.join("world.bfwj"), b"older journal").unwrap();
        let w = migrate(&d).unwrap().expect("moved");
        assert_eq!((w.name.as_str(), w.seed), ("MY WORLD", FIRST_SEED));
        let into = dir(&d, &w.folder);
        assert_eq!(fs::read(into.join("world_v4.bfwj")).unwrap(), b"journal");
        assert!(into.join("world.bfwj").exists() && into.join(STATE_FILE).exists());
        assert!(!d.join("world_v4.bfwj").exists() && !d.join(STATE_FILE).exists());
        assert!(fs::read_to_string(d.join(SETTINGS_FILE)).unwrap().contains("fog=true"));
        let listed: Vec<WorldInfo> = list(&d).into_iter().map(|w| WorldInfo { size_bytes: 0, ..w }).collect();
        assert_eq!(listed, vec![read(&d, &w.folder).unwrap()]);
        // Only once.
        assert_eq!(migrate(&d).unwrap(), None);
        // A fresh install gets an empty list.
        let e = temp("fresh");
        assert_eq!(migrate(&e).unwrap(), None);
        assert!(e.join(WORLDS_DIR).is_dir() && list(&e).is_empty());
        fs::remove_dir_all(&d).unwrap();
        fs::remove_dir_all(&e).unwrap();
    }

    #[test]
    fn typed_seeds() {
        assert_eq!(seed_from_text("1337"), 1337);
        assert_eq!(seed_from_text(" -5 "), (-5i64) as u64);
        assert_eq!(seed_from_text("GLACIER"), seed_from_text("GLACIER"));
        assert_ne!(seed_from_text("GLACIER"), seed_from_text("GLACIERS"));
        assert_ne!(seed_from_text(""), seed_from_text("GLACIER"));
    }

    #[test]
    fn labels_for_the_world_list() {
        assert_eq!(date_time_label(1_763_136_000, 0), "11/14/2025 4:00 PM");
        assert_eq!(date_time_label(1_763_136_000, -7 * 3600), "11/14/2025 9:00 AM");
        assert_eq!(date_time_label(0, 0), "01/01/1970 12:00 AM");
        assert_eq!(short_date(1_763_136_000, 0), "11/14/2025");
        assert_eq!(play_time_label(30), "Under a minute");
        assert_eq!(play_time_label(5 * 60 + 10), "5m");
        assert_eq!(play_time_label(12 * 3600 + 34 * 60), "12h 34m");
        assert_eq!(size_label(256 * 1_048_576), "256 MB");
        assert_eq!(size_label(1_258_291), "1.2 MB");
        assert_eq!(size_label(48 * 1024), "48 KB");
    }

    #[test]
    fn thumbnails_are_shrunk_saved_and_read_back() {
        let d = temp("thumbs");
        let w = create(&d, "Pic", 1).unwrap();
        assert_eq!(load_thumbnail(&d, &w.folder), None);
        // A 2400 x 1080 picture: left half red, right half blue.
        let (pw, ph) = (2400u32, 1080u32);
        let mut px = vec![0u8; (pw * ph * 4) as usize];
        for y in 0..ph {
            for x in 0..pw {
                let i = ((y * pw + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(if x < pw / 2 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        save_thumbnail(&d, &w.folder, &px, pw, ph).unwrap();
        let t = load_thumbnail(&d, &w.folder).expect("saved");
        assert_eq!(t.len(), (THUMB_W * THUMB_H * 4) as usize);
        assert_eq!(&t[0..4], &[255, 0, 0, 255]);
        let last = t.len() - 4;
        assert_eq!(&t[last..], &[0, 0, 255, 255]);
        // Play time adds up; the folder's size shows in the list.
        add_play_time(&d, &w.folder, 90).unwrap();
        add_play_time(&d, &w.folder, 30).unwrap();
        let listed = list(&d);
        assert_eq!(listed[0].play_seconds, 120);
        assert!(listed[0].size_bytes > 100, "{}", listed[0].size_bytes);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn dates() {
        assert_eq!(date_label(0), "1 JAN 1970");
        assert_eq!(date_label(1_763_136_000), "14 NOV 2025");
        assert_eq!(date_label(951_782_400), "29 FEB 2000");
    }
}
