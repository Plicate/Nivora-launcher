// разбирает и проверяет манифесты плагинов
use super::*;
use super::package::v2_package_path;

pub(super) fn parse_plugin_yml(text: &str) -> Result<Map<String, Value>, String> {
    if text.len() > PLUGIN_MAX_MANIFEST_BYTES {
        return Err("plugin.yml слишком большой".to_owned());
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut stack: Vec<Frame> = vec![Frame::root()];
    for (index, raw_line) in lines.iter().enumerate() {
        let number = index + 1;
        if raw_line.contains('\t') {
            return Err(format!("plugin.yml содержит tab на строке {number}"));
        }
        let line = strip_yaml_comment(raw_line).trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with("---") {
            continue;
        }
        let indent = (line.len() - line.trim_start_matches(' ').len()) as i64;
        let content = line.trim();
        while matches!(stack.last(), Some(frame) if indent <= frame.indent) {
            attach_frame(&mut stack);
        }
        if stack.is_empty() {
            return Err(format!("Некорректный отступ в plugin.yml на строке {number}"));
        }
        if let Some(item) = content.strip_prefix("- ") {
            match &mut stack.last_mut().expect("стек не пуст").node {
                YamlNode::List(items) => items.push(yaml_scalar(item.trim())?),
                YamlNode::Map(_) => {
                    return Err(format!("Список plugin.yml находится не под ключом на строке {number}"))
                }
            }
            continue;
        }
        let Some((raw_key, raw_value)) = content.split_once(':') else {
            return Err(format!("Некорректная строка plugin.yml: {number}"));
        };
        let key = raw_key.trim();
        if !YAML_KEY_RE.is_match(key) {
            return Err(format!("Небезопасный ключ plugin.yml: {key}"));
        }
        let value = raw_value.trim();
        let parent = &mut stack.last_mut().expect("стек не пуст").node;
        match parent {
            YamlNode::Map(fields) => {
                if !value.is_empty() {
                    fields.insert(key.to_owned(), yaml_scalar(value)?);
                } else {
                    // дочерний узел добавляем при снятии фрейма со стека, сохраняя порядок ключей
                    let child = if next_is_list(&lines, number) {
                        YamlNode::List(Vec::new())
                    } else {
                        YamlNode::Map(Map::new())
                    };
                    stack.push(Frame { indent, node: child, attach: Some(key.to_owned()) });
                }
            }
            YamlNode::List(_) => return Err(format!("Объект plugin.yml ожидается на строке {number}")),
        }
    }
    while stack.len() > 1 {
        attach_frame(&mut stack);
    }
    match stack.pop() {
        Some(Frame { node: YamlNode::Map(fields), .. }) => Ok(fields),
        _ => Ok(Map::new()),
    }
}

enum YamlNode {
    Map(Map<String, Value>),
    List(Vec<Value>),
}

struct Frame {
    indent: i64,
    node: YamlNode,
    attach: Option<String>,
}

impl Frame {
    fn root() -> Self {
        // корневой фрейм остаётся в стеке при любом отступе
        Self { indent: -1, node: YamlNode::Map(Map::new()), attach: None }
    }
}

fn attach_frame(stack: &mut Vec<Frame>) {
    let Some(frame) = stack.pop() else { return };
    let Some(key) = frame.attach else { return };
    let Some(parent) = stack.last_mut() else { return };
    if let YamlNode::Map(fields) = &mut parent.node {
        fields.insert(key, match frame.node {
            YamlNode::Map(fields) => Value::Object(fields),
            YamlNode::List(items) => Value::Array(items),
        });
    }
}

fn next_is_list(lines: &[&str], index: usize) -> bool {
    for raw in lines.iter().skip(index) {
        let value = strip_yaml_comment(raw).trim();
        if value.is_empty() {
            continue;
        }
        return value.starts_with('-');
    }
    false
}

fn strip_yaml_comment(value: &str) -> &str {
    let mut quote = '\0';
    for (index, character) in value.char_indices() {
        if character == '\'' || character == '"' {
            if quote == character {
                quote = '\0';
            } else if quote == '\0' {
                quote = character;
            }
        } else if character == '#' && quote == '\0' && (index == 0 || value[..index].ends_with(char::is_whitespace)) {
            return &value[..index];
        }
    }
    value
}

fn yaml_scalar(value: &str) -> Result<Value, String> {
    if value.starts_with('!') || value.starts_with('&') || value.starts_with('*') || value == "|" || value == ">" {
        return Err("plugin.yml не поддерживает YAML tags, anchors и blocks".to_owned());
    }
    if value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str(value).map_err(|_| "Некорректная строка plugin.yml".to_owned());
    }
    if value.starts_with('\'') && value.ends_with('\'') {
        let inner = if value.len() >= 2 { &value[1..value.len() - 1] } else { "" };
        return Ok(json!(inner.replace("''", "'")));
    }
    match value.to_lowercase().as_str() {
        "true" | "yes" => return Ok(json!(true)),
        "false" | "no" => return Ok(json!(false)),
        "null" | "~" => return Ok(Value::Null),
        _ => {}
    }
    if YAML_INT_RE.is_match(value) {
        // числа шире i64 сохраняем строками: формат манифеста их не поддерживает
        if let Ok(parsed) = value.parse::<i64>() {
            return Ok(json!(parsed));
        }
    }
    if YAML_FLOAT_RE.is_match(value) {
        if let Ok(parsed) = value.parse::<f64>() {
            return Ok(json!(parsed));
        }
    }
    if value.starts_with('[') && value.ends_with(']') {
        let parsed: Value =
            serde_json::from_str(value).map_err(|_| "Списки plugin.yml должны быть JSON-подобными".to_owned())?;
        if !parsed.is_array() {
            return Err("Некорректный список plugin.yml".to_owned());
        }
        return Ok(parsed);
    }
    Ok(json!(value))
}

pub(super) fn static_json_manifest(root: &Path) -> Result<Option<Value>, String> {
    let path = root.join(PLUGIN_V2_MANIFEST);
    if path_is_symlink(&path) || !path_is_file(&path) || path_size(&path).unwrap_or(0) > PLUGIN_MAX_MANIFEST_BYTES as u64 {
        return Ok(None);
    }
    let raw = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(fields) = parsed.as_object() else { return Ok(None) };
    let api_version = field_str(fields, "api_version", "1");
    if api_version.trim() != "1" {
        return Ok(None);
    }
    // при ошибке проверки прерываем выдачу списка
    Ok(Some(validate_manifest(&parsed)?))
}

pub(super) fn validate_manifest(value: &Value) -> Result<Value, String> {
    let Some(fields) = value.as_object() else {
        return Err("plugin.yml должен содержать объект".to_owned());
    };
    let plugin_id = field_str(fields, "id", "").trim().to_lowercase();
    let name = field_str(fields, "name", "").trim().to_owned();
    let version = field_str(fields, "version", "").trim().to_owned();
    let entrypoint = field_str(fields, "entrypoint", "").trim().replace('\\', "/");
    if !PLUGIN_ID_RE.is_match(&plugin_id) {
        return Err("plugin.yml содержит небезопасный id".to_owned());
    }
    if name.is_empty() || char_len(&name) > 120 || version.is_empty() {
        return Err("plugin.yml требует name и version".to_owned());
    }
    let entrypoint_parts: Vec<&str> = entrypoint.split('/').filter(|part| !part.is_empty()).collect();
    if entrypoint.is_empty()
        || entrypoint.starts_with('/')
        || entrypoint_parts.contains(&"..")
        || !entrypoint.ends_with(".js")
    {
        return Err("entrypoint должен быть относительным .js-файлом".to_owned());
    }
    let permissions = fields.get("permissions").cloned().unwrap_or_else(|| json!([]));
    let Some(permissions) = permissions.as_array() else {
        return Err("permissions в plugin.yml должен быть списком".to_owned());
    };
    let normalized_permissions: Vec<String> = permissions.iter().map(py_str).map(|item| item.trim().to_lowercase()).collect();
    if normalized_permissions.iter().any(|item| !PLUGIN_PERMISSION_NAMES.contains(item.as_str())) {
        return Err("plugin.yml содержит неизвестное permission".to_owned());
    }
    let network_hosts = fields.get("network_hosts").cloned().unwrap_or_else(|| json!([]));
    let Some(network_hosts) = network_hosts.as_array() else {
        return Err("network_hosts в plugin.yml должен быть списком".to_owned());
    };
    let hosts: Vec<String> = network_hosts
        .iter()
        .map(py_str)
        .filter(|item| !item.trim().is_empty())
        .map(|item| item.trim().to_lowercase().trim_end_matches('.').to_owned())
        .collect();
    for host in &hosts {
        if !HOSTNAME_RE.is_match(host) {
            return Err("network_hosts должен содержать только имена хостов".to_owned());
        }
        if host.parse::<std::net::IpAddr>().is_ok() {
            return Err("network_hosts не может содержать IP-адреса".to_owned());
        }
    }
    let mut sidebar_tab: Option<String> = None;
    if let Some(raw_tab) = fields.get("sidebar_tab") {
        let tab = raw_tab
            .as_object()
            .filter(|tab| tab.len() == 1 && tab.contains_key("label"))
            .ok_or_else(|| "sidebar_tab должен содержать только поле label".to_owned())?;
        let raw_label = tab.get("label").cloned().unwrap_or(Value::Null);
        let Some(label) = raw_label.as_str() else {
            return Err("sidebar_tab.label должен быть строкой".to_owned());
        };
        let label = label.trim();
        if label.is_empty() || char_len(label) > 40 || !py_printable(label) {
            return Err("sidebar_tab.label должен содержать от 1 до 40 печатных символов".to_owned());
        }
        if !normalized_permissions.iter().any(|item| item == "ui:compose") {
            return Err("sidebar_tab требует permission ui:compose".to_owned());
        }
        sidebar_tab = Some(label.to_owned());
    }

    let mut manifest = Map::new();
    manifest.insert("id".to_owned(), json!(plugin_id));
    manifest.insert("name".to_owned(), json!(name));
    manifest.insert("version".to_owned(), json!(truncate_chars(&version, 64)));
    manifest.insert("author".to_owned(), json!(truncate_chars(&field_str(fields, "author", "Local developer"), 120)));
    manifest.insert("description".to_owned(), json!(truncate_chars(&field_str(fields, "description", ""), 500)));
    manifest.insert("icon".to_owned(), json!(truncate_chars(&field_str(fields, "icon", "plug"), 64)));
    manifest.insert("category".to_owned(), json!(truncate_chars(&field_str(fields, "category", "Local"), 64)));
    manifest.insert("api_version".to_owned(), json!(truncate_chars(&field_str(fields, "api_version", "1"), 16)));
    manifest.insert("entrypoint".to_owned(), json!(entrypoint));
    manifest.insert("permissions".to_owned(), json!(dedup_strings(&normalized_permissions)));
    manifest.insert("network_hosts".to_owned(), json!(dedup_strings(&hosts)));
    manifest.insert(
        "featured".to_owned(),
        json!(py_truthy(fields.get("featured").unwrap_or(&Value::Null))),
    );
    if let Some(label) = sidebar_tab {
        manifest.insert("sidebar_tab".to_owned(), json!({ "label": label }));
    }
    Ok(Value::Object(manifest))
}

pub(super) fn validate_plugin_json_manifest(value: &Value) -> Result<Value, String> {
    if !value.is_object() {
        return Err("plugin.json must contain a JSON object".to_owned());
    }
    match value.get("api_version").and_then(Value::as_str) {
        Some("2") => validate_v2_manifest(value),
        Some("3") => validate_v3_manifest(value),
        _ => Err("plugin.json api_version must be \"2\" or \"3\"".to_owned()),
    }
}

pub(super) fn validate_v2_manifest(value: &Value) -> Result<Value, String> {
    validate_v2_or_v3(value, "2")
}

pub(super) fn validate_v3_manifest(value: &Value) -> Result<Value, String> {
    validate_v2_or_v3(value, "3")
}

fn validate_v2_or_v3(value: &Value, api_version: &str) -> Result<Value, String> {
    let version_label = api_version;
    let Some(fields) = value.as_object() else {
        return Err("plugin.json must contain a JSON object".to_owned());
    };
    let known_fields: BTreeSet<&str> = if api_version == "3" {
        V2_FIELDS.iter().chain(V3_EXTRA_FIELDS.iter()).copied().collect()
    } else {
        V2_FIELDS.iter().copied().collect()
    };
    let unknown: Vec<&str> = fields.keys().map(String::as_str).filter(|key| !known_fields.contains(key)).collect();
    if !unknown.is_empty() {
        let mut sorted = unknown;
        sorted.sort_unstable();
        return Err(format!("plugin.json contains unsupported field(s): {}", sorted.join(", ")));
    }
    for required in ["id", "name", "version", "api_version", "entrypoint", "permissions", "integrity"] {
        if !fields.contains_key(required) {
            return Err(format!("plugin.json is missing a required v{version_label} field"));
        }
    }
    for (field, limit) in [("id", 64), ("name", 120), ("version", 64), ("api_version", 16), ("entrypoint", 256)] {
        schema_text(
            fields.get(field).unwrap_or(&Value::Null),
            &format!("plugin.json.{field}"),
            limit,
            false,
        )?;
    }
    let entrypoint = field_str(fields, "entrypoint", "");
    v2_package_path(&entrypoint)?;
    if fields.get("api_version").and_then(Value::as_str) != Some(api_version) {
        return Err(format!("plugin.json api_version must be \"{api_version}\""));
    }
    let permission_names = if api_version == "3" { &*PLUGIN_V3_PERMISSION_NAMES } else { &*PLUGIN_V2_PERMISSION_NAMES };
    let Some(permissions) = fields.get("permissions").and_then(Value::as_array) else {
        return Err("plugin.json permissions must be an array of strings".to_owned());
    };
    if permissions.iter().any(|item| !item.is_string()) {
        return Err("plugin.json permissions must be an array of strings".to_owned());
    }
    if permissions.iter().any(|item| !item.as_str().map(|name| permission_names.contains(name)).unwrap_or(false)) {
        return Err("plugin.json contains an unknown permission".to_owned());
    }
    if let Some(network_hosts) = fields.get("network_hosts") {
        let hosts = network_hosts.as_array().cloned().unwrap_or_default();
        if !network_hosts.is_array() || hosts.iter().any(|item| !item.is_string()) {
            return Err("plugin.json network_hosts must be an array of strings".to_owned());
        }
    }
    for (field, limit) in [
        ("author", 120),
        ("description", 500),
        ("icon", 64),
        ("category", 64),
        ("update_channel", 64),
        ("sandbox_profile", 64),
    ] {
        if let Some(item) = fields.get(field) {
            schema_text(item, &format!("plugin.json.{field}"), limit, field == "description")?;
        }
    }
    schema_object(
        fields.get("integrity").unwrap_or(&Value::Null),
        "plugin.json.integrity",
        &["files"],
        &["files"],
    )?;
    if !fields
        .get("integrity")
        .and_then(|integrity| integrity.get("files"))
        .map(Value::is_object)
        .unwrap_or(false)
    {
        return Err("plugin.json integrity.files must be an object".to_owned());
    }
    if let Some(signature) = fields.get("signature") {
        if !signature.is_object() {
            return Err("plugin.json signature must be an object".to_owned());
        }
    }

    let validated = validate_manifest(value)?;
    // сравниваем идентификатор после нормализации регистра
    if value.get("id").and_then(Value::as_str) != validated.get("id").and_then(Value::as_str) {
        return Err("plugin.json id and entrypoint must use their normalized forms".to_owned());
    }
    if value.get("entrypoint").and_then(Value::as_str) != validated.get("entrypoint").and_then(Value::as_str) {
        return Err("plugin.json id and entrypoint must use their normalized forms".to_owned());
    }
    if let Some(config) = fields.get("config") {
        validate_v2_config(config)?;
    }
    if let Some(ui) = fields.get("ui") {
        validate_v2_ui(ui)?;
    }
    if let Some(i18n) = fields.get("i18n") {
        validate_v2_i18n(i18n)?;
    }
    if let Some(theme) = fields.get("theme") {
        validate_v2_theme(theme)?;
    }
    if let Some(actions) = fields.get("actions") {
        let actions = schema_array(actions, "plugin.json.actions", 256)?;
        validate_v2_named_records(
            actions,
            "plugin.json.actions[]",
            &["id", "label", "description", "icon", "confirmation"],
            &["id", "label"],
        )?;
    }
    if let Some(events) = fields.get("events") {
        let limit = if api_version == "3" { 128 } else { 64 };
        let events = schema_array(events, "plugin.json.events", limit)?;
        let known_events = if api_version == "3" { PLUGIN_V3_EVENTS } else { PLUGIN_V2_EVENTS };
        if events.iter().any(|item| !item.as_str().map(|name| known_events.contains(&name)).unwrap_or(false)) {
            return Err("plugin.json.events contains an unsupported launcher event".to_owned());
        }
    }
    if let Some(commands) = fields.get("commands") {
        validate_v2_commands(commands)?;
    }
    if let Some(shortcuts) = fields.get("shortcuts") {
        let shortcuts = schema_array(shortcuts, "plugin.json.shortcuts", 256)?;
        validate_v2_named_records(
            shortcuts,
            "plugin.json.shortcuts[]",
            &["id", "label", "default_key", "description"],
            &["id", "label", "default_key"],
        )?;
    }
    if api_version == "3" {
        for (field, maximum) in [
            ("capabilities", 64 * 1024usize),
            ("consent_scopes", 16 * 1024),
            ("extension_points", 16 * 1024),
            ("dependency_graph", 64 * 1024),
            ("compatibility_matrix", 64 * 1024),
            ("host_requirements", 32 * 1024),
        ] {
            if let Some(item) = fields.get(field) {
                validate_bounded_json(item, &format!("plugin.json.{field}"), maximum, 12)?;
            }
        }
    }

    let mut result = fields.clone();
    for (key, normalized) in validated.as_object().cloned().unwrap_or_default() {
        if matches!(key.as_str(), "id" | "name" | "version" | "author" | "description" | "icon" | "category" | "entrypoint" | "permissions" | "network_hosts") {
            result.insert(key, normalized);
        }
    }
    result.insert("api_version".to_owned(), json!(api_version));
    Ok(Value::Object(result))
}

fn schema_text<'a>(value: &'a Value, label: &str, maximum: usize, allow_empty: bool) -> Result<&'a str, String> {
    let invalid = || format!("{label} must be text of at most {maximum} characters");
    let Some(text) = value.as_str() else { return Err(invalid()) };
    // ограничение задано в кодовых точках, а не в байтах
    if !text_fits(text, maximum) || (!allow_empty && text.trim().is_empty()) || text.contains('\0') {
        return Err(invalid());
    }
    Ok(text)
}

fn schema_object<'a>(value: &'a Value, label: &str, allowed: &[&str], required: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let Some(fields) = value.as_object() else {
        return Err(format!("{label} has an invalid object shape"));
    };
    if !required.iter().all(|key| fields.contains_key(*key)) || fields.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{label} has an invalid object shape"));
    }
    Ok(fields)
}

fn schema_array<'a>(value: &'a Value, label: &str, maximum: usize) -> Result<&'a Vec<Value>, String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{label} must be an array of at most {maximum} items"));
    };
    if items.len() > maximum {
        return Err(format!("{label} must be an array of at most {maximum} items"));
    }
    Ok(items)
}

fn schema_identifier<'a>(value: &'a Value, label: &str) -> Result<&'a str, String> {
    let text = schema_text(value, label, 80, false)?;
    if !SCHEMA_IDENTIFIER_RE.is_match(text) {
        return Err(format!("{label} is not a safe identifier"));
    }
    Ok(text)
}

fn schema_optional_text(record: &Map<String, Value>, key: &str, label: &str, maximum: usize) -> Result<(), String> {
    if let Some(value) = record.get(key) {
        schema_text(value, &format!("{label}.{key}"), maximum, true)?;
    }
    Ok(())
}

fn schema_bool(record: &Map<String, Value>, key: &str, label: &str) -> Result<(), String> {
    if let Some(value) = record.get(key) {
        if !value.is_boolean() {
            return Err(format!("{label}.{key} must be a boolean"));
        }
    }
    Ok(())
}

fn schema_number(record: &Map<String, Value>, key: &str, label: &str) -> Result<(), String> {
    if let Some(value) = record.get(key) {
        if !finite_number(value) {
            return Err(format!("{label}.{key} must be a finite number"));
        }
    }
    Ok(())
}

fn finite_number(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_f64().map(|float| float.is_finite()).unwrap_or(true),
        _ => false,
    }
}

fn validate_bounded_json(value: &Value, label: &str, max_bytes: usize, max_depth: usize) -> Result<(), String> {
    fn walk(node: &Value, depth: usize, label: &str, max_depth: usize) -> Result<(), String> {
        if depth > max_depth {
            return Err(format!("{label} exceeds the supported nesting depth"));
        }
        match node {
            Value::Null | Value::String(_) | Value::Bool(_) => Ok(()),
            Value::Number(_) => {
                if !finite_number(node) {
                    return Err(format!("{label} contains a non-finite number"));
                }
                Ok(())
            }
            Value::Array(items) => {
                if items.len() > 1024 {
                    return Err(format!("{label} contains too many array items"));
                }
                for item in items {
                    walk(item, depth + 1, label, max_depth)?;
                }
                Ok(())
            }
            Value::Object(fields) => {
                if fields.len() > 1024 {
                    return Err(format!("{label} contains too many object fields"));
                }
                for (key, item) in fields {
                    if !text_fits(key, 256) {
                        return Err(format!("{label} contains an unsafe object key"));
                    }
                    walk(item, depth + 1, label, max_depth)?;
                }
                Ok(())
            }
        }
    }
    walk(value, 0, label, max_depth)?;
    let encoded = canonical_json_bytes(value, label)
        .map_err(|_| format!("{label} must be serializable as bounded JSON"))?;
    if encoded.len() > max_bytes {
        return Err(format!("{label} exceeds its bounded JSON size limit"));
    }
    Ok(())
}

fn validate_v2_option(value: &Value, label: &str) -> Result<(), String> {
    let option = schema_object(value, label, &["value", "label", "description", "disabled"], &["value", "label"])?;
    schema_text(option.get("value").unwrap_or(&Value::Null), &format!("{label}.value"), 256, true)?;
    schema_text(option.get("label").unwrap_or(&Value::Null), &format!("{label}.label"), 256, false)?;
    schema_optional_text(option, "description", label, 1000)?;
    schema_bool(option, "disabled", label)
}

fn validate_v2_config(value: &Value) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Err("plugin.json.config must be a bounded object".to_owned());
    };
    if fields.len() > 256 {
        return Err("plugin.json.config must be a bounded object".to_owned());
    }
    for (key, raw_field) in fields {
        if !CONFIG_KEY_RE.is_match(key) {
            return Err("plugin.json.config contains an unsafe key".to_owned());
        }
        let label = format!("plugin.json.config.{key}");
        let field = schema_object(
            raw_field,
            &label,
            &["type", "label", "description", "default", "minimum", "maximum", "options"],
            &["type", "label"],
        )?;
        let field_type = field.get("type").and_then(Value::as_str).unwrap_or_default();
        if !matches!(field_type, "string" | "number" | "boolean" | "select") {
            return Err(format!("{label}.type is unsupported"));
        }
        schema_text(field.get("label").unwrap_or(&Value::Null), &format!("{label}.label"), 256, false)?;
        schema_optional_text(field, "description", &label, 1000)?;
        if let Some(default) = field.get("default") {
            match field_type {
                "string" | "select" => {
                    if !default.is_string() {
                        return Err(format!("{label}.default must be text"));
                    }
                }
                "number" => {
                    if !finite_number(default) {
                        return Err(format!("{label}.default must be a finite number"));
                    }
                }
                "boolean" => {
                    if !default.is_boolean() {
                        return Err(format!("{label}.default must be a boolean"));
                    }
                }
                _ => {}
            }
        }
        for bound in ["minimum", "maximum"] {
            schema_number(field, bound, &label)?;
        }
        if let (Some(minimum), Some(maximum)) = (field.get("minimum"), field.get("maximum")) {
            if minimum.as_f64() > maximum.as_f64() {
                return Err(format!("{label}.minimum must not exceed maximum"));
            }
        }
        if let Some(options) = field.get("options") {
            let options = schema_array(options, &format!("{label}.options"), 128)?;
            let mut seen: HashSet<&str> = HashSet::new();
            for option in options {
                validate_v2_option(option, &format!("{label}.options[]"))?;
                if let Some(value) = option.get("value").and_then(Value::as_str) {
                    if !seen.insert(value) {
                        return Err(format!("{label}.options contains duplicate values"));
                    }
                }
            }
        }
    }
    Ok(())
}

struct UiBudget {
    nodes: usize,
    items: usize,
}

impl UiBudget {
    fn collect<'a>(&mut self, value: &'a Value, label: &str, maximum: usize) -> Result<&'a Vec<Value>, String> {
        let items = schema_array(value, label, maximum)?;
        self.items += items.len();
        if self.items > 512 {
            return Err("UI collection budget exceeded".to_owned());
        }
        Ok(items)
    }
}

fn validate_v2_ui(value: &Value) -> Result<(), String> {
    let ui = schema_object(value, "plugin.json.ui", &["views"], &["views"])?;
    let views = schema_array(ui.get("views").unwrap_or(&Value::Null), "plugin.json.ui.views", 128)?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut budget = UiBudget { nodes: 0, items: 0 };
    for raw_view in views {
        let view = schema_object(
            raw_view,
            "plugin.json.ui.views[]",
            &["id", "slot", "title", "description", "icon", "order", "root"],
            &["id", "slot", "title", "root"],
        )?;
        let view_id = schema_identifier(view.get("id").unwrap_or(&Value::Null), "plugin.json.ui.views[].id")?.to_owned();
        if !seen.insert(view_id.clone()) {
            return Err("plugin.json.ui.views contains duplicate ids".to_owned());
        }
        let slot = view.get("slot").and_then(Value::as_str).unwrap_or_default();
        if !view.get("slot").map(Value::is_string).unwrap_or(false) || !V2_UI_SLOTS.contains(&slot) {
            return Err("plugin.json.ui.views[].slot is unsupported".to_owned());
        }
        schema_text(view.get("title").unwrap_or(&Value::Null), "plugin.json.ui.views[].title", 160, false)?;
        schema_optional_text(view, "description", "plugin.json.ui.views[]", 2000)?;
        schema_optional_text(view, "icon", "plugin.json.ui.views[]", 64)?;
        schema_number(view, "order", "plugin.json.ui.views[]")?;
        validate_v2_ui_node(view.get("root").unwrap_or(&Value::Null), "plugin.json.ui.views[].root", 0, &mut budget)?;
    }
    Ok(())
}

fn validate_v2_ui_node(value: &Value, label: &str, depth: usize, budget: &mut UiBudget) -> Result<(), String> {
    budget.nodes += 1;
    if depth > 10 || budget.nodes > 512 {
        return Err("plugin.json UI tree exceeds its depth or node limit".to_owned());
    }
    if !value.is_object() || !value.get("type").map(Value::is_string).unwrap_or(false) {
        return Err(format!("{label} must be a plugin UI node"));
    }
    let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
    let (allowed, required): (&[&str], &[&str]) = match kind {
        "form" => (
            &["type", "id", "fields", "submitAction"],
            &["type", "id", "fields", "submitAction"],
        ),
        "tabs" => (&["type", "id", "tabs"], &["type", "id", "tabs"]),
        "accordion" => (&["type", "id", "sections"], &["type", "id", "sections"]),
        "table" => (&["type", "id", "columns", "rows"], &["type", "id", "columns", "rows"]),
        "chart" => (&["type", "id", "label", "values"], &["type", "id", "values"]),
        "include" => (&["type", "id", "fragment"], &["type", "id", "fragment"]),
        "text" => (&["type", "id", "text", "tone"], &["type", "text"]),
        "badge" => (&["type", "id", "label", "tone"], &["type", "label"]),
        "button" => (&["type", "id", "label", "action", "disabled", "tone"], &["type", "id", "label", "action"]),
        "input" => (
            &["type", "id", "label", "value", "placeholder", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "textarea" => (
            &["type", "id", "label", "value", "placeholder", "rows", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "number" => (
            &["type", "id", "label", "value", "min", "max", "step", "unit", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "slider" => (
            &["type", "id", "label", "value", "min", "max", "step", "unit", "config_key", "action", "onChange"],
            &["type", "id", "label", "min", "max"],
        ),
        "color" => (&["type", "id", "label", "value", "config_key", "onChange"], &["type", "id", "label"]),
        "datetime" => (
            &["type", "id", "label", "value", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "select" => (
            &["type", "id", "label", "options", "value", "config_key", "action", "onChange"],
            &["type", "id", "label", "options"],
        ),
        "checkbox" => (&["type", "id", "label", "checked", "config_key", "action", "onChange"], &["type", "id", "label"]),
        "switch" => (
            &["type", "id", "label", "checked", "hint", "config_key", "action", "onChange"],
            &["type", "id", "label"],
        ),
        "progress" => (&["type", "id", "value", "max", "label"], &["type", "value"]),
        "metric" => (&["type", "id", "label", "value", "unit", "tone"], &["type", "label", "value"]),
        "alert" => (&["type", "id", "title", "text", "tone"], &["type", "text"]),
        "code" => (&["type", "id", "text"], &["type", "text"]),
        "image" => (&["type", "id", "url", "alt", "height"], &["type", "url"]),
        "list" => (&["type", "id", "items"], &["type", "items"]),
        "link" => (&["type", "id", "label", "url", "action"], &["type", "label", "url"]),
        "divider" => (&["type", "id"], &["type"]),
        "group" => (&["type", "id", "label", "direction", "children"], &["type", "children"]),
        "card" => (&["type", "id", "label", "tone", "children"], &["type", "children"]),
        "grid" => (&["type", "id", "columns", "children"], &["type", "children"]),
        _ => return Err(format!("{label}.type is unsupported")),
    };
    let node = schema_object(value, label, allowed, required)?;
    for field in ["id", "action", "config_key", "onChange"] {
        if let Some(item) = node.get(field) {
            schema_identifier(item, &format!("{label}.{field}"))?;
        }
    }
    for field in ["label", "text", "placeholder", "url", "title", "hint", "unit", "alt"] {
        if let Some(item) = node.get(field) {
            schema_text(item, &format!("{label}.{field}"), 4096, matches!(field, "value" | "placeholder"))?;
        }
    }
    if matches!(kind, "input" | "select" | "textarea" | "color" | "datetime") {
        if let Some(item) = node.get("value") {
            schema_text(item, &format!("{label}.value"), 4096, true)?;
        }
    }
    if kind == "color" {
        if let Some(item) = node.get("value") {
            if !COLOR_RE.is_match(&py_str(item)) {
                return Err(format!("{label}.value must be #rrggbb"));
            }
        }
    }
    if kind == "image" {
        if let Some(item) = node.get("url") {
            schema_text(item, &format!("{label}.url"), 4096, false)?;
        }
    }
    if let Some(tone) = node.get("tone") {
        let value = tone.as_str().unwrap_or_default();
        if !tone.is_string() || !V2_UI_TONES.contains(&value) {
            return Err(format!("{label}.tone is unsupported"));
        }
    }
    for field in ["disabled", "checked"] {
        schema_bool(node, field, label)?;
    }
    if kind == "progress" {
        for field in ["value", "max"] {
            schema_number(node, field, label)?;
        }
    }
    if matches!(kind, "number" | "slider") {
        for field in ["value", "min", "max", "step"] {
            schema_number(node, field, label)?;
        }
        if let (Some(minimum), Some(maximum)) = (node.get("min"), node.get("max")) {
            if minimum.as_f64().unwrap_or(f64::NAN) > maximum.as_f64().unwrap_or(f64::NAN) {
                return Err(format!("{label}.min exceeds {label}.max"));
            }
        }
    }
    if kind == "textarea" {
        if let Some(rows) = node.get("rows") {
            schema_number(node, "rows", label)?;
            let value = rows.as_f64().unwrap_or(f64::NAN);
            if !(1.0..=24.0).contains(&value) {
                return Err(format!("{label}.rows is out of range"));
            }
        }
    }
    if kind == "image" {
        if let Some(height) = node.get("height") {
            schema_number(node, "height", label)?;
            let value = height.as_f64().unwrap_or(f64::NAN);
            if !(16.0..=720.0).contains(&value) {
                return Err(format!("{label}.height is out of range"));
            }
        }
    }
    if kind == "grid" {
        if let Some(columns) = node.get("columns") {
            schema_number(node, "columns", label)?;
            let value = columns.as_f64().unwrap_or(f64::NAN);
            if value.fract() != 0.0 || !(1..=4).contains(&(value as i64)) {
                return Err(format!("{label}.columns is out of range"));
            }
        }
    }
    if kind == "include" {
        schema_identifier(node.get("fragment").unwrap_or(&Value::Null), &format!("{label}.fragment"))?;
    }
    if kind == "form" {
        schema_identifier(node.get("submitAction").unwrap_or(&Value::Null), &format!("{label}.submitAction"))?;
        for child in budget.collect(node.get("fields").unwrap_or(&Value::Null), &format!("{label}.fields"), 512)?.clone() {
            validate_v2_ui_node(&child, &format!("{label}.fields[]"), depth + 1, budget)?;
        }
    }
    if matches!(kind, "tabs" | "accordion") {
        let key = if kind == "tabs" { "tabs" } else { "sections" };
        let mut ids: HashSet<String> = HashSet::new();
        for entry in budget
            .collect(node.get(key).unwrap_or(&Value::Null), &format!("{label}.{key}"), 64)?
            .clone()
        {
            let fields = schema_object(&entry, label, &["id", "label", "content"], &["id", "label", "content"])?;
            let identifier = schema_identifier(fields.get("id").unwrap_or(&Value::Null), label)?.to_owned();
            if !ids.insert(identifier) {
                return Err("Duplicate UI tab/section id".to_owned());
            }
            schema_text(fields.get("label").unwrap_or(&Value::Null), label, 256, false)?;
            validate_v2_ui_node(fields.get("content").unwrap_or(&Value::Null), label, depth + 1, budget)?;
        }
    }
    if kind == "table" {
        for column in budget.collect(node.get("columns").unwrap_or(&Value::Null), label, 32)?.clone() {
            let fields = schema_object(&column, label, &["key", "label"], &["key", "label"])?;
            schema_identifier(fields.get("key").unwrap_or(&Value::Null), label)?;
            schema_text(fields.get("label").unwrap_or(&Value::Null), label, 256, false)?;
        }
        for row in budget.collect(node.get("rows").unwrap_or(&Value::Null), label, 512)?.clone() {
            let Some(cells) = row.as_object() else {
                return Err("Invalid table row".to_owned());
            };
            if cells.len() > 32 {
                return Err("Invalid table row".to_owned());
            }
            for cell in cells.values() {
                if cell.is_string() {
                    schema_text(cell, label, 4096, true)?;
                } else if !finite_number(cell) {
                    return Err("Invalid table cell".to_owned());
                }
            }
        }
    }
    if kind == "chart" {
        for item in budget.collect(node.get("values").unwrap_or(&Value::Null), label, 512)?.clone() {
            if !finite_number(&item) {
                return Err("Invalid chart value".to_owned());
            }
        }
    }
    if kind == "select" {
        let options = budget.collect(node.get("options").unwrap_or(&Value::Null), &format!("{label}.options"), 128)?;
        if options.is_empty() {
            return Err(format!("{label}.options must not be empty"));
        }
        for option in options.clone() {
            validate_v2_option(&option, &format!("{label}.options[]"))?;
        }
    }
    if kind == "list" {
        for item in budget.collect(node.get("items").unwrap_or(&Value::Null), &format!("{label}.items"), 512)?.clone() {
            if item.is_string() {
                schema_text(&item, &format!("{label}.items[]"), 4096, true)?;
                continue;
            }
            let record =
                schema_object(&item, &format!("{label}.items[]"), &["id", "label", "detail", "badge", "action"], &["id", "label"])?;
            schema_identifier(record.get("id").unwrap_or(&Value::Null), &format!("{label}.items[].id"))?;
            schema_text(record.get("label").unwrap_or(&Value::Null), &format!("{label}.items[].label"), 256, false)?;
            for field in ["detail", "badge", "action"] {
                schema_optional_text(record, field, &format!("{label}.items[]"), 512)?;
            }
        }
    }
    if kind == "group" {
        if let Some(item) = node.get("label") {
            schema_text(item, &format!("{label}.label"), 256, true)?;
        }
        if let Some(direction) = node.get("direction") {
            let value = direction.as_str().unwrap_or_default();
            if !direction.is_string() || !V2_UI_DIRECTIONS.contains(&value) {
                return Err(format!("{label}.direction is unsupported"));
            }
        }
        for child in budget
            .collect(node.get("children").unwrap_or(&Value::Null), &format!("{label}.children"), 512)?
            .clone()
        {
            validate_v2_ui_node(&child, &format!("{label}.children[]"), depth + 1, budget)?;
        }
    }
    if matches!(kind, "card" | "grid") {
        if kind == "card" {
            if let Some(item) = node.get("label") {
                schema_text(item, &format!("{label}.label"), 256, true)?;
            }
        }
        for child in budget
            .collect(node.get("children").unwrap_or(&Value::Null), &format!("{label}.children"), 512)?
            .clone()
        {
            validate_v2_ui_node(&child, &format!("{label}.children[]"), depth + 1, budget)?;
        }
    }
    Ok(())
}

fn validate_v2_i18n(value: &Value) -> Result<(), String> {
    let i18n = schema_object(
        value,
        "plugin.json.i18n",
        &["default_locale", "messages"],
        &["default_locale", "messages"],
    )?;
    schema_text(
        i18n.get("default_locale").unwrap_or(&Value::Null),
        "plugin.json.i18n.default_locale",
        35,
        false,
    )?;
    let Some(messages) = i18n.get("messages").and_then(Value::as_object) else {
        return Err("plugin.json.i18n.messages must be a bounded object".to_owned());
    };
    if messages.len() > 32 {
        return Err("plugin.json.i18n.messages must be a bounded object".to_owned());
    }
    for (locale, raw_table) in messages {
        schema_text(&json!(locale), "plugin.json.i18n.locale", 35, false)?;
        let Some(table) = raw_table.as_object() else {
            return Err("plugin.json.i18n locale table must be a bounded object".to_owned());
        };
        if table.len() > 2048 {
            return Err("plugin.json.i18n locale table must be a bounded object".to_owned());
        }
        for (key, text) in table {
            schema_text(&json!(key), "plugin.json.i18n message key", 256, false)?;
            schema_text(text, "plugin.json.i18n message", 4096, true)?;
        }
    }
    Ok(())
}

fn validate_v2_theme(value: &Value) -> Result<(), String> {
    let theme = schema_object(value, "plugin.json.theme", &["name", "colors", "tokens"], &[])?;
    if let Some(name) = theme.get("name") {
        schema_text(name, "plugin.json.theme.name", 120, false)?;
    }
    for (field, allow_number) in [("colors", false), ("tokens", true)] {
        let Some(mapping) = theme.get(field).and_then(Value::as_object) else {
            continue;
        };
        if mapping.len() > 256 {
            return Err(format!("plugin.json.theme.{field} must be a bounded object"));
        }
        for (key, item) in mapping {
            schema_identifier(&json!(key), &format!("plugin.json.theme.{field} key"))?;
            if item.is_string() {
                schema_text(item, &format!("plugin.json.theme.{field}.{key}"), 256, true)?;
            } else if !(allow_number && finite_number(item)) {
                return Err(format!("plugin.json.theme.{field}.{key} has an unsupported value"));
            }
        }
    }
    Ok(())
}

fn validate_v2_named_records(
    value: &[Value],
    label: &str,
    allowed: &[&str],
    required: &[&str],
) -> Result<(), String> {
    let mut seen: HashSet<String> = HashSet::new();
    for item in value {
        let record = schema_object(item, label, allowed, required)?;
        let identifier = schema_identifier(record.get("id").unwrap_or(&Value::Null), &format!("{label}.id"))?.to_owned();
        if !seen.insert(identifier) {
            return Err(format!("{label} contains duplicate ids"));
        }
        schema_text(record.get("label").unwrap_or(&Value::Null), &format!("{label}.label"), 256, false)?;
        for field in required.iter().filter(|field| **field != "id" && **field != "label") {
            schema_text(
                record.get(*field).unwrap_or(&Value::Null),
                &format!("{label}.{field}"),
                256,
                false,
            )?;
        }
        for field in allowed.iter().filter(|field| !required.contains(field) && **field != "id") {
            schema_optional_text(record, field, label, 1000)?;
        }
    }
    Ok(())
}

fn validate_v2_commands(value: &Value) -> Result<(), String> {
    let commands = schema_array(value, "plugin.json.commands", 256)?;
    let mut seen: HashSet<String> = HashSet::new();
    for raw_command in commands {
        let command = schema_object(
            raw_command,
            "plugin.json.commands[]",
            &["id", "label", "description", "arguments"],
            &["id", "label"],
        )?;
        let identifier =
            schema_identifier(command.get("id").unwrap_or(&Value::Null), "plugin.json.commands[].id")?.to_owned();
        if !seen.insert(identifier) {
            return Err("plugin.json.commands contains duplicate ids".to_owned());
        }
        schema_text(command.get("label").unwrap_or(&Value::Null), "plugin.json.commands[].label", 256, false)?;
        schema_optional_text(command, "description", "plugin.json.commands[]", 1000)?;
        if let Some(arguments) = command.get("arguments") {
            let args = schema_array(arguments, "plugin.json.commands[].arguments", 64)?;
            let mut names: HashSet<String> = HashSet::new();
            for raw_arg in args {
                let arg = schema_object(
                    raw_arg,
                    "plugin.json.commands[].arguments[]",
                    &["name", "description", "required", "choices"],
                    &["name"],
                )?;
                let name =
                    schema_identifier(arg.get("name").unwrap_or(&Value::Null), "plugin.json.commands[].arguments[].name")?
                        .to_owned();
                if !names.insert(name) {
                    return Err("plugin.json command arguments contain duplicate names".to_owned());
                }
                schema_optional_text(arg, "description", "plugin.json.commands[].arguments[]", 1000)?;
                schema_bool(arg, "required", "plugin.json.commands[].arguments[]")?;
                if let Some(choices) = arg.get("choices") {
                    for choice in schema_array(choices, "plugin.json.commands[].arguments[].choices", 256)? {
                        schema_text(choice, "plugin.json command choice", 256, false)?;
                    }
                }
            }
        }
    }
    Ok(())
}
