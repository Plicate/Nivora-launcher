//! изометрический превью мира: разбирает level.dat и Anvil-регионы, рисует карту
//! настоящими ванильными текстурами Minecraft (встроенный атлас world_preview_textures.rs)
use flate2::read::{GzDecoder, ZlibDecoder};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::config::ConfigStore;
use super::state::resolve_instance_path;

const SAMPLE_COLUMNS: i32 = 144;
const SAMPLE_STEP: i32 = 2;
const TILE_W: i32 = 16;
const TILE_H: i32 = 8;
const SIDE_V: i32 = 8;
const MARGIN: i32 = 10;
const MAX_REGIONS: usize = 128;
const CONTENT_WINDOW_CAP: i32 = 400;

struct Nbt<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Nbt<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("NBT: данные оборваны".to_owned());
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8v(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16v(&mut self) -> Result<u16, String> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }

    fn i32v(&mut self) -> Result<i32, String> {
        let s = self.take(4)?;
        Ok(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn i64v(&mut self) -> Result<i64, String> {
        let s = self.take(8)?;
        Ok(i64::from_be_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ]))
    }

    fn string(&mut self) -> Result<String, String> {
        let len = self.u16v()? as usize;
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    fn payload(&mut self, tag: u8) -> Result<Value, String> {
        match tag {
            1 => Ok(json!(self.u8v()? as i8)),
            2 => {
                let s = self.take(2)?;
                Ok(json!(i16::from_be_bytes([s[0], s[1]])))
            }
            3 => Ok(json!(self.i32v()?)),
            4 => Ok(json!(self.i64v()?)),
            5 => {
                let s = self.take(4)?;
                Ok(json!(f32::from_be_bytes([s[0], s[1], s[2], s[3]]) as f64))
            }
            6 => {
                let s = self.take(8)?;
                Ok(json!(f64::from_be_bytes([
                    s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]
                ])))
            }
            7 => {
                let len = (self.i32v()?).max(0) as usize;
                let bytes = self.take(len)?.to_vec();
                Ok(Value::Array(
                    bytes.iter().map(|b| json!(*b as i8)).collect(),
                ))
            }
            8 => Ok(json!(self.string()?)),
            9 => {
                let item_tag = self.u8v()?;
                let len = (self.i32v()?).max(0);
                if item_tag == 0 || len == 0 {
                    return Ok(Value::Array(vec![]));
                }
                let mut items = Vec::with_capacity((len as usize).min(100_000));
                for _ in 0..len {
                    items.push(self.payload(item_tag)?);
                }
                Ok(Value::Array(items))
            }
            10 => self.compound(),
            11 => {
                let len = (self.i32v()?).max(0) as usize;
                let mut items = Vec::with_capacity(len.min(200_000));
                for _ in 0..len {
                    items.push(json!(self.i32v()?));
                }
                Ok(Value::Array(items))
            }
            12 => {
                let len = (self.i32v()?).max(0) as usize;
                let mut items = Vec::with_capacity(len.min(200_000));
                for _ in 0..len {
                    items.push(json!(self.i64v()?));
                }
                Ok(Value::Array(items))
            }
            other => Err(format!("NBT: неизвестный тег {other}")),
        }
    }

    fn compound(&mut self) -> Result<Value, String> {
        let mut map = serde_json::Map::new();
        loop {
            let tag = self.u8v()?;
            if tag == 0 {
                return Ok(Value::Object(map));
            }
            let name = self.string()?;
            let value = self.payload(tag)?;
            map.insert(name, value);
        }
    }

    fn root(&mut self) -> Result<Value, String> {
        let tag = self.u8v()?;
        if tag != 10 {
            return Err("NBT: корень не compound".to_owned());
        }
        let _name = self.string()?;
        self.compound()
    }
}

pub(crate) fn decode_nbt_bytes(raw: &[u8]) -> Result<Value, String> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut out = Vec::new();
        GzDecoder::new(raw)
            .read_to_end(&mut out)
            .map_err(|error| format!("NBT: gzip не читается: {error}"))?;
        return Nbt { data: &out, pos: 0 }.root();
    }
    if raw.len() >= 2 && raw[0] == 0x78 {
        let mut out = Vec::new();
        ZlibDecoder::new(raw)
            .read_to_end(&mut out)
            .map_err(|error| format!("NBT: zlib не читается: {error}"))?;
        return Nbt { data: &out, pos: 0 }.root();
    }
    Nbt { data: raw, pos: 0 }.root()
}

fn ceil_log2(value: usize) -> u32 {
    let mut bits = 4u32;
    while (1usize << bits) < value {
        bits += 1;
    }
    bits
}

#[derive(Clone)]
struct SectionBlocks {
    y: i32,
    palette: Vec<String>,
    props: Vec<String>,
    bits: u32,
    per_long: u32,
    mask: u64,
    data: Vec<i64>,
    air_only: bool,
}

impl SectionBlocks {
    fn parse(section: &Value) -> Option<SectionBlocks> {
        let states = section.get("block_states").unwrap_or(section);
        let palette_value = states.get("palette")?;
        let palette_value = match palette_value {
            Value::Array(items) => items,
            _ => return None,
        };
        let mut palette: Vec<String> = Vec::new();
        let mut props: Vec<String> = Vec::new();
        for entry in palette_value {
            let Some(name) = entry.get("Name").and_then(Value::as_str) else {
                continue;
            };
            palette.push(name.to_owned());
            props.push(properties_code(entry.get("Properties")));
        }
        if palette.is_empty() {
            return None;
        }
        let air_only = palette.iter().all(|name| {
            let base = name.trim_start_matches("minecraft:");
            base == "air" || base == "cave_air" || base == "void_air"
        });
        let data = states
            .get("data")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_i64).collect::<Vec<i64>>())
            .unwrap_or_default();
        let bits = if palette.len() <= 1 {
            4
        } else {
            ceil_log2(palette.len())
        };
        Some(SectionBlocks {
            y: section.get("Y").and_then(Value::as_i64).unwrap_or(0) as i32,
            per_long: 64 / bits,
            mask: (1u64 << bits) - 1,
            bits,
            palette,
            props,
            data,
            air_only,
        })
    }

    fn block_full(&self, x: usize, y: usize, z: usize) -> Option<(&str, &str)> {
        if self.data.is_empty() {
            let index = self.palette.first()?;
            return Some((
                index.as_str(),
                self.props.first().map(String::as_str).unwrap_or(""),
            ));
        }
        let index = y * 256 + z * 16 + x;
        let per = self.per_long.max(1) as usize;
        let long = self.data.get(index / per)?;
        let shift = ((index % per) as u32) * self.bits;
        let palette_index = (((*long as u64) >> shift) & self.mask) as usize;
        let name = self.palette.get(palette_index)?;
        Some((
            name.as_str(),
            self.props
                .get(palette_index)
                .map(String::as_str)
                .unwrap_or(""),
        ))
    }

    fn block(&self, x: usize, y: usize, z: usize) -> Option<&str> {
        self.block_full(x, y, z).map(|(name, _)| name)
    }
}

fn load_chunk_sections(file: &[u8], lcx: usize, lcz: usize) -> Option<Vec<SectionBlocks>> {
    if file.len() < 8192 {
        return None;
    }
    let index = lcz * 32 + lcx;
    let head = &file[index * 4..index * 4 + 4];
    let offset = ((head[0] as usize) << 16) | ((head[1] as usize) << 8) | head[2] as usize;
    if offset == 0 {
        return None;
    }
    let start = offset * 4096;
    if start + 5 > file.len() {
        return None;
    }
    let length = u32::from_be_bytes([
        file[start],
        file[start + 1],
        file[start + 2],
        file[start + 3],
    ]) as usize;
    let compression = file[start + 4];
    if length == 0 || start + 4 + length > file.len() {
        return None;
    }
    let body = &file[start + 5..start + 4 + length];
    let root = match compression {
        1 => {
            let mut out = Vec::new();
            GzDecoder::new(body).read_to_end(&mut out).ok()?;
            Nbt { data: &out, pos: 0 }.root().ok()
        }
        2 => {
            let mut out = Vec::new();
            ZlibDecoder::new(body).read_to_end(&mut out).ok()?;
            Nbt { data: &out, pos: 0 }.root().ok()
        }
        3 => Nbt { data: body, pos: 0 }.root().ok(),
        _ => return None,
    }?;
    let sections = root
        .get("sections")
        .or_else(|| root.get("Level").and_then(|level| level.get("Sections")))
        .and_then(Value::as_array)?;
    let mut parsed: Vec<SectionBlocks> = sections.iter().filter_map(SectionBlocks::parse).collect();
    parsed.sort_by(|a, b| b.y.cmp(&a.y));
    Some(parsed)
}

fn properties_code(value: Option<&Value>) -> String {
    let Some(Value::Object(map)) = value else {
        return String::new();
    };
    let mut parts: Vec<String> = map
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|item| format!("{key}={item}")))
        .collect();
    parts.sort();
    parts.join(";")
}

fn props_value<'a>(props: &'a str, key: &str) -> Option<&'a str> {
    props.split(';').find_map(|part| {
        let mut pieces = part.splitn(2, '=');
        let (part_key, value) = (pieces.next()?, pieces.next()?);
        (part_key == key_check(key)).then_some(value)
    })
}

fn key_check(key: &str) -> &str {
    key
}

/// четверти поворота верхнего тайла по facing (0..3) или rotation (0..15)
fn rotation_quarters(props: &str) -> u32 {
    if let Some(facing) = props_value(props, "facing") {
        return match facing {
            "east" => 1,
            "south" => 2,
            "west" => 3,
            _ => 0,
        };
    }
    if let Some(rot) = props_value(props, "rotation") {
        if let Ok(value) = rot.parse::<u32>() {
            return value / 4;
        }
    }
    0
}

fn horizontal_axis(props: &str) -> bool {
    matches!(props_value(props, "axis"), Some("x") | Some("z"))
}

fn rot_uv(u: f32, v: f32, rot: u32) -> (usize, usize) {
    let (nu, nv) = match rot % 4 {
        1 => (v, 1.0 - u),
        2 => (1.0 - u, 1.0 - v),
        3 => (1.0 - v, u),
        _ => (u, v),
    };
    (
        (nu * 16.0).clamp(0.0, 15.999) as usize,
        (nv * 16.0).clamp(0.0, 15.999) as usize,
    )
}

fn rot16(u: usize, v: usize, rot: u32) -> (usize, usize) {
    let m = 15usize;
    match rot % 4 {
        1 => (v, m - u),
        2 => (m - u, m - v),
        3 => (m - v, u),
        _ => (u, v),
    }
}

fn tint_for(name: &str) -> Option<[f32; 3]> {
    let base = name.trim_start_matches("minecraft:");
    if base == "grass_block"
        || base == "grass_block_top"
        || base == "short_grass"
        || base == "tall_grass"
        || base == "fern"
        || base == "large_fern_top"
    {
        return Some([145.0 / 255.0, 189.0 / 255.0, 89.0 / 255.0]);
    }
    if base.ends_with("_leaves") || base == "vine" {
        return Some([119.0 / 255.0, 171.0 / 255.0, 47.0 / 255.0]);
    }
    if base.contains("water") {
        return Some([63.0 / 255.0, 118.0 / 255.0, 228.0 / 255.0]);
    }
    None
}

fn is_vegetation(base: &str) -> bool {
    const NAMES: &[&str] = &[
        "short_grass",
        "tall_grass",
        "fern",
        "large_fern",
        "large_fern_top",
        "dead_bush",
        "sugar_cane",
        "poppy",
        "dandelion",
        "blue_orchid",
        "allium",
        "azure_bluet",
        "oxeye_daisy",
        "cornflower",
        "lily_of_the_valley",
        "wither_rose",
        "torchflower",
        "torchflower_crop",
        "pitcher_plant",
        "pitcher_crop",
        "sunflower",
        "lilac",
        "rose_bush",
        "peony",
        "pink_petals",
        "wildflowers",
        "glow_lichen",
        "hanging_roots",
        "spore_blossom",
        "sea_pickle",
        "lily_pad",
        "frogspawn",
        "bamboo",
        "bamboo_sapling",
        "nether_wart",
        "wheat",
        "carrots",
        "potatoes",
        "beetroots",
        "melon_stem",
        "pumpkin_stem",
        "attached_melon_stem",
        "attached_pumpkin_stem",
        "sweet_berry_bush",
        "cocoa",
        "crimson_roots",
        "warped_roots",
        "nether_sprouts",
        "red_mushroom",
        "brown_mushroom",
        "chorus_plant",
        "chorus_flower",
        "moss_carpet",
        "pale_moss_carpet",
        "azalea",
        "flowering_azalea",
        "bush",
        "firefly_bush",
        "cactus_flower",
        "short_dry_grass",
        "tall_dry_grass",
        "leaf_litter",
        "big_dripleaf",
        "small_dripleaf",
        "vine",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
    ];
    if NAMES.contains(&base) {
        return true;
    }
    const SUFFIXES: &[&str] = &[
        "_sapling",
        "_tulip",
        "_coral_fan",
        "_wall_fan",
        "_propagule",
        "_petals",
    ];
    for suffix in SUFFIXES {
        if base.ends_with(suffix) {
            return true;
        }
    }
    if base.ends_with("vine") || base.ends_with("vines") || base.ends_with("vines_plant") {
        return true;
    }
    if base.ends_with("seagrass") || base.ends_with("coral") && !base.ends_with("coral_block") {
        return true;
    }
    false
}

const WATER_TINT: [f32; 3] = [63.0 / 255.0, 118.0 / 255.0, 228.0 / 255.0];

/// вода не умножается на тинт (текстура серая, получается чёрной):
/// яркость перекрашивается в цвет воды с сохранением рельефа текстуры
fn recolor_water(mut pixel: [u8; 4], tint: [f32; 3], factor: f32) -> [u8; 4] {
    let luminance = (pixel[0] as f32 + pixel[1] as f32 + pixel[2] as f32) / (3.0 * 255.0);
    let scale = (0.72 + 0.28 * luminance) * (0.85 + 0.15 * factor);
    for (index, slot) in pixel.iter_mut().enumerate().take(3) {
        *slot = (tint[index] * scale * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    pixel
}

const MAP_PIXEL: i32 = 16;

fn encode_png(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut buffer = Vec::new();
    let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut buffer), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // дефолтный zlib на канвасе в ~20 мегапикселей ест секунды
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("Не удалось записать превью: {error}"))?;
    writer
        .write_image_data(data)
        .map_err(|error| format!("Не удалось записать превью: {error}"))?;
    drop(writer);
    Ok(buffer)
}

fn write_png(path: &Path, width: u32, height: u32, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Не удалось создать каталог превью: {error}"))?;
    }
    let bytes = encode_png(width, height, data)?;
    fs::write(path, &bytes).map_err(|error| format!("Не удалось создать превью: {error}"))
}

const COLUMN_CAP: usize = 64;

struct FullColumn {
    bx: i32,
    bz: i32,
    blocks: Vec<(i32, u16, u16)>,
}

struct FullColumns {
    names: Vec<String>,
    props: Vec<String>,
    columns: Vec<FullColumn>,
    index: HashMap<(i32, i32), usize>,
    total_blocks: usize,
    step: i32,
}

/// общие таблицы имён блоков и их свойств: оба прохода сборки колонок
/// (шаг 2 и достройка шагом 1) пишут в одни таблицы, чтобы id совпадали
#[derive(Default)]
struct NameTables {
    names: Vec<String>,
    name_ids: HashMap<String, u16>,
    props: Vec<String>,
    props_ids: HashMap<String, u16>,
}

impl NameTables {
    fn name_id(&mut self, name: &str) -> u16 {
        match self.name_ids.get(name) {
            Some(&existing) => existing,
            None => {
                let id = u16::try_from(self.names.len()).unwrap_or(u16::MAX);
                self.names.push(name.to_owned());
                self.name_ids.insert(name.to_owned(), id);
                id
            }
        }
    }

    fn props_id(&mut self, props: &str) -> u16 {
        match self.props_ids.get(props) {
            Some(&existing) => existing,
            None => {
                let id = u16::try_from(self.props.len()).unwrap_or(u16::MAX);
                self.props.push(props.to_owned());
                self.props_ids.insert(props.to_owned(), id);
                id
            }
        }
    }
}

struct Rendered {
    buf: Vec<u8>,
    width: i32,
    height: i32,
    focus: Option<(i32, i32, i32, i32)>,
    spawn_px: Option<(i32, i32)>,
    top_faces: usize,
    side_faces: usize,
}

/// двупроходный сбор колонок: сначала список нужных чанков, затем один проход
/// по каждому региону с разбором только нужных чанков (регион выгружается сразу).
/// Колонка хранит ВСЕ блоки сверху вниз (не только верхний), иначе изометрия
/// теряет нижние ярусы паркур-структур и фасады под навесами
fn needed_regions(
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    step: i32,
) -> HashMap<(i32, i32), Vec<(i32, i32)>> {
    use std::collections::BTreeSet;
    let mut needed: BTreeSet<(i32, i32, i32, i32)> = BTreeSet::new();
    for wx in (min_x..=max_x).step_by(step.max(1) as usize) {
        for wz in (min_z..=max_z).step_by(step.max(1) as usize) {
            let (cx, cz) = (wx >> 4, wz >> 4);
            needed.insert((cx >> 5, cz >> 5, cx & 31, cz & 31));
        }
    }
    let mut by_region: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
    for &(rx, rz, lcx, lcz) in &needed {
        by_region.entry((rx, rz)).or_default().push((lcx, lcz));
    }
    by_region
}

/// число потоков для тяжёлых фаз рендера карты: не больше 6, чтобы каждый
/// поток держал в памяти буфер лишь одного региона
fn preview_workers() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(4)
        .min(6)
}

fn load_regions(
    world: &Path,
    by_region: &HashMap<(i32, i32), Vec<(i32, i32)>>,
    center: (i32, i32),
) -> HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>> {
    // центр окна выборки: регионы сортируем по близости к нему (MAX_REGIONS)
    let mut regions: Vec<(i32, i32)> = by_region.keys().copied().collect();
    regions.sort_by_key(|(rx, rz)| {
        let dx = rx - center.0;
        let dz = rz - center.1;
        dx * dx + dz * dz
    });
    let picked: Vec<(i32, i32)> = regions.into_iter().take(MAX_REGIONS).collect();
    // чтение .mca и распаковка NBT — самая дорогая часть рендера; в мирах
    // с множеством прогруженных чанков регионы грузятся параллельно.
    // Потоков не больше 6: каждый держит буфер одного региона в памяти
    let workers = preview_workers().min(picked.len().max(1));
    let mut sections_map: HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>> = HashMap::new();
    if workers <= 1 {
        load_region_slice(world, by_region, &picked, &mut sections_map);
        return sections_map;
    }
    let per_worker = picked.len().div_ceil(workers);
    let locals: Vec<HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>>> =
        std::thread::scope(|scope| {
            let handles: Vec<_> = picked
                .chunks(per_worker)
                .map(|group| {
                    scope.spawn(move || {
                        let mut local: HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>> =
                            HashMap::new();
                        load_region_slice(world, by_region, group, &mut local);
                        local
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect()
        });
    for local in locals {
        sections_map.extend(local);
    }
    sections_map
}

fn load_region_slice(
    world: &Path,
    by_region: &HashMap<(i32, i32), Vec<(i32, i32)>>,
    regions: &[(i32, i32)],
    sections_map: &mut HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>>,
) {
    for &(rx, rz) in regions {
        let path = world.join("region").join(format!("r.{rx}.{rz}.mca"));
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let wanted = &by_region[&(rx, rz)];
        let chunk_bytes: &[u8] = &bytes;
        // распаковка NBT чанка дорога; когда в регионе много нужных чанков,
        // делим её между потоками, файл региона общий и read-only
        if wanted.len() <= 3 {
            for (lcx, lcz) in wanted {
                if let Some(sections) = load_chunk_sections(&bytes, *lcx as usize, *lcz as usize) {
                    sections_map.insert((rx, rz, *lcx, *lcz), sections);
                }
            }
            continue;
        }
        let per_group = wanted.len().div_ceil(4);
        let locals: Vec<HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>>> =
            std::thread::scope(|scope| {
                let handles: Vec<_> = wanted
                    .chunks(per_group)
                    .map(|group| {
                        scope.spawn(move || {
                            let mut local: HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>> =
                                HashMap::new();
                            for (lcx, lcz) in group {
                                if let Some(sections) =
                                    load_chunk_sections(chunk_bytes, *lcx as usize, *lcz as usize)
                                {
                                    local.insert((rx, rz, *lcx, *lcz), sections);
                                }
                            }
                            local
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .filter_map(|handle| handle.join().ok())
                    .collect()
            });
        for local in locals {
            sections_map.extend(local);
        }
    }
}

fn collect_grid_columns(
    sections_map: &HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>>,
    cells: impl Iterator<Item = (i32, i32)>,
    tables: &mut NameTables,
    top_only: bool,
) -> Vec<FullColumn> {
    let mut columns: Vec<FullColumn> = Vec::new();
    for (wx, wz) in cells {
        let (cx, cz) = (wx >> 4, wz >> 4);
        let Some(sections) = sections_map.get(&(cx >> 5, cz >> 5, cx & 31, cz & 31)) else {
            continue;
        };
        let lx = (wx & 15) as usize;
        let lz = (wz & 15) as usize;
        let mut blocks: Vec<(i32, u16, u16)> = Vec::new();
        'collect: for section in sections {
            if section.air_only {
                continue;
            }
            for y in (0..16usize).rev() {
                let Some((name, block_props)) = section.block_full(lx, y, lz) else {
                    continue;
                };
                let base = name.trim_start_matches("minecraft:");
                if base == "air" || base == "cave_air" || base == "void_air" {
                    continue;
                }
                if is_vegetation(base) {
                    continue;
                }
                let id = tables.name_id(name);
                let props_id = tables.props_id(block_props);
                blocks.push((section.y * 16 + y as i32, id, props_id));
                // карте нужен только верхний блок столбца, изометрии — вся
                // колонка до COLUMN_CAP; на плотных мирах это главная экономия
                if top_only || blocks.len() >= COLUMN_CAP {
                    break 'collect;
                }
            }
        }
        if !blocks.is_empty() {
            columns.push(FullColumn {
                bx: wx,
                bz: wz,
                blocks,
            });
        }
    }
    columns
}

fn collect_columns_full(
    world: &Path,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    step: i32,
    tables: &mut NameTables,
    top_only: bool,
) -> Vec<FullColumn> {
    let step = step.max(1);
    let by_region = needed_regions(min_x, min_z, max_x, max_z, step);
    let center = (((min_x + max_x) / 2) >> 9, ((min_z + max_z) / 2) >> 9);
    let sections_map = load_regions(world, &by_region, center);
    let cells: Vec<(i32, i32)> = (min_x..=max_x)
        .step_by(step as usize)
        .flat_map(move |wx| {
            (min_z..=max_z)
                .step_by(step as usize)
                .map(move |wz| (wx, wz))
        })
        .collect();
    let columns = collect_columns_parallel(&sections_map, cells, tables, top_only);
    columns
}

/// сбор колонок по всем сэмплам: в плотных мирах до четверти миллиона
/// клеток, поэтому работа делится между потоками; у каждого потока свои
/// таблицы имён, на слиянии id переклеиваются на общие таблицы
fn collect_columns_parallel(
    sections_map: &HashMap<(i32, i32, i32, i32), Vec<SectionBlocks>>,
    cells: Vec<(i32, i32)>,
    tables: &mut NameTables,
    top_only: bool,
) -> Vec<FullColumn> {
    let workers = preview_workers();
    if workers <= 1 || cells.len() < 8192 {
        return collect_grid_columns(sections_map, cells.into_iter(), tables, top_only);
    }
    let per_worker = cells.len().div_ceil(workers);
    let locals: Vec<(Vec<FullColumn>, NameTables)> = std::thread::scope(|scope| {
        let handles: Vec<_> = cells
            .chunks(per_worker)
            .map(|group| {
                scope.spawn(move || {
                    let mut local_tables = NameTables::default();
                    let columns = collect_grid_columns(
                        sections_map,
                        group.iter().copied(),
                        &mut local_tables,
                        top_only,
                    );
                    (columns, local_tables)
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });
    let mut merged: Vec<FullColumn> = Vec::with_capacity(cells.len());
    for (mut columns, local) in locals {
        let name_map: Vec<u16> = local
            .names
            .iter()
            .map(|name| tables.name_id(name))
            .collect();
        let props_map: Vec<u16> = local
            .props
            .iter()
            .map(|props| tables.props_id(props))
            .collect();
        for column in &mut columns {
            for block in &mut column.blocks {
                block.1 = name_map[block.1 as usize];
                block.2 = props_map[block.2 as usize];
            }
        }
        merged.extend(columns);
    }
    merged
}

fn build_full_columns(mut columns: Vec<FullColumn>, tables: &NameTables, step: i32) -> FullColumns {
    columns.sort_by(|a, b| (a.bx + a.bz).cmp(&(b.bx + b.bz)).then(a.bx.cmp(&b.bx)));
    let mut index: HashMap<(i32, i32), usize> = HashMap::new();
    for (position, column) in columns.iter().enumerate() {
        index.insert((column.bx, column.bz), position);
    }
    let total_blocks = columns.iter().map(|column| column.blocks.len()).sum();
    FullColumns {
        names: tables.names.clone(),
        props: tables.props.clone(),
        columns,
        index,
        total_blocks,
        step,
    }
}

/// максимальное mtime среди region-файлов мира: ключ кэша превью. Мир
/// меняется — штамп растёт, кэш честно перестраивается
fn region_stamp(world: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(world.join("region")) else {
        return 0;
    };
    let mut stamp = 0u64;
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|item| item.to_str()) != Some("mca") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if let Ok(secs) = modified.duration_since(std::time::UNIX_EPOCH) {
                    stamp = stamp.max(secs.as_secs());
                }
            }
        }
    }
    stamp
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &byte in bytes {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// вес региона = размер файла .mca (в квадрате, чтобы окно прилипало к
/// доминирующему скоплению): у постройки он на порядки больше, чем у
/// сгенерированного океана, поэтому взвешенный центр регионов держит окно
/// выборки на постройке, даже когда сгенерированный мир растянул bbox
fn content_center(world: &Path) -> Option<(i32, i32)> {
    let Ok(entries) = fs::read_dir(world.join("region")) else {
        return None;
    };
    let mut total = 0f64;
    let mut weight_x = 0f64;
    let mut weight_z = 0f64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|item| item.to_str()) != Some("mca") {
            continue;
        }
        let size = match entry.metadata() {
            Ok(meta) => meta.len(),
            Err(_) => continue,
        };
        if size < 8192 {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|item| item.to_str())
            .unwrap_or("");
        let parts: Vec<&str> = stem.split('.').collect();
        if parts.len() != 3 || parts[0] != "r" {
            continue;
        }
        let (Ok(rx), Ok(rz)) = (parts[1].parse::<i32>(), parts[2].parse::<i32>()) else {
            continue;
        };
        let weight = size as f64 * size as f64;
        total += weight;
        weight_x += (rx * 512 + 256) as f64 * weight;
        weight_z += (rz * 512 + 256) as f64 * weight;
    }
    if total == 0.0 {
        return None;
    }
    Some(((weight_x / total) as i32, (weight_z / total) as i32))
}

/// заголовки регионов вокруг спавна дают bbox реального контента мира:
/// раскрываем кольца регионов, пока встречаются непустые чанки
fn content_bounds(world: &Path, spawn_x: i32, spawn_z: i32) -> Option<(i32, i32, i32, i32)> {
    let spawn_region = (spawn_x >> 9, spawn_z >> 9);
    let mut min_cx = i32::MAX;
    let mut min_cz = i32::MAX;
    let mut max_cx = i32::MIN;
    let mut max_cz = i32::MIN;
    let mut found = 0usize;
    for ring in 0..=4i32 {
        let mut ring_found = 0usize;
        for dx in -ring..=ring {
            for dz in -ring..=ring {
                if dx.abs() != ring && dz.abs() != ring {
                    continue;
                }
                let (rx, rz) = (spawn_region.0 + dx, spawn_region.1 + dz);
                // для bbox достаточно таблицы оффсетов чанков (первые 8 КиБ .mca):
                // целый файл в плотных мирах — десятки МБ на регион
                let mut header = vec![0u8; 8192];
                let Ok(mut file) =
                    fs::File::open(world.join("region").join(format!("r.{rx}.{rz}.mca")))
                else {
                    continue;
                };
                let header_len = file.read(&mut header).unwrap_or(0);
                if header_len < 8192 {
                    continue;
                }
                let bytes = &header[..8192];
                for index in 0..1024usize {
                    let head = &bytes[index * 4..index * 4 + 4];
                    let offset =
                        ((head[0] as usize) << 16) | ((head[1] as usize) << 8) | head[2] as usize;
                    if offset == 0 {
                        continue;
                    }
                    let cx = rx * 32 + (index % 32) as i32;
                    let cz = rz * 32 + (index / 32) as i32;
                    min_cx = min_cx.min(cx);
                    min_cz = min_cz.min(cz);
                    max_cx = max_cx.max(cx);
                    max_cz = max_cz.max(cz);
                    ring_found += 1;
                }
            }
        }
        found += ring_found;
        if ring >= 1 && ring_found == 0 {
            break;
        }
    }
    if found == 0 {
        return None;
    }
    Some((min_cx * 16, min_cz * 16, max_cx * 16 + 15, max_cz * 16 + 15))
}

struct AtlasFrame {
    rgba: Vec<u8>,
    width: usize,
}

fn atlas() -> &'static AtlasFrame {
    static ATLAS: std::sync::OnceLock<AtlasFrame> = std::sync::OnceLock::new();
    ATLAS.get_or_init(|| {
        let decoder = png::Decoder::new(std::io::Cursor::new(
            super::world_preview_textures::ATLAS_PNG,
        ));
        let mut reader = decoder.read_info().expect("атлас текстур блоков читается");
        let mut raw = vec![0u8; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut raw)
            .expect("атлас текстур блоков декодируется");
        AtlasFrame {
            rgba: raw[..info.buffer_size()].to_vec(),
            width: info.width as usize,
        }
    })
}

fn sample_cell(cell: u16, tx: usize, ty: usize) -> [u8; 4] {
    use super::world_preview_textures::ATLAS_COLUMNS;
    let atlas = atlas();
    let cell_size = atlas.width / ATLAS_COLUMNS;
    let x = (cell as usize % ATLAS_COLUMNS) * cell_size + tx.min(cell_size - 1);
    let y = (cell as usize / ATLAS_COLUMNS) * cell_size + ty.min(cell_size - 1);
    let index = (y * atlas.width + x) * 4;
    [
        atlas.rgba[index],
        atlas.rgba[index + 1],
        atlas.rgba[index + 2],
        atlas.rgba[index + 3],
    ]
}

fn tint_pixel(mut pixel: [u8; 4], tint: Option<[f32; 3]>, factor: f32) -> [u8; 4] {
    if let Some(tint) = tint {
        for (index, slot) in pixel.iter_mut().enumerate().take(3) {
            *slot = (*slot as f32 * tint[index]).min(255.0) as u8;
        }
    }
    if (factor - 1.0).abs() > f32::EPSILON {
        for slot in pixel.iter_mut().take(3) {
            *slot = (*slot as f32 * factor).min(255.0) as u8;
        }
    }
    pixel
}

fn blend_pixel(
    buf: &mut [u8],
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    color: [u8; 4],
    solid_on_empty: bool,
) {
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let index = ((y * width + x) as usize) * 4;
    if index + 3 >= buf.len() {
        return;
    }
    // вода поверх пустого холста рисуется плотной: полупрозрачность годится
    // только там, где под ней есть суша, иначе океан на тёмном фоне «исчезает»
    if solid_on_empty && buf[index + 3] == 0 {
        buf[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
        return;
    }
    if color[3] >= 255 {
        buf[index..index + 4].copy_from_slice(&color);
        return;
    }
    if color[3] == 0 {
        return;
    }
    let alpha = color[3] as u32;
    for slot in 0..3 {
        let dst = buf[index + slot] as u32;
        buf[index + slot] = ((color[slot] as u32 * alpha + dst * (255 - alpha)) / 255) as u8;
    }
    let dst_alpha = buf[index + 3] as u32;
    buf[index + 3] = (alpha + dst_alpha * (255 - alpha) / 255).min(255) as u8;
}

#[allow(clippy::too_many_arguments)]
fn draw_top_face(
    buf: &mut [u8],
    width: i32,
    height: i32,
    cx: i32,
    cy: i32,
    cell: u16,
    span: i32,
    rot: u32,
    tint: Option<[f32; 3]>,
    water: bool,
) {
    // span: шаг выборки в блоках. При шаге 2 каждый разобранный столбец
    // представляет квадрат 2x2, и ромб верха рисуется во всю эту ширину,
    // иначе между тайлами остаются дырки-«решётка»
    let span = span.max(1);
    let half_w = (TILE_W * span) as f32 / 2.0;
    let half_h = (TILE_H * span) as f32 / 2.0;
    let texture = 16usize;
    for py in cy..cy + TILE_H * span {
        let fy = (py - cy) as f32 + 0.5;
        for px in (cx - TILE_W * span / 2)..(cx + TILE_W * span / 2) {
            let fx = (px - cx) as f32 + 0.5;
            let u = (fx / half_w + fy / half_h) / 2.0;
            let v = (fy / half_h - fx / half_w) / 2.0;
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                continue;
            }
            let (su, sv) = rot_uv(u, v, rot);
            let sample = sample_cell(cell, su, sv);
            if sample[3] == 0 {
                continue;
            }
            let pixel = if water {
                let mut pixel = recolor_water(sample, tint.unwrap_or(WATER_TINT), 1.0);
                pixel[3] = (pixel[3] as f32 * 0.6) as u8;
                pixel
            } else {
                tint_pixel(sample, tint, 1.0)
            };
            blend_pixel(buf, width, height, px, py, pixel, water);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_side_face(
    buf: &mut [u8],
    width: i32,
    height: i32,
    cx: i32,
    cy: i32,
    face: usize,
    drop: i32,
    span: i32,
    cell: u16,
    tint: Option<[f32; 3]>,
    factor: f32,
    water: bool,
) {
    let span = span.max(1);
    let depth = (drop * SIDE_V) as f32;
    if depth <= 0.0 {
        return;
    }
    let (ax, ay, bx, by) = if face == 0 {
        (
            cx as f32,
            (cy + TILE_H) as f32,
            (cx + TILE_W * span / 2) as f32,
            (cy + TILE_H - TILE_H * span / 2) as f32,
        )
    } else {
        (
            cx as f32,
            (cy + TILE_H) as f32,
            (cx - TILE_W * span / 2) as f32,
            (cy + TILE_H - TILE_H * span / 2) as f32,
        )
    };
    let texture = 16usize;
    let min_px = ax.min(bx).floor() as i32;
    let max_px = ax.max(bx).ceil() as i32;
    let min_py = ay.min(by).floor() as i32;
    let max_py = (ay.max(by) + depth).ceil() as i32;
    for py in min_py..=max_py {
        for px in min_px..=max_px {
            let sx = px as f32 + 0.5;
            let sy = py as f32 + 0.5;
            let u = (sx - ax) / (bx - ax);
            if !(0.0..1.0).contains(&u) {
                continue;
            }
            let edge = ay + u * (by - ay);
            if sy < edge {
                continue;
            }
            let v = ((sy - edge) / (SIDE_V as f32)) % 1.0;
            let v = if v < 0.0 { v + 1.0 } else { v };
            let sample = sample_cell(
                cell,
                (u * texture as f32) as usize,
                (v * texture as f32) as usize,
            );
            if sample[3] == 0 {
                continue;
            }
            let pixel = if water {
                let mut pixel = recolor_water(sample, tint.unwrap_or(WATER_TINT), factor);
                pixel[3] = (pixel[3] as f32 * 0.6) as u8;
                pixel
            } else {
                tint_pixel(sample, tint, factor)
            };
            blend_pixel(buf, width, height, px, py, pixel, water);
        }
    }
}

/// вид сверху: текстура верхнего блока колонки на всю ячейку, яркость по высоте
fn render_map_png(
    world: &FullColumns,
    spawn_x: i32,
    spawn_z: i32,
    win_x0: i32,
    win_z0: i32,
    win_w: i32,
    block_px: i32,
) -> Rendered {
    // холст размером с окно выборки и привязан К НЕМУ, а не к спавну: окно
    // центрируется по контенту и может лежать далеко от спавна, и холст вокруг
    // спавна тогда рисует весь контент за кадром — карта выходит пустой.
    // Фон непрозрачный: полупрозрачная вода поверх пустого холста на тёмной
    // сцене диалога читается как «пустая карта»
    let size = win_w.max(1) * block_px;
    const MAP_BG: [u8; 4] = [18, 22, 30, 255];
    let mut buf = vec![0u8; (size * size * 4) as usize];
    for chunk in buf.chunks_exact_mut(4) {
        chunk.copy_from_slice(&MAP_BG);
    }
    // кэш готовых тайлов (блок, свойства, поворот, уровень яркости): тысячи
    // столбцов плотного мира переиспользуют десятки тайлов вместо пересчёта
    // пикселей на каждый столбец
    let mut tile_cache: HashMap<(u16, u16, u32, u8), Vec<u8>> = HashMap::default();
    let min_y = world
        .columns
        .iter()
        .map(|column| column.blocks[0].0)
        .min()
        .unwrap_or(0);
    let max_y = world
        .columns
        .iter()
        .map(|column| column.blocks[0].0)
        .max()
        .unwrap_or(min_y + 1);
    let span = (max_y - min_y).max(1);
    let mut focus: Option<(i32, i32, i32, i32)> = None;
    for column in &world.columns {
        let (y, id, props_id) = column.blocks[0];
        let rel = (y - min_y) as f32 / span as f32;
        let factor = 0.7 + 0.3 * rel;
        let name = &world.names[id as usize];
        let (top_base, side_cell) = super::world_preview_textures::texture_cells(name);
        let props = world
            .props
            .get(props_id as usize)
            .map(String::as_str)
            .unwrap_or("");
        // горизонтальные брёвна лежат: сверху кора (боковая текстура), не кольца
        let (top, rot) = if horizontal_axis(props) {
            (side_cell, 0u32)
        } else {
            (top_base, rotation_quarters(props))
        };
        let tint = tint_for(name);
        let water = name.trim_start_matches("minecraft:").contains("water");
        let x0 = (column.bx - win_x0) * block_px;
        let z0 = (column.bz - win_z0) * block_px;
        if !water {
            let tile_w = world.step.max(1) * block_px;
            let (fx0, fy0, fx1, fy1) = focus.unwrap_or((x0, z0, x0 + tile_w, z0 + tile_w));
            focus = Some((
                fx0.min(x0),
                fy0.min(z0),
                fx1.max(x0 + tile_w),
                fy1.max(z0 + tile_w),
            ));
        }
        let span = world.step.max(1) as usize;
        let tile = span * block_px as usize;
        let tile_from = |tile: usize| -> Vec<u8> {
            let mut data = vec![0u8; tile * tile * 4];
            for chunk in data.chunks_exact_mut(4) {
                chunk.copy_from_slice(&MAP_BG);
            }
            for ty in 0..tile {
                for tx in 0..tile {
                    let (su, sv) = rot16(tx / span, ty / span, rot);
                    let sample = sample_cell(top, su, sv);
                    let pixel = if water {
                        // на карте вода непрозрачная, как на ванильной карте
                        let mut pixel = recolor_water(sample, tint.unwrap_or(WATER_TINT), factor);
                        pixel[3] = 255;
                        pixel
                    } else {
                        tint_pixel(sample, tint, factor)
                    };
                    if pixel[3] == 0 {
                        continue;
                    }
                    blend_pixel(
                        &mut data,
                        tile as i32,
                        tile as i32,
                        tx as i32,
                        ty as i32,
                        pixel,
                        water,
                    );
                }
            }
            data
        };
        let inside = x0 >= 0 && z0 >= 0 && x0 + tile as i32 <= size && z0 + tile as i32 <= size;
        if inside {
            let tile_key = (id, props_id, rot, ((factor * 16.0).round() as u8).min(255));
            let tile_data = match tile_cache.get(&tile_key) {
                Some(data) => data.clone(),
                None => {
                    let data = tile_from(tile);
                    if tile_cache.len() < 8192 {
                        tile_cache.insert(tile_key, data.clone());
                    }
                    data
                }
            };
            // тайл уже собран над фоном — копируем его строками целиком
            for ty in 0..tile {
                let src = ty * tile * 4;
                let dst = ((z0 as usize + ty) * size as usize + x0 as usize) * 4;
                buf[dst..dst + tile * 4].copy_from_slice(&tile_data[src..src + tile * 4]);
            }
        } else {
            for ty in 0..tile {
                for tx in 0..tile {
                    let (su, sv) = rot16(tx / span, ty / span, rot);
                    let sample = sample_cell(top, su, sv);
                    let pixel = if water {
                        let mut pixel = recolor_water(sample, tint.unwrap_or(WATER_TINT), factor);
                        pixel[3] = 255;
                        pixel
                    } else {
                        tint_pixel(sample, tint, factor)
                    };
                    if pixel[3] == 0 {
                        continue;
                    }
                    blend_pixel(
                        &mut buf,
                        size,
                        size,
                        x0 + tx as i32,
                        z0 + ty as i32,
                        pixel,
                        water,
                    );
                }
            }
        }
    }
    Rendered {
        buf,
        width: size,
        height: size,
        focus,
        spawn_px: Some(((spawn_x - win_x0) * block_px, (spawn_z - win_z0) * block_px)),
        top_faces: world.columns.len(),
        side_faces: 0,
    }
}

/// изометрия: настоящий по-блочный voxel-рендер. Каждый блок рисует верхнюю
/// грань (если над ним воздух) и обе видимые боковые грани (если соседняя
/// колонка сетки выборки на этой высоте пуста). Ничего не пропадает: нижние
/// ярусы, фасады под навесами и многоуровневые платформы рендерятся целиком
fn render_iso_png(world: &FullColumns) -> Rendered {
    let cy_of = |bx: i32, bz: i32, y: i32| (bx + bz) * TILE_H / 2 - y * SIDE_V;
    let min_cx = world
        .columns
        .iter()
        .map(|column| (column.bx - column.bz) * TILE_W / 2)
        .min()
        .unwrap_or(0);
    let max_cx = world
        .columns
        .iter()
        .map(|column| (column.bx - column.bz) * TILE_W / 2)
        .max()
        .unwrap_or(0);
    let mut min_cy = i32::MAX;
    let mut max_bottom = i32::MIN;
    let mut focus: Option<(i32, i32, i32, i32)> = None;
    for column in &world.columns {
        let cx = (column.bx - column.bz) * TILE_W / 2 - min_cx + MARGIN + TILE_W / 2;
        for &(y, id, _) in &column.blocks {
            let cy = cy_of(column.bx, column.bz, y);
            min_cy = min_cy.min(cy);
            max_bottom = max_bottom.max(cy + TILE_H * world.step + SIDE_V);
            if !world.names[id as usize]
                .trim_start_matches("minecraft:")
                .contains("water")
            {
                let fx0 = cx - TILE_W / 2;
                let fy1 = cy + TILE_H + SIDE_V;
                let (ox0, oy0, ox1, oy1) = focus.unwrap_or((fx0, cy, fx0 + TILE_W, fy1));
                focus = Some((
                    ox0.min(fx0),
                    oy0.min(cy),
                    ox1.max(fx0 + TILE_W),
                    oy1.max(fy1),
                ));
            }
        }
    }
    if min_cy == i32::MAX {
        return Rendered {
            buf: Vec::new(),
            width: 0,
            height: 0,
            focus: None,
            spawn_px: None,
            top_faces: 0,
            side_faces: 0,
        };
    }
    let width = max_cx - min_cx + TILE_W * world.step + MARGIN * 2;
    let height = max_bottom - min_cy + MARGIN * 2;
    let mut buf = vec![0u8; (width.max(1) * height.max(1) * 4) as usize];
    let mut top_faces = 0usize;
    let mut side_faces = 0usize;
    let step = world.step;
    for column in &world.columns {
        let cx = (column.bx - column.bz) * TILE_W / 2 - min_cx + MARGIN + TILE_W / 2;
        for (i, &(y, id, props_id)) in column.blocks.iter().enumerate() {
            let cy = cy_of(column.bx, column.bz, y) - min_cy + MARGIN;
            let name = &world.names[id as usize];
            let (top_base, side_cell) = super::world_preview_textures::texture_cells(name);
            let props = world
                .props
                .get(props_id as usize)
                .map(String::as_str)
                .unwrap_or("");
            let rot = rotation_quarters(props);
            let axis = props_value(props, "axis").unwrap_or("");
            // бревно по оси X/Z: сверху кора, на торцах (видимых боковых) кольца
            let top_cell = if axis == "x" || axis == "z" {
                side_cell
            } else {
                top_base
            };
            let face_cell = |face: usize| match (axis, face) {
                ("x", 0) | ("z", 1) => top_base,
                _ => side_cell,
            };
            let tint = tint_for(name);
            let water = name.trim_start_matches("minecraft:").contains("water");
            let occupied_above = i > 0 && column.blocks[i - 1].0 == y + 1;
            if !occupied_above {
                top_faces += 1;
                draw_top_face(
                    &mut buf, width, height, cx, cy, top_cell, step, rot, tint, water,
                );
            }
            for (face, (dx, dz), factor) in [(0usize, (step, 0), 0.78f32), (1, (0, step), 0.62f32)]
            {
                let exposed = match world.index.get(&(column.bx + dx, column.bz + dz)) {
                    Some(&ni) => world.columns[ni]
                        .blocks
                        .binary_search_by(|probe| probe.0.cmp(&y).reverse())
                        .is_err(),
                    None => true,
                };
                if exposed {
                    side_faces += 1;
                    draw_side_face(
                        &mut buf,
                        width,
                        height,
                        cx,
                        cy,
                        face,
                        1,
                        step,
                        face_cell(face),
                        tint,
                        factor,
                        water,
                    );
                }
            }
        }
    }
    Rendered {
        buf,
        width,
        height,
        focus,
        spawn_px: None,
        top_faces,
        side_faces,
    }
}

fn resolve_world_dir(saves: &Path, requested: &str) -> Result<(PathBuf, String), String> {
    let world: PathBuf = if requested.trim().is_empty() {
        let mut worlds: Vec<PathBuf> = saves
            .read_dir()
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.join("level.dat").is_file())
                    .collect()
            })
            .unwrap_or_default();
        worlds.sort();
        worlds.pop().ok_or("В экземпляре нет миров для превью")?
    } else {
        let path = saves.join(requested);
        if !path.join("level.dat").is_file() {
            return Err("Мир не найден".to_owned());
        }
        path
    };
    let world_folder = world
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((world, world_folder))
}

fn request_world(request: &Value) -> Result<(PathBuf, PathBuf, String), String> {
    let config = ConfigStore::load()?;
    let game_dir = config.game_dir()?;
    let name = request
        .get("instance")
        .and_then(Value::as_str)
        .ok_or("Не указан экземпляр")?;
    let instance_path = resolve_instance_path(&game_dir, name).ok_or("Экземпляр не найден")?;
    let requested = request.get("world").and_then(Value::as_str).unwrap_or("");
    let (world, world_folder) = resolve_world_dir(&instance_path.join("saves"), requested)?;
    Ok((instance_path, world, world_folder))
}

/// точки смерти из логов: строки со смертью игрока. Ванильный клиент пишет
/// смерть как [CHAT]-строку без координат, серверные и модовые логи обычно
/// содержат «(x, y, z)» в самой строке или соседней — берём оттуда. Смерти без
/// координат на карту не попадают.
fn collect_deaths(instance: &Path) -> Vec<Value> {
    const DEATH_WORDS: [&str; 30] = [
        "fell from a high place",
        "fell out of the world",
        "fell off",
        "hit the ground too hard",
        "was slain by",
        "was shot by",
        "was pummeled by",
        "blown up by",
        "was killed by",
        "was impaled by",
        "was skewered",
        "was squashed",
        "went up in flames",
        "burned to death",
        "drowned",
        "starved to death",
        "suffocated in a wall",
        "was struck by lightning",
        "withered away",
        "tried to swim in lava",
        "was frozen",
        "умер",
        "умерла",
        "погиб",
        "был убит",
        "упал с высоты",
        "выпал из мира",
        "сгорел",
        "утонул",
        "испустил дух",
    ];
    const MAX_DEATHS: usize = 200;
    const MAX_LOOKAHEAD: usize = 2;
    let logs_dir = instance.join("logs");
    let mut files: Vec<std::path::PathBuf> = match fs::read_dir(&logs_dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name == "latest.log" || name.ends_with(".log.gz"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    let mut deaths: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<(i64, i64, i64, String)> =
        std::collections::HashSet::new();
    for path in files {
        let lines = read_log_lines(&path);
        let mut index = 0usize;
        while index < lines.len() {
            let lower = lines[index].to_lowercase();
            if !DEATH_WORDS.iter().any(|word| lower.contains(word)) {
                index += 1;
                continue;
            }
            let cause = death_cause(&lines[index]);
            let mut coords = find_coords(&lines[index]);
            if coords.is_none() {
                for offset in 1..=MAX_LOOKAHEAD {
                    let next = lines.get(index + offset).map(String::as_str).unwrap_or("");
                    if next.is_empty() {
                        break;
                    }
                    // соседняя строка относится к этой смерти, только если сама
                    // не описывает другую смерть
                    let next_lower = next.to_lowercase();
                    if DEATH_WORDS.iter().any(|word| next_lower.contains(word)) {
                        break;
                    }
                    if let Some(found) = find_coords(next) {
                        coords = Some(found);
                        index += offset;
                        break;
                    }
                }
            }
            if let Some((x, y, z)) = coords {
                let key = (
                    x.round() as i64,
                    y.round() as i64,
                    z.round() as i64,
                    cause.chars().take(48).collect::<String>(),
                );
                if seen.insert(key) {
                    deaths.push(json!({
                        "x": x,
                        "y": y,
                        "z": z,
                        "ts": log_timestamp(&lines[index]),
                        "cause": cause,
                    }));
                }
            }
            index += 1;
        }
    }
    if deaths.len() > MAX_DEATHS {
        deaths.split_off(deaths.len() - MAX_DEATHS)
    } else {
        deaths
    }
}

fn read_log_lines(path: &Path) -> Vec<String> {
    use std::io::Read as _;
    const MAX_LOG_BYTES: usize = 32 * 1024 * 1024;
    let raw = match fs::read(path) {
        Ok(bytes) if bytes.len() <= MAX_LOG_BYTES => bytes,
        Ok(bytes) => {
            let start = bytes.len() - MAX_LOG_BYTES;
            let offset = bytes[start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|found| start + found + 1)
                .unwrap_or(start);
            bytes[offset..].to_vec()
        }
        Err(_) => return Vec::new(),
    };
    let text = if path.extension().and_then(|ext| ext.to_str()) == Some("gz") {
        let mut decoder = flate2::read::GzDecoder::new(raw.as_slice());
        let mut out = Vec::new();
        match decoder.read_to_end(&mut out) {
            Ok(_) => String::from_utf8_lossy(&out).into_owned(),
            Err(_) => return Vec::new(),
        }
    } else {
        String::from_utf8_lossy(&raw).into_owned()
    };
    text.lines().map(|line| line.to_owned()).collect()
}

fn death_cause(line: &str) -> String {
    let mut text = line.trim();
    // срезаем «[10:15:01] [Render thread/INFO]:» и «[CHAT]»
    if let Some(end) = text.find("]: ") {
        let head = &text[..end];
        if head.starts_with('[') {
            text = &text[end + 3..];
        }
    }
    let text = text.strip_prefix("[CHAT] ").unwrap_or(text).trim();
    let mut cause = String::new();
    for (count, char) in text.chars().enumerate() {
        if count >= 100 {
            cause.push('…');
            break;
        }
        cause.push(char);
    }
    cause
}

fn log_timestamp(line: &str) -> String {
    let mut token = String::new();
    for char in line.chars() {
        if char.is_ascii_digit() || char == ':' {
            token.push(char);
        } else {
            if token.matches(':').count() == 2
                && token.chars().filter(|item| item.is_ascii_digit()).count() >= 4
            {
                return token;
            }
            token.clear();
        }
    }
    if token.matches(':').count() == 2
        && token.chars().filter(|item| item.is_ascii_digit()).count() >= 4
    {
        token
    } else {
        String::new()
    }
}

/// «(x, y, z)» или «x, y, z» в произвольном месте строки
fn find_coords(line: &str) -> Option<(f64, f64, f64)> {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'-' || bytes[index].is_ascii_digit() {
            let mut starts = [0usize; 3];
            let mut ends = [0usize; 3];
            let mut ok = true;
            for part in 0..3 {
                let begin = index;
                if bytes.get(begin) == Some(&b'-') {
                    index += 1;
                }
                let digits_start = index;
                while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.')
                {
                    index += 1;
                }
                if index == digits_start {
                    ok = false;
                    break;
                }
                starts[part] = begin;
                ends[part] = index;
                if part < 2 {
                    if bytes.get(index) == Some(&b',') {
                        index += 1;
                    }
                    if bytes.get(index) == Some(&b' ') {
                        index += 1;
                    }
                    match bytes.get(index) {
                        Some(&byte) if byte == b'-' || byte.is_ascii_digit() => {}
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if ok {
                let x: f64 = line
                    .get(starts[0]..ends[0])
                    .and_then(|part| part.parse().ok())?;
                let y: f64 = line
                    .get(starts[1]..ends[1])
                    .and_then(|part| part.parse().ok())?;
                let z: f64 = line
                    .get(starts[2]..ends[2])
                    .and_then(|part| part.parse().ok())?;
                if x.abs() < 30_000_000.0 && y.abs() < 4000.0 && z.abs() < 30_000_000.0 {
                    return Some((x, y, z));
                }
            }
        } else {
            index += 1;
        }
    }
    None
}

pub fn instance_world_preview(request: &Value) -> Result<Value, String> {
    let (instance_path, world, world_folder) = request_world(request)?;
    let deaths = collect_deaths(&instance_path);
    let mode = if request.get("mode").and_then(Value::as_str) == Some("iso") {
        "iso"
    } else {
        "map"
    };
    let level = decode_nbt_bytes(
        &fs::read(world.join("level.dat"))
            .map_err(|error| format!("level.dat не читается: {error}"))?,
    )?;
    let data = level.get("Data").cloned().unwrap_or(json!({}));
    let raw_name = data
        .get("LevelName")
        .and_then(Value::as_str)
        .unwrap_or("мир");
    let mut cleaned = String::with_capacity(raw_name.len());
    let mut chars = raw_name.chars();
    while let Some(char) = chars.next() {
        if char == '\u{a7}' {
            chars.next();
            continue;
        }
        cleaned.push(char);
    }
    let world_name = cleaned.trim().to_owned();
    let spawn_x = data.get("SpawnX").and_then(Value::as_i64).unwrap_or(0) as i32;
    let spawn_z = data.get("SpawnZ").and_then(Value::as_i64).unwrap_or(0) as i32;
    // кэш превью: пока region-файлы мира не менялись, повторное открытие
    // отдаёт готовый PNG без разбора мира и рендера. Смерти, имя и путь
    // мира всегда свежие — они не зависят от рендера
    let cache_stamp = region_stamp(&world);
    let cache_dir = super::state::launcher_roots()
        .cache_home
        .join("world_previews");
    let cache_base = format!(
        "preview-{}-{}-v{}-{:08x}",
        world_folder.replace(['/', '\\', ':', ' '], "_"),
        mode,
        cache_stamp,
        fnv1a(world.to_string_lossy().as_bytes())
    );
    if cache_stamp > 0 {
        let cache_png = cache_dir.join(format!("{}.png", cache_base));
        let cache_meta = cache_dir.join(format!("{}.json", cache_base));
        if let (Ok(bytes), Ok(meta_text)) = (fs::read(&cache_png), fs::read_to_string(&cache_meta))
        {
            if let Ok(mut cached) = serde_json::from_str::<serde_json::Value>(&meta_text) {
                cached["png_path"] = json!(cache_png.display().to_string());
                cached["png_base64"] = if bytes.len() <= 1_500_000 {
                    use base64::Engine as _;
                    json!(base64::engine::general_purpose::STANDARD.encode(&bytes))
                } else {
                    serde_json::Value::Null
                };
                cached["world"] = json!(world_name);
                cached["world_folder"] = json!(world_folder);
                cached["deaths"] = json!(deaths);
                return Ok(cached);
            }
        }
    }
    // Окно выборки по реальному контенту: заголовки регионов вокруг спавна
    // дают bbox непустых чанков. Компактные миры (паркур, спираль, острова)
    // рендерятся целиком, плотные падают на окно спавна.
    let content = content_bounds(&world, spawn_x, spawn_z);
    // Контент шире окна: центрируем окно на содержимом, а не на спавне —
    // длинные спирали и паркур иначе обрезаются до точки входа
    let mut window = match content {
        Some((x0, z0, x1, z1)) => {
            let span = (x1 - x0).max(z1 - z0);
            if span <= CONTENT_WINDOW_CAP {
                (x0 - 8, z0 - 8, x1 + 8, z1 + 8)
            } else {
                let half = CONTENT_WINDOW_CAP / 2 + 8;
                // сгенерированный океан растягивает bbox, и окно по его центру
                // уходит с постройки: карта показывает только воду. Центр окна
                // держим на самом плотном контенте мира
                let (cx, cz) = content_center(&world).unwrap_or(((x0 + x1) / 2, (z0 + z1) / 2));
                (cx - half, cz - half, cx + half, cz + half)
            }
        }
        None => {
            let half = SAMPLE_COLUMNS / 2;
            (
                spawn_x - (half - 1) * SAMPLE_STEP,
                spawn_z - (half - 1) * SAMPLE_STEP,
                spawn_x + (half - 1) * SAMPLE_STEP,
                spawn_z + (half - 1) * SAMPLE_STEP,
            )
        }
    };
    // выборка всегда шагом 1: тонкие структуры шириной в блок перестают
    // «шахматить», плотные миры рисуются настоящими блоками, а не тайлами
    // 2×2 (диалог зумит, и укрупнённые клетки выглядят размытыми пятнами).
    // Шаг 2 остаётся только запасным сценарием для патологически широких
    // окон, чтобы не съесть память
    let window_area = (window.2 - window.0 + 1).max(1) * (window.3 - window.1 + 1).max(1);
    let step = if window_area <= 250_000 {
        1
    } else {
        SAMPLE_STEP
    };
    let mut tables = NameTables::default();
    let top_only = mode != "iso";
    let mut columns = collect_columns_full(
        &world,
        window.0,
        window.1,
        window.2,
        window.3,
        step,
        &mut tables,
        top_only,
    );
    if columns.is_empty() {
        // content_bounds может натянуться на два далёких скопления с пустой
        // серединой: окно по центру bbox тогда берёт пустоту и мир «не
        // найден». Пробуем точку контента, ближайшую к спавну, затем углы
        // bbox — что-нибудь из этого лежит на постройке
        if let Some((x0, z0, x1, z1)) = content {
            let half = ((window.2 - window.0) / 2).max(8);
            let mut centers = vec![(spawn_x.clamp(x0, x1), spawn_z.clamp(z0, z1))];
            for (cx, cz) in [(x0, z0), (x1, z0), (x0, z1), (x1, z1)] {
                centers.push((cx, cz));
            }
            for (cx, cz) in centers {
                let alt = (cx - half, cz - half, cx + half, cz + half);
                let found = collect_columns_full(
                    &world,
                    alt.0,
                    alt.1,
                    alt.2,
                    alt.3,
                    step,
                    &mut tables,
                    top_only,
                );
                if !found.is_empty() {
                    columns = found;
                    window = alt;
                    break;
                }
            }
        }
    }
    if columns.is_empty() {
        return Err("В области спавна нет разобранных чанков".to_owned());
    }
    let world_columns = build_full_columns(columns, &tables, step);
    let win_w = (window.2 - window.0 + 1).max(1);
    let map_px = (4864 / (win_w + 1)).clamp(4, MAP_PIXEL);
    let rendered = if mode == "iso" {
        render_iso_png(&world_columns)
    } else {
        render_map_png(
            &world_columns,
            spawn_x,
            spawn_z,
            window.0,
            window.1,
            win_w,
            map_px,
        )
    };

    let (top_faces, side_faces) = (rendered.top_faces, rendered.side_faces);
    // карта рендерится квадратом на всю область выборки; обрезаем до контента
    let mut spawn_px = rendered.spawn_px;
    // мировые координаты левого верхнего пикселя карты: после обрезки до
    // контента спавн может остаться за кадром, а маркерам (смерти, метки,
    // поиск) всё равно нужна привязка картинки к миру
    let mut map_origin: Option<(f64, f64)> = if mode == "map" {
        Some((window.0 as f64, window.1 as f64))
    } else {
        None
    };
    let (buf, width, height, focus) = if mode == "iso" {
        (
            rendered.buf,
            rendered.width,
            rendered.height,
            rendered.focus,
        )
    } else {
        match rendered.focus {
            Some((fx, fy, fx1, fy1)) => {
                const CROP_MARGIN: i32 = 24;
                let mut x0 = (fx - CROP_MARGIN).max(0);
                let mut y0 = (fy - CROP_MARGIN).max(0);
                let mut x1 = (fx1 + CROP_MARGIN).min(rendered.width);
                let mut y1 = (fy1 + CROP_MARGIN).min(rendered.height);
                // пол кропа: узкий остаток контента (тонкая спираль среди
                // сгенерированного океана) не должен превращать карту в
                // нечитаемый зум-фрагмент — расширяем прямоугольник до минимума
                const CROP_FLOOR_BLOCKS: i32 = 128;
                let crop_floor = CROP_FLOOR_BLOCKS * map_px;
                if crop_floor > 0 {
                    if x1 - x0 < crop_floor {
                        let mid = (x0 + x1) / 2;
                        x0 = (mid - crop_floor / 2).max(0);
                        x1 = (x0 + crop_floor).min(rendered.width);
                        if x1 - x0 < crop_floor {
                            x0 = (x1 - crop_floor).max(0);
                        }
                    }
                    if y1 - y0 < crop_floor {
                        let mid = (y0 + y1) / 2;
                        y0 = (mid - crop_floor / 2).max(0);
                        y1 = (y0 + crop_floor).min(rendered.height);
                        if y1 - y0 < crop_floor {
                            y0 = (y1 - crop_floor).max(0);
                        }
                    }
                }
                if x1 > x0 && y1 > y0 {
                    let new_width = x1 - x0;
                    let new_height = y1 - y0;
                    let mut cropped = vec![0u8; (new_width * new_height * 4) as usize];
                    for row in 0..new_height {
                        let src_index = ((y0 + row) * rendered.width + x0) as usize * 4;
                        let dst_index = (row * new_width) as usize * 4;
                        let len = new_width as usize * 4;
                        cropped[dst_index..dst_index + len]
                            .copy_from_slice(&rendered.buf[src_index..src_index + len]);
                    }
                    spawn_px = spawn_px
                        .map(|(sx, sy)| (sx - x0, sy - y0))
                        .filter(|&(sx, sy)| {
                            sx >= 0 && sy >= 0 && sx < new_width && sy < new_height
                        });
                    map_origin = map_origin.map(|(origin_x, origin_z)| {
                        (
                            origin_x + x0 as f64 / map_px as f64,
                            origin_z + y0 as f64 / map_px as f64,
                        )
                    });
                    (cropped, new_width, new_height, None)
                } else {
                    (
                        rendered.buf,
                        rendered.width,
                        rendered.height,
                        rendered.focus,
                    )
                }
            }
            None => (
                rendered.buf,
                rendered.width,
                rendered.height,
                rendered.focus,
            ),
        }
    };
    // файл превью = сам кэш-файл: следующий запуск с тем же штампом мира
    // переиспользует его без рендера
    let out_path = cache_dir.join(format!("{}.png", cache_base));
    // кодируем один раз: раньше PNG кодировался дважды (в base64 и в файл)
    let png_bytes = encode_png(width as u32, height as u32, &buf)?;
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Не удалось создать каталог превью: {error}"))?;
    }
    fs::write(&out_path, &png_bytes)
        .map_err(|error| format!("Не удалось создать превью: {error}"))?;
    let png_base64 = if png_bytes.len() <= 1_500_000 {
        use base64::Engine as _;
        json!(base64::engine::general_purpose::STANDARD.encode(&png_bytes))
    } else {
        // большие превью грузятся по png_path (asset-протокол): мегабайты
        // base64 через IPC тормозят открытие карты на плотных мирах
        serde_json::Value::Null
    };
    let result = json!({
        "png_path": out_path.to_string_lossy(),
        "png_base64": png_base64,
        "width": width,
        "height": height,
        "mode": mode,
        "world": world_name,
        "world_folder": world_folder,
        "columns": world_columns.columns.len(),
        "blocks": world_columns.total_blocks,
        "top_faces": top_faces,
        "side_faces": side_faces,
        "spawn": { "x": spawn_x, "z": spawn_z },
        "deaths": deaths,
        "block_px": if mode == "map" {
            map_px
        } else {
            MAP_PIXEL
        },
        "map_origin": match map_origin {
            Some((origin_x, origin_z)) => json!({ "x": origin_x, "z": origin_z }),
            None => serde_json::Value::Null,
        },
        "spawn_px": match spawn_px {
            Some((sx, sy)) => json!({ "x": sx, "y": sy }),
            None => serde_json::Value::Null,
        },
        "focus": match focus {
            Some((fx, fy, fx1, fy1)) => {
                json!({ "x": fx, "y": fy, "w": fx1 - fx, "h": fy1 - fy })
            }
            None => serde_json::Value::Null,
        },
    });
    let _ = fs::write(
        cache_dir.join(format!("{}.json", cache_base)),
        serde_json::to_string(&result).unwrap_or_default(),
    );
    Ok(result)
}

// ---------------------------------------------------------------------------
// Карта мира 2.0: поиск по чанкам (структуры, биомы, руды) и метки игрока
// ---------------------------------------------------------------------------

const SEARCH_REGION_RINGS: i32 = 4;
const SEARCH_CHUNK_CAP: usize = 2400;
const SEARCH_STRUCTURE_CAP: usize = 240;
const SEARCH_BIOME_CAP: usize = 320;
const SEARCH_ORE_CAP: usize = 1200;
const ORE_SECTION_HITS: usize = 6;

fn is_ore_name(base: &str) -> bool {
    base.ends_with("_ore") || base == "ancient_debris"
}

fn load_chunk_root(file: &[u8], lcx: usize, lcz: usize) -> Option<Value> {
    if file.len() < 8192 {
        return None;
    }
    let index = lcz * 32 + lcx;
    let head = &file[index * 4..index * 4 + 4];
    let offset = ((head[0] as usize) << 16) | ((head[1] as usize) << 8) | head[2] as usize;
    if offset == 0 {
        return None;
    }
    let start = offset * 4096;
    if start + 5 > file.len() {
        return None;
    }
    let length = u32::from_be_bytes([
        file[start],
        file[start + 1],
        file[start + 2],
        file[start + 3],
    ]) as usize;
    let compression = file[start + 4];
    if length == 0 || start + 4 + length > file.len() {
        return None;
    }
    let body = &file[start + 5..start + 4 + length];
    match compression {
        1 => {
            let mut out = Vec::new();
            GzDecoder::new(body).read_to_end(&mut out).ok()?;
            Nbt { data: &out, pos: 0 }.root().ok()
        }
        2 => {
            let mut out = Vec::new();
            ZlibDecoder::new(body).read_to_end(&mut out).ok()?;
            Nbt { data: &out, pos: 0 }.root().ok()
        }
        3 => Nbt { data: body, pos: 0 }.root().ok(),
        _ => None,
    }
}

/// поиск по миру: структуры (starts в чанках), биомы (палитры секций) и руды
/// (полный разбор секций с рудами в палитре). Регионы раскрываются кольцами
/// вокруг спавна, ограничение по чанкам и по числу попаданий на каждый вид.
pub fn world_search(request: &Value) -> Result<Value, String> {
    let (_instance_path, world, _world_folder) = request_world(request)?;
    let kinds: std::collections::HashSet<String> = request
        .get("kinds")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| {
            ["structures", "biomes", "ores"]
                .iter()
                .map(|kind| (*kind).to_owned())
                .collect()
        });
    let want = |kind: &str| kinds.contains(kind);
    let level = decode_nbt_bytes(
        &fs::read(world.join("level.dat"))
            .map_err(|error| format!("level.dat не читается: {error}"))?,
    )?;
    let data = level.get("Data").cloned().unwrap_or(json!({}));
    let spawn_x = data.get("SpawnX").and_then(Value::as_i64).unwrap_or(0) as i32;
    let spawn_z = data.get("SpawnZ").and_then(Value::as_i64).unwrap_or(0) as i32;
    let spawn_region = (spawn_x >> 9, spawn_z >> 9);
    let mut structures: Vec<Value> = Vec::new();
    let mut biomes: Vec<Value> = Vec::new();
    let mut ores: Vec<Value> = Vec::new();
    let mut structure_seen: std::collections::HashSet<(String, i64, i64)> = Default::default();
    let mut biome_seen: std::collections::HashSet<String> = Default::default();
    let mut chunk_count = 0usize;
    'rings: for ring in 0..=SEARCH_REGION_RINGS {
        for dx in -ring..=ring {
            for dz in -ring..=ring {
                if dx.abs() != ring && dz.abs() != ring {
                    continue;
                }
                let (rx, rz) = (spawn_region.0 + dx, spawn_region.1 + dz);
                let Ok(bytes) = fs::read(world.join("region").join(format!("r.{rx}.{rz}.mca")))
                else {
                    continue;
                };
                for lcx in 0..32usize {
                    for lcz in 0..32usize {
                        if chunk_count >= SEARCH_CHUNK_CAP {
                            break 'rings;
                        }
                        let Some(root) = load_chunk_root(&bytes, lcx, lcz) else {
                            continue;
                        };
                        chunk_count += 1;
                        let cx = rx * 32 + lcx as i32;
                        let cz = rz * 32 + lcz as i32;
                        let base_x = cx * 16 + 8;
                        let base_z = cz * 16 + 8;
                        if want("structures") {
                            if let Some(starts) = root
                                .get("structures")
                                .and_then(|item| item.get("starts"))
                                .and_then(Value::as_object)
                            {
                                for (id, start) in starts {
                                    let Some(bb) = start.get("bb").and_then(Value::as_array) else {
                                        continue;
                                    };
                                    let coords: Vec<i64> =
                                        bb.iter().filter_map(Value::as_i64).collect();
                                    if coords.len() != 6
                                        || coords[0] > coords[3]
                                        || coords[2] > coords[5]
                                    {
                                        continue;
                                    }
                                    let clean = id.trim_start_matches("minecraft:").to_owned();
                                    let x = (coords[0] + coords[3]) / 2;
                                    let z = (coords[2] + coords[5]) / 2;
                                    if !structure_seen.insert((clean.clone(), x, z)) {
                                        continue;
                                    }
                                    if structures.len() < SEARCH_STRUCTURE_CAP {
                                        structures.push(json!({
                                            "name": clean,
                                            "x": x,
                                            "y": (coords[1] + coords[4]) / 2,
                                            "z": z,
                                        }));
                                    }
                                }
                            }
                        }
                        if !want("biomes") && !want("ores") {
                            continue;
                        }
                        let Some(sections) = root.get("sections").and_then(Value::as_array) else {
                            continue;
                        };
                        let mut chunk_biomes: std::collections::HashSet<String> =
                            Default::default();
                        for section in sections {
                            let sy = section.get("Y").and_then(Value::as_i64).unwrap_or(0);
                            if want("biomes") && biomes.len() < SEARCH_BIOME_CAP {
                                if let Some(palette) = section
                                    .get("biomes")
                                    .and_then(|item| item.get("palette"))
                                    .and_then(Value::as_array)
                                {
                                    for entry in palette {
                                        let Some(raw) = entry.as_str() else {
                                            continue;
                                        };
                                        if !chunk_biomes.insert(raw.to_owned()) {
                                            continue;
                                        }
                                        let clean = raw.trim_start_matches("minecraft:").to_owned();
                                        if biome_seen.insert(clean.clone())
                                            && !biomes.iter().any(|item| {
                                                item.get("name").and_then(Value::as_str)
                                                    == Some(clean.as_str())
                                            })
                                        {
                                            biomes.push(json!({
                                                "name": clean,
                                                "x": base_x,
                                                "y": sy * 16 + 8,
                                                "z": base_z,
                                            }));
                                        }
                                    }
                                }
                            }
                            if want("ores") && ores.len() < SEARCH_ORE_CAP {
                                let Some(blocks) = SectionBlocks::parse(section) else {
                                    continue;
                                };
                                if blocks.air_only {
                                    continue;
                                }
                                let has_ore = blocks
                                    .palette
                                    .iter()
                                    .any(|raw| is_ore_name(raw.trim_start_matches("minecraft:")));
                                if !has_ore {
                                    continue;
                                }
                                let mut section_counts: HashMap<String, usize> = HashMap::new();
                                for y in 0..16usize {
                                    for z in 0..16usize {
                                        for x in 0..16usize {
                                            let Some((raw, _)) = blocks.block_full(x, y, z) else {
                                                continue;
                                            };
                                            let base = raw.trim_start_matches("minecraft:");
                                            if !is_ore_name(base) {
                                                continue;
                                            }
                                            let count =
                                                section_counts.entry(base.to_owned()).or_insert(0);
                                            if *count >= ORE_SECTION_HITS {
                                                continue;
                                            }
                                            *count += 1;
                                            if ores.len() >= SEARCH_ORE_CAP {
                                                break 'rings;
                                            }
                                            ores.push(json!({
                                                "name": base,
                                                "x": i64::from(cx) * 16 + x as i64,
                                                "y": sy * 16 + y as i64,
                                                "z": i64::from(cz) * 16 + z as i64,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(json!({
        "structures": structures,
        "biomes": biomes,
        "ores": ores,
        "scanned": chunk_count,
        "spawn": { "x": spawn_x, "z": spawn_z },
    }))
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|char| {
            if char.is_alphanumeric() || char == '-' || char == '_' || char == '.' {
                char
            } else {
                '_'
            }
        })
        .collect()
}

fn marker_file(instance: &str, world_folder: &str) -> PathBuf {
    super::state::launcher_roots()
        .cache_home
        .join("world_markers")
        .join(format!(
            "{}__{}.json",
            safe_component(instance),
            safe_component(world_folder)
        ))
}

/// метки мира: пользовательские точки (хранятся в кэше лаунчера) плюс
/// позиция игрока и кровать из playerdata (для телепорта в игре)
pub fn world_markers_get(request: &Value) -> Result<Value, String> {
    let (instance_path, world, world_folder) = request_world(request)?;
    let name = request
        .get("instance")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let markers = fs::read_to_string(marker_file(name, &world_folder))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("markers").cloned())
        .unwrap_or_else(|| json!([]));
    let mut player = Value::Null;
    let mut bed = Value::Null;
    if let Ok(entries) = fs::read_dir(world.join("playerdata")) {
        if let Some(path) = entries.flatten().map(|entry| entry.path()).min() {
            if let Ok(raw) = fs::read(&path) {
                if let Ok(root) = decode_nbt_bytes(&raw) {
                    let pos: Vec<f64> = root
                        .get("Pos")
                        .and_then(Value::as_array)
                        .map(|items| items.iter().filter_map(Value::as_f64).collect())
                        .unwrap_or_default();
                    if pos.len() == 3 {
                        player = json!({
                            "x": pos[0] as i64,
                            "y": pos[1] as i64,
                            "z": pos[2] as i64,
                        });
                    }
                    let spawn_x = root.get("SpawnX").and_then(Value::as_i64);
                    let spawn_y = root.get("SpawnY").and_then(Value::as_i64);
                    let spawn_z = root.get("SpawnZ").and_then(Value::as_i64);
                    if let (Some(sx), Some(sy), Some(sz)) = (spawn_x, spawn_y, spawn_z) {
                        bed = json!({ "x": sx, "y": sy, "z": sz });
                    }
                }
            }
        }
    }
    Ok(json!({
        "markers": markers,
        "player": player,
        "bed": bed,
        "world": world_folder,
        "_instance_dir": instance_path.to_string_lossy(),
    }))
}

pub fn world_markers_set(request: &Value) -> Result<Value, String> {
    let (_instance_path, _world, world_folder) = request_world(request)?;
    let name = request
        .get("instance")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let markers = request.get("markers").cloned().unwrap_or_else(|| json!([]));
    let count = markers.as_array().map(|items| items.len()).unwrap_or(0);
    let path = marker_file(name, &world_folder);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Не удалось создать каталог меток: {error}"))?;
    }
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({ "markers": markers }))
            .map_err(|error| format!("Метки не сериализуются: {error}"))?,
    )
    .map_err(|error| format!("Метки не сохраняются: {error}"))?;
    Ok(json!({ "saved": true, "count": count }))
}
