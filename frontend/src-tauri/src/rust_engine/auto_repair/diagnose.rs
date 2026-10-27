//! формирует отчёт диагностики по игровой сессии

use super::*;

pub(crate) struct DiagnoseInput<'a> {
    pub lines: Vec<String>,
    pub exit_code: Option<i64>,
    pub instance_path: Option<&'a Path>,
    pub instance_settings: Map<String, Value>,
    pub effective_memory: String,
    pub loader: String,
    pub minecraft_version: String,
    pub error: String,
    pub since: f64,
    pub muted: Vec<String>,
    pub crash_report: Option<String>,
    pub loader_version: String,
    pub accounts: Vec<Value>,
    pub current_account_id: String,
}

pub(crate) fn diagnose(input: &DiagnoseInput) -> Map<String, Value> {
    let settings = &input.instance_settings;
    let loader = if input.loader.trim().is_empty() {
        "vanilla".to_owned()
    } else {
        input.loader.trim().to_lowercase()
    };
    let loader_title_text = loader_title(&loader);
    let loader_ids: Vec<String> = loader_dependency_ids(&loader).iter().map(|id| id.to_string()).collect();
    let mut log_lines: Vec<String> = input
        .lines
        .iter()
        .rev()
        .take(MAX_LOG_LINES)
        .rev()
        .map(|line| line.to_owned())
        .collect();
    if !input.error.is_empty() {
        log_lines.push(input.error.clone());
    }
    let report = match &input.crash_report {
        Some(text) => text.clone(),
        None => input
            .instance_path
            .map(|path| latest_crash_report(path, input.since))
            .unwrap_or_default(),
    };
    let text = log_lines.join("\n");
    let combined = if report.is_empty() { text.clone() } else { format!("{text}\n{report}") };
    let mods = if loader != "vanilla" {
        input.instance_path.map(scan_mods_full).unwrap_or_default()
    } else {
        Vec::new()
    };
    let enabled_mods: Vec<&ModEntry> = mods.iter().filter(|mod_entry| mod_entry.enabled).collect();
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (index, mod_entry) in mods.iter().enumerate() {
        if mod_entry.enabled && !mod_entry.mod_id.is_empty() {
            by_id.insert(mod_entry.mod_id.clone(), index); // при дублях остаётся последняя запись
        }
    }
    let mut sink = IssueSink::default();

    let dependency_id_of = |captures: &regex::Captures| -> String {
        let value = captures
            .get(5)
            .or_else(|| captures.get(6))
            .or_else(|| captures.get(7))
            .or_else(|| captures.get(4))
            .map(|m| m.as_str())
            .unwrap_or_default();
        value.trim().to_lowercase()
    };

    let mut missing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut requesters_wrong_version: Vec<(String, String, String)> = Vec::new();
    let mut wrong_game_version: Vec<String> = Vec::new();
    let mut java_required: i64 = 0;
    // моды, которым нужен более новый загрузчик: (requester, минимум, строка лога)
    let mut loader_requirements: Vec<(String, String, String)> = Vec::new();
    for captures in patterns().fabric_requires.captures_iter(&combined) {
        let requester = captures.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let dependency = dependency_id_of(&captures);
        if dependency == "java" || is_loader_dependency_id(&dependency) {
            // чужой загрузчик (quilt_loader на Fabric) ловит проверка wrong_loader_mods
            continue;
        }
        if dependency == "minecraft" {
            wrong_game_version.push(requester);
            continue;
        }
        missing.entry(dependency).or_default().push(requester);
    }
    for captures in patterns().fabric_wrong_version.captures_iter(&combined) {
        let requester = captures.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let dependency = dependency_id_of(&captures);
        if dependency == "java" {
            java_required = java_required.max(first_int(captures.get(3).map(|m| m.as_str()).unwrap_or_default()));
        } else if dependency == "minecraft" {
            wrong_game_version.push(requester);
        } else if loader_ids.contains(&dependency) {
            loader_requirements.push((
                requester,
                first_version(captures.get(3).map(|m| m.as_str()).unwrap_or_default()),
                captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default(),
            ));
        } else {
            requesters_wrong_version.push((
                requester,
                dependency,
                captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default(),
            ));
        }
    }
    for captures in patterns().forge_dependency.captures_iter(&combined) {
        let dependency = captures.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let requester = captures.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let expected = captures.get(3).map(|m| m.as_str()).unwrap_or_default();
        let actual = captures.get(4).map(|m| m.as_str()).unwrap_or_default();
        if dependency == "java" {
            java_required = java_required.max(first_int(expected));
        } else if dependency == "minecraft" {
            wrong_game_version.push(requester);
        } else if loader_ids.contains(&dependency) {
            loader_requirements.push((
                requester,
                first_version(expected),
                captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default(),
            ));
        } else if actual.to_lowercase().contains("missing") || actual.trim().is_empty() {
            missing.entry(dependency).or_default().push(requester);
        } else {
            requesters_wrong_version.push((
                requester,
                dependency,
                captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default(),
            ));
        }
    }
    for captures in patterns().fml_requires.captures_iter(&combined) {
        let requester = captures.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let dependency = captures.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let minimum = captures.get(3).map(|m| m.as_str()).unwrap_or_default();
        let state = captures.get(5).map(|m| m.as_str()).unwrap_or_default();
        if dependency == "minecraft" {
            wrong_game_version.push(requester);
        } else if loader_ids.contains(&dependency) {
            loader_requirements.push((
                requester,
                first_version(minimum),
                captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default(),
            ));
        } else if dependency == "java" || is_loader_dependency_id(&dependency) {
            continue;
        } else if state.to_lowercase() == "not installed" {
            missing.entry(dependency).or_default().push(requester);
        } else {
            requesters_wrong_version.push((
                requester,
                dependency,
                captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default(),
            ));
        }
    }
    // статический анализ: обязательные зависимости, которых нет в папке mods
    let installed_ids: BTreeSet<String> = mods
        .iter()
        .filter(|mod_entry| mod_entry.enabled && !mod_entry.mod_id.is_empty())
        .map(|mod_entry| mod_entry.mod_id.clone())
        .collect();
    let provided = provided_ids(&enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>());
    for mod_entry in mods.iter().filter(|entry| entry.enabled) {
        for dependency in &mod_entry.dependencies {
            let dependency_id = dependency
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            if dependency_id.is_empty()
                || dependency.get("optional").and_then(Value::as_bool) == Some(true)
                || NOISE_IDS.contains(&dependency_id.as_str())
                || dependency_id == "java"
                || dependency_id == "minecraft"
                || is_loader_dependency_id(&dependency_id)
                || installed_ids.contains(&dependency_id)
                || provided.contains(&dependency_id)
                // подмодули Fabric/Quilt API лежат jar-in-jar внутри самого API
                || (dependency_id.starts_with("fabric-") || dependency_id.starts_with("fabric_")) && dependency_id != "fabric-api"
                || dependency_id.starts_with("quilt_")
                || dependency_id.starts_with("quilted_")
                || dependency_id.starts_with("forge")
                || dependency_id.starts_with("neoforge")
                || dependency_id.starts_with("java")
            {
                continue;
            }
            // статикой ловим только то, что лог тоже назвал, или когда лога нет вовсе
            if missing.contains_key(&dependency_id) || combined.trim().is_empty() {
                let requester_id = mod_entry.mod_id.clone();
                missing.entry(dependency_id).or_default().push(requester_id);
            }
        }
    }
    if !missing.is_empty() {
        let names: Vec<String> = missing.keys().take(8).cloned().collect();
        let mut requesters = unique(
            missing
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<String>>(),
        );
        requesters.sort();
        let requester_files = files_for_ids(&requesters, &by_id, &mods);
        let mut detail = format!("Модам не хватает обязательных библиотек: {}", names.join(", "));
        if !requesters.is_empty() {
            detail.push_str(&format!(" (нужны для: {})", requesters.iter().take(6).cloned().collect::<Vec<_>>().join(", ")));
        }
        detail.push('.');
        let mut evidence_needles: Vec<&str> = names.iter().map(String::as_str).collect();
        evidence_needles.push("missing");
        sink.add(
            "missing_dependencies",
            &detail,
            vec![
                install_fix(&missing.keys().cloned().collect::<Vec<_>>()),
                disable_fix(&requester_files, "Отключить моды, которым не хватает зависимостей"),
            ],
            evidence_lines(&combined, &evidence_needles),
            "",
            "",
        );
    }
    if !loader_requirements.is_empty() && !loader_ids.is_empty() {
        let mut minimum = String::new();
        for (_, value, _) in &loader_requirements {
            if !value.is_empty() && version_key(value) > version_key(&minimum) {
                minimum = value.clone();
            }
        }
        let requesters = unique(loader_requirements.iter().map(|(requester, _, _)| requester.clone()).collect::<Vec<_>>());
        let detail = format!(
            "Модам нужен {loader_title_text} {}{} (требуют: {}).",
            if minimum.is_empty() { "новее".to_owned() } else { format!("{minimum} или новее") },
            if !input.loader_version.is_empty() { format!(", а установлен {}", input.loader_version) } else { String::new() },
            requesters.iter().take(6).cloned().collect::<Vec<_>>().join(", "),
        );
        sink.add(
            "loader_outdated",
            &detail,
            vec![
                update_loader_fix(&loader, &minimum, &input.loader_version, &input.minecraft_version, false),
                disable_fix(
                    &files_for_ids(&requesters, &by_id, &mods),
                    &format!("Отключить моды, которым нужен новый {loader_title_text}"),
                ),
            ],
            loader_requirements.iter().map(|(_, _, line)| line.clone()).collect(),
            "",
            "",
        );
    }
    if !requesters_wrong_version.is_empty() {
        let files = files_for_ids(
            &requesters_wrong_version.iter().map(|(requester, _, _)| requester.clone()).collect::<Vec<_>>(),
            &by_id,
            &mods,
        );
        let pairs: Vec<String> = requesters_wrong_version
            .iter()
            .take(5)
            .map(|(requester, dependency, _)| format!("{requester} → {dependency}"))
            .collect();
        sink.add(
            "incompatible_mods",
            &format!("Моды требуют другую версию зависимости: {}.", pairs.join("; ")),
            vec![disable_fix(&files, "Отключить моды с неподходящими версиями зависимостей")],
            requesters_wrong_version.iter().map(|(_, _, line)| line.clone()).collect(),
            "",
            "",
        );
    }
    if !wrong_game_version.is_empty() {
        let files = files_for_ids(&wrong_game_version, &by_id, &mods);
        let suffix = if input.minecraft_version.is_empty() {
            String::new()
        } else {
            format!(" (у экземпляра {})", input.minecraft_version)
        };
        sink.add(
            "incompatible_mods",
            &format!("Часть модов собрана для другой версии Minecraft{suffix}."),
            vec![disable_fix(&files, "Отключить моды для другой версии Minecraft")],
            evidence_lines(&combined, &["minecraft"]),
            "",
            "",
        );
    }
    let mut incompatible_files: Vec<String> = Vec::new();
    let mut incompatible_lines: Vec<String> = Vec::new();
    for captures in patterns().fabric_incompatible.captures_iter(&combined) {
        incompatible_files.extend(files_for_ids(
            &[captures.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default()],
            &by_id,
            &mods,
        ));
        incompatible_lines.push(captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default());
    }
    for pattern in [&patterns().module_conflict, &patterns().module_reads] {
        for captures in pattern.captures_iter(&combined) {
            let names: Vec<String> = captures
                .iter()
                .skip(1)
                .filter_map(|group| group.map(|m| m.as_str().to_owned()))
                .collect();
            incompatible_files.extend(files_for_names(&names, &enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>()));
            incompatible_lines.push(captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default());
        }
    }
    if !incompatible_lines.is_empty() {
        sink.add(
            "incompatible_mods",
            "Загрузчик сообщил о конфликте модов между собой.",
            vec![disable_fix(&incompatible_files, "Отключить конфликтующие моды")],
            incompatible_lines,
            "",
            "",
        );
    }
    let resolution_failed = Regex::new(r"(?i)some of your mods are incompatible|incompatible mod set|ModResolutionException|Mod resolution failed")
        .unwrap()
        .is_match(&combined);
    if resolution_failed && !sink.has("incompatible_mods") && !sink.has("missing_dependencies") {
        let suspects = crash_report_suspects(&report, &enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>());
        sink.add(
            "incompatible_mods",
            "Загрузчик не смог собрать набор модов, но не назвал конкретного виновника.",
            vec![
                disable_fix(&suspects, "Отключить подозреваемые моды"),
                recent_fix(&enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>(), input.instance_path),
            ],
            evidence_lines(&combined, &["incompatible", "resolution"]),
            "",
            "",
        );
    }

    let wrong_loader_files: Vec<String> = mods
        .iter()
        .filter(|mod_entry| mod_entry.enabled && wrong_loader(mod_entry, &loader, by_id.contains_key("connector")))
        .map(|mod_entry| mod_entry.file.clone())
        .collect();
    if !wrong_loader_files.is_empty() {
        sink.add(
            "wrong_loader_mods",
            &format!(
                "В папке mods есть моды, рассчитанные не на {}: они не загрузятся или уронят игру.",
                loader.capitalize_first()
            ),
            vec![disable_fix(
                &wrong_loader_files,
                &format!("Отключить моды не для {}", loader.capitalize_first()),
            )],
            wrong_loader_files.iter().take(MAX_EVIDENCE).cloned().collect(),
            "",
            "",
        );
    }

    let mut duplicates: Vec<String> = Vec::new();
    let mut seen_ids: BTreeMap<String, Vec<&ModEntry>> = BTreeMap::new();
    for mod_entry in mods.iter().filter(|entry| entry.enabled && !entry.mod_id.is_empty()) {
        seen_ids.entry(mod_entry.mod_id.clone()).or_default().push(mod_entry);
    }
    let mut duplicate_lines: Vec<String> = Vec::new();
    for (mod_id, group) in &seen_ids {
        if group.len() > 1 {
            // оставляем самый свежий файл, остальные выключаем
            let mut sorted = group.clone();
            sorted.sort_by(|left, right| right.mtime.partial_cmp(&left.mtime).unwrap_or(std::cmp::Ordering::Equal));
            duplicates.extend(sorted[1..].iter().map(|entry| entry.file.clone()));
            duplicate_lines.push(format!(
                "{mod_id}: {}",
                sorted.iter().map(|entry| entry.file.clone()).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    for captures in patterns().forge_duplicate.captures_iter(&combined) {
        duplicate_lines.push(captures.get(0).map(|m| m.as_str().trim().to_owned()).unwrap_or_default());
    }
    if !duplicates.is_empty() {
        sink.add(
            "duplicate_mods",
            "Один и тот же мод лежит в папке mods несколько раз (разные версии).",
            vec![disable_fix(&duplicates, "Оставить самую новую копию, остальные отключить")],
            duplicate_lines,
            "",
            "",
        );
    }

    let mut mixin_mods: Vec<String> = Vec::new();
    for pattern in [&patterns().mixin_for_mod, &patterns().mixin_from_mod] {
        mixin_mods.extend(
            pattern
                .captures_iter(&combined)
                .filter_map(|captures| captures.get(1).map(|m| m.as_str().to_lowercase().to_owned())),
        );
    }
    let mut mixin_files = files_for_ids(&mixin_mods, &by_id, &mods);
    let mixin_word = Regex::new(r"(?i)mixin").unwrap().is_match(&combined);
    let mixin_error = Regex::new(
        r"(?i)MixinApplyError|MixinTransformerError|InvalidInjectionException|InjectionError|Mixin apply|Critical injection failure|MixinPrepareError",
    )
    .unwrap()
    .is_match(&combined);
    if mixin_word && mixin_error {
        let configs: BTreeSet<String> = patterns()
            .mixin_config
            .captures_iter(&combined)
            .filter_map(|captures| captures.get(1).map(|m| m.as_str().to_lowercase().to_owned()))
            .collect();
        for mod_entry in mods.iter().filter(|entry| entry.enabled) {
            if !configs.is_disjoint(&mod_entry.mixins) {
                mixin_files.push(mod_entry.file.clone());
            }
        }
        let mixin_files = unique(mixin_files);
        let culprit_suffix = if mixin_files.is_empty() {
            String::new()
        } else {
            format!(" Виновник: {}.", mixin_files.iter().take(4).cloned().collect::<Vec<_>>().join(", "))
        };
        sink.add(
            "mixin_failure",
            &format!(
                "Мод не смог внедрить свой код в игру (Mixin). Обычно это мод для другой версии игры или конфликт с другим модом.{culprit_suffix}"
            ),
            vec![
                disable_fix(&mixin_files, "Отключить мод с ошибкой Mixin"),
                recent_fix(&enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>(), input.instance_path),
            ],
            evidence_lines(&combined, &["mixin"]),
            "",
            "",
        );
    }

    let corrupted: Vec<String> = mods
        .iter()
        .filter(|mod_entry| mod_entry.enabled && mod_entry.corrupted)
        .map(|mod_entry| mod_entry.file.clone())
        .collect();
    if !corrupted.is_empty() {
        let mut evidence: Vec<String> = corrupted.iter().take(2).cloned().collect();
        evidence.extend(evidence_lines(&combined, &["zip"]));
        sink.add(
            "corrupted_mods",
            "Часть jar-файлов в папке mods не открывается как архив — вероятно, загрузка прервалась.",
            vec![disable_fix(&corrupted, "Отключить повреждённые файлы (потом скачайте их заново)")],
            evidence,
            "",
            "",
        );
    }

    let config_files = broken_configs(&combined, input.instance_path);
    let config_keywords = Regex::new(
        r"(?i)ParsingException|JsonSyntaxException|MalformedJsonException|Failed (?:loading|to load) config|Error loading config|Config(?:uration)? (?:file )?(?:is )?(?:corrupt|invalid|not correct)|TomlParsingException|ParseException",
    )
    .unwrap()
    .is_match(&combined);
    if !config_files.is_empty() && config_keywords {
        sink.add(
            "broken_config",
            "Мод не смог прочитать свой конфиг. После переноса файла мод создаст настройки по умолчанию.",
            vec![config_fix(&config_files)],
            evidence_lines(&combined, &["config"]),
            "",
            "",
        );
    }

    let total_kib = super::config::system_memory_kib();
    let memory_setting = if input.effective_memory.is_empty() {
        settings.get("memory").and_then(Value::as_str).unwrap_or("auto").to_owned()
    } else {
        input.effective_memory.clone()
    };
    let current_kib = super::config::parse_memory_kib(&memory_setting).unwrap_or_else(|_| super::config::recommended_memory_kib());
    if patterns().memory_too_high.is_match(&combined) {
        let target = (super::config::recommended_memory_kib().min(current_kib / 2).max(1024 * 1024) / (512 * 1024)) * 512 * 1024;
        let target = if target == 0 { 1024 * 1024 } else { target };
        if target < current_kib {
            sink.add(
                "memory_too_high",
                &format!(
                    "Java не смогла зарезервировать {} — системе не хватает свободной памяти.",
                    super::config::format_memory_kib(current_kib)
                ),
                vec![memory_fix(current_kib, target, "Уменьшить память экземпляра")],
                evidence_lines(&combined, &["memory", "heap", "reserve"]),
                "",
                "",
            );
        }
    }
    if patterns().oom.is_match(&combined) {
        // Потолок: не больше 3/4 доступной RAM (или 8 ГиБ, если объём неизвестен).
        let ceiling = if total_kib > 0 { total_kib * 3 / 4 } else { 8 * 1024 * 1024 };
        let desired = (current_kib + 2 * 1024 * 1024).max(current_kib * 3 / 2).max(4 * 1024 * 1024);
        let target = ceiling.min(desired) / (512 * 1024) * 512 * 1024;
        if target > current_kib {
            sink.add(
                "out_of_memory",
                &format!(
                    "Игре не хватило выделенной памяти ({}). Тяжёлым сборкам обычно нужно 4–8 ГБ.",
                    super::config::format_memory_kib(current_kib)
                ),
                vec![memory_fix(current_kib, target, "Увеличить память экземпляра")],
                evidence_lines(&combined, &["OutOfMemory", "heap"]),
                "",
                "",
            );
        } else {
            // Увеличивать некуда: память уже занимает почти всю доступную RAM.
            // Это всё равно проблема, поэтому сообщаем о ней с ручной инструкцией.
            sink.add(
                "out_of_memory",
                &format!(
                    "Игре не хватило памяти ({}), а свободной RAM в системе недостаточно ({}) — увеличить выделение нельзя.",
                    super::config::format_memory_kib(current_kib),
                    super::config::format_memory_kib(total_kib)
                ),
                Vec::new(),
                evidence_lines(&combined, &["OutOfMemory", "heap"]),
                "",
                "Закройте другие программы, уменьшите количество модов или добавьте оперативную память.",
            );
        }
    }

    for captures in patterns().class_file_version.captures_iter(&combined) {
        if combined.contains("UnsupportedClassVersion") || combined.contains("more recent version of the Java") {
            let version: i64 = captures.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            java_required = java_required.max(version - 44);
        }
    }
    for captures in patterns().requires_java.captures_iter(&combined) {
        let joined: String = captures
            .iter()
            .skip(1)
            .filter_map(|group| group.map(|m| m.as_str().to_owned()))
            .collect();
        java_required = java_required.max(first_int(&joined));
    }
    let java_path = settings.get("java_path").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    if java_required >= 8 || combined.contains("UnsupportedClassVersionError") {
        sink.add(
            "java_version",
            &format!("Игре или модам нужна Java {}, а запуск шёл на более старой.", if java_required > 0 { java_required.to_string() } else { "новее".to_owned() }),
            java_fixes(java_required, &java_path).into_iter().map(Some).collect(),
            evidence_lines(&combined, &["java", "class file version"]),
            "",
            &format!(
                "Если подходящей Java нет, установите Java {} (например, Eclipse Temurin) и выберите её в настройках экземпляра.",
                if java_required > 0 { java_required.to_string() } else { "новее".to_owned() }
            ),
        );
    } else if patterns().java_missing.is_match(&combined) {
        sink.add(
            "java_version",
            "Лаунчер не нашёл подходящей Java для запуска.",
            java_fixes(0, &java_path).into_iter().map(Some).collect(),
            evidence_lines(&combined, &["java"]),
            "Java не найдена",
            "",
        );
    }

    let custom_jvm = !settings
        .get("jvm_arguments")
        .and_then(Value::as_array)
        .map(|items| items.is_empty())
        .unwrap_or(true)
        || settings.get("jvm_preset").and_then(Value::as_str).map(|preset| preset != "none").unwrap_or(false);
    if patterns().jvm_arguments.is_match(&combined) && !sink.has("memory_too_high") {
        let mut fixes: Vec<Option<Value>> = Vec::new();
        if custom_jvm {
            fixes.push(Some(jvm_reset_fix(settings)));
        }
        if !custom_jvm && Regex::new(r"(?i)heap size").unwrap().is_match(&combined) {
            fixes.push(memory_fix(
                current_kib,
                super::config::recommended_memory_kib(),
                "Сбросить память на рекомендуемую",
            ));
        }
        sink.add(
            "jvm_arguments",
            "Java отказалась стартовать из-за параметров запуска (аргументы JVM или пресет GC).",
            fixes,
            evidence_lines(&combined, &["VM option", "Virtual Machine", "option"]),
            "",
            "",
        );
    }
    if patterns().jvm_crash.is_match(&combined) {
        let driver_crash = patterns().driver_frame.is_match(&combined);
        let mut fixes: Vec<Option<Value>> = Vec::new();
        if driver_crash {
            fixes.extend(graphics_fixes(settings, &enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>()).into_iter().map(Some));
        }
        if custom_jvm {
            fixes.push(Some(jvm_reset_fix(settings)));
        }
        if !java_path.is_empty() {
            // для старых версий игры нужна совместимая Java; оставляем возврат к автовыбору
            fixes.extend(
                java_fixes(0, &java_path)
                    .into_iter()
                    .filter(|fix| fix.pointer("/params/java_path").and_then(Value::as_str) == Some(""))
                    .map(Some),
            );
        }
        sink.add(
            "jvm_crash",
            &format!(
                "Упала сама Java, а не Minecraft{}",
                if driver_crash { " — сбой внутри графического драйвера." } else { ". Частые причины: кастомные аргументы JVM, сбойная сборка Java или драйвер." }
            ),
            fixes,
            evidence_lines(&combined, &["fatal error", "SIGSEGV", "ACCESS_VIOLATION", "hs_err"]),
            "",
            if driver_crash { "Если краш повторяется, обновите драйвер видеокарты и Java." } else { "" },
        );
    }

    if !loader_ids.is_empty() && patterns().loader_broken.is_match(&combined) {
        let loader_suffix = if input.loader_version.is_empty() {
            ".".to_owned()
        } else {
            format!(" (версия {}).", input.loader_version)
        };
        sink.add(
            "loader_broken",
            &format!("{loader_title_text} не смог стартовать: файлы загрузчика отсутствуют, повреждены или установлены не до конца{loader_suffix}"),
            vec![
                Some(repair_fix(&loader)),
                update_loader_fix(&loader, "", &input.loader_version, &input.minecraft_version, true),
            ],
            evidence_lines(&combined, &["main class", "ClassNotFound", "NoClassDefFound", "installer", "jar", "mainClass", "FindException", "system mod", "metadata"]),
            "",
            if patterns().non_latin_path.is_match(&combined) {
                "В пути к игре есть не-латинские символы. Если переустановка не поможет, обновите лаунчер или перенесите папку игры в путь из латинских букв (например, C:\\Games\\Nivora)."
            } else {
                ""
            },
        );
    }

    diagnose_account(&mut sink, &combined, &input.accounts, &input.current_account_id);
    diagnose_sandbox(
        &mut sink,
        &combined,
        &report,
        settings,
        &enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>(),
        input.instance_path,
    );

    let game_files_hit = patterns().game_files.is_match(&combined);
    let zip_in_libraries = patterns().corrupted_zip.is_match(&combined)
        && Regex::new(r"(?:libraries|versions)[/\\]").unwrap().is_match(&combined);
    if (game_files_hit || zip_in_libraries) && !sink.has("loader_broken") {
        sink.add(
            "game_files",
            "Не хватает или повреждены файлы версии, библиотек или модлоадера.",
            vec![Some(repair_fix(&loader))],
            evidence_lines(&combined, &["ClassNotFound", "main class", "lwjgl", "libraries", "download", "mismatch"]),
            "",
            "",
        );
    }

    if patterns().graphics.is_match(&combined) {
        sink.add(
            "graphics",
            "Minecraft не смог создать окно с OpenGL. Обычно помогает обновить драйвер видеокарты; лаунчер может попробовать обходные пути.",
            graphics_fixes(settings, &enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>()).into_iter().map(Some).collect(),
            evidence_lines(&combined, &["OpenGL", "GLFW", "GL", "window"]),
            "",
            "Установите свежий драйвер видеокарты с сайта NVIDIA/AMD/Intel; на ноутбуке убедитесь, что игра запускается на дискретной видеокарте.",
        );
    }

    if sink.issues.is_empty() && (input.exit_code.unwrap_or(0) != 0 || !input.error.is_empty()) {
        let enabled_owned = enabled_mods.iter().map(|entry| (*entry).clone()).collect::<Vec<_>>();
        let suspects = crash_report_suspects(&report, &enabled_owned);
        if !suspects.is_empty() {
            sink.add(
                "incompatible_mods",
                "Отчёт о краше указывает на конкретные моды.",
                vec![disable_fix(&suspects, "Отключить моды из отчёта о краше")],
                evidence_lines(&report, &["Suspected", "Mod File"]),
                "Краш вызвал мод",
                "",
            );
        }
        if let Some(recent) = recent_fix(&enabled_owned, input.instance_path) {
            let files: Vec<String> = recent
                .pointer("/params/files")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            sink.add(
                "recent_mods",
                "Явной причины в логе нет, но после последнего успешного запуска появились новые моды — частый источник крашей.",
                vec![Some(recent)],
                files[..MAX_EVIDENCE.min(files.len())].to_vec(),
                "",
                "",
            );
        }
        if sink.issues.is_empty() {
            sink.add(
                "unknown_crash",
                "Явных признаков в логе не нашлось. Можно проверить и восстановить файлы версии и загрузчика — моды, миры и настройки не затрагиваются.",
                vec![Some(repair_fix(&loader))],
                tail_evidence(&log_lines),
                "",
                "",
            );
        }
    }

    let muted_set: BTreeSet<String> = input.muted.iter().cloned().collect();
    // сохраняем ISSUE_ORDER; заглушенные проблемы записываем отдельно
    let ordered: Vec<Value> = ISSUE_ORDER
        .iter()
        .filter(|(kind, _)| !muted_set.contains(*kind))
        .filter_map(|(kind, _)| sink.issues.iter().find(|(existing, _)| existing == kind))
        .map(|(_, issue)| Value::Object(issue.clone()))
        .collect();
    let muted_issues: Vec<Value> = ISSUE_ORDER
        .iter()
        .filter(|(kind, _)| muted_set.contains(*kind) && sink.issues.iter().any(|(existing, _)| existing == kind))
        .map(|(kind, _)| json!(kind))
        .collect();
    let mut result = Map::new();
    result.insert("exit_code".to_owned(), json!(input.exit_code));
    result.insert("issues".to_owned(), Value::Array(ordered));
    result.insert("muted_issues".to_owned(), Value::Array(muted_issues));
    result
}

