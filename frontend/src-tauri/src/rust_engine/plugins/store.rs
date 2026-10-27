use super::*;
use super::manifest::static_json_manifest;

impl PluginStore {
    pub(crate) fn new(config: &ConfigStore) -> Result<Self, String> {
        let data_home = config.paths.data_home.clone();
        let root = data_home.join("plugins");
        if path_is_symlink(&root) {
            return Err("Каталог плагинов является символьной ссылкой".to_owned());
        }
        create_dir_private(&root).map_err(|_| "Каталог плагинов является символьной ссылкой".to_owned())?;
        let trust_store = PluginTrustStore::new(&data_home.join("plugin-trust"))?;
        let permission_path = data_home.join(PLUGIN_PERMISSION_FILE);
        read_permission_state(&permission_path)?;
        Ok(Self {
            root,
            trust_store,
            permission_path,
            settings: config.settings().clone(),
            access_lock: Mutex::new(()),
        })
    }

    pub(crate) fn list(&self) -> Result<Vec<Value>, String> {
        let state = {
            let _guard = self.access_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            read_permission_state(&self.permission_path)?
        };
        discover_plugins_inner(&self.root, &self.trust_store, &state)
    }

    pub(crate) fn install_archive(&self, content: &[u8], enable: bool, replace: bool) -> Result<Value, String> {
        if content.is_empty() || content.len() > PLUGIN_MAX_ARCHIVE_BYTES {
            return Err("Архив плагина пустой или слишком большой".to_owned());
        }
        let staging = self.root.join(format!(".install-{}", random_hex(8)));
        if create_dir_private(&staging).is_err() {
            return Err("Не удалось установить плагин: could not create staging directory".to_owned());
        }
        let outcome = self.install_staged(&staging, content, enable, replace);
        if outcome.is_err() {
            let _ = remove_tree(&staging);
        }
        match outcome {
            Ok(value) => Ok(value),
            Err(InstallFail::User(message)) => Err(message),
            Err(InstallFail::Os(message)) => Err(format!("Не удалось установить плагин: {message}")),
        }
    }

    fn install_staged(
        &self,
        staging: &Path,
        content: &[u8],
        enable: bool,
        replace: bool,
    ) -> Result<Value, InstallFail> {
        let mut archive = zip::ZipArchive::new(Cursor::new(content))
            .map_err(|error| InstallFail::Os(error.to_string()))?;
        let members_len = archive.len();
        if members_len > PLUGIN_MAX_FILES {
            return Err(InstallFail::User("Архив плагина содержит слишком много файлов".to_owned()));
        }
        struct Member {
            name: String,
            is_dir: bool,
            size: u64,
            is_symlink: bool,
        }
        let mut members: Vec<Member> = Vec::with_capacity(members_len);
        for index in 0..members_len {
            let file = archive.by_index(index).map_err(|error| InstallFail::Os(error.to_string()))?;
            let mode = file.unix_mode().unwrap_or(0);
            members.push(Member {
                name: file.name().map(|v| v.to_string()).unwrap_or_default(),
                is_dir: file.is_dir(),
                size: file.size(),
                is_symlink: (mode & 0o170000) == 0o120000,
            });
        }

        let mut total_size: u64 = 0;
        let mut seen_members: HashSet<String> = HashSet::new();
        for (index, member) in members.iter().enumerate() {
            let path = archive_member_path(staging, &member.name)?;
            let normalized = path
                .strip_prefix(staging)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| member.name.clone());
            if !seen_members.insert(normalized) {
                return Err(InstallFail::User("Архив плагина содержит повторяющийся путь".to_owned()));
            }
            if member.is_symlink {
                return Err(InstallFail::User("Архив плагина содержит символьную ссылку".to_owned()));
            }
            if member.is_dir {
                create_dir_private(&path).map_err(|error| InstallFail::Os(error.to_string()))?;
                continue;
            }
            total_size += member.size;
            if member.size > PLUGIN_MAX_UNPACKED_BYTES as u64 || total_size > PLUGIN_MAX_UNPACKED_BYTES as u64 {
                return Err(InstallFail::User("Распакованный плагин слишком большой".to_owned()));
            }
            if let Some(parent) = path.parent() {
                create_dir_private(parent).map_err(|error| InstallFail::Os(error.to_string()))?;
            }
            let mut source = archive.by_index(index).map_err(|error| InstallFail::Os(error.to_string()))?;
            let mut payload = Vec::with_capacity(member.size as usize);
            source
                .by_ref()
                .take(PLUGIN_MAX_UNPACKED_BYTES as u64 + 1)
                .read_to_end(&mut payload)
                .map_err(|error| InstallFail::Os(error.to_string()))?;
            if payload.len() > PLUGIN_MAX_UNPACKED_BYTES {
                return Err(InstallFail::User("Распакованный плагин слишком большой".to_owned()));
            }
            let mut target = File::create(&path).map_err(|error| InstallFail::Os(error.to_string()))?;
            target.write_all(&payload).map_err(|error| InstallFail::Os(error.to_string()))?;
            restrict_mode(&path, 0o600);
        }

        let legacy_path = staging.join(PLUGIN_MANIFEST);
        let v2_path = staging.join(PLUGIN_V2_MANIFEST);
        if path_exists(&legacy_path) == path_exists(&v2_path) {
            return Err(InstallFail::User(
                "Архив должен содержать ровно один корневой plugin.yml или plugin.json".to_owned(),
            ));
        }
        let is_v2 = path_exists(&v2_path);

        let mut package: Option<V2Package> = None;
        let manifest: Value;
        if is_v2 {
            match static_json_manifest(staging)? {
                Some(static_json) => {
                    manifest = static_json;
                    let entrypoint = manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let entrypoint_path = safe_child(staging, &entrypoint)?;
                    if !path_is_file(&entrypoint_path) {
                        return Err(InstallFail::User("entrypoint плагина отсутствует в архиве".to_owned()));
                    }
                }
                None => {
                    reject_v2_runtime_paths(staging)?;
                    let parsed = read_v2_package(staging, None)?;
                    manifest = parsed.manifest.clone();
                    package = Some(parsed);
                }
            }
        } else {
            if !path_is_file(&legacy_path)
                || path_is_symlink(&legacy_path)
                || path_size(&legacy_path).unwrap_or(0) > PLUGIN_MAX_MANIFEST_BYTES as u64
            {
                return Err(InstallFail::User("В архиве отсутствует корректный plugin.yml".to_owned()));
            }
            let text = fs::read_to_string(&legacy_path).map_err(|error| InstallFail::Os(error.to_string()))?;
            let parsed = Value::Object(parse_plugin_yml(&text).map_err(InstallFail::User)?);
            manifest = validate_manifest(&parsed).map_err(InstallFail::User)?;
            let entrypoint = manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
            let entrypoint_path = safe_child(staging, &entrypoint)?;
            if !path_is_file(&entrypoint_path) {
                return Err(InstallFail::User("entrypoint плагина отсутствует в архиве".to_owned()));
            }
        }
        self.finish_install(staging, manifest, package, is_v2, enable, replace)
    }

    fn finish_install(
        &self,
        staging: &Path,
        manifest: Value,
        package: Option<V2Package>,
        is_v2: bool,
        enable: bool,
        replace: bool,
    ) -> Result<Value, InstallFail> {
        let plugin_id = manifest.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let target = self.root.join(&plugin_id);
        if path_exists(&target) || path_is_symlink(&target) {
            if !replace {
                return Err(InstallFail::User(format!("Плагин уже установлен: {plugin_id}")));
            }
            if path_is_symlink(&target) {
                return Err(InstallFail::User(
                    "Каталог существующего плагина является символьной ссылкой".to_owned(),
                ));
            }
            self.reconcile_replacement_permissions(&plugin_id, &target, if is_v2 { package.as_ref() } else { None })
                .map_err(InstallFail::User)?;
            for name in ["storage", "files", "config", "database", ".enabled"] {
                let old = target.join(name);
                if path_exists(&old) || path_is_symlink(&old) {
                    move_path(&old, &staging.join(name)).map_err(|error| InstallFail::Os(error.to_string()))?;
                }
            }
            remove_tree(&target).map_err(|error| InstallFail::Os(error.to_string()))?;
        }
        fs::rename(staging, &target).map_err(|error| InstallFail::Os(error.to_string()))?;
        restrict_mode(&target, 0o700);
        let enabled_marker = target.join(".enabled");
        if enable {
            // для статического `plugin.json` маркер включения не создаётся
            let executable = !is_v2 || self.can_execute(package.as_ref(), &plugin_id);
            if executable {
                write_enabled_marker(&enabled_marker).map_err(|error| InstallFail::Os(error.to_string()))?;
            } else if path_exists(&enabled_marker) || path_is_symlink(&enabled_marker) {
                fs::remove_file(&enabled_marker).map_err(|error| InstallFail::Os(error.to_string()))?;
            }
        } else if path_exists(&enabled_marker) {
            fs::remove_file(&enabled_marker).map_err(|error| InstallFail::Os(error.to_string()))?;
        }
        self.namespace(&plugin_id).map_err(InstallFail::User)?;
        self.plugin_status(&plugin_id).map_err(InstallFail::User)
    }

    pub(crate) fn set_enabled(&self, plugin_id: &str, enabled: bool) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        let marker = directory.join(".enabled");
        if enabled {
            if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
                return Err("Плагин отозван локально или списком отзыва".to_owned());
            }
            if path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
                let package = read_v2_package(&directory, Some(plugin_id))?;
                self.require_executable(&package, plugin_id, false)?;
            }
            write_enabled_marker(&marker).map_err(|error| error.to_string())?;
        } else if path_exists(&marker) || path_is_symlink(&marker) {
            if path_is_symlink(&marker) {
                return Err("Маркер состояния плагина является символьной ссылкой".to_owned());
            }
            fs::remove_file(&marker).map_err(|error| error.to_string())?;
        }
        self.plugin_status(plugin_id)
    }

    pub(crate) fn is_enabled(&self, plugin_id: &str) -> Result<bool, String> {
        Ok(enabled_state(&self.existing_plugin_directory(plugin_id)?))
    }

    pub(crate) fn uninstall(&self, plugin_id: &str) -> Result<(), String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        remove_tree(&directory).map_err(|error| error.to_string())?;
        self.update_permission_record(plugin_id, None)
    }

    pub(crate) fn namespace(&self, plugin_id: &str) -> Result<PathBuf, String> {
        let directory = self.plugin_directory(plugin_id)?;
        for name in ["storage", "files", "config", "database"] {
            let path = directory.join(name);
            if path_is_symlink(&path) {
                return Err(format!("Каталог плагина является символьной ссылкой: {name}"));
            }
            create_dir_private(&path).map_err(|error| error.to_string())?;
        }
        Ok(directory)
    }

    pub(crate) fn manifest(&self, plugin_id: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        let v2_path = directory.join(PLUGIN_V2_MANIFEST);
        if path_exists(&v2_path) {
            if let Some(static_json) = static_json_manifest(&directory)? {
                if static_json.get("id").and_then(Value::as_str) != Some(plugin_id) {
                    return Err("id plugin.json не совпадает с каталогом".to_owned());
                }
                return Ok(static_json);
            }
            return Ok(read_v2_package(&directory, Some(plugin_id))?.manifest);
        }
        if path_is_symlink(&v2_path) {
            return Err("plugin.json является символьной ссылкой".to_owned());
        }
        let path = directory.join(PLUGIN_MANIFEST);
        if path_is_symlink(&path) || !path_is_file(&path) || path_size(&path).unwrap_or(0) > PLUGIN_MAX_MANIFEST_BYTES as u64 {
            return Err("plugin.yml не найден".to_owned());
        }
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let value = validate_manifest(&Value::Object(parse_plugin_yml(&text)?))?;
        if value.get("id").and_then(Value::as_str) != Some(plugin_id) {
            return Err("id plugin.yml не совпадает с каталогом".to_owned());
        }
        if path_exists(&v2_path) {
            return Err("Каталог содержит конфликтующие plugin.yml и plugin.json".to_owned());
        }
        let entrypoint = value.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
        let entrypoint_path = safe_child(&directory, &entrypoint)?;
        if !path_is_file(&entrypoint_path) {
            return Err("entrypoint плагина отсутствует".to_owned());
        }
        Ok(value)
    }

    pub(crate) fn authorize_operation(&self, plugin_id: &str, permission: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            let package = read_v2_package(&directory, Some(plugin_id))?;
            self.require_executable(&package, plugin_id, true)?;
            let declared: Vec<String> = package
                .manifest
                .get("permissions")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            if !declared.iter().any(|item| item.as_str() == permission) {
                return Err(format!("Плагину не запрошено разрешение {permission}"));
            }
            let granted = self.granted_permissions(plugin_id, &declared)?;
            if !granted.iter().any(|item| item.as_str() == permission) {
                return Err(format!("Пользователь не выдал плагину разрешение {permission}"));
            }
            return Ok(package.manifest);
        }

        let manifest = self.manifest(plugin_id)?;
        if !self.is_enabled(plugin_id)? {
            return Err("Плагин выключен".to_owned());
        }
        if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
            return Err("Плагин отозван локально или списком отзыва".to_owned());
        }
        let declared = manifest.get("permissions").and_then(Value::as_array).cloned().unwrap_or_default();
        if !declared.iter().any(|item| item.as_str() == Some(permission)) {
            return Err(format!("Плагину не выдано разрешение {permission}"));
        }
        Ok(manifest)
    }

    pub(crate) fn set_permission_grants(&self, plugin_id: &str, permissions: &Value) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Постоянные grants поддерживаются только для plugin.json".to_owned());
        }
        let manifest = read_v2_package(&directory, Some(plugin_id))?.manifest;
        let list = permissions
            .as_array()
            .ok_or_else(|| "Список grants должен содержать строки".to_owned())?;
        if list.iter().any(|item| !item.is_string()) {
            return Err("Список grants должен содержать строки".to_owned());
        }
        let requested: BTreeSet<String> = manifest
            .get("permissions")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let raw: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        let granted: BTreeSet<String> = raw.iter().cloned().collect();
        if granted.len() != raw.len() || !granted.iter().all(|item| requested.contains(item)) {
            return Err("Можно выдать только уникальные permissions из plugin.json".to_owned());
        }
        let record = json!({
            "grants": sorted_set(&raw),
            "untrusted_execution_hash": self.untrusted_execution_hash(plugin_id)?,
        });
        self.update_permission_record(plugin_id, Some(record.as_object().cloned().unwrap()))?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn reduce_permission_grants(&self, plugin_id: &str, permissions: &Value) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Постоянные grants поддерживаются только для plugin.json".to_owned());
        }
        let manifest = read_v2_package(&directory, Some(plugin_id))?.manifest;
        let list = permissions
            .as_array()
            .ok_or_else(|| "Список grants должен содержать строки".to_owned())?;
        if list.iter().any(|item| !item.is_string()) {
            return Err("Список grants должен содержать строки".to_owned());
        }
        let declared: BTreeSet<String> = manifest
            .get("permissions")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let raw: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        let requested: BTreeSet<String> = raw.iter().cloned().collect();
        if requested.len() != raw.len() || !requested.iter().all(|item| declared.contains(item)) {
            return Err("Можно сохранить только уникальные permissions из plugin.json".to_owned());
        }

        let _guard = self.access_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = read_permission_state(&self.permission_path)?;
        let plugins = state
            .get_mut("plugins")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "Plugin permission grants have an unsupported format".to_owned())?;
        let current = plugins.get(plugin_id).cloned().unwrap_or_else(default_permission_record);
        let current_grants: BTreeSet<String> = current
            .get("grants")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        if !requested.is_subset(&current_grants) {
            return Err("Плагин может только отзывать выданные разрешения; новые выдаются в менеджере плагинов".to_owned());
        }
        let execution_hash = current.get("untrusted_execution_hash").and_then(Value::as_str).unwrap_or_default();
        plugins.insert(
            plugin_id.to_owned(),
            json!({
                "grants": sorted_set(&raw),
                "untrusted_execution_hash": execution_hash,
            }),
        );
        write_permission_state(&self.permission_path, &state)?;
        drop(_guard);
        self.plugin_status(plugin_id)
    }

    pub(crate) fn set_local_untrusted_execution(&self, plugin_id: &str, allowed: bool) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Локальный запуск поддерживается только для plugin.json".to_owned());
        }
        let package = read_v2_package(&directory, Some(plugin_id))?;
        if package.signed {
            return Err("Локальный untrusted-запуск доступен только для unsigned plugin.json".to_owned());
        }
        let record = json!({
            "grants": self.stored_grants(plugin_id)?,
            "untrusted_execution_hash": if allowed { package.package_digest.clone() } else { String::new() },
        });
        self.update_permission_record(plugin_id, Some(record.as_object().cloned().unwrap()))?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn pin_publisher(&self, key_id: &str, public_key: &str, revocation_authority: bool) -> Result<Value, String> {
        self.trust_store
            .pin_publisher_key(key_id, &json!(public_key), revocation_authority)?;
        let publisher = self.trust_store.publisher_status(key_id, Some(&json!(public_key)))?;
        let mut publisher_fields = publisher.as_object().cloned().unwrap_or_default();
        publisher_fields.insert("public_key".to_owned(), json!(public_key));
        let mut result = self.security_status()?;
        result["publisher"] = Value::Object(publisher_fields);
        Ok(result)
    }

    pub(crate) fn unpin_publisher(&self, key_id: &str) -> Result<Value, String> {
        let removed = self.trust_store.unpin_publisher_key(key_id)?;
        let mut result = self.security_status()?;
        result["key_id"] = json!(key_id);
        result["unpinned"] = json!(removed);
        Ok(result)
    }

    pub(crate) fn set_publisher_revocation_authority(&self, key_id: &str, enabled: bool) -> Result<Value, String> {
        self.trust_store.set_revocation_authority(key_id, enabled)?;
        self.security_status()
    }

    pub(crate) fn import_revocations_json(&self, document_json: &str) -> Result<Value, String> {
        self.trust_store.import_revocations_json(document_json)
    }

    pub(crate) fn revoke_plugin(&self, plugin_id: &str, reason: &str) -> Result<Value, String> {
        self.trust_store.revoke_plugin(plugin_id, reason)?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn unrevoke_plugin(&self, plugin_id: &str) -> Result<Value, String> {
        self.trust_store.unrevoke_plugin(plugin_id)?;
        self.plugin_status(plugin_id)
    }

    pub(crate) fn runtime_source(&self, plugin_id: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Выполняемый runtime доступен только для plugin.json".to_owned());
        }
        let package = read_v2_package(&directory, Some(plugin_id))?;
        self.require_executable(&package, plugin_id, true)?;
        let entrypoint = package.manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
        let source_bytes = package.files.get(&entrypoint);
        let Some(source_bytes) = source_bytes else {
            return Err("entrypoint плагина отсутствует или превышает лимит runtime".to_owned());
        };
        if source_bytes.len() > PLUGIN_RUNTIME_MAX_SOURCE_BYTES {
            return Err("entrypoint плагина отсутствует или превышает лимит runtime".to_owned());
        }
        let source = std::str::from_utf8(source_bytes).map_err(|_| "entrypoint плагина должен быть UTF-8".to_owned())?;
        Ok(json!({
            "manifest": package.manifest,
            "entrypoint": entrypoint,
            "source": source,
            "package_sha256": package.package_digest,
        }))
    }

    pub(crate) fn runtime_info(&self, plugin_id: &str) -> Result<Value, String> {
        let directory = self.existing_plugin_directory(plugin_id)?;
        if !path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            return Err("Выполняемый runtime доступен только для plugin.json".to_owned());
        }
        let package = read_v2_package(&directory, Some(plugin_id))?;
        let manifest = &package.manifest;
        let api_version = manifest.get("api_version").and_then(Value::as_str).unwrap_or_default();
        let default_profile = if api_version == "3" { "strict" } else { "quickjs-legacy" };
        Ok(json!({
            "plugin_id": plugin_id,
            "api_version": api_version,
            "permissions": manifest.get("permissions").cloned().unwrap_or_else(|| json!([])),
            "capabilities": runtime_capabilities_for_manifest(manifest),
            "sandbox_profile": field_str(manifest.as_object().unwrap_or(&Map::new()), "sandbox_profile", default_profile),
            "package_sha256": package.package_digest,
        }))
    }

    pub(crate) fn permission_summary(&self, plugin_id: &str) -> Result<Value, String> {
        let plugin = self.plugin_status(plugin_id)?;
        Ok(json!({
            "plugin_id": plugin_id,
            "requested_permissions": plugin.get("requested_permissions").and_then(Value::as_array).cloned().unwrap_or_default(),
            "granted_permissions": plugin.get("granted_permissions").and_then(Value::as_array).cloned().unwrap_or_default(),
            "execution_allowed": plugin.get("execution_allowed").and_then(Value::as_bool).unwrap_or(false),
            "api_version": plugin.get("api_version").cloned().unwrap_or_else(|| json!("1")),
        }))
    }

    pub(crate) fn security_status(&self) -> Result<Value, String> {
        let plugins = self.list()?;
        let mut publishers: Vec<Value> = Vec::new();
        for plugin in &plugins {
            let Some(key_id) = plugin.get("publisher_key_id").and_then(Value::as_str) else { continue };
            let Some(public_key) = plugin.get("publisher_public_key").and_then(Value::as_str) else { continue };
            let Ok(status) = self.trust_store.publisher_status(key_id, Some(&json!(public_key))) else { continue };
            let mut fields = status.as_object().cloned().unwrap_or_default();
            fields.insert("public_key".to_owned(), json!(public_key));
            fields.insert(
                "fingerprint".to_owned(),
                plugin.get("publisher_fingerprint").cloned().unwrap_or(Value::Null),
            );
            publishers.push(Value::Object(fields));
        }
        Ok(json!({ "plugins": plugins, "publishers": publishers }))
    }

    pub(crate) fn quota_usage(&self, plugin_id: &str) -> Result<Value, String> {
        let namespace = self.namespace(plugin_id)?;
        let buckets: [(&str, PathBuf); 4] = [
            ("storage", namespace.join("storage")),
            ("config", namespace.join("config")),
            ("files", namespace.join("files")),
            ("database", namespace.join("database")),
        ];
        let mut directories = Map::new();
        for (name, path) in &buckets {
            directories.insert((*name).to_owned(), directory_usage(path)?);
        }
        let total_bytes: i64 = directories.values().filter_map(|item| item.get("bytes").and_then(Value::as_i64)).sum();
        let total_files: i64 = directories.values().filter_map(|item| item.get("files").and_then(Value::as_i64)).sum();
        Ok(json!({
            "plugin_id": plugin_id,
            "totals": { "bytes": total_bytes, "files": total_files },
            "directories": directories,
        }))
    }

    pub(crate) fn projected_launcher_config(&self, schema_id: &str) -> Result<Value, String> {
        const LAUNCHER_PUBLIC_CONFIG: &[&str] = &[
            "theme", "active_theme_id", "accent_color", "sidebar_position", "sidebar_mode", "sidebar_width_mode",
            "sidebar_width", "sidebar_floating", "sidebar_visibility", "animations", "density", "ui_scale",
            "graphics_mode", "launcher_version", "startup_view", "last_view", "remember_last_view",
            "default_version_filter", "default_loader", "max_downloads", "download_timeout_seconds", "download_retries",
            "show_activity_panel", "show_toasts", "font_family", "keep_logs_days", "developer_mode",
        ];
        const NETWORK_DIAGNOSTICS: &[&str] = &[
            "max_downloads", "download_timeout_seconds", "download_retries", "telemetry", "offline_debug_limit_seconds",
        ];
        let allowed: &[&str] = match schema_id {
            "launcher-public-config" => LAUNCHER_PUBLIC_CONFIG,
            "network-diagnostics" => NETWORK_DIAGNOSTICS,
            _ => return Err("Неизвестная схема projected config".to_owned()),
        };
        let removed: &[&str] = match schema_id {
            "launcher-public-config" => &[
                "client_id", "telemetry_endpoint", "http_proxy", "cloud_username", "cloud_url", "cloud_remote_path",
            ],
            _ => &["http_proxy", "client_id", "cloud_username", "cloud_url"],
        };
        let mut data = Map::new();
        if let Some(settings) = self.settings.as_object() {
            for key in allowed {
                if let Some(value) = settings.get(*key) {
                    data.insert(key.to_string(), value.clone());
                }
            }
        }
        Ok(json!({
            "schema_id": schema_id,
            "data": data,
            "removed_fields": removed,
        }))
    }

    pub(crate) fn launcher_root(&self) -> Result<PathBuf, String> {
        let executable = std::env::current_exe().map_err(|_| "Не удалось определить корень лаунчера".to_owned())?;
        let resolved = fs::canonicalize(&executable).unwrap_or(executable);
        resolved
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Не удалось определить корень лаунчера".to_owned())
    }

    pub(crate) fn list_launcher_files(&self, directory: &str) -> Result<Vec<Value>, String> {
        let root = self.launcher_root()?;
        let target = if directory.is_empty() { root.clone() } else { safe_child(&root, directory)? };
        if path_is_symlink(&target) || !path_exists(&target) {
            return Ok(Vec::new());
        }
        if !path_is_dir(&target) {
            return Err("Каталог launcher-root недоступен".to_owned());
        }
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        collect_regular_files(&target, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<Value> = Vec::new();
        for (_, path) in files {
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
            let relative = path.strip_prefix(&root).unwrap_or(&path);
            out.push(json!({
                "path": relative.to_string_lossy().replace('\\', "/"),
                "size": path_size(&path).unwrap_or(0),
                "modified_at": file_mtime(&path),
            }));
        }
        Ok(out)
    }

    pub(crate) fn read_launcher_file(&self, relative_path: &str) -> Result<String, String> {
        let path = safe_child(&self.launcher_root()?, relative_path)?;
        if !path_is_file(&path) || path_size(&path).unwrap_or(0) > MAX_LAUNCHER_FILE_BYTES as u64 {
            return Err("Файл launcher-root не найден или слишком большой".to_owned());
        }
        fs::read_to_string(&path).map_err(|error| format!("Не удалось прочитать файл launcher-root: {error}"))
    }

    pub(crate) fn read_storage(&self, plugin_id: &str, key: &str) -> Result<Value, String> {
        let path = self.storage_path(plugin_id, key)?;
        if !path_exists(&path) {
            return Ok(Value::Null);
        }
        if path_is_symlink(&path) || path_size(&path).unwrap_or(0) > MAX_STORAGE_BYTES as u64 {
            return Err("Данные плагина повреждены или слишком большие".to_owned());
        }
        let raw = fs::read(&path).map_err(|error| format!("Не удалось прочитать данные плагина: {error}"))?;
        serde_json::from_slice(&raw).map_err(|error| format!("Не удалось прочитать данные плагина: {error}"))
    }

    pub(crate) fn write_storage(&self, plugin_id: &str, key: &str, value: &Value) -> Result<(), String> {
        let path = self.storage_path(plugin_id, key)?;
        let encoded = serde_json::to_vec(value).map_err(|error| format!("Данные плагина слишком большие: {error}"))?;
        if encoded.len() > MAX_STORAGE_BYTES {
            return Err("Данные плагина слишком большие".to_owned());
        }
        atomic_json_write(&path, value, true)
    }

    pub(crate) fn remove_storage(&self, plugin_id: &str, key: &str) -> Result<(), String> {
        let path = self.storage_path(plugin_id, key)?;
        if path_is_symlink(&path) {
            return Err("Данные плагина являются символьной ссылкой".to_owned());
        }
        if path_exists(&path) {
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// ограничивает размер базы SQLite и разрешённые SQL-операции
    pub(crate) fn database(&self, plugin_id: &str, read_only: bool) -> Result<rusqlite::Connection, String> {
        const PLUGIN_DATABASE_MAX_BYTES: u64 = 64 * 1024 * 1024;
        let directory = self.namespace(plugin_id)?.join("database");
        let path = directory.join("plugin.sqlite3");
        if path_is_symlink(&path) {
            return Err("База данных плагина является символьной ссылкой".to_owned());
        }
        fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать каталог базы плагина: {error}"))?;
        if path_exists(&path) && path_size(&path).unwrap_or(0) > PLUGIN_DATABASE_MAX_BYTES {
            return Err("База данных плагина слишком большая".to_owned());
        }
        let connection = rusqlite::Connection::open(&path)
            .map_err(|error| format!("Не удалось открыть базу данных плагина: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA trusted_schema=OFF;
                 PRAGMA foreign_keys=ON;
                 PRAGMA max_page_count=16384;
                 PRAGMA busy_timeout=1000;",
            )
            .map_err(|error| format!("Не удалось настроить базу данных плагина: {error}"))?;
        connection.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            use rusqlite::hooks::{AuthAction, Authorization};
            match context.action {
                AuthAction::Read { .. } | AuthAction::Select | AuthAction::Function { .. } | AuthAction::Transaction { .. } => {
                    Authorization::Allow
                }
                AuthAction::CreateIndex { .. }
                | AuthAction::CreateTable { .. }
                | AuthAction::CreateTempIndex { .. }
                | AuthAction::CreateTempTable { .. }
                | AuthAction::CreateTempTrigger { .. }
                | AuthAction::CreateTempView { .. }
                | AuthAction::CreateTrigger { .. }
                | AuthAction::CreateView { .. }
                | AuthAction::Delete { .. }
                | AuthAction::DropIndex { .. }
                | AuthAction::DropTable { .. }
                | AuthAction::DropTempIndex { .. }
                | AuthAction::DropTempTable { .. }
                | AuthAction::DropTempTrigger { .. }
                | AuthAction::DropTempView { .. }
                | AuthAction::DropTrigger { .. }
                | AuthAction::DropView { .. }
                | AuthAction::Insert { .. }
                | AuthAction::Update { .. }
                    if !read_only =>
                {
                    Authorization::Allow
                }
                _ => Authorization::Deny,
            }
        }));
        Ok(connection)
    }

    pub(crate) fn read_config(&self, plugin_id: &str) -> Result<Map<String, Value>, String> {
        let path = self.config_path(plugin_id)?;
        if !path_exists(&path) {
            return Ok(Map::new());
        }
        if path_is_symlink(&path) || path_size(&path).unwrap_or(0) > MAX_CONFIG_BYTES as u64 {
            return Err("config.yml плагина повреждён или слишком большой".to_owned());
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => return Err(format!("Не удалось прочитать config.yml плагина: {error}")),
        };
        parse_plugin_yml(&text)
    }

    pub(crate) fn write_config(&self, plugin_id: &str, value: &Value) -> Result<(), String> {
        let Some(fields) = value.as_object() else {
            return Err("Конфигурация плагина должна быть объектом".to_owned());
        };
        let path = self.config_path(plugin_id)?;
        let encoded = dump_yaml(fields)?;
        if encoded.len() > MAX_CONFIG_BYTES {
            return Err("config.yml плагина слишком большой".to_owned());
        }
        if let Some(parent) = path.parent() {
            create_dir_private(parent).map_err(|error| error.to_string())?;
        }
        let temporary = path.with_file_name(format!(".config.{}", random_hex(8)));
        let outcome = (|| -> Result<(), std::io::Error> {
            let mut file = open_private_file(&temporary)?;
            file.write_all(encoded.as_bytes())?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        outcome.map_err(|error| format!("Не удалось сохранить config.yml плагина: {error}"))
    }

    pub(crate) fn config_get(&self, plugin_id: &str, key: &str) -> Result<Value, String> {
        validate_config_key(key)?;
        let config = self.read_config(plugin_id)?;
        Ok(config.get(key).cloned().unwrap_or(Value::Null))
    }

    pub(crate) fn config_set(&self, plugin_id: &str, key: &str, value: &Value) -> Result<(), String> {
        validate_config_key(key)?;
        let mut config = self.read_config(plugin_id)?;
        config.insert(key.to_owned(), value.clone());
        self.write_config(plugin_id, &Value::Object(config))
    }

    pub(crate) fn config_remove(&self, plugin_id: &str, key: &str) -> Result<(), String> {
        validate_config_key(key)?;
        let mut config = self.read_config(plugin_id)?;
        config.remove(key);
        self.write_config(plugin_id, &Value::Object(config))
    }

    pub(crate) fn file_path(&self, plugin_id: &str, relative_path: &str) -> Result<PathBuf, String> {
        let directory = self.namespace(plugin_id)?.join("files");
        safe_child(&directory, relative_path)
    }

    pub(crate) fn list_files(&self, plugin_id: &str, directory: &str) -> Result<Vec<String>, String> {
        let root = self.namespace(plugin_id)?.join("files");
        let target = if directory.is_empty() { root.clone() } else { safe_child(&root, directory)? };
        if path_is_symlink(&target) || !path_exists(&target) {
            return Ok(Vec::new());
        }
        if !path_is_dir(&target) {
            return Err("Каталог файлов плагина недоступен".to_owned());
        }
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        collect_regular_files(&target, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<String> = Vec::new();
        for (_, path) in files {
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
            out.push(path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/"));
        }
        Ok(out)
    }

    pub(crate) fn list_file_info(&self, plugin_id: &str, directory: &str) -> Result<Vec<Value>, String> {
        let root = self.namespace(plugin_id)?.join("files");
        let target = if directory.is_empty() { root.clone() } else { safe_child(&root, directory)? };
        if path_is_symlink(&target) || !path_exists(&target) {
            return Ok(Vec::new());
        }
        if !path_is_dir(&target) {
            return Err("Каталог файлов плагина недоступен".to_owned());
        }
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        collect_regular_files(&target, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<Value> = Vec::new();
        for (_, path) in files {
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
            out.push(json!({
                "path": path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/"),
                "size": path_size(&path).unwrap_or(0),
                "modified_at": file_mtime(&path),
            }));
        }
        Ok(out)
    }

    pub(crate) fn read_file(&self, plugin_id: &str, relative_path: &str) -> Result<String, String> {
        let path = self.file_path(plugin_id, relative_path)?;
        if !path_is_file(&path) || path_size(&path).unwrap_or(0) > MAX_PLUGIN_FILE_BYTES as u64 {
            return Err("Файл плагина не найден или слишком большой".to_owned());
        }
        fs::read_to_string(&path).map_err(|error| format!("Не удалось прочитать файл плагина: {error}"))
    }

    pub(crate) fn write_file(&self, plugin_id: &str, relative_path: &str, content: &str) -> Result<(), String> {
        let path = self.file_path(plugin_id, relative_path)?;
        let encoded = content.as_bytes();
        if encoded.len() > MAX_PLUGIN_FILE_BYTES {
            return Err("Файл плагина слишком большой".to_owned());
        }
        let files_root = self.namespace(plugin_id)?.join("files");
        if let Some(parent) = path.parent() {
            create_dir_private(parent).map_err(|error| error.to_string())?;
        }
        assert_no_symlink_chain(path.parent().unwrap_or(&path), &files_root)?;
        if path_exists(&path) || path_is_symlink(&path) {
            if path_is_symlink(&path) {
                return Err("Символьная ссылка в файлах плагина запрещена".to_owned());
            }
        }
        fs::write(&path, encoded).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(crate) fn remove_file(&self, plugin_id: &str, relative_path: &str) -> Result<(), String> {
        let path = self.file_path(plugin_id, relative_path)?;
        if path_is_symlink(&path) {
            return Err("Символьная ссылка в файлах плагина запрещена".to_owned());
        }
        if path_exists(&path) {
            if !path_is_file(&path) {
                return Err("Можно удалить только файл плагина".to_owned());
            }
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn require_executable(&self, package: &V2Package, plugin_id: &str, require_enabled: bool) -> Result<(), String> {
        if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
            return Err("Плагин отозван локально или списком отзыва".to_owned());
        }
        if package.signed {
            let key_id = package.key_id.clone().unwrap_or_default();
            let public_key = package.public_key.clone().unwrap_or_default();
            if !self
                .trust_store
                .is_trusted_publisher(&key_id, Some(&json!(public_key)))
                .unwrap_or(false)
            {
                return Err("Издатель не закреплён в локальном trust store или отозван".to_owned());
            }
        } else if self.untrusted_execution_hash(plugin_id)? != package.package_digest {
            return Err("Для unsigned-плагина требуется явное согласие на локальный untrusted-запуск".to_owned());
        }
        if require_enabled && !enabled_state(&self.existing_plugin_directory(plugin_id)?) {
            return Err("Плагин выключен".to_owned());
        }
        Ok(())
    }

    fn can_execute(&self, package: Option<&V2Package>, plugin_id: &str) -> bool {
        let Some(package) = package else { return false };
        if self.trust_store.is_plugin_revoked(plugin_id).unwrap_or(false) {
            return false;
        }
        if package.signed {
            let key_id = package.key_id.clone().unwrap_or_default();
            let public_key = package.public_key.clone().unwrap_or_default();
            return self
                .trust_store
                .is_trusted_publisher(&key_id, Some(&json!(public_key)))
                .unwrap_or(false);
        }
        self.untrusted_execution_hash(plugin_id).unwrap_or_default() == package.package_digest
    }

    fn plugin_status(&self, plugin_id: &str) -> Result<Value, String> {
        for plugin in self.list()? {
            if plugin.get("id").and_then(Value::as_str) == Some(plugin_id) {
                return Ok(plugin);
            }
        }
        Err("Плагин не установлен".to_owned())
    }

    fn permission_record(&self, plugin_id: &str) -> Result<Value, String> {
        let state = {
            let _guard = self.access_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            read_permission_state(&self.permission_path)?
        };
        Ok(state
            .pointer("/plugins")
            .and_then(Value::as_object)
            .and_then(|fields| fields.get(plugin_id))
            .cloned()
            .unwrap_or_else(default_permission_record))
    }

    fn stored_grants(&self, plugin_id: &str) -> Result<Vec<String>, String> {
        Ok(self
            .permission_record(plugin_id)?
            .get("grants")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default())
    }

    fn untrusted_execution_hash(&self, plugin_id: &str) -> Result<String, String> {
        Ok(py_str(&self.permission_record(plugin_id)?.get("untrusted_execution_hash").cloned().unwrap_or(Value::Null)))
    }

    fn granted_permissions(&self, plugin_id: &str, declared: &[String]) -> Result<Vec<String>, String> {
        let stored = self.stored_grants(plugin_id)?;
        Ok(sorted_set(&declared.iter().filter(|item| stored.contains(item)).cloned().collect::<Vec<_>>()))
    }

    fn update_permission_record(&self, plugin_id: &str, value: Option<Map<String, Value>>) -> Result<(), String> {
        let _guard = self.access_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = read_permission_state(&self.permission_path)?;
        if let Some(plugins) = state.get_mut("plugins").and_then(Value::as_object_mut) {
            match value {
                Some(record) => {
                    plugins.insert(plugin_id.to_owned(), Value::Object(record));
                }
                None => {
                    plugins.remove(plugin_id);
                }
            }
        }
        write_permission_state(&self.permission_path, &state)
    }

    fn reconcile_replacement_permissions(
        &self,
        plugin_id: &str,
        directory: &Path,
        replacement: Option<&V2Package>,
    ) -> Result<(), String> {
        let previous = if path_exists(&directory.join(PLUGIN_V2_MANIFEST)) {
            read_v2_package(directory, Some(plugin_id)).ok()
        } else {
            None
        };

        let manifest_permissions = |package: Option<&V2Package>| -> BTreeSet<String> {
            package
                .map(|package| {
                    package
                        .manifest
                        .get("permissions")
                        .and_then(Value::as_array)
                        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                        .unwrap_or_default()
                })
                .unwrap_or_default()
        };
        let previous_permissions = manifest_permissions(previous.as_ref());
        let next_permissions = manifest_permissions(replacement);
        let record = self.permission_record(plugin_id)?;
        let stored_grants: Vec<String> = record
            .get("grants")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let grants: Vec<String> = stored_grants
            .iter()
            .filter(|item| previous_permissions.contains(*item) && next_permissions.contains(*item))
            .cloned()
            .collect();
        let grants = sorted_set(&grants);
        let mut execution_hash = String::new();
        if let (Some(previous), Some(replacement)) = (previous.as_ref(), replacement) {
            if previous.package_digest == replacement.package_digest {
                execution_hash = py_str(&record.get("untrusted_execution_hash").cloned().unwrap_or(Value::Null));
            }
        }

        if grants.is_empty() && execution_hash.is_empty() {
            self.update_permission_record(plugin_id, None)
        } else {
            let record = json!({"grants": grants, "untrusted_execution_hash": execution_hash});
            self.update_permission_record(plugin_id, Some(record.as_object().cloned().unwrap()))
        }
    }

    fn plugin_directory(&self, plugin_id: &str) -> Result<PathBuf, String> {
        if !PLUGIN_ID_RE.is_match(plugin_id) {
            return Err("Небезопасный идентификатор плагина".to_owned());
        }
        let directory = self.root.join(plugin_id);
        if path_is_symlink(&directory) {
            return Err("Каталог плагина является символьной ссылкой".to_owned());
        }
        create_dir_private(&directory).map_err(|error| error.to_string())?;
        Ok(directory)
    }

    fn existing_plugin_directory(&self, plugin_id: &str) -> Result<PathBuf, String> {
        if !PLUGIN_ID_RE.is_match(plugin_id) {
            return Err("Небезопасный идентификатор плагина".to_owned());
        }
        let directory = self.root.join(plugin_id);
        if path_is_symlink(&directory) {
            return Err("Каталог плагина является символьной ссылкой".to_owned());
        }
        let legacy = directory.join(PLUGIN_MANIFEST);
        let v2 = directory.join(PLUGIN_V2_MANIFEST);
        let has_legacy = path_exists(&legacy) || path_is_symlink(&legacy);
        let has_v2 = path_exists(&v2) || path_is_symlink(&v2);
        if !path_is_dir(&directory) || !(has_legacy || has_v2) {
            return Err("Плагин не установлен".to_owned());
        }
        Ok(directory)
    }

    fn storage_path(&self, plugin_id: &str, key: &str) -> Result<PathBuf, String> {
        let normalized = key.trim();
        if !STORAGE_KEY_RE.is_match(normalized) {
            return Err("Небезопасный ключ storage".to_owned());
        }
        Ok(self.namespace(plugin_id)?.join("storage").join(format!("{normalized}.json")))
    }

    fn config_path(&self, plugin_id: &str) -> Result<PathBuf, String> {
        let path = self.namespace(plugin_id)?.join("config").join("config.yml");
        if path_is_symlink(&path) {
            return Err("config.yml плагина является символьной ссылкой".to_owned());
        }
        Ok(path)
    }
}
