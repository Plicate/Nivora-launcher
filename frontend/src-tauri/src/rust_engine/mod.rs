// команды GUI-движка
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

mod accounts;
mod backups;
mod block_icons;
mod bootstrap;
mod cloud;
mod fs_browser;
mod github_plugins;
mod graphics;
mod wallpapers;
pub mod bedrock;
mod config;
mod constants;
mod crash_reports;
pub mod events;
mod installer;
mod instance_files;
mod instances;
mod instance_versions;
pub mod global_search;
mod instance_workspace;
pub mod world_import;
pub mod instance_guard;
mod loaders;
mod metadata;
mod metrics;
mod mod_dependencies;
mod auto_repair;
mod mod_audit;
mod server_files;
mod launcher;
mod shader_benchmark;
mod autotune;
mod hardware;
mod instance_manifest;
mod instance_compare;
mod instance_pins;
mod opener;
mod options_file;
mod plugin_dev;
mod modcat;
mod modpack_export;
mod modpacks;
mod mods;
mod mods_ops;
mod mod_list;
mod net;
mod operations;

pub(crate) fn ensure_runtime_dir() -> Result<PathBuf, String> {
    operations::ensure_runtime_dir()
}
mod ops;
mod optimization;
mod plugin_catalogs;
mod plugin_guard;
mod plugin_ops;
mod plugin_reload;
mod plugin_security;
mod plugins;
mod portable_fs;
mod profiles;
pub mod registry;
mod release_check;
mod runtime;
mod safe_update;
mod server;
mod server_network;
mod server_traffic;
mod snapshots;
mod process_guard;
mod recorder;
mod skins;
pub mod shortcuts;
mod platform;
mod platform_support;
mod security;
// типы AppContainer используются только в Windows-ветках
#[cfg_attr(not(windows), allow(dead_code))]
mod windows_sandbox;
mod turbo;
pub mod shader_analysis;
pub mod state;
mod system;
mod themes;
mod upnp;

pub const NATIVE_OPS: &[&str] = &[
    "bootstrap",
    "hash_file",
    "hash_directory",
    "fs_browse",
    "fs_read_file",
    "fs_write_file",
    "fs_create_file",
    "fs_create_directory",
    "fs_rename",
    "fs_delete",
    "fs_save_text_file",
    "fs_open_file",
    "fs_reveal",
    "instance_file_list",
    "instance_file_read",
    "instance_file_write",
    "instance_file_mkdir",
    "instance_file_upload",
    "instance_file_delete",
    "instance_log_archives",
    "instance_log_archive_read",
    "instance_groups",
    "create_instance_group",
    "delete_instance_group",
    "rename_instance_group",
    "delete_instance",
    "duplicate_instance",
    "update_instance_profile",
    "instance_icon_blocks",
    "instance_details",
    "save_instance_settings",
    "remove_instance_asset",
    "toggle_mod",
    "import_asset",
    "mods_remove_files",
    "mods_remove",
    "instance_versions",
    "instance_version_override_save",
    "instance_version_import",
    "instance_repair_version",
    "save_settings",
    "plugin_list",
    "java_validate",
    "bedrock_runtime_status",
    "bedrock_versions",
    "bedrock_setup_version",
    "bedrock_setup_status",
    "bedrock_setup_cancel",
    "bedrock_store_login",
    "bedrock_store_login_status",
    "bedrock_store_login_cancel",
    "bedrock_account_status",
    "bedrock_screenshots_list",
    "bedrock_create_instance",
    "bedrock_content_list",
    "bedrock_content_import",
    "bedrock_content_remove",
    "bedrock_stop",
    "engine_info",
    "system_metrics",
    "overlay_state",
    "jvm_presets_list",
    "launch_history",
    "cache_stats",
    "cache_prune",
    "theme_list",
    "theme_preview",
    "theme_save",
    "theme_import",
    "theme_delete",
    "theme_export",
    "github_theme_search",
    "github_theme_install",
    "settings_profiles_list",
    "settings_profile_delete",
    "settings_profile_export",
    "settings_profile_import",
    "settings_profile_save",
    "settings_profile_apply",
    "wallpaper_import",
    "wallpaper_clear",
    "java_options",
    "security_status",
    "server_cores",
    "world_backup_list",
    "world_backup_create",
    "world_backup_restore",
    "world_backup_delete",
    "world_backup_schedule_get",
    "world_backup_schedule_set",
    "create_offline_account",
    "delete_account",
    "set_default_account",
    "move_account",
    "login_microsoft",
    "login_ely",
    "microsoft_login_status",
    "install",
    "server_start",
    "server_stop",
    "server_restart",
    "server_command",
    "server_log",
    "server_status",
    "mod_dependency_graph",
    "mod_removal_impact",
    "external_instances_scan",
    "external_instance_import",
    "server_network_diagnose",
    "server_network_status",
    "server_network_prepare",
    "server_network_open",
    "server_network_close",
    "plugin_api_quota_usage",
    "plugin_dev_changes",
    "versions",
    "loader_choices",
    "server_core_versions",
    "check_remote_release",
    "shader_analyze",
    "global_search",
    "world_import_path",
    "world_import_begin",
    "world_import_chunk",
    "world_import_cancel",
    "world_import_finish",
    "recording_start",
    "recording_stop",
    "recording_status",
    "cloud_status",
    "cloud_configure",
    "cloud_test",
    "cloud_list",
    "cloud_sync",
    "cloud_restore",
    "cloud_delete",
    "cloud_disconnect",
    "github_plugin_status",
    "github_plugin_search",
    "github_plugin_details",
    "github_plugin_install",
    "github_plugin_oauth_start",
    "github_plugin_oauth_poll",
    "github_plugin_sign_out",
    "github_plugin_publish",
    "launch",
    "launch_instance",
    "shader_benchmark_start",
    "shader_benchmark_status",
    "skins_list",
    "skins_import",
    "skins_export",
    "skin_profile",
    "apply_skin",
    "clear_skin",
    "apply_cape",
    "clear_cape",
    "open_skins_folder",
    "skins_folder",
    "mods_search",
    "mods_details",
    "mods_check_updates",
    "mods_install",
    "mods_update",
    "mods_update_all_safe",
    "mods_update_rollback",
    "mods_update_sessions",
    "modpack_import",
    "optimization_analyze",
    "optimization_apply",
    "graphics_diagnostics",
    "instance_shortcut_status",
    "instance_shortcut_create",
    "instance_shortcut_remove",
    "shortcut_take_launch_request",
    "export_modpack",
    "instance_mod_list_export",
    "instance_autotune",
    "auto_repair_diagnose",
    "auto_repair_mute",
    "auto_repair_apply",
    "plugin_dev_init",
    "plugin_dev_lint",
    "plugin_dev_pack",
    "plugin_dev_verify",
    "plugin_dev_keypair",
    "instance_snapshot_create",
    "instance_snapshot_delete",
    "instance_snapshot_list",
    "instance_snapshot_restore",
    "instance_log",
    "instance_compare",
    "instance_port_changes",
    "instance_pins_get",
    "instance_pin_toggle",
    "open_external",
    "open_folder",
    "system_notify",
    "mod_audit_prepare",
    "mod_audit_accept",
    "mod_audit_cancel",
    "mod_audit_status",
    "server_file_list",
    "server_file_read",
    "server_file_write",
    "server_file_mkdir",
    "server_file_upload",
    "server_file_delete",
    "server_snapshot_list",
    "server_snapshot_create",
    "server_snapshot_delete",
    "server_snapshot_restore",
    "server_details",
    "server_save_config",
    "server_plugin_catalogs",
    "save_server_plugin_catalogs",
    "server_plugin_search",
    "server_plugin_details",
    "server_plugin_updates",
    "server_plugin_update",
    "server_plugin_install",
    "server_plugin_remove",
    "plugin_security_status",
    "plugin_permission_grants_set",
    "plugin_local_untrusted_execution",
    "plugin_publisher_pin",
    "plugin_publisher_unpin",
    "plugin_publisher_revocation_authority_set",
    "plugin_revocations_import",
    "plugin_local_revoke",
    "plugin_local_unrevoke",
    "plugin_install",
    "plugin_set_enabled",
    "plugin_uninstall",
    "plugin_api_source",
    "plugin_api_runtime_info",
    "plugin_api_operation_trace",
    "plugin_api_capabilities",
    "plugin_api_permissions_get",
    "plugin_api_permissions_set",
    "plugin_api_authorize",
    "plugin_api_open_external",
    "plugin_api_instances_list",
    "plugin_api_process_metrics",
    "plugin_api_config_projected_read",
    "plugin_api_files_list",
    "plugin_api_files_read",
    "plugin_api_files_write",
    "plugin_api_files_remove",
    "plugin_network_fetch",
    "plugin_http_request",
    "plugin_network_dns",
    "plugin_network_ping",
    "plugin_network_port_check",
    "plugin_api_proxy_routes_get",
    "plugin_api_proxy_routes_save",
    "plugin_api_proxy_routes_add",
    "plugin_api_proxy_routes_remove",
    "plugin_server_list_get",
    "plugin_server_list_save",
    "plugin_server_list_add",
    "plugin_server_list_remove",
    "plugin_storage_get",
    "plugin_storage_set",
    "plugin_storage_remove",
    "plugin_config_get",
    "plugin_config_set",
    "plugin_config_remove",
    "plugin_file_list",
    "plugin_file_read",
    "plugin_file_write",
    "plugin_file_remove",
    "plugin_database_query",
    "plugin_database_execute",
    "server_create",
    "server_upload_core",
    "server_core_download",
];

#[cfg_attr(not(test), allow(dead_code))]
pub fn is_native(operation: &str) -> bool {
    NATIVE_OPS.contains(&operation)
}

pub fn handle(operation: &str, request: &Value) -> Result<Value, String> {
    match operation {
        "bootstrap" => Ok(ok_response(operation, bootstrap::bootstrap(request)?)),
        "hash_file" => hash_file(request),
        "hash_directory" => hash_directory(request),
        "fs_browse" => Ok(ok_response(operation, fs_browser::fs_browse(request)?)),
        "fs_read_file" => Ok(ok_response(operation, fs_browser::fs_read_file(request)?)),
        "fs_write_file" => Ok(ok_response(operation, fs_browser::fs_write_file(request)?)),
        "fs_create_file" => Ok(ok_response(operation, fs_browser::fs_create_file(request)?)),
        "fs_create_directory" => Ok(ok_response(operation, fs_browser::fs_create_directory(request)?)),
        "fs_rename" => Ok(ok_response(operation, fs_browser::fs_rename(request)?)),
        "fs_delete" => Ok(ok_response(operation, fs_browser::fs_delete(request)?)),
        "fs_save_text_file" => Ok(ok_response(operation, fs_browser::fs_save_text_file(request)?)),
        "fs_open_file" => Ok(ok_response(operation, fs_browser::fs_open_file(request)?)),
        "fs_reveal" => Ok(ok_response(operation, fs_browser::fs_reveal(request)?)),
        "instance_file_list" => Ok(ok_response(operation, instance_files::instance_file_list(request)?)),
        "instance_file_read" => Ok(ok_response(operation, instance_files::instance_file_read(request)?)),
        "instance_file_write" => Ok(ok_response(operation, instance_files::instance_file_write(request)?)),
        "instance_file_mkdir" => Ok(ok_response(operation, instance_files::instance_file_mkdir(request)?)),
        "instance_file_upload" => Ok(ok_response(operation, instance_files::instance_file_upload(request)?)),
        "instance_file_delete" => Ok(ok_response(operation, instance_files::instance_file_delete(request)?)),
        "instance_log_archives" => Ok(ok_response(operation, instance_files::instance_log_archives(request)?)),
        "instance_log_archive_read" => Ok(ok_response(operation, instance_files::instance_log_archive_read(request)?)),
        "instance_groups" => Ok(ok_response(operation, instances::instance_groups(request)?)),
        "create_instance_group" => Ok(ok_response(operation, instances::create_instance_group(request)?)),
        "delete_instance_group" => Ok(ok_response(operation, instances::delete_instance_group(request)?)),
        "rename_instance_group" => Ok(ok_response(operation, instances::rename_instance_group(request)?)),
        "delete_instance" => Ok(ok_response(operation, instances::delete_instance(request)?)),
        "duplicate_instance" => Ok(ok_response(operation, instances::duplicate_instance(request)?)),
        "update_instance_profile" => Ok(ok_response(operation, instances::update_instance_profile(request)?)),
        "export_modpack" => Ok(ok_response(operation, modpack_export::export_modpack(request)?)),
        "instance_icon_blocks" => Ok(ok_response(operation, block_icons::instance_icon_blocks(request)?)),
        "instance_details" => Ok(ok_response(operation, instance_workspace::instance_details(request)?)),
        "instance_versions" => Ok(ok_response(operation, instance_versions::instance_versions(request)?)),
        "instance_version_override_save" => Ok(ok_response(operation, instance_versions::instance_version_override_save(request)?)),
        "instance_version_import" => Ok(ok_response(operation, instance_versions::instance_version_import(request)?)),
        "mods_remove" => Ok(ok_response(operation, instance_workspace::mods_remove(request)?)),
        "save_instance_settings" => Ok(ok_response(operation, instance_workspace::save_instance_settings(request)?)),
        "remove_instance_asset" => Ok(ok_response(operation, instance_workspace::remove_instance_asset(request)?)),
        "toggle_mod" => Ok(ok_response(operation, instance_workspace::toggle_mod(request)?)),
        "import_asset" => Ok(ok_response(operation, instance_workspace::import_asset(request)?)),
        "mods_remove_files" => Ok(ok_response(operation, instance_workspace::mods_remove_files(request)?)),
        "save_settings" => Ok(ok_response(operation, config::save_settings(request)?)),
        "plugin_list" => Ok(ok_response(operation, plugins::plugin_list(request)?)),
        "java_validate" => Ok(ok_response(operation, system::java_validate(request)?)),
        "system_metrics" => Ok(ok_response(operation, metrics::system_metrics(request)?)),
        "overlay_state" => Ok(ok_response(operation, state::overlay_state(request)?)),
        "jvm_presets_list" => Ok(ok_response(operation, ops::jvm_presets_list(request)?)),
        "launch_history" => Ok(ok_response(operation, ops::launch_history(request)?)),
        "cache_stats" => Ok(ok_response(operation, ops::cache_stats(request)?)),
        "cache_prune" => Ok(ok_response(operation, ops::cache_prune(request)?)),
        "theme_list" => Ok(ok_response(operation, themes::theme_list(request)?)),
        "theme_preview" => Ok(ok_response(operation, themes::theme_preview(request)?)),
        "theme_save" => Ok(ok_response(operation, themes::theme_save(request)?)),
        "theme_import" => Ok(ok_response(operation, themes::theme_import(request)?)),
        "theme_delete" => Ok(ok_response(operation, themes::theme_delete(request)?)),
        "theme_export" => Ok(ok_response(operation, themes::theme_export(request)?)),
        "github_theme_search" => Ok(ok_response(operation, github_plugins::github_theme_search(request)?)),
        "github_theme_install" => Ok(ok_response(operation, github_plugins::github_theme_install(request)?)),
        "settings_profiles_list" => Ok(ok_response(operation, profiles::profiles_list(request)?)),
        "settings_profile_delete" => Ok(ok_response(operation, profiles::profile_delete(request)?)),
        "settings_profile_export" => Ok(ok_response(operation, profiles::profile_export(request)?)),
        "settings_profile_import" => Ok(ok_response(operation, profiles::profile_import(request)?)),
        "settings_profile_save" => Ok(ok_response(operation, profiles::settings_profile_save(request)?)),
        "settings_profile_apply" => Ok(ok_response(operation, profiles::settings_profile_apply(request)?)),
        "wallpaper_import" => Ok(ok_response(operation, wallpapers::wallpaper_import(request)?)),
        "wallpaper_clear" => Ok(ok_response(operation, wallpapers::wallpaper_clear(request)?)),
        "java_options" => Ok(ok_response(operation, system::java_options(request)?)),
        "security_status" => Ok(ok_response(operation, system::security_status(request)?)),
        "server_cores" => Ok(ok_response(operation, system::server_cores(request)?)),
        "world_backup_list" => Ok(ok_response(operation, backups::world_backup_list(request)?)),
        "world_backup_create" => Ok(ok_response(operation, backups::world_backup_create(request)?)),
        "world_backup_restore" => Ok(ok_response(operation, backups::world_backup_restore(request)?)),
        "world_backup_delete" => Ok(ok_response(operation, backups::world_backup_delete(request)?)),
        "world_backup_schedule_get" => Ok(ok_response(operation, backups::world_backup_schedule_get(request)?)),
        "world_backup_schedule_set" => Ok(ok_response(operation, backups::world_backup_schedule_set(request)?)),
        "create_offline_account" => Ok(ok_response(operation, accounts::create_offline_account(request)?)),
        "delete_account" => Ok(ok_response(operation, accounts::delete_account(request)?)),
        "set_default_account" => Ok(ok_response(operation, accounts::set_default_account(request)?)),
        "move_account" => Ok(ok_response(operation, accounts::move_account(request)?)),
        "microsoft_login_status" => Ok(ok_response(operation, accounts::microsoft_login_status(request)?)),
        "login_microsoft" => Ok(ok_response(operation, accounts::login_microsoft(request)?)),
        "login_ely" => Ok(ok_response(operation, accounts::login_ely(request)?)),
        "install" => Ok(ok_response(operation, installer::install(request)?)),
        "instance_repair_version" => Ok(ok_response(operation, installer::instance_repair_version(request)?)),
        "server_start" => Ok(ok_response(operation, server::server_start(request)?)),
        "server_stop" => Ok(ok_response(operation, server::server_stop(request)?)),
        "server_restart" => Ok(ok_response(operation, server::server_restart(request)?)),
        "server_command" => Ok(ok_response(operation, server::server_command(request)?)),
        "server_log" => Ok(ok_response(operation, server::server_log(request)?)),
        "server_status" => Ok(ok_response(operation, server::server_status(request)?)),
        "mod_dependency_graph" => Ok(ok_response(operation, mod_dependencies::mod_dependency_graph(request)?)),
        "mod_removal_impact" => Ok(ok_response(operation, mod_dependencies::mod_removal_impact(request)?)),
        "external_instances_scan" => Ok(ok_response(operation, instances::external_instances_scan(request)?)),
        "external_instance_import" => Ok(ok_response(operation, instances::external_instance_import(request)?)),
        "server_network_diagnose" => Ok(ok_response(operation, server_network::server_network_diagnose(request)?)),
        "server_network_status" => Ok(ok_response(operation, server_network::server_network_status(request)?)),
        "server_network_prepare" => Ok(ok_response(operation, server_network::server_network_prepare(request)?)),
        "server_network_open" => Ok(ok_response(operation, server_network::server_network_open(request)?)),
        "server_network_close" => Ok(ok_response(operation, server_network::server_network_close(request)?)),
        "plugin_api_quota_usage" => Ok(ok_response(operation, plugin_guard::plugin_api_quota_usage(request)?)),
        "plugin_dev_changes" => Ok(ok_response(operation, plugin_reload::plugin_dev_changes(request)?)),
        "versions" => Ok(ok_response(operation, metadata::versions(request)?)),
        "loader_choices" => Ok(ok_response(operation, loaders::loader_choices(request)?)),
        "server_core_versions" => Ok(ok_response(operation, server::server_core_versions(request)?)),
        "check_remote_release" => Ok(ok_response(operation, release_check::check_remote_release(request)?)),
        "shader_analyze" => Ok(ok_response(operation, shader_analysis::shader_analyze(request)?)),
        "global_search" => Ok(ok_response(operation, global_search::global_search(request)?)),
        "mods_search" => Ok(ok_response(operation, modcat::mods_search(request)?)),
        "mods_details" => Ok(ok_response(operation, modcat::mods_details(request)?)),
        "mods_check_updates" => Ok(ok_response(operation, modcat::mods_check_updates(request)?)),
        "mods_install" => Ok(ok_response(operation, mods_ops::mods_install(request)?)),
        "mods_update" => Ok(ok_response(operation, mods_ops::mods_update(request)?)),
        "mods_update_all_safe" => Ok(ok_response(operation, mods_ops::mods_update_all_safe(request)?)),
        "mods_update_rollback" => Ok(ok_response(operation, mods_ops::mods_update_rollback(request)?)),
        "mods_update_sessions" => Ok(ok_response(operation, mods_ops::mods_update_sessions(request)?)),
        "modpack_import" => Ok(ok_response(operation, modpacks::modpack_import(request)?)),
        "optimization_analyze" => Ok(ok_response(operation, optimization::optimization_analyze(request)?)),
        "world_import_path" => Ok(ok_response(operation, world_import::world_import_path(request)?)),
        "world_import_begin" => Ok(ok_response(operation, world_import::world_import_begin(request)?)),
        "world_import_chunk" => Ok(ok_response(operation, world_import::world_import_chunk(request)?)),
        "world_import_cancel" => Ok(ok_response(operation, world_import::world_import_cancel(request)?)),
        "world_import_finish" => Ok(ok_response(operation, world_import::world_import_finish(request)?)),
        "optimization_apply" => Ok(ok_response(operation, optimization::optimization_apply(request)?)),
        "graphics_diagnostics" => Ok(ok_response(operation, graphics::graphics_diagnostics(request)?)),
        "recording_start" => Ok(ok_response(operation, recorder::recording_start(request)?)),
        "recording_stop" => Ok(ok_response(operation, recorder::recording_stop(request)?)),
        "recording_status" => Ok(ok_response(operation, recorder::recording_status(request)?)),
        "cloud_status" => Ok(ok_response(operation, cloud::cloud_status(request)?)),
        "cloud_configure" => Ok(ok_response(operation, cloud::cloud_configure(request)?)),
        "cloud_test" => Ok(ok_response(operation, cloud::cloud_test(request)?)),
        "cloud_list" => Ok(ok_response(operation, cloud::cloud_list(request)?)),
        "cloud_sync" => Ok(ok_response(operation, cloud::cloud_sync(request)?)),
        "cloud_restore" => Ok(ok_response(operation, cloud::cloud_restore(request)?)),
        "cloud_delete" => Ok(ok_response(operation, cloud::cloud_delete(request)?)),
        "cloud_disconnect" => Ok(ok_response(operation, cloud::cloud_disconnect(request)?)),
        "github_plugin_status" => Ok(ok_response(operation, github_plugins::github_plugin_status(request)?)),
        "github_plugin_search" => Ok(ok_response(operation, github_plugins::github_plugin_search(request)?)),
        "github_plugin_details" => Ok(ok_response(operation, github_plugins::github_plugin_details(request)?)),
        "github_plugin_install" => Ok(ok_response(operation, github_plugins::github_plugin_install(request)?)),
        "github_plugin_oauth_start" => Ok(ok_response(operation, github_plugins::github_plugin_oauth_start(request)?)),
        "github_plugin_oauth_poll" => Ok(ok_response(operation, github_plugins::github_plugin_oauth_poll(request)?)),
        "github_plugin_sign_out" => Ok(ok_response(operation, github_plugins::github_plugin_sign_out(request)?)),
        "github_plugin_publish" => Ok(ok_response(operation, github_plugins::github_plugin_publish(request)?)),
        "launch" => Ok(ok_response(operation, launcher::launch(request)?)),
        "launch_instance" => Ok(ok_response(operation, launcher::launch_instance(request)?)),
        "shader_benchmark_start" => Ok(ok_response(operation, shader_benchmark::shader_benchmark_start(request)?)),
        "shader_benchmark_status" => Ok(ok_response(operation, shader_benchmark::shader_benchmark_status(request)?)),
        "skins_list" => Ok(ok_response(operation, skins::skins_list(request)?)),
        "skins_import" => Ok(ok_response(operation, skins::skins_import(request)?)),
        "skins_export" => Ok(ok_response(operation, skins::skins_export(request)?)),
        "skin_profile" => Ok(ok_response(operation, skins::skin_profile(request)?)),
        "apply_skin" => Ok(ok_response(operation, skins::apply_skin_op(request)?)),
        "clear_skin" => Ok(ok_response(operation, skins::clear_skin_op(request)?)),
        "apply_cape" => Ok(ok_response(operation, skins::apply_cape_op(request)?)),
        "clear_cape" => Ok(ok_response(operation, skins::clear_cape_op(request)?)),
        "open_skins_folder" => Ok(ok_response(operation, skins::open_skins_folder(request)?)),
        "skins_folder" => Ok(ok_response(operation, skins::skins_folder(request)?)),
        "bedrock_runtime_status" => Ok(ok_response(operation, bedrock::runtime_status(request)?)),
        "bedrock_versions" => Ok(ok_response(operation, bedrock::versions(request)?)),
        "bedrock_setup_version" => Ok(ok_response(operation, bedrock::setup_version(request)?)),
        "bedrock_setup_status" => Ok(ok_response(operation, bedrock::setup_status(request)?)),
        "bedrock_setup_cancel" => Ok(ok_response(operation, bedrock::setup_cancel(request)?)),
        "bedrock_store_login" => Ok(ok_response(operation, bedrock::store_login(request)?)),
        "bedrock_store_login_status" => Ok(ok_response(operation, bedrock::store_login_status(request)?)),
        "bedrock_store_login_cancel" => Ok(ok_response(operation, bedrock::store_login_cancel(request)?)),
        "bedrock_account_status" => Ok(ok_response(operation, bedrock::account_status(request)?)),
        "bedrock_screenshots_list" => Ok(ok_response(operation, bedrock::screenshots_list(request)?)),
        "bedrock_create_instance" => Ok(ok_response(operation, bedrock::create_instance(request)?)),
        "bedrock_content_list" => Ok(ok_response(operation, bedrock::content_list(request)?)),
        "bedrock_content_import" => Ok(ok_response(operation, bedrock::content_import(request)?)),
        "bedrock_content_remove" => Ok(ok_response(operation, bedrock::content_remove(request)?)),
        "bedrock_stop" => Ok(ok_response(operation, bedrock::stop(request)?)),
        "engine_info" => Ok(ok_response(operation, engine_info())),
        "instance_shortcut_status" => Ok(ok_response(operation, shortcuts::instance_shortcut_status(request)?)),
        "instance_shortcut_create" => Ok(ok_response(operation, shortcuts::instance_shortcut_create(request)?)),
        "instance_shortcut_remove" => Ok(ok_response(operation, shortcuts::instance_shortcut_remove(request)?)),
        "shortcut_take_launch_request" => Ok(ok_response(operation, shortcuts::shortcut_take_launch_request(request)?)),
        "instance_mod_list_export" => Ok(ok_response(operation, mod_list::instance_mod_list_export(request)?)),
        "instance_autotune" => Ok(ok_response(operation, autotune::instance_autotune(request)?)),
        "auto_repair_diagnose" => Ok(ok_response(operation, auto_repair::auto_repair_diagnose(request)?)),
        "auto_repair_mute" => Ok(ok_response(operation, auto_repair::auto_repair_mute(request)?)),
        "auto_repair_apply" => Ok(ok_response(operation, auto_repair::auto_repair_apply(request)?)),
        "plugin_dev_init" => Ok(ok_response(operation, plugin_dev::plugin_dev_init(request)?)),
        "plugin_dev_lint" => Ok(ok_response(operation, plugin_dev::plugin_dev_lint(request)?)),
        "plugin_dev_pack" => Ok(ok_response(operation, plugin_dev::plugin_dev_pack(request)?)),
        "plugin_dev_verify" => Ok(ok_response(operation, plugin_dev::plugin_dev_verify(request)?)),
        "plugin_dev_keypair" => Ok(ok_response(operation, plugin_dev::plugin_dev_keypair(request)?)),
        "instance_snapshot_create" => Ok(ok_response(operation, snapshots::instance_snapshot_create(request)?)),
        "instance_snapshot_delete" => Ok(ok_response(operation, snapshots::instance_snapshot_delete(request)?)),
        "instance_snapshot_list" => Ok(ok_response(operation, snapshots::instance_snapshot_list(request)?)),
        "instance_snapshot_restore" => Ok(ok_response(operation, snapshots::instance_snapshot_restore(request)?)),
        "instance_log" => Ok(ok_response(operation, metrics::instance_log(request)?)),
        "instance_compare" => Ok(ok_response(operation, instance_compare::instance_compare(request)?)),
        "instance_port_changes" => Ok(ok_response(operation, instance_compare::instance_port_changes(request)?)),
        "instance_pins_get" => Ok(ok_response(operation, instance_pins::instance_pins_get(request)?)),
        "instance_pin_toggle" => Ok(ok_response(operation, instance_pins::instance_pin_toggle(request)?)),
        "open_external" => Ok(ok_response(operation, opener::open_external(request)?)),
        "open_folder" => Ok(ok_response(operation, opener::open_folder(request)?)),
        "system_notify" => Ok(ok_response(operation, opener::system_notify(request)?)),
        "mod_audit_prepare" => Ok(ok_response(operation, mod_audit::mod_audit_prepare(request)?)),
        "mod_audit_accept" => Ok(ok_response(operation, mod_audit::mod_audit_accept(request)?)),
        "mod_audit_cancel" => Ok(ok_response(operation, mod_audit::mod_audit_cancel(request)?)),
        "mod_audit_status" => Ok(ok_response(operation, mod_audit::mod_audit_status(request)?)),
        "server_file_list" => Ok(ok_response(operation, server_files::server_file_list(request)?)),
        "server_file_read" => Ok(ok_response(operation, server_files::server_file_read(request)?)),
        "server_file_write" => Ok(ok_response(operation, server_files::server_file_write(request)?)),
        "server_file_mkdir" => Ok(ok_response(operation, server_files::server_file_mkdir(request)?)),
        "server_file_upload" => Ok(ok_response(operation, server_files::server_file_upload(request)?)),
        "server_file_delete" => Ok(ok_response(operation, server_files::server_file_delete(request)?)),
        "server_snapshot_list" => Ok(ok_response(operation, server_files::server_snapshot_list(request)?)),
        "server_snapshot_create" => Ok(ok_response(operation, server_files::server_snapshot_create(request)?)),
        "server_snapshot_delete" => Ok(ok_response(operation, server_files::server_snapshot_delete(request)?)),
        "server_snapshot_restore" => Ok(ok_response(operation, server_files::server_snapshot_restore(request)?)),
        "server_details" => Ok(ok_response(operation, server_files::server_details(request)?)),
        "server_save_config" => Ok(ok_response(operation, server_files::server_save_config(request)?)),
        "server_plugin_catalogs" => Ok(ok_response(operation, plugin_catalogs::server_plugin_catalogs(request)?)),
        "save_server_plugin_catalogs" => Ok(ok_response(operation, plugin_catalogs::save_server_plugin_catalogs(request)?)),
        "server_plugin_search" => Ok(ok_response(operation, plugin_catalogs::server_plugin_search(request)?)),
        "server_plugin_details" => Ok(ok_response(operation, plugin_catalogs::server_plugin_details(request)?)),
        "server_plugin_updates" => Ok(ok_response(operation, plugin_catalogs::server_plugin_updates(request)?)),
        "server_plugin_update" => Ok(ok_response(operation, plugin_catalogs::server_plugin_update(request)?)),
        "server_plugin_install" => Ok(ok_response(operation, plugin_catalogs::server_plugin_install(request)?)),
        "server_plugin_remove" => Ok(ok_response(operation, plugin_catalogs::server_plugin_remove(request)?)),
        "plugin_security_status" => Ok(ok_response(operation, plugin_ops::plugin_security_status(request)?)),
        "plugin_permission_grants_set" => Ok(ok_response(operation, plugin_ops::plugin_permission_grants_set(request)?)),
        "plugin_local_untrusted_execution" => Ok(ok_response(operation, plugin_ops::plugin_local_untrusted_execution(request)?)),
        "plugin_publisher_pin" => Ok(ok_response(operation, plugin_ops::plugin_publisher_pin(request)?)),
        "plugin_publisher_unpin" => Ok(ok_response(operation, plugin_ops::plugin_publisher_unpin(request)?)),
        "plugin_publisher_revocation_authority_set" => Ok(ok_response(operation, plugin_ops::plugin_publisher_revocation_authority_set(request)?)),
        "plugin_revocations_import" => Ok(ok_response(operation, plugin_ops::plugin_revocations_import(request)?)),
        "plugin_local_revoke" => Ok(ok_response(operation, plugin_ops::plugin_local_revoke(request)?)),
        "plugin_local_unrevoke" => Ok(ok_response(operation, plugin_ops::plugin_local_unrevoke(request)?)),
        "plugin_install" => Ok(ok_response(operation, plugin_ops::plugin_install(request)?)),
        "plugin_set_enabled" => Ok(ok_response(operation, plugin_ops::plugin_set_enabled(request)?)),
        "plugin_uninstall" => Ok(ok_response(operation, plugin_ops::plugin_uninstall(request)?)),
        "plugin_api_source" => Ok(ok_response(operation, plugin_ops::plugin_api_source(request)?)),
        "plugin_api_runtime_info" => Ok(ok_response(operation, plugin_ops::plugin_api_runtime_info(request)?)),
        "plugin_api_operation_trace" => Ok(ok_response(operation, plugin_ops::plugin_api_operation_trace(request)?)),
        "plugin_api_capabilities" => Ok(ok_response(operation, plugin_ops::plugin_api_capabilities(request)?)),
        "plugin_api_permissions_get" => Ok(ok_response(operation, plugin_ops::plugin_api_permissions_get(request)?)),
        "plugin_api_permissions_set" => Ok(ok_response(operation, plugin_ops::plugin_api_permissions_set(request)?)),
        "plugin_api_authorize" => Ok(ok_response(operation, plugin_ops::plugin_api_authorize(request)?)),
        "plugin_api_open_external" => Ok(ok_response(operation, plugin_ops::plugin_api_open_external(request)?)),
        "plugin_api_instances_list" => Ok(ok_response(operation, plugin_ops::plugin_api_instances_list(request)?)),
        "plugin_api_process_metrics" => Ok(ok_response(operation, plugin_ops::plugin_api_process_metrics(request)?)),
        "plugin_api_config_projected_read" => Ok(ok_response(operation, plugin_ops::plugin_api_config_projected_read(request)?)),
        "plugin_api_files_list" => Ok(ok_response(operation, plugin_ops::plugin_api_files(request, "list")?)),
        "plugin_api_files_read" => Ok(ok_response(operation, plugin_ops::plugin_api_files(request, "read")?)),
        "plugin_api_files_write" => Ok(ok_response(operation, plugin_ops::plugin_api_files(request, "write")?)),
        "plugin_api_files_remove" => Ok(ok_response(operation, plugin_ops::plugin_api_files(request, "remove")?)),
        "plugin_network_fetch" => Ok(ok_response(operation, plugin_ops::plugin_network_fetch(request)?)),
        "plugin_http_request" => Ok(ok_response(operation, plugin_ops::plugin_http_request(request)?)),
        "plugin_network_dns" => Ok(ok_response(operation, plugin_ops::plugin_network_dns(request)?)),
        "plugin_network_ping" => Ok(ok_response(operation, plugin_ops::plugin_network_ping(request)?)),
        "plugin_network_port_check" => Ok(ok_response(operation, plugin_ops::plugin_network_ping(request)?)),
        "plugin_api_proxy_routes_get" => Ok(ok_response(operation, plugin_ops::plugin_api_proxy_routes_get(request)?)),
        "plugin_api_proxy_routes_save" => Ok(ok_response(operation, plugin_ops::plugin_api_proxy_routes_save(request)?)),
        "plugin_api_proxy_routes_add" => Ok(ok_response(operation, plugin_ops::plugin_api_proxy_routes_add(request)?)),
        "plugin_api_proxy_routes_remove" => Ok(ok_response(operation, plugin_ops::plugin_api_proxy_routes_remove(request)?)),
        "plugin_server_list_get" => Ok(ok_response(operation, plugin_ops::plugin_server_list_get(request)?)),
        "plugin_server_list_save" => Ok(ok_response(operation, plugin_ops::plugin_server_list_save(request)?)),
        "plugin_server_list_add" => Ok(ok_response(operation, plugin_ops::plugin_server_list_add(request)?)),
        "plugin_server_list_remove" => Ok(ok_response(operation, plugin_ops::plugin_server_list_remove(request)?)),
        "plugin_storage_get" => Ok(ok_response(operation, plugin_ops::plugin_storage_get(request)?)),
        "plugin_storage_set" => Ok(ok_response(operation, plugin_ops::plugin_storage_set(request)?)),
        "plugin_storage_remove" => Ok(ok_response(operation, plugin_ops::plugin_storage_remove(request)?)),
        "plugin_config_get" => Ok(ok_response(operation, plugin_ops::plugin_config_get(request)?)),
        "plugin_config_set" => Ok(ok_response(operation, plugin_ops::plugin_config_set(request)?)),
        "plugin_config_remove" => Ok(ok_response(operation, plugin_ops::plugin_config_remove(request)?)),
        "plugin_file_list" => Ok(ok_response(operation, plugin_ops::plugin_file_list(request)?)),
        "plugin_file_read" => Ok(ok_response(operation, plugin_ops::plugin_file_read(request)?)),
        "plugin_file_write" => Ok(ok_response(operation, plugin_ops::plugin_file_write(request)?)),
        "plugin_file_remove" => Ok(ok_response(operation, plugin_ops::plugin_file_remove(request)?)),
        "plugin_database_query" => Ok(ok_response(operation, plugin_ops::plugin_database_query(request)?)),
        "plugin_database_execute" => Ok(ok_response(operation, plugin_ops::plugin_database_execute(request)?)),
        "server_create" => Ok(ok_response(operation, server::server_create(request)?)),
        "server_upload_core" => Ok(ok_response(operation, server::server_upload_core(request)?)),
        "server_core_download" => Ok(ok_response(operation, server::server_core_download(request)?)),
        other => Err(format!("rust-движок не обслуживает операцию {other}")),
    }
}

fn ok_response(operation: &str, data: Value) -> Value {
    // конверт протокола; фронтенд ждёт данные в envelope.data
    json!({ "ok": true, "operation": operation, "data": data })
}

fn request_field<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    request
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Поле «{name}» обязательно для этой операции"))
}

fn sha256_hex_of_file(path: &Path) -> Result<(u64, String), String> {
    let file = File::open(path).map_err(|error| format!("Не удалось открыть {}: {error}", path.display()))?;
    let size = file
        .metadata()
        .map_err(|error| format!("Не удалось прочитать свойства {}: {error}", path.display()))?
        .len();
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("Не удалось прочитать {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn hash_file(request: &Value) -> Result<Value, String> {
    let path = request_field(request, "path")?;
    let path = Path::new(path);
    if !path.is_file() {
        return Err(format!("Файл не найден: {}", path.display()));
    }
    let (size, sha256) = sha256_hex_of_file(path)?;
    Ok(ok_response(
        "hash_file",
        json!({ "path": path.to_string_lossy(), "size": size, "sha256": sha256 }),
    ))
}

fn hash_directory(request: &Value) -> Result<Value, String> {
    let root = PathBuf::from(request_field(request, "path")?);
    if !root.is_dir() {
        return Err(format!("Каталог не найден: {}", root.display()));
    }
    let mut files: Vec<Value> = Vec::new();
    let mut total_size: u64 = 0;
    // обход вглубь от корня: пути собираются ОТ корня, поэтому "выход"
    fn walk(dir: &Path, prefix: &str, files: &mut Vec<Value>, total_size: &mut u64) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|error| format!("Не удалось прочитать каталог {}: {error}", dir.display()))?
            .filter_map(|entry| entry.ok())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let file_type = match entry.file_type() {
                Ok(value) => value,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let relative = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let path = entry.path();
            if file_type.is_dir() {
                walk(&path, &relative, files, total_size)?;
            } else if file_type.is_file() {
                if let Ok((size, sha256)) = sha256_hex_of_file(&path) {
                    *total_size += size;
                    files.push(json!({ "path": relative, "size": size, "sha256": sha256 }));
                }
            }
        }
        Ok(())
    }
    walk(&root, "", &mut files, &mut total_size)?;
    Ok(ok_response(
        "hash_directory",
        json!({
            "path": root.to_string_lossy(),
            "count": files.len(),
            "total_size": total_size,
            "files": files,
        }),
    ))
}

fn engine_info() -> Value {
    json!({
        "engine": "rust",
        "crate_version": env!("CARGO_PKG_VERSION"),
        "native_ops": NATIVE_OPS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-rust-engine-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    #[test]
    fn native_ops_are_listed_and_routable() {
        for operation in NATIVE_OPS {
            assert!(is_native(operation), "{operation}");
        }
        assert!(is_native("bootstrap"));
        assert!(is_native("mods_search"));
        let info = handle("engine_info", &json!({})).expect("engine_info");
        assert_eq!(info["ok"], json!(true));
        assert_eq!(info["data"]["engine"], json!("rust"));
        assert_eq!(info["data"]["native_ops"].as_array().map(Vec::len), Some(NATIVE_OPS.len()));
    }

    #[test]
    fn hash_file_matches_reference_vector() {
        let root = temp_root("file");
        let file = root.join("hello.txt");
        fs::write(&file, b"hello\n").expect("записать файл");
        let response = handle("hash_file", &json!({ "path": file.to_string_lossy() })).expect("hash_file");
        assert_eq!(response["ok"], json!(true));
        assert_eq!(response["data"]["size"], json!(6));
        assert_eq!(response["data"]["sha256"], json!("5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"));
        let missing = handle("hash_file", &json!({ "path": root.join("nope").to_string_lossy() }));
        assert!(missing.is_err());
    }

    #[test]
    #[cfg(unix)] // создание symlink без DevMode на Windows недоступно
    fn hash_directory_walks_sorted_and_skips_symlinks() {
        let root = temp_root("dir");
        fs::write(root.join("a.txt"), b"aaa").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("b.txt"), b"bbb").unwrap();
        symlink(root.join("a.txt"), root.join("link.txt")).expect("создать ссылку");
        let response = handle("hash_directory", &json!({ "path": root.to_string_lossy() })).expect("hash_directory");
        assert_eq!(response["ok"], json!(true));
        assert_eq!(response["data"]["count"], json!(2));
        assert_eq!(response["data"]["total_size"], json!(6));
        let paths: Vec<&str> = response["data"]["files"].as_array().unwrap().iter().map(|item| item["path"].as_str().unwrap()).collect();
        // в манифесте важны сортировка и прямые слеши
        assert_eq!(paths, vec!["a.txt", "sub/b.txt"]);
        let missing = handle("hash_directory", &json!({ "path": root.join("nope").to_string_lossy() }));
        assert!(missing.is_err());
    }

    #[test]
    fn required_fields_are_reported_in_russian() {
        let error = handle("hash_file", &json!({})).expect_err("path обязателен");
        assert!(error.contains("path"), "текст ошибки: {error}");
    }

    #[test]
    fn live_parity_domain_fixture() {
        let root = std::env::var("NIVORA_PARITY_DOMAIN_ROOT").ok();
        let (Some(root), Ok(output)) = (root, std::env::var("NIVORA_PARITY_DOMAIN_OUT")) else {
            return;
        };
        let root = PathBuf::from(root);
        let theme_request = |name: &str| {
            let path = root.join(format!("{name}.json"));
            let raw = fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)
        };
        let mut report = serde_json::Map::new();
        let save = themes::theme_save(&theme_request("theme-save"));
        report.insert("theme-save".into(), outcome(&save));
        let export = themes::theme_export(&theme_request("theme-export"));
        if let Ok(Value::Object(fields)) = &export {
            let mut normalized = fields.clone();
            if let Some(Value::String(encoded)) = normalized.remove("data") {
                let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap_or_default();
                let archive = {
                    // содержимое архива важнее байтов deflate-потока
                    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&decoded)).unwrap();
                    let mut parts = serde_json::Map::new();
                    for index in 0..archive.len() {
                        let mut member = archive.by_index(index).unwrap();
                        let name = member.name().map(|v| v.to_string()).unwrap_or_default();
                        let mut buffer = String::new();
                        member.by_ref().read_to_string(&mut buffer).unwrap();
                        parts.insert(name, json!(buffer));
                    }
                    Value::Object(parts)
                };
                normalized.insert("zip".into(), archive);
            }
            report.insert("theme-export".into(), Value::Object(normalized));
        } else {
            report.insert("theme-export".into(), outcome(&export));
        }
        report.insert("theme-list".into(), outcome(&themes::theme_list(&json!({}))));
        report.insert("theme-import".into(), outcome(&themes::theme_import(&theme_request("theme-import"))));
        report.insert("profile-import".into(), outcome(&profiles::profile_import(&theme_request("profile-import"))));
        report.insert("profiles-list".into(), outcome(&profiles::profiles_list(&json!({}))));
        report.insert("profile-export".into(), outcome(&profiles::profile_export(&theme_request("profile-export"))));
        report.insert("server-cores".into(), outcome(&system::server_cores(&json!({}))));
        report.insert("java-options".into(), outcome(&system::java_options(&json!({}))));
        report.insert("security-status".into(), outcome(&system::security_status(&json!({}))));
        fs::write(&output, serde_json::to_string_pretty(&Value::Object(report)).unwrap()).unwrap();
    }

    #[test]
    fn live_parity_backup_fixture() {
        let Ok(output) = std::env::var("NIVORA_PARITY_STATE_OUT") else {
            return;
        };
        let roots = state::launcher_roots();
        let game_dir = state::settings_game_dir(&roots.config_home, &roots.data_home);
        let instance = game_dir.join("instances").join("Alpha");
        let mut report = serde_json::Map::new();

        let normalize_backup = |value: Value| -> Value { normalize_backup_value(value) };
        let created = backups::world_backup_create(&json!({ "name": "Alpha", "destination": "local", "reason": "manual" }));
        let backup_id = created
            .as_ref()
            .ok()
            .and_then(|value| value["backup"]["backup_id"].as_str())
            .unwrap_or_default()
            .to_owned();
        report.insert("backup-create".into(), outcome(&created.map(normalize_backup)));
        report.insert("backup-list".into(), outcome(&backups::world_backup_list(&json!({ "name": "Alpha" })).map(normalize_backup)));
        report.insert(
            "schedule-set".into(),
            outcome(&backups::world_backup_schedule_set(&json!({ "name": "Alpha", "values": { "interval_hours": 48, "keep": 2, "destination": "local" }, "override": true })).map(normalize_backup)),
        );
        report.insert("schedule-get".into(), outcome(&backups::world_backup_schedule_get(&json!({ "name": "Alpha" })).map(normalize_backup)));

        // портим мир и восстанавливаем: содержимое обязано вернуться
        fs::write(instance.join("saves/New World/level.dat"), "сломано").unwrap();
        report.insert("backup-restore".into(), outcome(&backups::world_backup_restore(&json!({ "name": "Alpha", "backup_id": backup_id })).map(normalize_backup)));
        let restored = fs::read_to_string(instance.join("saves/New World/level.dat")).unwrap_or_default();
        report.insert("restored-content".into(), json!(restored));
        report.insert("backup-delete".into(), outcome(&backups::world_backup_delete(&json!({ "name": "Alpha", "backup_id": backup_id })).map(normalize_backup)));

        // аккаунты: файл, порядок и default_account_id обязаны совпадать
        report.insert("account-create".into(), outcome(&accounts::create_offline_account(&json!({ "username": "  Игрок_77!  " }))));
        let accounts_file = roots.config_home.join("accounts.json");
        let mut records: Vec<Value> = serde_json::from_str(&fs::read_to_string(&accounts_file).unwrap_or_default().to_owned()).unwrap_or_default();
        records.push(json!({ "account_id": "beta", "kind": "offline", "username": "Beta", "alias": "Бета", "expires_at": 0, "xuid": "", "used_seconds": 0, "limit_seconds": 0, "is_default": false }));
        let pretty = {
            let mut text = serde_json::to_string_pretty(&Value::Array(records)).unwrap();
            text.push('\n');
            text
        };
        fs::write(&accounts_file, pretty).unwrap();
        report.insert(
            "account-default".into(),
            outcome(&accounts::set_default_account(&json!({ "account_id": "beta" }))),
        );
        let settings_default = state::read_bounded_json(&roots.config_home.join("settings.json"), 1024 * 1024)
            .and_then(|value| value.get("default_account_id").cloned())
            .unwrap_or(Value::Null);
        report.insert("settings-default-account".into(), settings_default);
        report.insert("account-move".into(), outcome(&accounts::move_account(&json!({ "account_id": "beta", "direction": "up" }))));
        report.insert("account-delete".into(), outcome(&accounts::delete_account(&json!({ "account_id": "offline-debug" }))));
        report.insert("accounts-file".into(), json!(fs::read_to_string(&accounts_file).unwrap_or_default()));

        fs::create_dir_all(&roots.cache_home).unwrap();
        fs::write(roots.cache_home.join("microsoft-login.json"), json!({ "status": "device_code", "code": "XYZ 123" }).to_string()).unwrap();
        report.insert("login-status".into(), outcome(&accounts::microsoft_login_status(&json!({}))));

        fs::write(&output, serde_json::to_string_pretty(&Value::Object(report)).unwrap()).unwrap();
    }

    fn normalize_backup_value(value: Value) -> Value {
        let strip_id = |id: &str| id.chars().skip(16).collect::<String>();
        let fix_entry = |mut entry: Value| {
            if let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_owned) {
                entry["id"] = json!(strip_id(&id));
            }
            if let Some(object) = entry.as_object_mut() {
                if let Some(created) = object.get_mut("created_at") {
                    *created = json!(0);
                }
                if let Some(size) = object.get_mut("size_bytes") {
                    *size = json!(size.as_i64().unwrap_or(0) > 0);
                }
            }
            entry
        };
        let mut value = value;
        if let Some(object) = value.as_object_mut() {
            if let Some(backup) = object.get_mut("backup").and_then(Value::as_object_mut) {
                if let Some(id) = backup.get("backup_id").and_then(Value::as_str).map(str::to_owned) {
                    backup.insert("backup_id".into(), json!(strip_id(&id)));
                }
                if let Some(size) = backup.get_mut("size_bytes") {
                    *size = json!(size.as_i64().unwrap_or(0) > 0);
                }
                if let Some(backups) = backup.get_mut("backups").and_then(Value::as_array_mut) {
                    for entry in backups.iter_mut() {
                        *entry = fix_entry(entry.clone());
                    }
                }
            }
            if let Some(backups) = object.get_mut("backups").and_then(Value::as_array_mut) {
                for entry in backups.iter_mut() {
                    *entry = fix_entry(entry.clone());
                }
            }
            if let Some(schedule) = object.get_mut("schedule").and_then(Value::as_object_mut) {
                schedule.insert("last_run".into(), json!(0));
            }
        }
        value
    }

    fn outcome(result: &Result<Value, String>) -> Value {
        match result {
            Ok(value) => value.clone(),
            Err(error) => json!({ "__error__": error }),
        }
    }
}
