use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::RwLock;

use crate::commands::connection::{ensure_connection_writable, AppState};
use crate::commands::transfer::get_db_type;

// Re-export types for backward compatibility
pub use dbx_core::table_import::{
    TableImportPreview, TableImportPreviewRequest, TableImportProgress, TableImportRequest, TableImportSummary,
};

static CANCELLED_IMPORTS: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

fn cancelled_imports() -> &'static RwLock<HashSet<String>> {
    CANCELLED_IMPORTS.get_or_init(|| RwLock::new(HashSet::new()))
}

fn emit_progress(app: &AppHandle, progress: TableImportProgress) {
    let _ = app.emit("table-import-progress", progress);
}

fn emit_command_progress(app: &AppHandle, mut progress: TableImportProgress) {
    if progress.status == dbx_core::table_import::TableImportStatus::Done {
        progress.status = dbx_core::table_import::TableImportStatus::Running;
        progress.phase = dbx_core::table_import::TableImportPhase::Finalizing;
    }
    emit_progress(app, progress);
}

async fn is_cancelled(import_id: &str) -> bool {
    cancelled_imports().read().await.contains(import_id)
}

async fn clear_cancelled(import_id: &str) {
    cancelled_imports().write().await.remove(import_id);
}

#[tauri::command]
pub async fn preview_table_import_file(request: TableImportPreviewRequest) -> Result<TableImportPreview, String> {
    dbx_core::table_import::preview_table_import_file_with_request(request).await
}

#[tauri::command]
pub async fn import_table_file(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    request: TableImportRequest,
) -> Result<TableImportSummary, String> {
    let command_started_at = Instant::now();
    clear_cancelled(&request.import_id).await;
    // Reject import early if the connection is read-only — importing is inherently a write operation
    ensure_connection_writable(&state, &request.connection_id, "Import").await?;
    let db_type = get_db_type(&state, &request.connection_id).await?;
    let database = (!request.database.trim().is_empty()).then_some(request.database.as_str());
    let client_session_id = dbx_core::table_import::table_import_client_session_id(&request.import_id);
    let pool_key =
        state.get_or_create_pool_for_session(&request.connection_id, database, Some(&client_session_id)).await?;

    let core_started_at = Instant::now();
    let result = dbx_core::table_import::import_table_file_core(
        &state,
        &request,
        &db_type,
        &pool_key,
        |import_id| Box::pin(is_cancelled(import_id)),
        |progress| emit_command_progress(&app, progress),
    )
    .await;
    let core_ms = core_started_at.elapsed().as_millis();

    let cleanup_state = Arc::clone(state.inner());
    let cleanup_connection_id = request.connection_id.clone();
    let cleanup_database = database.map(str::to_string);
    let cleanup_session_id = client_session_id.clone();
    let cleanup_import_id = request.import_id.clone();
    tokio::spawn(async move {
        let cleanup_started_at = Instant::now();
        let detached = cleanup_state
            .detach_client_session_pool(&cleanup_connection_id, cleanup_database.as_deref(), &cleanup_session_id)
            .await;
        log::info!(
            "[table-import:cleanup] import_id={} cleanup_detach_ms={} pool_detached={}",
            cleanup_import_id,
            cleanup_started_at.elapsed().as_millis(),
            detached.unwrap_or(false)
        );
    });
    clear_cancelled(&request.import_id).await;
    log::info!(
        "[table-import:command] import_id={} core_ms={} command_ms={} cleanup_scheduled=true",
        request.import_id,
        core_ms,
        command_started_at.elapsed().as_millis()
    );
    result
}

#[tauri::command]
pub async fn cancel_table_import(import_id: String) -> Result<bool, String> {
    cancelled_imports().write().await.insert(import_id);
    Ok(true)
}
