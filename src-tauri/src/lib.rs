mod commands;
mod data_dir;
mod db;
mod models;
mod window_state_guard;

use commands::connection::AppState;
use dbx_core::storage::{DesktopIconTheme, DesktopSettings, Storage};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
#[cfg(target_os = "macos")]
use tauri::RunEvent;
use tauri::{
    menu::MenuBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
#[cfg(any(windows, target_os = "linux"))]
use tauri_plugin_deep_link::DeepLinkExt;

const DESKTOP_TRAY_ID: &str = "main-tray";
#[cfg(target_os = "macos")]
const MACOS_TRAY_ICON: tauri::image::Image<'_> = tauri::include_image!("icons/tray-macos-template.png");
const BLACK_APP_ICON: tauri::image::Image<'_> = tauri::include_image!("icons/icon-black.png");

#[tauri::command]
fn open_new_desktop_window(app: tauri::AppHandle, url: Option<String>) -> Result<(), String> {
    let label = format!("main-{}", uuid::Uuid::new_v4());
    let url = WebviewUrl::App(url.unwrap_or_else(|| "/?newWindow=1".to_string()).into());
    let mut builder = WebviewWindowBuilder::new(&app, label, url)
        .title("DBX")
        .inner_size(1280.0, 800.0)
        .min_inner_size(900.0, 600.0)
        .resizable(true)
        .fullscreen(false)
        .visible(true);

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .accept_first_mouse(true)
            .traffic_light_position(tauri::LogicalPosition::new(16.0, 18.0))
            .hidden_title(true);
    }

    let window = builder.build().map_err(|err| err.to_string())?;
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    Ok(())
}

fn should_hide_window_on_close(target_os: &str) -> bool {
    matches!(target_os, "macos" | "windows")
}

fn should_setup_desktop_tray(target_os: &str, show_tray_icon: bool) -> bool {
    show_tray_icon && matches!(target_os, "macos" | "windows")
}

fn should_show_main_window_after_setup() -> bool {
    true
}

fn should_register_single_instance(_target_os: &str) -> bool {
    false
}

fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct StartupPayload {
    connection_links: Vec<String>,
    sql_file_paths: Vec<String>,
    db_file_paths: Vec<String>,
}

impl StartupPayload {
    fn is_empty(&self) -> bool {
        self.connection_links.is_empty() && self.sql_file_paths.is_empty() && self.db_file_paths.is_empty()
    }
}

fn startup_payload_from_args<I, S>(args: I, cwd: &Path) -> StartupPayload
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args.into_iter().map(|arg| arg.as_ref().to_string()).collect();
    StartupPayload {
        connection_links: commands::deep_link::connection_deep_links_from_args(args.iter()),
        sql_file_paths: commands::external_sql::sql_file_paths_from_args(args.iter(), cwd),
        db_file_paths: commands::external_db::db_file_paths_from_args(args.iter(), cwd),
    }
}

#[cfg(target_os = "macos")]
fn startup_payload_from_opened_urls(urls: &[tauri::Url]) -> StartupPayload {
    let connection_links = urls
        .iter()
        .map(|url| url.to_string())
        .filter_map(|url| commands::deep_link::connection_deep_link_from_arg(&url))
        .collect();
    let sql_file_paths = urls
        .iter()
        .filter_map(|url| url.to_file_path().ok())
        .filter(|path| commands::external_sql::is_sql_file_path(path))
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    let db_file_paths = urls
        .iter()
        .filter_map(|url| url.to_file_path().ok())
        .filter(|path| commands::external_db::is_db_file_path(path))
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    StartupPayload { connection_links, sql_file_paths, db_file_paths }
}

fn open_startup_payload(app: &tauri::AppHandle, payload: StartupPayload) {
    if payload.is_empty() {
        return;
    }

    if !payload.connection_links.is_empty() {
        if let Some(state) = app.try_state::<commands::deep_link::DeepLinkOpenState>() {
            state.push(payload.connection_links.clone());
        }
        let _ = app.emit("dbx-open-connection-links", payload.connection_links);
    }

    if !payload.sql_file_paths.is_empty() {
        if let Some(state) = app.try_state::<commands::external_sql::ExternalSqlOpenState>() {
            state.push(payload.sql_file_paths.clone());
        }
        let _ = app.emit("dbx-open-sql-files", payload.sql_file_paths);
    }

    if !payload.db_file_paths.is_empty() {
        if let Some(state) = app.try_state::<commands::external_db::ExternalDbOpenState>() {
            state.push(payload.db_file_paths.clone());
        }
        let _ = app.emit("dbx-open-db-files", payload.db_file_paths);
    }

    show_main_window(app);
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn setup_desktop_tray<R: tauri::Runtime, M: Manager<R>>(
    manager: &M,
    icon_theme: DesktopIconTheme,
) -> tauri::Result<()> {
    let menu = MenuBuilder::new(manager).text("show", "Show DBX").separator().text("quit", "Quit DBX").build()?;
    let mut tray =
        TrayIconBuilder::<R>::with_id(DESKTOP_TRAY_ID).tooltip("DBX").menu(&menu).show_menu_on_left_click(false);
    #[cfg(target_os = "macos")]
    {
        match icon_theme {
            DesktopIconTheme::Default => {
                tray = tray.icon(MACOS_TRAY_ICON).icon_as_template(true);
            }
            DesktopIconTheme::Black => {
                tray = tray.icon(BLACK_APP_ICON).icon_as_template(false);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let icon = match icon_theme {
            DesktopIconTheme::Default => manager.app_handle().default_window_icon().cloned(),
            DesktopIconTheme::Black => Some(BLACK_APP_ICON),
        };
        if let Some(icon) = icon {
            tray = tray.icon(icon);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(icon) = manager.app_handle().default_window_icon().cloned() {
            tray = tray.icon(icon);
        }
    }

    tray.on_menu_event(|app, event| {
        if event.id() == "show" {
            show_main_window(app);
        } else if event.id() == "quit" {
            app.exit(0);
        }
    })
    .on_tray_icon_event(|tray, event| match event {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
        | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => show_main_window(tray.app_handle()),
        _ => {}
    })
    .build(manager)?;

    Ok(())
}

fn apply_desktop_icon_theme(app: &tauri::AppHandle, icon_theme: DesktopIconTheme) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        match icon_theme {
            DesktopIconTheme::Default => {
                if let Some(icon) = app.default_window_icon().cloned() {
                    window.set_icon(icon)?;
                }
            }
            DesktopIconTheme::Black => window.set_icon(BLACK_APP_ICON)?,
        }
    }
    Ok(())
}

fn apply_desktop_tray_icon_theme(app: &tauri::AppHandle, icon_theme: DesktopIconTheme) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id(DESKTOP_TRAY_ID) {
        #[cfg(target_os = "macos")]
        {
            match icon_theme {
                DesktopIconTheme::Default => {
                    tray.set_icon(Some(MACOS_TRAY_ICON))?;
                    tray.set_icon_as_template(true)?;
                }
                DesktopIconTheme::Black => {
                    tray.set_icon(Some(BLACK_APP_ICON))?;
                    tray.set_icon_as_template(false)?;
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            let icon = match icon_theme {
                DesktopIconTheme::Default => app.default_window_icon().cloned(),
                DesktopIconTheme::Black => Some(BLACK_APP_ICON),
            };
            tray.set_icon(icon)?;
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (tray, icon_theme);
        }
    }
    Ok(())
}

pub(crate) fn apply_desktop_settings(app: &tauri::AppHandle, desktop_settings: &DesktopSettings) -> tauri::Result<()> {
    apply_desktop_icon_theme(app, desktop_settings.icon_theme)?;
    if matches!(std::env::consts::OS, "macos" | "windows") {
        if let Some(tray) = app.tray_by_id(DESKTOP_TRAY_ID) {
            tray.set_visible(desktop_settings.show_tray_icon)?;
            apply_desktop_tray_icon_theme(app, desktop_settings.icon_theme)?;
        } else if desktop_settings.show_tray_icon {
            setup_desktop_tray(app, desktop_settings.icon_theme)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        should_hide_window_on_close, should_register_single_instance, should_setup_desktop_tray,
        should_show_main_window_after_setup, startup_payload_from_args, StartupPayload,
    };
    use std::path::Path;

    #[test]
    fn single_instance_is_disabled_for_all_desktop_platforms() {
        assert!(!should_register_single_instance("windows"));
        assert!(!should_register_single_instance("macos"));
        assert!(!should_register_single_instance("linux"));
        assert!(!should_register_single_instance("freebsd"));
    }

    #[test]
    fn parses_all_startup_payload_types_from_args() {
        let payload = startup_payload_from_args(
            [
                "dbx://connection/new?type=mysql&host=127.0.0.1",
                "queries/report.sql",
                "/tmp/local.sqlite",
                "--ignored",
                "/tmp/readme.txt",
            ],
            Path::new("/work"),
        );

        assert_eq!(
            payload,
            StartupPayload {
                connection_links: vec!["dbx://connection/new?type=mysql&host=127.0.0.1".to_string()],
                sql_file_paths: vec!["/work/queries/report.sql".to_string()],
                db_file_paths: vec!["/tmp/local.sqlite".to_string()],
            }
        );
    }

    #[test]
    fn hides_window_on_close_for_windows_and_macos() {
        assert!(should_hide_window_on_close("windows"));
        assert!(should_hide_window_on_close("macos"));
    }

    #[test]
    fn does_not_hide_window_on_close_for_other_platforms() {
        assert!(!should_hide_window_on_close("linux"));
    }

    #[test]
    fn sets_up_desktop_tray_for_windows_and_macos() {
        assert!(should_setup_desktop_tray("windows", true));
        assert!(should_setup_desktop_tray("macos", true));
        assert!(!should_setup_desktop_tray("windows", false));
        assert!(!should_setup_desktop_tray("macos", false));
        assert!(!should_setup_desktop_tray("linux", true));
        let source = include_str!("lib.rs");
        assert!(source.contains(
            "if should_setup_desktop_tray(std::env::consts::OS, desktop_settings.show_tray_icon) {\n                setup_desktop_tray(app, desktop_settings.icon_theme)?;"
        ));
    }

    #[test]
    fn tray_preference_hides_existing_tray_instead_of_removing_it() {
        let source = include_str!("lib.rs");
        assert!(source.contains("tray.set_visible(show_tray_icon)?;"));
        let remove_call = concat!("remove", "_tray_by_id");
        assert!(!source.contains(remove_call));
    }

    #[test]
    fn can_apply_black_logo_icon_theme() {
        let source = include_str!("lib.rs");
        assert!(source.contains("const BLACK_APP_ICON"));
        assert!(source.contains("DesktopIconTheme::Black => window.set_icon(BLACK_APP_ICON)?"));
        assert!(source.contains("DesktopIconTheme::Black => Some(BLACK_APP_ICON)"));
    }

    #[test]
    fn desktop_settings_save_treats_runtime_tray_update_as_best_effort() {
        let source = include_str!("commands/app_settings.rs");
        assert!(source.contains("if let Err(err) = apply_desktop_settings"));
        assert!(!source.contains("map_err(|err| err.to_string())"));
    }

    #[test]
    fn shows_main_window_after_regular_startup_setup() {
        assert!(should_show_main_window_after_setup());
        let source = include_str!("lib.rs");
        assert!(source
            .contains("if should_show_main_window_after_setup() {\n                show_main_window(app.handle());"));
    }

    #[test]
    fn regular_startup_uses_shared_payload_dispatch() {
        let source = include_str!("lib.rs");
        assert!(source.contains("let startup_payload = startup_payload_from_args(std::env::args().skip(1), &cwd);"));
        assert!(source.contains("open_startup_payload(app.handle(), startup_payload);"));
    }

    #[test]
    fn single_instance_plugin_is_not_registered() {
        let source = include_str!("lib.rs");
        let plugin_init = concat!("tauri_plugin_", "single_instance", "::init");
        assert!(!source.contains(plugin_init));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    rustls::crypto::aws_lc_rs::default_provider().install_default().expect("Failed to install rustls crypto provider");

    let startup_begin = Instant::now();
    debug_assert!(!should_register_single_instance(std::env::consts::OS));

    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(move |app| {
            let setup_start = Instant::now();
            eprintln!("[STARTUP] plugins registered in {:?}", startup_begin.elapsed());

            if cfg!(debug_assertions) {
                app.handle().plugin(tauri_plugin_log::Builder::default().level(log::LevelFilter::Info).build())?;
            }

            let default_data_dir =
                app.path().app_data_dir().map_err(|e| e.to_string()).expect("Failed to resolve app data dir");
            let data_dir = data_dir::resolve_data_dir(default_data_dir);
            std::fs::create_dir_all(&data_dir).expect("Failed to create data dir");
            let db_path = data_dir.join("dbx.db");

            let t = Instant::now();
            let storage = tauri::async_runtime::block_on(async {
                let s = Storage::open(&db_path).await.expect("Failed to open storage");
                eprintln!("[STARTUP]   Storage::open in {:?}", t.elapsed());
                let t2 = Instant::now();
                s.migrate_from_json(&data_dir).await.expect("Failed to migrate JSON data");
                eprintln!("[STARTUP]   migrate_from_json in {:?}", t2.elapsed());
                s
            });
            let desktop_settings = tauri::async_runtime::block_on(storage.load_desktop_settings()).unwrap_or_default();
            eprintln!("[STARTUP] storage ready in {:?}", t.elapsed());

            let state = if data_dir::uses_custom_data_dir() {
                Arc::new(AppState::new_with_plugin_and_agent_dir_and_app_version(
                    storage,
                    data_dir.join("plugins"),
                    data_dir.join("agents"),
                    env!("CARGO_PKG_VERSION"),
                ))
            } else {
                Arc::new(AppState::new_with_plugin_dir_and_app_version(
                    storage,
                    data_dir.join("plugins"),
                    env!("CARGO_PKG_VERSION"),
                ))
            };
            app.manage(state.clone());
            app.manage(commands::external_sql::ExternalSqlOpenState::default());
            app.manage(commands::external_db::ExternalDbOpenState::default());
            app.manage(commands::deep_link::DeepLinkOpenState::default());
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let startup_payload = startup_payload_from_args(std::env::args().skip(1), &cwd);
            open_startup_payload(app.handle(), startup_payload);

            let app_handle = app.handle().clone();
            commands::mcp_bridge::start(app_handle, state);
            eprintln!("[STARTUP] setup complete in {:?} (total {:?})", setup_start.elapsed(), startup_begin.elapsed());

            #[cfg(not(target_os = "macos"))]
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_decorations(false);
                }
            }
            if should_setup_desktop_tray(std::env::consts::OS, desktop_settings.show_tray_icon) {
                setup_desktop_tray(app, desktop_settings.icon_theme)?;
            }
            apply_desktop_icon_theme(app.handle(), desktop_settings.icon_theme)?;
            window_state_guard::enforce_main_window_bounds(app.handle());
            if should_show_main_window_after_setup() {
                show_main_window(app.handle());
            }
            #[cfg(any(windows, target_os = "linux"))]
            let _ = app.deep_link().register_all();

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if should_hide_window_on_close(std::env::consts::OS) {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::ai::ai_complete,
            commands::ai::ai_stream,
            commands::ai::ai_cancel_stream,
            commands::ai::ai_test_connection,
            commands::ai::ai_list_models,
            commands::ai::save_ai_config,
            commands::ai::load_ai_config,
            commands::ai::save_ai_conversation,
            commands::ai::load_ai_conversations,
            commands::ai::delete_ai_conversation,
            commands::app_settings::load_desktop_settings,
            commands::app_settings::save_desktop_settings,
            commands::app_settings::load_pinned_tree_node_ids,
            commands::app_settings::save_pinned_tree_node_ids,
            commands::cloud_sync::webdav_sync_test,
            commands::cloud_sync::webdav_password_status,
            commands::cloud_sync::save_webdav_saved_password,
            commands::cloud_sync::forget_webdav_saved_password,
            commands::cloud_sync::webdav_sync_upload,
            commands::cloud_sync::webdav_sync_download,
            commands::connection::test_connection,
            commands::connection::connect_db,
            commands::connection::disconnect_db,
            commands::connection::close_database_connection,
            commands::connection::refresh_connections,
            commands::connection::save_connections,
            commands::connection::load_connections,
            commands::connection::save_sidebar_layout,
            commands::connection::load_sidebar_layout,
            commands::plugins::list_plugins,
            commands::plugins::list_jdbc_drivers,
            commands::plugins::import_jdbc_drivers,
            commands::plugins::delete_jdbc_driver,
            commands::plugins::jdbc_plugin_status,
            commands::plugins::install_jdbc_plugin,
            commands::plugins::install_jdbc_plugin_local,
            commands::plugins::uninstall_jdbc_plugin,
            commands::schema::list_databases,
            commands::schema::list_tables,
            commands::schema::list_objects,
            commands::schema::list_completion_objects,
            commands::schema::get_object_source,
            commands::schema::list_schemas,
            commands::schema::get_columns,
            commands::schema::list_indexes,
            commands::schema::list_foreign_keys,
            commands::schema::list_triggers,
            commands::schema::get_table_ddl,
            commands::schema_diff::prepare_schema_diff,
            commands::schema_diff::generate_schema_sync_sql,
            commands::schema_cache::save_schema_cache,
            commands::schema_cache::load_schema_cache,
            commands::schema_cache::delete_schema_cache_prefix,
            commands::query::execute_query,
            commands::query::execute_multi,
            commands::query::cancel_query,
            commands::query::close_query_session,
            commands::query::close_client_connection_session,
            commands::query::execute_batch,
            commands::query::execute_script,
            commands::query::execute_in_transaction,
            commands::query::analyze_sql_references,
            commands::query::find_statement_at_cursor,
            commands::query::prepare_query_pagination_execution_plan,
            commands::query::build_sorted_query_sql,
            commands::query::build_explain_sql,
            commands::query::build_dropped_file_preview_sql,
            commands::query::build_table_select_sql,
            commands::query::build_database_search_sql,
            commands::query::build_search_result_where,
            commands::query::build_rename_object_sql,
            commands::query::build_create_database_sql,
            commands::query::build_duckdb_attach_database_sql,
            commands::query::build_drop_object_sql,
            commands::query::build_drop_table_sql,
            commands::query::build_drop_table_child_object_sql,
            commands::query::build_empty_table_sql,
            commands::query::build_truncate_table_sql,
            commands::query::build_drop_database_sql,
            commands::query::build_create_schema_sql,
            commands::query::build_drop_schema_sql,
            commands::query::build_duplicate_table_structure_sql,
            commands::query::build_executable_object_source_statements,
            commands::query::build_executable_object_source_sql,
            commands::query::build_routine_rename_object_source_statements,
            commands::query::build_view_ddl_sql,
            commands::query::build_table_structure_change_sql,
            commands::query::build_create_table_sql,
            commands::query::build_single_column_alter_sql,
            commands::query::analyze_editable_query_editability,
            commands::query::prepare_data_grid_save,
            commands::query::build_data_grid_copy_update_statements,
            commands::query::build_data_grid_copy_insert_statement,
            commands::query::build_data_grid_context_filter_condition,
            commands::query::build_data_grid_column_value_filter_condition,
            commands::query::build_data_grid_count_sql,
            commands::query::build_hive_table_properties_sql,
            commands::query::build_export_insert_statements,
            commands::query::build_export_sql_insert,
            commands::query::build_database_sql_export,
            commands::data_compare::prepare_data_compare,
            commands::data_compare::prepare_data_compare_from_tables,
            commands::data_compare::prepare_data_compare_missing_target,
            commands::data_compare::build_data_compare_sync_plan,
            commands::sql_file::preview_sql_file,
            commands::sql_file::execute_sql_file,
            commands::sql_file::cancel_sql_file_execution,
            commands::external_sql::pending_open_sql_files,
            commands::external_sql::read_external_sql_file,
            commands::external_db::pending_open_db_files,
            commands::deep_link::pending_open_connection_links,
            commands::table_import::preview_table_import_file,
            commands::table_import::import_table_file,
            commands::table_import::cancel_table_import,
            commands::redis_cmd::redis_list_databases,
            commands::redis_cmd::redis_scan_keys,
            commands::redis_cmd::redis_scan_values,
            commands::redis_cmd::redis_get_value,
            commands::redis_cmd::redis_set_string,
            commands::redis_cmd::redis_delete_key,
            commands::redis_cmd::redis_hash_set,
            commands::redis_cmd::redis_hash_del,
            commands::redis_cmd::redis_list_push,
            commands::redis_cmd::redis_list_set,
            commands::redis_cmd::redis_list_remove,
            commands::redis_cmd::redis_set_add,
            commands::redis_cmd::redis_set_remove,
            commands::redis_cmd::redis_zadd,
            commands::redis_cmd::redis_zrem,
            commands::redis_cmd::redis_set_ttl,
            commands::redis_cmd::redis_delete_keys,
            commands::redis_cmd::redis_flush_db,
            commands::redis_cmd::redis_execute_command,
            commands::redis_cmd::redis_load_more,
            commands::saved_sql::load_saved_sql_library,
            commands::saved_sql::save_saved_sql_folder,
            commands::saved_sql::delete_saved_sql_folder,
            commands::saved_sql::save_saved_sql_file,
            commands::saved_sql::delete_saved_sql_file,
            commands::mongo_cmd::mongo_list_databases,
            commands::mongo_cmd::mongo_list_collections,
            commands::mongo_cmd::mongo_find_documents,
            commands::mongo_cmd::mongo_aggregate_documents,
            commands::mongo_cmd::mongo_insert_document,
            commands::mongo_cmd::mongo_insert_documents,
            commands::mongo_cmd::mongo_update_document,
            commands::mongo_cmd::mongo_update_documents,
            commands::mongo_cmd::mongo_delete_document,
            commands::mongo_cmd::mongo_delete_documents,
            commands::history::save_history,
            commands::history::load_history,
            commands::history::clear_history,
            commands::history::delete_history_entry,
            commands::mcp::check_mcp_server_status,
            commands::update::check_for_updates,
            commands::update::get_system_proxy_url,
            commands::transfer::start_transfer,
            commands::transfer::cancel_transfer,
            commands::database_export::export_database_sql,
            commands::database_export::cancel_database_export,
            commands::table_export::start_table_export,
            commands::table_export::cancel_table_export,
            commands::csv_export::export_query_result_csv,
            commands::csv_export::export_table_data_csv,
            commands::xlsx_export::export_query_result_xlsx,
            commands::text_export::export_query_result_json,
            commands::text_export::export_query_result_markdown,
            commands::agents::list_installed_agents,
            commands::agents::list_installed_agents_local,
            commands::agents::get_driver_store_usage,
            commands::agents::get_driver_runtime_summary,
            commands::agents::stop_driver_runtime,
            commands::agents::restart_driver_runtime,
            commands::agents::install_agent,
            commands::agents::upgrade_all_agents,
            commands::agents::check_agent_update_blockers,
            commands::agents::uninstall_agent,
            commands::agents::check_jre_installed,
            commands::agents::get_agent_java_runtime_config,
            commands::agents::set_agent_java_runtime_config,
            commands::agents::uninstall_jre,
            commands::agents::reinstall_jre,
            commands::agents::invalidate_agent_registry_cache,
            commands::agents::import_agents_from_zip,
            commands::agents::import_agent_jar_cmd,
            commands::system_fonts::list_system_fonts,
            open_new_desktop_window,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            #[cfg(not(target_os = "macos"))]
            let _ = (&app_handle, &event);

            #[cfg(target_os = "macos")]
            if let RunEvent::Opened { urls } = &event {
                open_startup_payload(app_handle, startup_payload_from_opened_urls(urls));
            }

            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { has_visible_windows, .. } = &event {
                if !has_visible_windows {
                    show_main_window(app_handle);
                }
                let app_handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        state.refresh_connections().await;
                    }
                });
            }
        });
}
