//! The menus' texture: fonts and icons as signed distance fields, painted frames and the
//! logo (`assets/ui`, made by `tools/make_ui_atlas.py`). This reads its table of contents.

use std::collections::HashMap;
use std::sync::OnceLock;

pub const ATLAS_PNG: &[u8] = include_bytes!("../../assets/ui/atlas.png");
const ATLAS_JSON: &str = include_str!("../../assets/ui/atlas.json");

/// A painted piece: where it is, and how much of each edge (left, top, right, bottom) keeps
/// its size when it is stretched (nine-slice; a piece with no top and bottom margins only
/// stretches sideways and keeps its proportions in height).
#[derive(Clone, Copy, Debug)]
pub struct Piece {
    pub rect: [f32; 4],
    pub margins: [f32; 4],
}

/// One character or icon at the font's `em` size: its place in the atlas (if it has ink),
/// where that sits from the pen on the baseline (y down), and how far the pen moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Glyph {
    pub rect: Option<[f32; 4]>,
    pub off: [f32; 2],
    pub adv: f32,
}

#[derive(Clone, Debug)]
pub struct Font {
    pub em: f32,
    pub spread: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Printable ASCII, from ' ' (32) to '~' (126).
    glyphs: Vec<Glyph>,
}

impl Font {
    pub fn glyph(&self, c: char) -> Glyph {
        let i = c as usize;
        if (32..127).contains(&i) {
            self.glyphs[i - 32]
        } else {
            self.glyphs['?' as usize - 32]
        }
    }

    /// Width of `s` at `size` pixels to the em.
    pub fn width(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.glyph(c).adv).sum::<f32>() * size / self.em
    }

    /// Height of capitals above the baseline, at `size`, roughly (for centring).
    pub fn cap_height(&self, size: f32) -> f32 {
        self.glyph('H').off[1].abs() * size / self.em * 0.94
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontId {
    /// Crimson Text: most menu text.
    Serif,
    /// Pixelify Sans: the main menu's buttons and the keyboard.
    Pixel,
}

pub struct Atlas {
    pub size: f32,
    pieces: HashMap<String, Piece>,
    serif: Font,
    pixel: Font,
    pub icon_em: f32,
    icons: HashMap<String, Glyph>,
}

impl Atlas {
    pub fn piece(&self, name: &str) -> Piece {
        *self.pieces.get(name).unwrap_or_else(|| panic!("no piece {name} in the UI atlas"))
    }

    pub fn font(&self, f: FontId) -> &Font {
        match f {
            FontId::Serif => &self.serif,
            FontId::Pixel => &self.pixel,
        }
    }

    pub fn icon(&self, name: &str) -> Glyph {
        *self.icons.get(name).unwrap_or_else(|| panic!("no icon {name} in the UI atlas"))
    }

    pub fn has_icon(&self, name: &str) -> bool {
        self.icons.contains_key(name)
    }
}

fn rect(v: &serde_json::Value) -> Option<[f32; 4]> {
    let a = v.as_array()?;
    (a.len() == 4).then(|| [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap_or(0.0) as f32))
}

fn glyph(v: &serde_json::Value) -> Glyph {
    let off = v.get("off").and_then(|o| o.as_array()).map(|o| [o[0].as_f64().unwrap_or(0.0) as f32, o[1].as_f64().unwrap_or(0.0) as f32]);
    Glyph { rect: v.get("rect").and_then(rect), off: off.unwrap_or([0.0; 2]), adv: v["adv"].as_f64().unwrap_or(0.0) as f32 }
}

fn font(v: &serde_json::Value) -> Font {
    let glyphs = (32..127).map(|c| v["glyphs"].get(c.to_string()).map(glyph).unwrap_or_default()).collect();
    let num = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
    Font { em: num("em"), spread: num("spread"), ascent: num("ascent"), descent: num("descent"), glyphs }
}

/// The atlas's table of contents (read once).
pub fn atlas() -> &'static Atlas {
    static A: OnceLock<Atlas> = OnceLock::new();
    A.get_or_init(|| {
        let j: serde_json::Value = serde_json::from_str(ATLAS_JSON).expect("assets/ui/atlas.json");
        let pieces = j["pieces"]
            .as_object()
            .expect("pieces")
            .iter()
            .map(|(k, v)| {
                let p = Piece { rect: rect(&v["rect"]).expect("piece rect"), margins: rect(&v["margins"]).unwrap_or([0.0; 4]) };
                (k.clone(), p)
            })
            .collect();
        let icons = j["icons"]["glyphs"].as_object().expect("icons").iter().map(|(k, v)| (k.clone(), glyph(v))).collect();
        Atlas {
            size: j["size"].as_f64().unwrap_or(2048.0) as f32,
            pieces,
            serif: font(&j["fonts"]["serif"]),
            pixel: font(&j["fonts"]["pixel"]),
            icon_em: j["icons"]["em"].as_f64().unwrap_or(72.0) as f32,
            icons,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_atlas_has_everything_the_menus_draw() {
        let a = atlas();
        for p in ["white", "button", "bar", "panel", "plaque", "glow", "field", "toggle_on", "toggle_off", "gem", "logo"] {
            let r = a.piece(p).rect;
            assert!(r[2] > 0.0 && r[3] > 0.0 && r[0] + r[2] <= a.size && r[1] + r[3] <= a.size, "{p}");
        }
        for f in [FontId::Serif, FontId::Pixel] {
            let font = a.font(f);
            for c in (33u8..127).map(char::from) {
                let g = font.glyph(c);
                assert!(g.rect.is_some() && g.adv > 0.0, "{f:?} {c:?}");
            }
            assert!(font.glyph(' ').rect.is_none() && font.glyph(' ').adv > 0.0);
            assert!(font.width("Single Player", 30.0) > font.width("Single", 30.0));
        }
        assert!(a.has_icon("person") && a.icon("settings").rect.is_some());
        // The PNG is the size the table says.
        let dec = png::Decoder::new(ATLAS_PNG);
        let r = dec.read_info().unwrap();
        assert_eq!(r.info().width as f32, a.size);
    }
}
