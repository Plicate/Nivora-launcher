//! запуск игрового клиента

use super::*;

pub(crate) struct GameLauncher {
    config: ConfigStore,
    http: Arc<HttpClient>,
    runtime: JavaRuntimeManager,
    cancellation: Option<Arc<CancellationToken>>,
}

impl GameLauncher {
    /// токен отмены текущего запуска — нужен вызывающему коду для проверки статуса
    pub(crate) fn cancellation_token(&self) -> Option<&Arc<CancellationToken>> {
        self.cancellation.as_ref()
    }

    pub(crate) fn new(config: ConfigStore, http: Arc<HttpClient>, cancellation: Option<Arc<CancellationToken>>) -> Result<Self, String> {
        let runtime = JavaRuntimeManager::new(&config, Some(http.clone()))?;
        Ok(Self { config, http, runtime, cancellation })
    }

    fn resolved_graphics_mode(&self, launch_settings: Option<&Value>) -> String {
        let requested = launch_settings
            .and_then(|settings| settings.get("graphics_mode"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let configured = self.config.str_setting("graphics_mode");
        let configured = if configured.trim().is_empty() { "auto".to_owned() } else { configured.trim().to_lowercase() };
        if requested.is_empty() || requested == "auto" {
            configured
        } else {
            requested
        }
    }

    fn nvidia_gpu_available(&self) -> bool {
        let Some(nvidia_smi) = which("nvidia-smi") else {
            return false;
        };
        let mut command = platform::command(&nvidia_smi);
        command
            .args(["--query-gpu=name", "--format=csv,noheader"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear();
        for (name, value) in safe_process_environment(false) {
            command.env(name, value);
        }
        let Ok(output) = command.output() else {
            return false;
        };
        output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
    }

    fn wayland_glfw_library(&self, instance_dir: &Path) -> Result<PathBuf, String> {
        if machine_arch() != "x86_64" {
            return Err("Native Wayland GLFW fallback доступен только для Linux x86_64".to_owned());
        }
        let library_dir = security::ensure_instance_cell(instance_dir)?.cache.join("wayland-glfw-v1.1.0");
        if library_dir.is_symlink() || (library_dir.exists() && !library_dir.is_dir()) {
            return Err("Каталог native Wayland GLFW небезопасен".to_owned());
        }
        fs::create_dir_all(&library_dir).map_err(|error| format!("Каталог native Wayland GLFW недоступен: {error}"))?;
        platform::restrict_dir(&library_dir);
        let library = library_dir.join("libglfw.so");
        if library.is_symlink() || (library.exists() && !library.is_file()) {
            return Err("Файл native Wayland GLFW небезопасен".to_owned());
        }
        if self
            .http
            .download(WAYLAND_GLFW_URL, &library, None, Some(WAYLAND_GLFW_SHA256), None, Some(WAYLAND_GLFW_SIZE), None, None, None)
            .is_err()
        {
            return Err("Не удалось загрузить native Wayland GLFW".to_owned());
        }
        Ok(library)
    }

    fn build_command(
        &self,
        install: &InstallResult,
        account: &Map<String, Value>,
        launch_settings: &Value,
    ) -> Result<Vec<String>, String> {
        let java_override = launch_settings.get("java_path").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let java = if java_override.is_empty() {
            self.runtime.java_for(&install.version_json, None)?
        } else {
            self.runtime.java_for(&install.version_json, Some(&java_override))?
        };
        let mut game_assets = install.assets_root.clone();
        if install
            .version_json
            .get("assetIndex")
            .and_then(|index| index.get("map_to_resources"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            game_assets = install.instance_dir.join("resources");
        }
        let access_token = rec_str(account, "access_token");
        let account_kind = rec_str(account, "kind");
        let use_placeholder = matches!(account_kind.as_str(), "microsoft" | "ely") && !access_token.is_empty();
        let token_value = if use_placeholder { AUTH_TOKEN_PLACEHOLDER.to_owned() } else { access_token.clone() };
        let pathsep = if cfg!(windows) { ";" } else { ":" };
        let classpath = install
            .libraries
            .iter()
            .chain(std::iter::once(&install.client_jar))
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(pathsep);
        let mut placeholders = BTreeMap::new();
        placeholders.insert("auth_player_name".to_owned(), rec_str(account, "username"));
        placeholders.insert("version_name".to_owned(), install.version_id.clone());
        placeholders.insert("game_directory".to_owned(), install.instance_dir.to_string_lossy().to_string());
        placeholders.insert("assets_root".to_owned(), install.assets_root.to_string_lossy().to_string());
        placeholders.insert("game_assets".to_owned(), game_assets.to_string_lossy().to_string());
        placeholders.insert("assets_index_name".to_owned(), install.asset_index_name.clone());
        placeholders.insert("auth_uuid".to_owned(), rec_str(account, "uuid"));
        placeholders.insert("auth_access_token".to_owned(), token_value.clone());
        placeholders.insert("auth_session".to_owned(), token_value);
        placeholders.insert("auth_profile_id".to_owned(), rec_str(account, "uuid"));
        placeholders.insert("user_properties".to_owned(), "{}".to_owned());
        placeholders.insert(
            "user_type".to_owned(),
            match account_kind.as_str() {
                "microsoft" => "msa".to_owned(),
                "ely" => "mojang".to_owned(),
                _ => "legacy".to_owned(),
            },
        );
        placeholders.insert(
            "version_type".to_owned(),
            install
                .version_json
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("release")
                .to_owned(),
        );
        placeholders.insert("natives_directory".to_owned(), install.natives_dir.to_string_lossy().to_string());
        placeholders.insert("launcher_name".to_owned(), "Nivora Launcher".to_owned());
        placeholders.insert("launcher_version".to_owned(), self.config.str_setting("launcher_version"));
        placeholders.insert(
            "library_directory".to_owned(),
            install.instance_dir.join("libraries").to_string_lossy().to_string(),
        );
        placeholders.insert("classpath_separator".to_owned(), pathsep.to_owned());
        placeholders.insert("classpath".to_owned(), classpath);
        placeholders.insert("auth_xuid".to_owned(), rec_str(account, "xuid"));
        placeholders.insert("clientid".to_owned(), self.config.str_setting("client_id"));
        placeholders.insert(
            "resolution_width".to_owned(),
            setting_text(launch_settings, "window_width", "1280"),
        );
        placeholders.insert(
            "resolution_height".to_owned(),
            setting_text(launch_settings, "window_height", "720"),
        );
        placeholders.insert(
            "path".to_owned(),
            install
                .logging_file
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
        );

        let features = BTreeMap::from([("has_custom_resolution", true)]);
        let arguments = install.version_json.get("arguments").cloned().unwrap_or(Value::Null);
        let mut jvm_items: Vec<Value> = Vec::new();
        let empty = Vec::new();
        let jvm_source = arguments.get("jvm").and_then(Value::as_array).unwrap_or(&empty);
        jvm_items.extend(jvm_source.iter().cloned());
        let game_items: Vec<Value> = match arguments.get("game").and_then(Value::as_array) {
            Some(items) if !jvm_source.is_empty() => items.clone(),
            _ => match install.version_json.get("minecraftArguments").and_then(Value::as_str) {
                Some(legacy) => shlex_split(legacy, true).map(|tokens| tokens.into_iter().map(Value::String).collect()).unwrap_or_default(),
                None => Vec::new(),
            },
        };
        let mut jvm_args = expand_arguments(&Value::Array(jvm_items), &placeholders, &features);
        let mut game_args = expand_arguments(&Value::Array(game_items), &placeholders, &features);
        let profile_jvm_args = safe_extra_arguments(launch_settings.get("jvm_arguments"));
        let profile_game_args = safe_extra_arguments(launch_settings.get("game_arguments"));
        if !profile_jvm_args.is_empty() {
            let mut merged = profile_jvm_args;
            merged.extend(jvm_args);
            jvm_args = merged;
        }
        if !profile_game_args.is_empty() {
            game_args.extend(profile_game_args);
        }
        // пресет JVM: сначала снять чужие GC/экспериментальные флаги
        let preset_id = launch_settings
            .get("jvm_preset")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .trim();
        let preset_id = if preset_id.is_empty() { "none" } else { preset_id };
        let preset_flags = optimization::preset_flags(preset_id);
        if !preset_flags.is_empty() {
            const GC_PREFIXES: [&str; 4] = ["-XX:", "-Dusing.aikars", "-Daikars", "-Dfml.earlyprogresswindow"];
            jvm_args.retain(|argument| !GC_PREFIXES.iter().any(|prefix| argument.starts_with(prefix)));
            let mut merged: Vec<String> = preset_flags.into_iter().map(str::to_owned).collect();
            merged.extend(jvm_args);
            jvm_args = merged;
        }
        let graphics_mode = self.resolved_graphics_mode(Some(launch_settings));
        if is_legacy_version(&install.version_id) {
            let mut compatibility_args = vec![
                "-XX:+IgnoreUnrecognizedVMOptions".to_owned(),
                "-Dorg.lwjgl.opengl.Display.useXRandR=false".to_owned(),
            ];
            // для LWJGL2 программный OpenGL включаем только в software-режиме,
            // чтобы сбой аппаратного рендеринга не вызвал незаметного снижения производительности
            if graphics_mode == "software" {
                compatibility_args.insert(1, "-Dorg.lwjgl.opengl.Display.allowSoftwareOpenGL=true".to_owned());
            }
            let mut merged = compatibility_args;
            merged.extend(jvm_args);
            jvm_args = merged;
        }
        if launch_settings.get("fullscreen").and_then(Value::as_bool).unwrap_or(false) && !game_args.iter().any(|value| value == "--fullscreen") {
            game_args.push("--fullscreen".to_owned());
        }
        let quick_world = launch_settings.get("quick_play_world").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let quick_server = launch_settings.get("quick_play_server").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        if !quick_world.is_empty() {
            game_args.extend(["--quickPlaySingleplayer".to_owned(), quick_world]);
        } else if !quick_server.is_empty() {
            game_args.extend(["--quickPlayMultiplayer".to_owned(), quick_server]);
        }
        if graphics_mode == "software"
            && !jvm_args
                .iter()
                .any(|argument| argument == "-Dorg.lwjgl.opengl.Display.allowSoftwareOpenGL=true")
        {
            jvm_args.insert(0, "-Dorg.lwjgl.opengl.Display.allowSoftwareOpenGL=true".to_owned());
        }
        if matches!(graphics_mode.as_str(), "" | "auto" | "software") && is_legacy_version(&install.version_id) {
            let argument = "-Dorg.lwjgl.opengl.Display.useXRandR=false";
            if !jvm_args.iter().any(|value| value == argument) {
                jvm_args.insert(0, argument.to_owned());
            }
        }
        let jvm_args = ensure_native_library_path(jvm_args, &install.natives_dir);
        let memory = {
            let requested = launch_settings.get("memory").and_then(Value::as_str).unwrap_or("");
            if requested.trim().is_empty() {
                self.config.str_setting("memory")
            } else {
                requested.to_owned()
            }
        };
        let mut jvm_args = apply_memory_setting(&jvm_args, &memory)?;
        let classpath = placeholders.get("classpath").cloned().unwrap_or_default();
        if !jvm_args.iter().any(|value| value == "-cp" || value == "-classpath") {
            jvm_args.extend(["-cp".to_owned(), classpath]);
        }
        let main_class = install
            .version_json
            .get("mainClass")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("У версии {} отсутствует mainClass", install.version_id))?
            .to_owned();
        let mut command = vec![java.to_string_lossy().to_string()];
        command.extend(jvm_args);
        command.push(main_class);
        command.extend(game_args);
        Ok(command)
    }

    /// формирует окружение графики, включая PRIME/GLVND и настройки экземпляра
    pub(crate) fn launch_environment(&self, launch_settings: &Value, version_id: Option<&str>) -> BTreeMap<String, String> {
        let security_profile = launch_settings
            .get("security_profile")
            .and_then(Value::as_str)
            .unwrap_or("strict")
            .trim()
            .to_lowercase();
        let security_profile = if security_profile.is_empty() { "strict".to_owned() } else { security_profile };
        let mut environment = BTreeMap::new();
        for (name, value) in safe_process_environment(matches!(security_profile.as_str(), "compat" | "off")) {
            environment.insert(name, value);
        }
        let mut mode = self.resolved_graphics_mode(Some(launch_settings));
        if mode == "auto" && self.nvidia_gpu_available() {
            mode = "nvidia".to_owned();
        }
        // убираем унаследованные настройки программного рендеринга; для режима software задаём их ниже
        for key in [
            "__GLX_VENDOR_LIBRARY_NAME",
            "MESA_LOADER_DRIVER_OVERRIDE",
            "GALLIUM_DRIVER",
            "LIBGL_ALWAYS_SOFTWARE",
            "LIBGL_DRI3_DISABLE",
            "LIBGL_ALWAYS_INDIRECT",
            "MESA_GL_VERSION_OVERRIDE",
            "MESA_GLSL_VERSION_OVERRIDE",
            "__NV_PRIME_RENDER_OFFLOAD",
            "__VK_LAYER_NV_optimus",
        ] {
            environment.remove(key);
        }
        sanitize_inherited_ld_library_path(&mut environment);
        if matches!(mode.as_str(), "mesa" | "software") {
            environment.remove("__NV_PRIME_RENDER_OFFLOAD");
            environment.remove("__VK_LAYER_NV_optimus");
        }
        match mode.as_str() {
            "nvidia" => {
                environment.insert("__NV_PRIME_RENDER_OFFLOAD".to_owned(), "1".to_owned());
                environment.insert("__GLX_VENDOR_LIBRARY_NAME".to_owned(), "nvidia".to_owned());
            }
            "mesa" => {
                environment.insert("__GLX_VENDOR_LIBRARY_NAME".to_owned(), "mesa".to_owned());
            }
            "software" => {
                environment.insert("__GLX_VENDOR_LIBRARY_NAME".to_owned(), "mesa".to_owned());
                environment.insert("LIBGL_ALWAYS_SOFTWARE".to_owned(), "1".to_owned());
                environment.insert("GALLIUM_DRIVER".to_owned(), "llvmpipe".to_owned());
                environment.remove("MESA_LOADER_DRIVER_OVERRIDE");
                environment.insert("LIBGL_DRI3_DISABLE".to_owned(), "1".to_owned());
                environment.insert("LIBGL_ALWAYS_INDIRECT".to_owned(), "0".to_owned());
                if is_legacy_version(version_id.unwrap_or("")) {
                    environment.insert("MESA_GL_VERSION_OVERRIDE".to_owned(), "2.1".to_owned());
                    environment.insert("MESA_GLSL_VERSION_OVERRIDE".to_owned(), "120".to_owned());
                } else {
                    environment.insert("MESA_GL_VERSION_OVERRIDE".to_owned(), "3.3".to_owned());
                    environment.insert("MESA_GLSL_VERSION_OVERRIDE".to_owned(), "330".to_owned());
                }
            }
            _ => {}
        }
        environment.insert(
            "TRIAL_LAUNCHER_GRAPHICS_MODE".to_owned(),
            if mode.is_empty() { "auto".to_owned() } else { mode },
        );
        merge_launch_environment(&mut environment, launch_settings);
        environment
    }

    /// запускает игру, передаёт журнал в `sink` и сохраняет PID для бенчмарка шейдеров
    pub(crate) fn run(
        &self,
        install: &InstallResult,
        account: &Map<String, Value>,
        dry_run: bool,
        launch_id: &str,
        log_id: &str,
        sink: Arc<Mutex<dyn FnMut(&str) + Send>>,
        launch_settings: &Value,
        pid_slot: Option<&Arc<Mutex<Option<i32>>>>,
    ) -> Result<i32, String> {
        let publish = |line: &str| {
            if let Ok(mut guard) = sink.lock() {
                guard(line);
            }
        };
        let account_kind = rec_str(account, "kind");
        if account_kind == "offline" && matches!(accounts::offline_remaining(account), Some(0)) {
            return Err("Ограничение автономного аккаунта исчерпано. Используйте Microsoft-авторизацию.".to_owned());
        }
        let access_token = rec_str(account, "access_token");
        let mut command = materialize_auth_token(&self.build_command(install, account, launch_settings)?, &access_token);
        {
            let mut emit = |line: &str| publish(line);
            command = adapt_command_for_old_java(&command, &mut emit);
        }
        if dry_run {
            let secrets = [access_token.as_str(), AUTH_TOKEN_PLACEHOLDER];
            publish(&shell_join(&redact_command(&command, &secrets)));
            return Ok(0);
        }
        let logs_dir = self.config.paths.logs_dir.clone();
        fs::create_dir_all(&logs_dir).map_err(|error| format!("Каталог логов недоступен: {error}"))?;
        let log_path = logs_dir.join(log_id);
        let live_state_path = install.instance_dir.join(".nivora-launcher-live.json");
        if live_state_path.is_symlink() || log_path.is_symlink() {
            return Err("Файлы состояния запуска являются символьными ссылками".to_owned());
        }
        ensure_private_file(&log_path);
        let launch_preset = launch_settings
            .get("jvm_preset")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .trim()
            .to_owned();
        let launch_preset = if launch_preset.is_empty() { "none".to_owned() } else { launch_preset };
        let launch_memory = {
            let requested = launch_settings.get("memory").and_then(Value::as_str).unwrap_or("");
            if requested.trim().is_empty() {
                self.config.str_setting("memory")
            } else {
                requested.to_owned()
            }
        };
        optimization::record_launch_start(
            &install.instance_dir,
            launch_id,
            &install.version_id,
            &launch_preset,
            &launch_memory,
            &install.version_id,
        );
        let launch_wall_start = Instant::now();
        let started = Instant::now();
        let mut environment = self.launch_environment(launch_settings, Some(&install.version_id));
        let event = |line: &str| {
            if let Ok(mut file) = OpenOptions::new().write(true).create(true).append(true).open(&log_path) {
                let _ = writeln!(file, "{line}");
            }
            publish(line);
        };
        event(&format!("Лог запуска: {}", log_path.display()));
        event(&format!("Запуск {} для {}", install.version_id, rec_str(account, "username")));
        let version_json = &install.version_json;
        let is_wayland = is_wayland_session(&environment, &install.version_id);
        let has_custom_glfw = command[1..].iter().any(|argument| argument.starts_with("-Dorg.lwjgl.glfw.libname="));
        let mut native_wayland_first = environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").map(String::as_str) == Some("nvidia") && is_wayland && !has_custom_glfw;
        let mut native_wayland_unavailable = false;
        let java_metadata = version_json.get("javaVersion").cloned().unwrap_or(Value::Null);
        let required_java_major = java_metadata.get("majorVersion").and_then(Value::as_i64);
        let actual_java_major = if required_java_major.is_some() { java_version(Path::new(&command[0])) } else { None };
        if let Some(actual) = actual_java_major {
            let mut java_line = format!("Java: {actual}");
            if let Some(required) = required_java_major {
                java_line.push_str(&format!("; Minecraft metadata: {required}"));
                if actual != required {
                    let configured_java = !launch_settings
                        .get("java_path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                        || !self.config.str_setting("java_path").trim().is_empty();
                    let source = if configured_java { "выбранная вручную" } else { "системная" };
                    event(&format!(
                        "Предупреждение: {source} Java {actual} отличается от Java {required} из метаданных версии; совместимость не гарантирована."
                    ));
                }
            }
            event(&java_line);
        }
        let has_token = matches!(account_kind.as_str(), "microsoft" | "ely") && !access_token.is_empty();
        let mut argument_file_java = actual_java_major;
        if argument_file_java.is_none() && (has_token || cfg!(windows)) {
            argument_file_java = java_version(Path::new(&command[0]));
        }
        let use_argument_file = should_use_argument_file(has_token, argument_file_java);
        if has_token && !use_argument_file {
            event("Java 8 не поддерживает файлы аргументов (@argfile): параметры передаются напрямую.");
        }
        let mut captured: Vec<String> = Vec::new();
        let mut wrong_nvidia_renderer = false;
        let mut java_argument_files: Vec<JavaArgumentFile> = Vec::new();
        let original_command = command.clone();
        let mut launch_command = command.clone();
        let mut process_command = launch_command.clone();
        let wrapper_tokens = {
            let mut emit = |line: &str| event(line);
            launch_wrapper_tokens(launch_settings, &mut emit)
        };
        if !wrapper_tokens.is_empty() {
            let label = Path::new(&wrapper_tokens[0])
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| wrapper_tokens[0].clone());
            let mut parts = vec![label];
            parts.extend(wrapper_tokens[1..].iter().cloned());
            event(&format!("Обёртка запуска: {}", shell_join(&parts)));
        }
        let roots = launcher_roots();
        let instance_name = install
            .instance_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| install.version_id.clone());
        let mut process_lock = InstanceProcessLock::new(&roots.data_home, &instance_name);
        process_lock.acquire()?;
        let pre_launch = {
            let mut emit = |line: &str| event(line);
            run_user_launch_command(launch_settings, "pre_launch_command", &install.instance_dir, &mut emit, true)
        };
        if let Err(error) = pre_launch {
            process_lock.release();
            return Err(error);
        }
        write_live_state(&install.instance_dir, true, 0, None, epoch_seconds(), log_id, launch_id);
        let cancellation = self.cancellation.clone();
        let mut return_code: Option<i32> = None;
        let launch_result: Result<i32, String> = (|| {
            if native_wayland_first {
                match self.wayland_glfw_library(&install.instance_dir).and_then(|library| {
                    let main_class = version_json.get("mainClass").and_then(Value::as_str).unwrap_or_default();
                    native_wayland_command(&command, main_class, &library)
                }) {
                    Ok(native_command) => {
                        command = native_command;
                        if use_argument_file {
                            match create_java_argument_file(&command[1..], &install.instance_dir, Some(&roots.data_home)) {
                                Ok(argument_file) => {
                                    launch_command = vec![command[0].clone(), argument_file.argument.clone()];
                                    java_argument_files.push(argument_file);
                                }
                                Err(_) => launch_command = command.clone(),
                            }
                        } else {
                            launch_command = command.clone();
                        }
                        process_command = launch_command.clone();
                        native_wayland_environment(&mut environment);
                        event("NVIDIA + Wayland: использую native Wayland GLFW с первой попытки, без предварительного GLX.");
                    }
                    Err(error) => {
                        native_wayland_unavailable = true;
                        native_wayland_first = false;
                        command = original_command.clone();
                        launch_command = command.clone();
                        process_command = launch_command.clone();
                        event(&format!("Native Wayland GLFW недоступен; пробую обычный запуск через GLX: {error}"));
                    }
                }
            }
            let graphics_summary = format!(
                "Графика: mode={}, GLX={}, PRIME={}, GLFW={}, session={}, legacy_lwjgl={}",
                environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").cloned().unwrap_or_else(|| self.config.str_setting("graphics_mode")),
                environment.get("__GLX_VENDOR_LIBRARY_NAME").cloned().unwrap_or_else(|| "system".to_owned()),
                if environment.get("__NV_PRIME_RENDER_OFFLOAD").map(String::as_str) == Some("1") { "on" } else { "off" },
                environment.get("TRIAL_LAUNCHER_GRAPHICS_BACKEND").cloned().unwrap_or_else(|| "bundled".to_owned()),
                environment.get("XDG_SESSION_TYPE").cloned().unwrap_or_else(|| "unknown".to_owned()),
                if is_legacy_version(&install.version_id) { "yes" } else { "no" },
            );
            event(&graphics_summary);
            let mut current_argument_file: Option<usize> = None;
            if use_argument_file {
                let argument_file = create_java_argument_file(&command[1..], &install.instance_dir, Some(&roots.data_home))?;
                launch_command = vec![command[0].clone(), argument_file.argument.clone()];
                process_command = launch_command.clone();
                java_argument_files.push(argument_file);
                current_argument_file = Some(java_argument_files.len() - 1);
            }
            let prepared = security::prepare_process(
                &process_command,
                &install.instance_dir,
                &environment,
                launch_settings,
                current_argument_file.and_then(|index| java_argument_files.get(index)).and_then(|file| file.fd),
                current_argument_file.map(|_| JAVA_ARGUMENT_FILE_TARGET),
            )?;
            if let Some(warning) = &prepared.warning {
                event(&format!("Безопасность: {warning}"));
            }
            let code = run_process(RunProcess {
                log_path: &log_path,
                instance_dir: &install.instance_dir,
                log_id,
                launch_id,
                captured: &mut captured,
                sink: &sink,
                wrapper_tokens: &wrapper_tokens,
                process_command: prepared.command,
                process_cwd: prepared.cwd,
                process_environment: prepared.environment,
                argument_fd: current_argument_file.and_then(|index| java_argument_files.get(index)).and_then(|file| file.fd),
                cancellation: cancellation.clone(),
                pid_slot,
                append_log: true,
                wrong_nvidia_renderer: &mut wrong_nvidia_renderer,
                nvidia_mode: environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").map(String::as_str) == Some("nvidia"),
            })?;
            let mut return_code_value = code;
            let requested_mode = self.resolved_graphics_mode(Some(launch_settings));
            if return_code_value != 0
                && requested_mode != "software"
                && looks_like_glx_context_error(&captured)
            {
                if environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").map(String::as_str) == Some("nvidia") {
                    event("NVIDIA не создала OpenGL-контекст; не переключаю запуск на llvmpipe.");
                } else {
                    if is_wayland && !has_custom_glfw && !native_wayland_unavailable {
                        let mut native_argument_file: Option<JavaArgumentFile> = None;
                        let retry = self
                            .wayland_glfw_library(&install.instance_dir)
                            .and_then(|library| {
                                let main_class = version_json.get("mainClass").and_then(Value::as_str).unwrap_or_default();
                                let native_command = native_wayland_command(&command, main_class, &library)?;
                                if current_argument_file.is_some() {
                                    native_argument_file = Some(create_java_argument_file(
                                        &native_command[1..],
                                        &install.instance_dir,
                                        Some(&roots.data_home),
                                    )?);
                                }
                                Ok(native_command)
                            });
                        match retry {
                            Ok(native_command) => {
                                let native_launch_command = match &native_argument_file {
                                    Some(file) => vec![native_command[0].clone(), file.argument.clone()],
                                    None => native_command.clone(),
                                };
                                let mut native_environment = self.launch_environment(launch_settings, Some(&install.version_id));
                                native_wayland_environment(&mut native_environment);
                                let native_prepared = security::prepare_process(
                                    &native_launch_command,
                                    &install.instance_dir,
                                    &native_environment,
                                    launch_settings,
                                    native_argument_file.as_ref().and_then(|file| file.fd),
                                    native_argument_file.as_ref().map(|_| JAVA_ARGUMENT_FILE_TARGET),
                                );
                                match native_prepared {
                                    Ok(prepared) => {
                                        event("GLX не создал контекст; пробую native Wayland GLFW v1.1.0 с проверкой SHA-256.");
                                        if let Some(warning) = &prepared.warning {
                                            event(&format!("Безопасность: {warning}"));
                                        }
                                        match run_process(RunProcess {
                                            log_path: &log_path,
                                            instance_dir: &install.instance_dir,
                                            log_id,
                                            launch_id,
                                            captured: &mut captured,
                                            sink: &sink,
                                            wrapper_tokens: &wrapper_tokens,
                                            process_command: prepared.command,
                                            process_cwd: prepared.cwd,
                                            process_environment: prepared.environment,
                                            argument_fd: native_argument_file.as_ref().and_then(|file| file.fd),
                                            cancellation: cancellation.clone(),
                                            pid_slot,
                                            append_log: true,
                                            wrong_nvidia_renderer: &mut wrong_nvidia_renderer,
                                            nvidia_mode: native_environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").map(String::as_str) == Some("nvidia"),
                                        }) {
                                            Ok(code) => return_code_value = code,
                                            Err(error) => event(&format!("Native Wayland GLFW retry недоступен: {error}")),
                                        }
                                    }
                                    Err(error) => event(&format!("Native Wayland GLFW retry недоступен: {error}")),
                                }
                            }
                            Err(error) => event(&format!("Native Wayland GLFW retry недоступен: {error}")),
                        }
                        if let Some(mut file) = native_argument_file {
                            file.close();
                        }
                    }
                    if return_code_value != 0 {
                        let mut retry_settings = launch_settings.clone();
                        retry_settings["graphics_mode"] = json!("software");
                        let retry_environment = self.launch_environment(&retry_settings, Some(&install.version_id));
                        event(&format!(
                            "Графический запуск не удался; повторяю запуск в явно отмеченном режиме {}/llvmpipe.",
                            retry_environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").cloned().unwrap_or_else(|| "software".to_owned())
                        ));
                        event(
                            "ВНИМАНИЕ: это программный рендер без GPU — FPS будет низким. Аппаратный OpenGL не создался: проверьте драйверы, режим графики в настройках экземпляра и профиль безопасности (кастомная strict-политика без права desktop:display не пробрасывает /dev/dri в песочницу).",
                        );
                        // при повторном запуске добавляем allowSoftwareOpenGL для LWJGL2
                        let mut retry_command =
                            materialize_auth_token(&self.build_command(install, account, &retry_settings)?, &access_token);
                        {
                            let mut emit = |line: &str| event(line);
                            retry_command = adapt_command_for_old_java(&retry_command, &mut emit);
                        }
                        let mut retry_argument_file: Option<JavaArgumentFile> = None;
                        if current_argument_file.is_some() {
                            retry_argument_file = Some(create_java_argument_file(
                                &retry_command[1..],
                                &install.instance_dir,
                                Some(&roots.data_home),
                            )?);
                        }
                        let retry_prepared = security::prepare_process(
                            &retry_command,
                            &install.instance_dir,
                            &retry_environment,
                            &retry_settings,
                            retry_argument_file.as_ref().and_then(|file| file.fd),
                            retry_argument_file.as_ref().map(|_| JAVA_ARGUMENT_FILE_TARGET),
                        )?;
                        if let Some(warning) = &retry_prepared.warning {
                            event(&format!("Безопасность: {warning}"));
                        }
                        return_code_value = run_process(RunProcess {
                            log_path: &log_path,
                            log_id,
                            launch_id,
                            instance_dir: &install.instance_dir,
                            captured: &mut captured,
                            sink: &sink,
                            wrapper_tokens: &wrapper_tokens,
                            process_command: retry_prepared.command,
                            process_cwd: retry_prepared.cwd,
                            process_environment: retry_prepared.environment,
                            argument_fd: retry_argument_file.as_ref().and_then(|file| file.fd),
                            cancellation: cancellation.clone(),
                            pid_slot,
                            append_log: true,
                            wrong_nvidia_renderer: &mut wrong_nvidia_renderer,
                            nvidia_mode: retry_environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").map(String::as_str) == Some("nvidia"),
                        })?;
                        if let Some(mut file) = retry_argument_file {
                            file.close();
                        }
                    }
                }
            }
            if wrong_nvidia_renderer {
                event("Запуск остановлен: NVIDIA была выбрана, но игра сообщила OpenGL renderer не NVIDIA.");
            } else if return_code_value != 0
                && self.resolved_graphics_mode(Some(launch_settings)) != "software"
                && is_wayland
                && !looks_like_glx_context_error(&captured)
            {
                if native_wayland_first {
                    event("Native Wayland NVIDIA запуск завершился без подтверждённого NVIDIA renderer.");
                } else {
                    event(
                        "Игра завершилась с ошибкой, но в выводе нет распознанного GLX-сбоя; native Wayland retry не запускался. Нужен конец лога запуска.",
                    );
                }
            }
            return_code = Some(return_code_value);
            Ok(return_code_value)
        })();
        // при любом результате обновляем состояние, снимаем блокировку и завершаем учёт запуска
        let exit_code_for_state = match return_code {
            Some(code) => Some(code as i64),
            None => return_code_for_live_state(&launch_result),
        };
        write_live_state(&install.instance_dir, false, 0, exit_code_for_state, 0.0, log_id, launch_id);
        process_lock.release();
        {
            let mut emit = |line: &str| publish(line);
            if let Err(error) = run_user_launch_command(launch_settings, "post_launch_command", &install.instance_dir, &mut emit, false) {
                publish(&format!("Команда после запуска: {error}"));
            }
        }
        for mut argument_file in java_argument_files {
            argument_file.close();
        }
        optimization::record_launch_end(
            &install.instance_dir,
            launch_id,
            launch_result.as_ref().copied().unwrap_or(-1),
            launch_wall_start.elapsed().as_secs_f64(),
            None,
        );
        let played_seconds = started.elapsed().as_secs().max(0) as i64;
        if let Ok(store) = InstanceStore::new(&self.config) {
            let _ = store.add_played_seconds(&install.instance_dir, played_seconds);
        }
        if account_kind == "offline" {
            let remaining = accounts::offline_finish(&roots.config_home, account, played_seconds);
            match remaining {
                Some(value) => publish(&format!("Осталось debug-времени: {} мин {} сек", value / 60, value % 60)),
                None => publish("Осталось debug-времени: без лимита"),
            }
        }
        launch_result
    }
}

