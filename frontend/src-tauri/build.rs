fn main() {
    // Встроенный zip бэкенда (portable nivora-launcher.exe) подключается,
    // только если build_windows_exe.sh / CI его подготовили. Без архива
    // сборка не падает на include_bytes!, а приложение ищет бэкенд рядом с
    // собой (установщик) и честно сообщает, если его нет.
    println!("cargo::rustc-check-cfg=cfg(embedded_backend)");
    let archive = std::path::Path::new("resources/backend/nivora-launcher-backend.zip");
    println!("cargo:rerun-if-changed=resources/backend/nivora-launcher-backend.zip");
    let target_windows = std::env::var("CARGO_CFG_WINDOWS").is_ok();
    if target_windows && archive.is_file() {
        println!("cargo:rustc-cfg=embedded_backend");
    } else if target_windows && std::env::var("NIVORA_REQUIRE_EMBEDDED_BACKEND").as_deref() == Ok("1") {
        panic!("NIVORA_REQUIRE_EMBEDDED_BACKEND=1, но {} не найден", archive.display());
    }
    tauri_build::build()
}
