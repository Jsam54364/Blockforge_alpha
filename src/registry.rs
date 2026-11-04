//! Data-driven block registry, loaded from `assets/blocks.toml`.
//!
//! Uses a tiny parser for the TOML subset we need (`[[block]]` tables with
//! string/bool/number values) so the core has zero dependencies. Swapping in the
//! `toml` crate later only touches `parse_tables`.

use std::collections::HashMap;
use std::fmt;

use crate::block::{BlockState, MAX_BLOCK_IDS};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderLayer {
    Opaque,
    /// Alpha-tested (leaves). Expensive on tile-based GPUs: keep to nearby chunks.
    Cutout,
    /// Blended (water, glass). Drawn last.
    Translucent,
}

#[derive(Clone, Debug)]
pub struct BlockDef {
    pub name: String,
    pub solid: bool,
    pub opaque: bool,
    pub render: RenderLayer,
    pub light_emission: u8,
    pub hardness: f32,
    /// Texture id per face, indexed by `Face::index()`.
    pub tex: [u16; 6],
    pub behavior: Option<String>,
}

#[derive(Debug)]
pub struct RegistryError(pub String);

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block registry: {}", self.0)
    }
}

impl std::error::Error for RegistryError {}

#[derive(Debug)]
pub struct BlockRegistry {
    defs: Vec<BlockDef>,
    by_name: HashMap<String, u16>,
    textures: Vec<String>,
    source: String,
}

#[derive(Debug, Clone)]
enum Value {
    Str(String),
    Bool(bool),
    Num(f64),
}

type Table = HashMap<String, Value>;

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_value(s: &str) -> Option<Value> {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return Some(Value::Str(s[1..s.len() - 1].to_string()));
    }
    match s {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => s.parse::<f64>().ok().map(Value::Num),
    }
}

fn parse_tables(src: &str) -> Result<Vec<Table>, RegistryError> {
    let mut tables: Vec<Table> = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let lineno = n + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[block]]" {
            tables.push(Table::new());
            continue;
        }
        if line.starts_with('[') {
            return Err(RegistryError(format!("line {lineno}: only [[block]] tables are supported")));
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| RegistryError(format!("line {lineno}: expected `key = value`")))?;
        let value = parse_value(v.trim())
            .ok_or_else(|| RegistryError(format!("line {lineno}: bad value `{}`", v.trim())))?;
        let table = tables
            .last_mut()
            .ok_or_else(|| RegistryError(format!("line {lineno}: key outside of a [[block]]")))?;
        table.insert(k.trim().to_string(), value);
    }
    Ok(tables)
}

fn get_str<'a>(t: &'a Table, key: &str, who: &str) -> Result<Option<&'a str>, RegistryError> {
    match t.get(key) {
        None => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(RegistryError(format!("block '{who}': '{key}' must be a string"))),
    }
}

fn get_bool(t: &Table, key: &str, who: &str) -> Result<Option<bool>, RegistryError> {
    match t.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(RegistryError(format!("block '{who}': '{key}' must be true/false"))),
    }
}

fn get_num(t: &Table, key: &str, who: &str) -> Result<Option<f64>, RegistryError> {
    match t.get(key) {
        None => Ok(None),
        Some(Value::Num(n)) => Ok(Some(*n)),
        Some(_) => Err(RegistryError(format!("block '{who}': '{key}' must be a number"))),
    }
}

impl BlockRegistry {
    pub fn from_toml(src: &str) -> Result<Self, RegistryError> {
        let tables = parse_tables(src)?;
        let mut reg = BlockRegistry {
            defs: Vec::new(),
            by_name: HashMap::new(),
            textures: Vec::new(),
            source: src.to_string(),
        };

        for t in &tables {
            let name = get_str(t, "name", "?")?
                .ok_or_else(|| RegistryError("a [[block]] has no name".to_string()))?
                .to_string();
            if reg.by_name.contains_key(&name) {
                return Err(RegistryError(format!("duplicate block '{name}'")));
            }
            if reg.defs.len() >= MAX_BLOCK_IDS {
                return Err(RegistryError(format!("more than {MAX_BLOCK_IDS} blocks")));
            }

            let solid = get_bool(t, "solid", &name)?.unwrap_or(true);
            let opaque = get_bool(t, "opaque", &name)?.unwrap_or(true);
            let render = match get_str(t, "render", &name)?.unwrap_or("opaque") {
                "opaque" => RenderLayer::Opaque,
                "cutout" => RenderLayer::Cutout,
                "translucent" => RenderLayer::Translucent,
                other => {
                    return Err(RegistryError(format!("block '{name}': unknown render layer '{other}'")))
                }
            };
            let light = get_num(t, "light", &name)?.unwrap_or(0.0).clamp(0.0, 15.0) as u8;
            let hardness = get_num(t, "hardness", &name)?.unwrap_or(1.0) as f32;

            let all = get_str(t, "texture", &name)?.unwrap_or(name.as_str()).to_string();
            let top = get_str(t, "top", &name)?.unwrap_or(all.as_str()).to_string();
            let bottom = get_str(t, "bottom", &name)?.unwrap_or(all.as_str()).to_string();
            let side = get_str(t, "side", &name)?.unwrap_or(all.as_str()).to_string();
            let (top, bottom, side) = (
                reg.intern_texture(&top),
                reg.intern_texture(&bottom),
                reg.intern_texture(&side),
            );
            // Order must match Face: +X, -X, +Y, -Y, +Z, -Z
            let tex = [side, side, top, bottom, side, side];

            let behavior = get_str(t, "behavior", &name)?.map(|s| s.to_string());

            let id = reg.defs.len() as u16;
            reg.by_name.insert(name.clone(), id);
            reg.defs.push(BlockDef {
                name,
                solid,
                opaque,
                render,
                light_emission: light,
                hardness,
                tex,
                behavior,
            });
        }

        if reg.defs.first().map(|d| d.name.as_str()) != Some("air") {
            return Err(RegistryError("the first block must be \"air\" (id 0)".to_string()));
        }
        Ok(reg)
    }

    pub fn builtin() -> Self {
        Self::from_toml(include_str!("../assets/blocks.toml")).expect("builtin blocks.toml is valid")
    }

    fn intern_texture(&mut self, name: &str) -> u16 {
        if let Some(i) = self.textures.iter().position(|t| t == name) {
            return i as u16;
        }
        self.textures.push(name.to_string());
        (self.textures.len() - 1) as u16
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn id(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }

    /// Full-shape state for a named block.
    pub fn state(&self, name: &str) -> Option<BlockState> {
        self.id(name).map(BlockState::full)
    }

    /// Unknown ids fall back to air's definition instead of panicking.
    pub fn def(&self, id: u16) -> &BlockDef {
        self.defs.get(id as usize).unwrap_or(&self.defs[0])
    }

    pub fn get(&self, state: BlockState) -> &BlockDef {
        self.def(state.id())
    }

    pub fn texture_names(&self) -> &[String] {
        &self.textures
    }

    /// The exact TOML this registry was built from (sent to clients on join).
    pub fn source(&self) -> &str {
        &self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::Face;

    #[test]
    fn builtin_loads() {
        let r = BlockRegistry::builtin();
        assert_eq!(r.id("air"), Some(0));
        assert!(r.id("stone").is_some());
        let grass = r.def(r.id("grass").unwrap());
        assert_ne!(grass.tex[Face::PosY.index()], grass.tex[Face::PosX.index()]);
        assert_eq!(grass.tex[Face::PosX.index()], grass.tex[Face::NegZ.index()]);
        assert_eq!(r.def(r.id("water").unwrap()).render, RenderLayer::Translucent);
        assert!(r.def(r.id("bedrock").unwrap()).hardness < 0.0);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(BlockRegistry::from_toml("[[block]]\nname = \"stone\"\n").is_err()); // air not first
        assert!(BlockRegistry::from_toml("[[block]]\nname = \"air\"\n[[block]]\nname = \"air\"\n").is_err());
        assert!(BlockRegistry::from_toml("name = \"air\"\n").is_err());
        assert!(BlockRegistry::from_toml("[[block]]\nname = \"air\"\nrender = \"weird\"\n").is_err());
    }

    #[test]
    fn comments_and_hash_in_strings() {
        let r = BlockRegistry::from_toml("[[block]]\nname = \"air\" # comment\n[[block]]\nname = \"a#b\"\n").unwrap();
        assert!(r.id("a#b").is_some());
    }

    #[test]
    fn unknown_id_falls_back_to_air() {
        let r = BlockRegistry::builtin();
        assert_eq!(r.def(4000).name, "air");
    }
}
