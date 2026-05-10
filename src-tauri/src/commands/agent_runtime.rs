use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, RwLock};

use super::connection::AppState;
use dbx_core::handoff::{HandoffItem, HandoffStatus};
use dbx_core::models::connection::ConnectionConfig;
use dbx_core::sql_safety::{classify_sql, risk_for, risk_for_connection, OperationClass, RiskContext, RiskLevel};

const BIND_ADDR: &str = "127.0.0.1:0";
const DISCOVERY_FILE: &str = "agent-runtime.json";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeSnapshot {
    pub active_connection_id: Option<String>,
    pub active_connection_name: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub active_tab_id: Option<String>,
    pub active_tab_title: Option<String>,
    pub sql: Option<String>,
    pub selected_sql: Option<String>,
    pub selection: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct AgentRuntimeState {
    pub token: String,
    pub snapshot: Arc<RwLock<AgentRuntimeSnapshot>>,
    pub handoffs: Arc<RwLock<Vec<dbx_core::handoff::HandoffItem>>>,
    pub app_state: Option<Arc<AppState>>,
}

pub struct AgentRuntimeServer {
    state: AgentRuntimeState,
    discovery_path: PathBuf,
    shutdown: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

impl AgentRuntimeServer {
    pub fn state(&self) -> &AgentRuntimeState {
        &self.state
    }

    pub fn cleanup(&self) {
        if let Ok(mut shutdown) = self.shutdown.lock() {
            if let Some(tx) = shutdown.take() {
                let _ = tx.send(());
            }
        }
        cleanup_discovery_file(&self.discovery_path);
    }
}

impl Drop for AgentRuntimeServer {
    fn drop(&mut self) {
        if let Ok(mut shutdown) = self.shutdown.lock() {
            if let Some(tx) = shutdown.take() {
                let _ = tx.send(());
            }
        }
        cleanup_discovery_file(&self.discovery_path);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RuntimeResponse {
    status: &'static str,
    body: serde_json::Value,
}

#[derive(Debug)]
struct RuntimeRequest {
    first_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

#[tauri::command]
pub async fn agent_runtime_update_snapshot(
    runtime: tauri::State<'_, AgentRuntimeServer>,
    snapshot: AgentRuntimeSnapshot,
) -> Result<(), String> {
    *runtime.state().snapshot.write().await = snapshot;
    Ok(())
}

#[tauri::command]
pub async fn agent_runtime_load_handoffs(
    app_state: tauri::State<'_, Arc<AppState>>,
    runtime: tauri::State<'_, AgentRuntimeServer>,
) -> Result<Vec<dbx_core::handoff::HandoffItem>, String> {
    let mut items = app_state.storage.load_pending_handoffs().await?;
    items.extend(pending_runtime_handoffs(runtime.state()).await);
    Ok(items)
}

#[tauri::command]
pub async fn agent_runtime_mark_handoff_shown(
    app_state: tauri::State<'_, Arc<AppState>>,
    runtime: tauri::State<'_, AgentRuntimeServer>,
    id: String,
) -> Result<bool, String> {
    update_handoff_status(app_state.inner().as_ref(), runtime.state(), &id, HandoffStatus::Shown).await
}

#[tauri::command]
pub async fn agent_runtime_reject_handoff(
    app_state: tauri::State<'_, Arc<AppState>>,
    runtime: tauri::State<'_, AgentRuntimeServer>,
    id: String,
) -> Result<bool, String> {
    update_handoff_status(app_state.inner().as_ref(), runtime.state(), &id, HandoffStatus::Rejected).await
}

pub fn start(app: AppHandle, app_state: Arc<AppState>) -> AgentRuntimeServer {
    let token = uuid::Uuid::new_v4().to_string();
    let state = AgentRuntimeState {
        token: token.clone(),
        snapshot: Arc::new(RwLock::new(AgentRuntimeSnapshot::default())),
        handoffs: Arc::new(RwLock::new(Vec::new())),
        app_state: Some(app_state),
    };
    let discovery_path =
        app.path().app_data_dir().map(|dir| dir.join(DISCOVERY_FILE)).unwrap_or_else(|_| PathBuf::from(DISCOVERY_FILE));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server_state = state.clone();

    tauri::async_runtime::spawn(async move {
        run_server(app, server_state, shutdown_rx).await;
    });

    AgentRuntimeServer { state, discovery_path, shutdown: std::sync::Mutex::new(Some(shutdown_tx)) }
}

async fn run_server(app: AppHandle, state: AgentRuntimeState, mut shutdown: oneshot::Receiver<()>) {
    let listener = match TcpListener::bind(BIND_ADDR).await {
        Ok(listener) => listener,
        Err(err) => {
            log::warn!("Agent runtime failed to bind {BIND_ADDR}: {err}");
            return;
        }
    };
    let port = listener.local_addr().map(|addr| addr.port()).unwrap_or(0);
    let discovery_path = match app.path().app_data_dir() {
        Ok(dir) => match write_discovery_file(&dir, port, &state.token) {
            Ok(path) => Some(path),
            Err(err) => {
                log::warn!("Agent runtime discovery write failed: {err}");
                None
            }
        },
        Err(err) => {
            log::warn!("Agent runtime app data dir unavailable: {err}");
            None
        }
    };
    log::info!("Agent runtime listening on 127.0.0.1:{port}");

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                if let Some(path) = discovery_path.as_deref() {
                    cleanup_discovery_file(path);
                }
                break;
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let st = state.clone();
                tokio::spawn(async move {
                    handle_connection(stream, st).await;
                });
            }
        }
    }
}

async fn handle_connection(mut stream: TcpStream, state: AgentRuntimeState) {
    let request = match read_request(&mut stream).await {
        Ok(Some(request)) => request,
        Ok(None) => return,
        Err(response) => {
            respond_json(&mut stream, response.status, response.body).await;
            return;
        }
    };

    if !is_authorized_headers(&request.headers, &state.token) {
        respond_json(&mut stream, "401 Unauthorized", serde_json::json!({"error": "unauthorized"})).await;
        return;
    }

    let response = route_request(&request.first_line, &request.body, &state).await;
    respond_json(&mut stream, response.status, response.body).await;
}

async fn read_request(stream: &mut TcpStream) -> Result<Option<RuntimeRequest>, RuntimeResponse> {
    let mut buf = Vec::new();
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() >= MAX_HEADER_BYTES {
            return Err(RuntimeResponse {
                status: "431 Request Header Fields Too Large",
                body: serde_json::json!({"error": "headers too large"}),
            });
        }

        let mut chunk = [0u8; 8192];
        let n = stream.read(&mut chunk).await.map_err(|_| RuntimeResponse {
            status: "400 Bad Request",
            body: serde_json::json!({"error": "invalid request"}),
        })?;
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(RuntimeResponse {
                    status: "400 Bad Request",
                    body: serde_json::json!({"error": "incomplete request"}),
                })
            };
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let header_text = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_text.lines();
    let first_line = lines.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    let content_length = content_length(&headers)?;
    if content_length > MAX_BODY_BYTES {
        return Err(RuntimeResponse {
            status: "413 Payload Too Large",
            body: serde_json::json!({"error": "body too large"}),
        });
    }

    let body_start = header_end + 4;
    let body_end = body_start + content_length;
    while buf.len() < body_end {
        let mut chunk = [0u8; 8192];
        let n = stream.read(&mut chunk).await.map_err(|_| RuntimeResponse {
            status: "400 Bad Request",
            body: serde_json::json!({"error": "invalid request"}),
        })?;
        if n == 0 {
            return Err(RuntimeResponse {
                status: "400 Bad Request",
                body: serde_json::json!({"error": "incomplete body"}),
            });
        }
        buf.extend_from_slice(&chunk[..n]);
    }

    Ok(Some(RuntimeRequest {
        first_line,
        headers,
        body: String::from_utf8_lossy(&buf[body_start..body_end]).to_string(),
    }))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &[(String, String)]) -> Result<usize, RuntimeResponse> {
    match headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("content-length")) {
        Some((_, value)) => value.parse::<usize>().map_err(|_| RuntimeResponse {
            status: "400 Bad Request",
            body: serde_json::json!({"error": "invalid content-length"}),
        }),
        None => Ok(0),
    }
}

#[cfg(test)]
fn is_authorized(request: &str, token: &str) -> bool {
    request.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("authorization") && value.trim() == format!("Bearer {token}")
    })
}

fn is_authorized_headers(headers: &[(String, String)], token: &str) -> bool {
    headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("authorization") && value == &format!("Bearer {token}"))
}

async fn route_request(first_line: &str, body: &str, state: &AgentRuntimeState) -> RuntimeResponse {
    if first_line.starts_with("GET /context ") || first_line.starts_with("GET /context?") {
        return RuntimeResponse {
            status: "200 OK",
            body: serde_json::to_value(&*state.snapshot.read().await).unwrap_or_else(|_| serde_json::json!({})),
        };
    }

    if first_line.starts_with("GET /selection ") || first_line.starts_with("GET /selection?") {
        let snapshot = state.snapshot.read().await;
        return RuntimeResponse {
            status: "200 OK",
            body: snapshot.selection.clone().unwrap_or_else(|| serde_json::json!({"type": "none"})),
        };
    }

    if first_line.starts_with("GET /result/current ") || first_line.starts_with("GET /result/current?") {
        let snapshot = state.snapshot.read().await;
        let mut body = snapshot.result.clone().unwrap_or_else(|| serde_json::json!({"columns": [], "rows": []}));
        if let Some(limit) = query_limit(first_line) {
            truncate_result_rows(&mut body, limit);
        }
        return RuntimeResponse { status: "200 OK", body };
    }

    if first_line.starts_with("POST /handoff ") {
        let mut item = match serde_json::from_str::<dbx_core::handoff::HandoffItem>(body) {
            Ok(item) => item,
            Err(_) => {
                return RuntimeResponse {
                    status: "400 Bad Request",
                    body: serde_json::json!({"error": "invalid handoff"}),
                };
            }
        };
        let snapshot = state.snapshot.read().await.clone();
        recompute_handoff_risk(&mut item, &snapshot, state.app_state.as_deref()).await;
        item.status = dbx_core::handoff::HandoffStatus::Shown;
        let id = item.id.clone();
        state.handoffs.write().await.push(item);
        return RuntimeResponse { status: "200 OK", body: serde_json::json!({"id": id, "status": "shown"}) };
    }

    RuntimeResponse { status: "404 Not Found", body: serde_json::json!({"error": "not found"}) }
}

async fn recompute_handoff_risk(item: &mut HandoffItem, snapshot: &AgentRuntimeSnapshot, app_state: Option<&AppState>) {
    let connection = load_handoff_connection(app_state, &item.connection_id).await;
    let risk = match connection {
        ConnectionLookup::Found(config) => {
            risk_for_connection(&item.sql, config.name.as_str(), config.color.as_deref())
        }
        ConnectionLookup::Missing => match matching_snapshot_connection_name(item, snapshot) {
            Some(connection_name) => risk_for_connection(&item.sql, connection_name, None),
            None => conservative_production_risk(&item.sql),
        },
        ConnectionLookup::ReadFailed => conservative_production_risk(&item.sql),
    };
    item.operation_class = risk.operation_class;
    item.risk_level = risk.risk_level;
    item.is_production = risk.is_production;
}

enum ConnectionLookup {
    Found(ConnectionConfig),
    Missing,
    ReadFailed,
}

async fn load_handoff_connection(app_state: Option<&AppState>, connection_id: &str) -> ConnectionLookup {
    let connection_id = connection_id.trim();
    if connection_id.is_empty() {
        return ConnectionLookup::Missing;
    }
    let Some(app_state) = app_state else {
        return ConnectionLookup::Missing;
    };

    match app_state.storage.load_connections().await {
        Ok(configs) => configs
            .into_iter()
            .find(|config| config.id == connection_id)
            .map(ConnectionLookup::Found)
            .unwrap_or(ConnectionLookup::Missing),
        Err(err) => {
            log::warn!("Agent runtime failed to load connection metadata for handoff risk: {err}");
            ConnectionLookup::ReadFailed
        }
    }
}

fn matching_snapshot_connection_name<'a>(
    item: &HandoffItem,
    snapshot: &'a AgentRuntimeSnapshot,
) -> Option<&'a str> {
    let handoff_connection_id = item.connection_id.trim();
    let active_connection_id = snapshot.active_connection_id.as_deref().map(str::trim)?;
    if handoff_connection_id.is_empty() || handoff_connection_id != active_connection_id {
        return None;
    }
    snapshot.active_connection_name.as_deref().map(str::trim).filter(|name| !name.is_empty())
}

fn conservative_production_risk(sql: &str) -> dbx_core::sql_safety::RiskMetadata {
    let mut risk = risk_for(
        sql,
        RiskContext { connection_name: "unknown", color: None, environment_label: Some("Production") },
    );
    risk.is_production = true;
    risk.risk_level = match classify_sql(sql) {
        OperationClass::Ddl => RiskLevel::Critical,
        OperationClass::Write if risk.risk_level == RiskLevel::Critical => RiskLevel::Critical,
        _ => RiskLevel::High,
    };
    risk
}

async fn update_handoff_status(
    app_state: &AppState,
    runtime: &AgentRuntimeState,
    id: &str,
    status: HandoffStatus,
) -> Result<bool, String> {
    let stored = app_state.storage.update_handoff_status(id, status.clone()).await?;
    let runtime_updated = update_runtime_handoff_status(runtime, id, status).await;
    Ok(stored || runtime_updated)
}

async fn update_runtime_handoff_status(state: &AgentRuntimeState, id: &str, status: HandoffStatus) -> bool {
    let mut handoffs = state.handoffs.write().await;
    if let Some(item) = handoffs.iter_mut().find(|item| item.id == id) {
        if !can_update_runtime_handoff_status(&item.status, &status) {
            return false;
        }
        item.status = status;
        return true;
    }
    false
}

fn can_update_runtime_handoff_status(current: &HandoffStatus, next: &HandoffStatus) -> bool {
    matches!(
        (current, next),
        (HandoffStatus::Queued | HandoffStatus::Shown, HandoffStatus::Shown)
            | (HandoffStatus::Queued | HandoffStatus::Shown, HandoffStatus::Rejected)
            | (HandoffStatus::Queued | HandoffStatus::Shown | HandoffStatus::Approved, HandoffStatus::Approved)
            | (HandoffStatus::Approved | HandoffStatus::Executed, HandoffStatus::Executed)
            | (HandoffStatus::Approved | HandoffStatus::Executed | HandoffStatus::Failed, HandoffStatus::Failed)
    )
}

async fn pending_runtime_handoffs(state: &AgentRuntimeState) -> Vec<HandoffItem> {
    state
        .handoffs
        .read()
        .await
        .iter()
        .filter(|item| matches!(item.status, HandoffStatus::Queued | HandoffStatus::Shown))
        .cloned()
        .collect()
}

fn query_limit(first_line: &str) -> Option<usize> {
    let target = first_line.split_whitespace().nth(1)?;
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "limit").then(|| value.parse::<usize>().ok()).flatten()
    })
}

fn truncate_result_rows(result: &mut serde_json::Value, limit: usize) {
    if let Some(rows) = result.get_mut("rows").and_then(|rows| rows.as_array_mut()) {
        rows.truncate(limit);
    }
}

fn write_discovery_file(dir: &Path, port: u16, token: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let path = dir.join(DISCOVERY_FILE);
    let temp_path = dir.join(format!("{DISCOVERY_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let payload = serde_json::json!({ "port": port, "token": token });
    let body = serde_json::to_vec(&payload).map_err(|err| err.to_string())?;

    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temp_path).map_err(|err| err.to_string())?;
    file.write_all(&body).map_err(|err| err.to_string())?;
    file.sync_all().map_err(|err| err.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata().map_err(|err| err.to_string())?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions).map_err(|err| err.to_string())?;
    }
    drop(file);

    if let Err(err) = std::fs::rename(&temp_path, &path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err.to_string());
    }

    Ok(path)
}

fn cleanup_discovery_file(path: &Path) {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

async fn respond_json(stream: &mut TcpStream, status: &str, body: serde_json::Value) {
    let body = serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string());
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn runtime_state() -> AgentRuntimeState {
        AgentRuntimeState {
            token: "secret-token".to_string(),
            snapshot: Arc::new(RwLock::new(AgentRuntimeSnapshot::default())),
            handoffs: Arc::new(RwLock::new(Vec::new())),
            app_state: None,
        }
    }

    async fn runtime_state_with_connections(
        configs: Vec<dbx_core::models::connection::ConnectionConfig>,
    ) -> AgentRuntimeState {
        let db_path = std::env::temp_dir().join(format!("dbx-agent-runtime-test-{}.db", uuid::Uuid::new_v4()));
        let storage = dbx_core::storage::Storage::open(&db_path).await.unwrap();
        storage.save_connections(&configs).await.unwrap();
        AgentRuntimeState { app_state: Some(Arc::new(AppState::new(storage))), ..runtime_state() }
    }

    fn connection_config(
        id: &str,
        name: &str,
        color: Option<&str>,
    ) -> dbx_core::models::connection::ConnectionConfig {
        dbx_core::models::connection::ConnectionConfig {
            id: id.to_string(),
            name: name.to_string(),
            db_type: dbx_core::models::connection::DatabaseType::Postgres,
            driver_profile: None,
            driver_label: None,
            url_params: None,
            host: "127.0.0.1".to_string(),
            port: 5432,
            username: "postgres".to_string(),
            password: "secret".to_string(),
            database: Some("postgres".to_string()),
            color: color.map(str::to_string),
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_user: String::new(),
            ssh_password: String::new(),
            ssh_key_path: String::new(),
            ssh_key_passphrase: String::new(),
            ssh_expose_lan: false,
            ssh_connect_timeout_secs: dbx_core::models::connection::default_ssh_connect_timeout_secs(),
            ssl: false,
            sysdba: false,
            connection_string: None,
            jdbc_driver_class: None,
            jdbc_driver_paths: Vec::new(),
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    async fn serve_once(state: AgentRuntimeState) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection(stream, state).await;
        });
        addr
    }

    #[test]
    fn authorization_requires_exact_bearer_token() {
        assert!(is_authorized("GET /context HTTP/1.1\r\nAuthorization: Bearer secret-token\r\n\r\n", "secret-token",));
        assert!(is_authorized("GET /context HTTP/1.1\r\nauthorization: Bearer secret-token\r\n\r\n", "secret-token",));
        assert!(!is_authorized("GET /context HTTP/1.1\r\nAuthorization: Bearer wrong\r\n\r\n", "secret-token",));
        assert!(!is_authorized("GET /context HTTP/1.1\r\n\r\n", "secret-token"));
    }

    #[tokio::test]
    async fn accepts_reqwest_lowercase_authorization_header() {
        let state = runtime_state();
        *state.snapshot.write().await = AgentRuntimeSnapshot {
            active_connection_id: Some("conn-1".to_string()),
            ..AgentRuntimeSnapshot::default()
        };
        let addr = serve_once(state).await;

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/context"))
            .bearer_auth("secret-token")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["activeConnectionId"], "conn-1");
    }

    #[tokio::test]
    async fn routes_context_selection_result_and_handoff_from_shared_state() {
        let state = runtime_state();
        *state.snapshot.write().await = AgentRuntimeSnapshot {
            active_connection_id: Some("conn-1".to_string()),
            active_connection_name: Some("Local".to_string()),
            selection: Some(serde_json::json!({"type": "grid-cells", "cells": [[1]]})),
            result: Some(serde_json::json!({"columns": ["id"], "rows": [[1]]})),
            ..AgentRuntimeSnapshot::default()
        };

        let context = route_request("GET /context HTTP/1.1", "", &state).await;
        assert_eq!(context.status, "200 OK");
        assert_eq!(context.body["activeConnectionId"], "conn-1");

        let selection = route_request("GET /selection HTTP/1.1", "", &state).await;
        assert_eq!(selection.status, "200 OK");
        assert_eq!(selection.body["type"], "grid-cells");

        let result = route_request("GET /result/current?limit=50 HTTP/1.1", "", &state).await;
        assert_eq!(result.status, "200 OK");
        assert_eq!(result.body["columns"][0], "id");

        let item = dbx_core::handoff::HandoffItem::queued(
            "conn-1".to_string(),
            "Local".to_string(),
            Some("main".to_string()),
            "Review SQL".to_string(),
            None,
            "update users set name = 'a'".to_string(),
            dbx_core::sql_safety::OperationClass::Write,
            dbx_core::sql_safety::RiskLevel::Medium,
            false,
        );
        let handoff = route_request("POST /handoff HTTP/1.1", &serde_json::to_string(&item).unwrap(), &state).await;
        assert_eq!(handoff.status, "200 OK");
        assert_eq!(handoff.body["id"], item.id);
        assert_eq!(state.handoffs.read().await.len(), 1);
    }

    #[tokio::test]
    async fn handoff_recomputes_risk_from_sql_instead_of_trusting_client_fields() {
        let state = runtime_state();
        *state.snapshot.write().await = AgentRuntimeSnapshot {
            active_connection_id: Some("conn-1".to_string()),
            active_connection_name: Some("prod-main".to_string()),
            ..AgentRuntimeSnapshot::default()
        };
        let item = dbx_core::handoff::HandoffItem::queued(
            "conn-1".to_string(),
            "client-supplied-dev".to_string(),
            Some("main".to_string()),
            "Review SQL".to_string(),
            None,
            "drop table users".to_string(),
            dbx_core::sql_safety::OperationClass::Read,
            dbx_core::sql_safety::RiskLevel::Low,
            false,
        );

        let handoff = route_request("POST /handoff HTTP/1.1", &serde_json::to_string(&item).unwrap(), &state).await;

        assert_eq!(handoff.status, "200 OK");
        let stored = state.handoffs.read().await;
        assert_eq!(stored[0].operation_class, dbx_core::sql_safety::OperationClass::Ddl);
        assert_eq!(stored[0].risk_level, dbx_core::sql_safety::RiskLevel::Critical);
        assert!(stored[0].is_production);
    }

    #[tokio::test]
    async fn handoff_uses_conservative_production_when_connection_metadata_is_unavailable() {
        let state = runtime_state();
        let item = dbx_core::handoff::HandoffItem::queued(
            "conn-1".to_string(),
            "client-supplied-dev".to_string(),
            None,
            "Review SQL".to_string(),
            None,
            "update users set name = 'a' where id = 1".to_string(),
            dbx_core::sql_safety::OperationClass::Read,
            dbx_core::sql_safety::RiskLevel::Low,
            false,
        );

        let handoff = route_request("POST /handoff HTTP/1.1", &serde_json::to_string(&item).unwrap(), &state).await;

        assert_eq!(handoff.status, "200 OK");
        let stored = state.handoffs.read().await;
        assert_eq!(stored[0].operation_class, dbx_core::sql_safety::OperationClass::Write);
        assert_eq!(stored[0].risk_level, dbx_core::sql_safety::RiskLevel::High);
        assert!(stored[0].is_production);
    }

    #[tokio::test]
    async fn handoff_does_not_use_active_snapshot_when_target_connection_differs() {
        let state = runtime_state_with_connections(vec![connection_config("target-dev", "dev-target", None)]).await;
        *state.snapshot.write().await = AgentRuntimeSnapshot {
            active_connection_id: Some("active-prod".to_string()),
            active_connection_name: Some("prod-main".to_string()),
            ..AgentRuntimeSnapshot::default()
        };
        let item = dbx_core::handoff::HandoffItem::queued(
            "target-dev".to_string(),
            "dev-target".to_string(),
            Some("main".to_string()),
            "Review SQL".to_string(),
            None,
            "update users set name = 'a' where id = 1".to_string(),
            dbx_core::sql_safety::OperationClass::Read,
            dbx_core::sql_safety::RiskLevel::Low,
            false,
        );

        let handoff = route_request("POST /handoff HTTP/1.1", &serde_json::to_string(&item).unwrap(), &state).await;

        assert_eq!(handoff.status, "200 OK");
        let stored = state.handoffs.read().await;
        assert_eq!(stored[0].operation_class, dbx_core::sql_safety::OperationClass::Write);
        assert_eq!(stored[0].risk_level, dbx_core::sql_safety::RiskLevel::Medium);
        assert!(!stored[0].is_production);
    }

    #[tokio::test]
    async fn result_current_limit_truncates_rows() {
        let state = runtime_state();
        *state.snapshot.write().await = AgentRuntimeSnapshot {
            result: Some(serde_json::json!({"columns": ["id"], "rows": [[1], [2], [3]]})),
            ..AgentRuntimeSnapshot::default()
        };

        let result = route_request("GET /result/current?limit=2 HTTP/1.1", "", &state).await;

        assert_eq!(result.status, "200 OK");
        assert_eq!(result.body["rows"], serde_json::json!([[1], [2]]));
    }

    #[tokio::test]
    async fn reads_fragmented_handoff_body_until_content_length() {
        let state = runtime_state();
        let addr = serve_once(state.clone()).await;
        let item = dbx_core::handoff::HandoffItem::queued(
            "conn-1".to_string(),
            "Local".to_string(),
            Some("main".to_string()),
            "Review SQL".to_string(),
            None,
            "select ".to_string() + &"1".repeat(70_000),
            dbx_core::sql_safety::OperationClass::Read,
            dbx_core::sql_safety::RiskLevel::Low,
            false,
        );
        let body = serde_json::to_string(&item).unwrap();
        let head = format!(
            "POST /handoff HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer secret-token\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let split_at = body.len() / 2;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(body[..split_at].as_bytes()).await.unwrap();
        tokio::task::yield_now().await;
        stream.write_all(body[split_at..].as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();

        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert_eq!(state.handoffs.read().await.len(), 1);
    }

    #[tokio::test]
    async fn rejects_body_larger_than_limit() {
        let state = runtime_state();
        let addr = serve_once(state).await;
        let body_len = 1_048_577;
        let request = format!(
            "POST /handoff HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer secret-token\r\nContent-Length: {body_len}\r\n\r\n"
        );
        let mut stream = TcpStream::connect(addr).await.unwrap();

        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();

        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large"), "{response}");
    }

    #[tokio::test]
    async fn runtime_handoff_status_updates_filter_pending_items() {
        let state = runtime_state();
        let item = dbx_core::handoff::HandoffItem::queued(
            "conn-1".to_string(),
            "Local".to_string(),
            Some("main".to_string()),
            "Review SQL".to_string(),
            None,
            "update users set name = 'a'".to_string(),
            dbx_core::sql_safety::OperationClass::Write,
            dbx_core::sql_safety::RiskLevel::Medium,
            false,
        );
        let id = item.id.clone();
        state.handoffs.write().await.push(item);

        assert!(update_runtime_handoff_status(&state, &id, dbx_core::handoff::HandoffStatus::Shown).await);
        assert_eq!(pending_runtime_handoffs(&state).await[0].status, dbx_core::handoff::HandoffStatus::Shown);

        assert!(update_runtime_handoff_status(&state, &id, dbx_core::handoff::HandoffStatus::Rejected).await);
        assert!(pending_runtime_handoffs(&state).await.is_empty());
        assert!(!update_runtime_handoff_status(&state, &id, dbx_core::handoff::HandoffStatus::Shown).await);
    }

    #[test]
    fn discovery_file_is_owner_only_and_removed_on_cleanup() {
        let dir = std::env::temp_dir().join(format!("dbx-agent-runtime-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let path = write_discovery_file(&dir, 4321, "secret-token").unwrap();
        let payload: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(payload["port"], 4321);
        assert_eq!(payload["token"], "secret-token");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);

        cleanup_discovery_file(&path);
        assert!(!path.exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_file_replaces_existing_symlink() {
        let dir = std::env::temp_dir().join(format!("dbx-agent-runtime-symlink-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.json");
        let link = dir.join(DISCOVERY_FILE);
        std::fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let path = write_discovery_file(&dir, 4321, "secret-token").unwrap();

        assert!(!std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        assert_eq!(mode(&path), 0o600);

        let _ = std::fs::remove_dir_all(dir);
    }
}
