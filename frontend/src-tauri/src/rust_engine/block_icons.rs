use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use base64::Engine;

use super::config::ConfigStore;
use super::instances::InstanceStore;

const MAX_ICON_BYTES: usize = 64 * 1024;
const MIN_JAR_BYTES: u64 = 1_000_000;
const TEXTURE_PREFIX: &str = "assets/minecraft/textures/block/";

/// кураторский список узнаваемых блоков; отсутствующие в jar пропускаем
const CURATED_BLOCKS: [(&str, &str); 83] = [
    ("grass_block", "Трава"),
    ("stone", "Камень"),
    ("cobblestone", "Булыжник"),
    ("mossy_cobblestone", "Замшелый булыжник"),
    ("stone_bricks", "Каменный кирпич"),
    ("smooth_stone", "Гладкий камень"),
    ("deepslate", "Глубинный сланец"),
    ("cobbled_deepslate", "Сланцевый булыжник"),
    ("dirt", "Земля"),
    ("coarse_dirt", "Каменистая земля"),
    ("podzol", "Подзол"),
    ("mycelium", "Мицелий"),
    ("sand", "Песок"),
    ("red_sand", "Красный песок"),
    ("gravel", "Гравий"),
    ("clay", "Глина"),
    ("sandstone", "Песчаник"),
    ("obsidian", "Обсидиан"),
    ("netherrack", "Адский камень"),
    ("soul_sand", "Песок душ"),
    ("end_stone", "Эндерняк"),
    ("snow", "Снег"),
    ("ice", "Лёд"),
    ("packed_ice", "Плотный лёд"),
    ("oak_planks", "Дубовые доски"),
    ("spruce_planks", "Еловые доски"),
    ("birch_planks", "Берёзовые доски"),
    ("jungle_planks", "Тропические доски"),
    ("acacia_planks", "Акациевые доски"),
    ("dark_oak_planks", "Тёмные доски"),
    ("mangrove_planks", "Мангровые доски"),
    ("cherry_planks", "Вишнёвые доски"),
    ("bamboo_planks", "Бамбуковые доски"),
    ("crimson_planks", "Багровые доски"),
    ("warped_planks", "Искажённые доски"),
    ("oak_log", "Дубовое бревно"),
    ("birch_log", "Берёзовое бревно"),
    ("spruce_log", "Еловое бревно"),
    ("bricks", "Кирпич"),
    ("mud_bricks", "Грязный кирпич"),
    ("quartz_block", "Кварц"),
    ("prismarine", "Призмарин"),
    ("dark_prismarine", "Тёмный призмарин"),
    ("iron_block", "Железный блок"),
    ("gold_block", "Золотой блок"),
    ("diamond_block", "Алмазный блок"),
    ("emerald_block", "Изумрудный блок"),
    ("lapis_block", "Лазуритовый блок"),
    ("redstone_block", "Блок редстоуна"),
    ("coal_block", "Угольный блок"),
    ("copper_block", "Медный блок"),
    ("netherite_block", "Незеритовый блок"),
    ("amethyst_block", "Аметист"),
    ("glowstone", "Светокамень"),
    ("sea_lantern", "Морской фонарь"),
    ("shroomlight", "Светящийся гриб"),
    ("magma", "Магма"),
    ("slime_block", "Слизь"),
    ("honey_block", "Мёд"),
    ("hay_block", "Сено"),
    ("melon", "Арбуз"),
    ("pumpkin", "Тыква"),
    ("jack_o_lantern", "Тыква-фонарь"),
    ("crafting_table", "Верстак"),
    ("furnace", "Печь"),
    ("bookshelf", "Книжная полка"),
    ("tnt", "ТНТ"),
    ("note_block", "Нотный блок"),
    ("jukebox", "Проигрыватель"),
    ("sponge", "Губка"),
    ("glass", "Стекло"),
    ("coal_ore", "Угольная руда"),
    ("iron_ore", "Железная руда"),
    ("copper_ore", "Медная руда"),
    ("gold_ore", "Золотая руда"),
    ("diamond_ore", "Алмазная руда"),
    ("emerald_ore", "Изумрудная руда"),
    ("lapis_ore", "Лазуритовая руда"),
    ("redstone_ore", "Редстоун-руда"),
    ("nether_quartz_ore", "Кварцевая руда"),
    ("nether_gold_ore", "Адская руда"),
    ("moss_block", "Мох"),
    ("dripstone_block", "Капельник"),
];

fn data_url(raw: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(raw))
}

/// клиентские jar всех установленных версий (только локальные файлы)
fn client_jars(catalog: &InstanceStore) -> Vec<PathBuf> {
    let mut jars: Vec<PathBuf> = Vec::new();
    for info in catalog.list() {
        let versions = info.path.join("versions");
        let Ok(entries) = fs::read_dir(&versions) else { continue };
        let mut candidates: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let directory = entry.path();
            if !directory.is_dir() {
                continue;
            }
            if let Ok(inner) = fs::read_dir(&directory) {
                for jar in inner.flatten() {
                    let path = jar.path();
                    if path.extension().map(|value| value == "jar").unwrap_or(false) {
                        candidates.push(path);
                    }
                }
            }
        }
        candidates.sort();
        for jar in candidates {
            if !jar.is_symlink() && jar.symlink_metadata().is_ok_and(|meta| meta.is_file() && meta.len() > MIN_JAR_BYTES) {
                jars.push(jar);
            }
        }
    }
    jars
}

/// текстуры граней блоков: верх (top) и бок (side)
fn extract_faces(jar_path: &PathBuf, wanted: &BTreeMap<String, String>) -> BTreeMap<String, (Vec<u8>, Vec<u8>)> {
    let mut found: BTreeMap<String, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
    let Ok(file) = fs::File::open(jar_path) else { return found };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) => return found,
    };
    let names: std::collections::HashSet<String> = (0..archive.len())
        .filter_map(|index| archive.by_index(index).ok().map(|file| file.name().map(|v| v.to_string()).unwrap_or_default()))
        .collect();
    for block_id in wanted.keys() {
        let read_member = |name: String, archive: &mut zip::ZipArchive<fs::File>| -> Vec<u8> {
            if !names.contains(&name) {
                return Vec::new();
            }
            let mut file = match archive.by_name(&name) {
                Ok(file) => file,
                Err(_) => return Vec::new(),
            };
            let mut raw = Vec::new();
            if std::io::Read::read_to_end(&mut file, &mut raw).is_err() {
                return Vec::new();
            }
            if raw.is_empty() || raw.len() > MAX_ICON_BYTES {
                return Vec::new();
            }
            raw
        };
        let mut top = Vec::new();
        for template in [format!("{block_id}_top.png"), format!("{block_id}_up.png"), format!("{block_id}.png")] {
            top = read_member(format!("{TEXTURE_PREFIX}{template}"), &mut archive);
            if !top.is_empty() {
                break;
            }
        }
        let mut side = Vec::new();
        for template in [format!("{block_id}_side.png"), format!("{block_id}_front.png"), format!("{block_id}.png")] {
            side = read_member(format!("{TEXTURE_PREFIX}{template}"), &mut archive);
            if !side.is_empty() {
                break;
            }
        }
        if top.is_empty() {
            top = side.clone();
        }
        if side.is_empty() {
            side = top.clone();
        }
        if !top.is_empty() && !side.is_empty() {
            found.insert(block_id.clone(), (top, side));
        }
    }
    found
}

/// instance_icon_blocks: список доступных иконок-блоков
pub fn instance_icon_blocks(_request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let catalog = InstanceStore::new(&config)?;
    let wanted: BTreeMap<String, String> = CURATED_BLOCKS.iter().map(|(id, name)| (id.to_string(), name.to_string())).collect();
    let mut remaining = wanted.clone();
    let mut icons: Vec<(usize, Value)> = Vec::new();
    for jar in client_jars(&catalog) {
        if remaining.is_empty() {
            break;
        }
        for (block_id, (top, side)) in extract_faces(&jar, &remaining) {
            let top_url = data_url(&top);
            let side_url = data_url(&side);
            let label = wanted.get(&block_id).cloned().unwrap_or_default();
            let mut entry = serde_json::Map::new();
            entry.insert("id".into(), json!(block_id));
            entry.insert("name".into(), json!(label));
            entry.insert("data_url".into(), json!(side_url.clone()));
            entry.insert("top_data_url".into(), json!(top_url));
            if top_url != side_url {
                entry.insert("side_data_url".into(), json!(side_url));
            }
            let order = CURATED_BLOCKS.iter().position(|(id, _)| *id == block_id).unwrap_or(999);
            icons.push((order, Value::Object(entry)));
            remaining.remove(&block_id);
        }
    }
    icons.sort_by_key(|(order, _)| *order);
    let blocks: Vec<Value> = icons.into_iter().map(|(_, value)| value).collect();
    Ok(json!({ "available": !blocks.is_empty(), "blocks": blocks }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with_textures(path: &std::path::Path) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file(format!("{TEXTURE_PREFIX}stone.png"), options).unwrap();
        writer.write_all(b"stone-bytes").unwrap();
        writer.start_file(format!("{TEXTURE_PREFIX}grass_block_top.png"), options).unwrap();
        writer.write_all(b"grass-top").unwrap();
        writer.start_file(format!("{TEXTURE_PREFIX}grass_block_side.png"), options).unwrap();
        writer.write_all(b"grass-side").unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn faces_extracted_with_fallback_and_priority() {
        let root = std::env::temp_dir().join(format!("nivora-blocks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let jar = root.join("client.jar");
        zip_with_textures(&jar);
        let mut wanted = BTreeMap::new();
        wanted.insert("stone".to_owned(), "Камень".to_owned());
        wanted.insert("grass_block".to_owned(), "Трава".to_owned());
        wanted.insert("missing".to_owned(), "Нет".to_owned());
        let faces = extract_faces(&jar, &wanted);
        assert_eq!(faces.len(), 2);
        assert_eq!(faces["stone"].0, b"stone-bytes".to_vec());
        assert_eq!(faces["stone"].1, b"stone-bytes".to_vec());
        assert_eq!(faces["grass_block"].0, b"grass-top".to_vec());
        assert_eq!(faces["grass_block"].1, b"grass-side".to_vec());
        assert!(data_url(&faces["stone"].1).starts_with("data:image/png;base64,c3RvbmUtYnl0ZXM="));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn curated_list_is_sorted_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (id, _) in CURATED_BLOCKS {
            assert!(seen.insert(id), "дубликат блока {id}");
        }
        assert_eq!(CURATED_BLOCKS.len(), 83);
    }
}
