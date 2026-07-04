#![cfg(feature = "duckdb-bundled")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::db::duckdb_worker_protocol::{
    DuckDbWorkerConnectParams, DuckDbWorkerError, DuckDbWorkerExecuteParams, DuckDbWorkerMethod, DuckDbWorkerRequest,
    DuckDbWorkerResponse,
};
use crate::models::connection::AttachedDatabaseConfig;

const DEFAULT_WORKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_WORKER_KILL_WAIT: Duration = Duration::from_secs(3);
pub const DEFAULT_DUCKDB_WORKER_CANCEL_GRACE: Duration = Duration::from_millis(1500);

type PendingRequests = Arc<Mutex<HashMap<String, PendingRequest>>>;

struct PendingRequest {
    generation: u64,
    sender: oneshot::Sender<DuckDbWorkerResponse>,
}

#[derive(Clone)]
pub struct DuckDbWorkerClient {
    inner: Arc<DuckDbWorkerClientInner>,
}

struct DuckDbWorkerClientInner {
    state: Mutex<WorkerProcessState>,
    pending: PendingRequests,
    connect_lock: Mutex<()>,
    executable: PathBuf,
    connect_params: DuckDbWorkerConnectParams,
    request_timeout: Duration,
    cancel_grace: Duration,
    next_id: AtomicU64,
}

#[derive(Default)]
struct WorkerProcessState {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    connected: bool,
    generation: u64,
}

impl DuckDbWorkerClient {
    pub async fn open(path: String, attached_databases: Vec<AttachedDatabaseConfig>) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        Self::open_with_executable(executable, path, attached_databases).await
    }

    pub async fn open_with_executable(
        executable: PathBuf,
        path: String,
        attached_databases: Vec<AttachedDatabaseConfig>,
    ) -> Result<Self, String> {
        let client = Self {
            inner: Arc::new(DuckDbWorkerClientInner {
                state: Mutex::new(WorkerProcessState::default()),
                pending: Arc::new(Mutex::new(HashMap::new())),
                connect_lock: Mutex::new(()),
                executable,
                connect_params: DuckDbWorkerConnectParams { path, attached_databases },
                request_timeout: DEFAULT_WORKER_REQUEST_TIMEOUT,
                cancel_grace: DEFAULT_DUCKDB_WORKER_CANCEL_GRACE,
                next_id: AtomicU64::new(1),
            }),
        };
        client.ensure_connected().await?;
        Ok(client)
    }

    pub async fn execute(
        &self,
        database: Option<String>,
        sql: String,
        max_rows: Option<usize>,
        cancel_token: Option<CancellationToken>,
        query_timeout: Option<Duration>,
    ) -> Result<db::QueryResult, String> {
        let client = self.clone();
        let future = async move {
            client
                .request::<db::QueryResult>(
                    DuckDbWorkerMethod::Execute,
                    DuckDbWorkerExecuteParams { sql, database, max_rows },
                    None,
                )
                .await
        };
        tokio::pin!(future);

        match (cancel_token, query_timeout) {
            (Some(token), Some(duration)) => {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => self.cancel_or_kill(future.as_mut(), crate::query::canceled_error()).await,
                    result = &mut future => result,
                    _ = tokio::time::sleep(duration) => self.cancel_or_kill(future.as_mut(), crate::query::timeout_error()).await,
                }
            }
            (Some(token), None) => {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => self.cancel_or_kill(future.as_mut(), crate::query::canceled_error()).await,
                    result = &mut future => result,
                }
            }
            (None, Some(duration)) => {
                tokio::select! {
                    result = &mut future => result,
                    _ = tokio::time::sleep(duration) => self.cancel_or_kill(future.as_mut(), crate::query::timeout_error()).await,
                }
            }
            (None, None) => future.await,
        }
    }

    async fn cancel_or_kill<F>(
        &self,
        future: std::pin::Pin<&mut F>,
        final_error: String,
    ) -> Result<db::QueryResult, String>
    where
        F: std::future::Future<Output = Result<db::QueryResult, String>>,
    {
        let _ = self.cancel().await;
        match tokio::time::timeout(self.inner.cancel_grace, future).await {
            Ok(_) => Err(final_error),
            Err(_) => {
                self.kill().await;
                Err(final_error)
            }
        }
    }

    pub async fn list_databases(&self) -> Result<Vec<db::DatabaseInfo>, String> {
        self.request(DuckDbWorkerMethod::ListDatabases, serde_json::json!({}), Some(self.inner.request_timeout)).await
    }

    pub async fn list_schemas(&self, database: String) -> Result<Vec<String>, String> {
        self.request(
            DuckDbWorkerMethod::ListSchemas,
            serde_json::json!({ "database": database }),
            Some(self.inner.request_timeout),
        )
        .await
    }

    pub async fn list_tables(&self, database: String, schema: String) -> Result<Vec<db::TableInfo>, String> {
        self.request(
            DuckDbWorkerMethod::ListTables,
            serde_json::json!({ "database": database, "schema": schema }),
            Some(self.inner.request_timeout),
        )
        .await
    }

    pub async fn list_columns(
        &self,
        database: String,
        schema: String,
        table: String,
    ) -> Result<Vec<db::ColumnInfo>, String> {
        self.request(
            DuckDbWorkerMethod::ListColumns,
            serde_json::json!({ "database": database, "schema": schema, "table": table }),
            Some(self.inner.request_timeout),
        )
        .await
    }

    pub async fn attach_database(&self, attached: AttachedDatabaseConfig) -> Result<(), String> {
        self.request::<serde_json::Value>(
            DuckDbWorkerMethod::AttachDatabase,
            attached,
            Some(self.inner.request_timeout),
        )
        .await?;
        Ok(())
    }

    pub async fn cancel(&self) -> Result<(), String> {
        self.send_notification(DuckDbWorkerMethod::Cancel, serde_json::json!({})).await?;
        Ok(())
    }

    pub async fn shutdown(&self) {
        let _ = self
            .send_request::<serde_json::Value>(
                DuckDbWorkerMethod::Shutdown,
                serde_json::json!({}),
                Some(self.inner.request_timeout),
            )
            .await;
        self.kill().await;
    }

    pub async fn kill(&self) {
        let (child, generation) = {
            let mut state = self.inner.state.lock().await;
            let child = state.child.take();
            let generation = state.generation;
            state.stdin = None;
            state.connected = false;
            (child, generation)
        };

        if let Some(mut child) = child {
            let _ = child.start_kill();
            match tokio::time::timeout(DEFAULT_WORKER_KILL_WAIT, child.wait()).await {
                Ok(Ok(status)) => log::info!("[duckdb-worker:kill:exit] status={status}"),
                Ok(Err(err)) => log::warn!("[duckdb-worker:kill:wait-failed] error={err}"),
                Err(_) => {
                    log::warn!("[duckdb-worker:kill:wait-timeout] wait_ms={}", DEFAULT_WORKER_KILL_WAIT.as_millis())
                }
            }
        }
        self.fail_pending_for_generation(generation, "duckdb_worker_killed", "DuckDB worker process was killed").await;
    }

    async fn request<T>(
        &self,
        method: DuckDbWorkerMethod,
        params: impl serde::Serialize,
        timeout: Option<Duration>,
    ) -> Result<T, String>
    where
        T: serde::de::DeserializeOwned,
    {
        self.ensure_connected().await?;
        self.send_request(method, params, timeout).await
    }

    async fn ensure_connected(&self) -> Result<(), String> {
        let _guard = self.inner.connect_lock.lock().await;
        {
            let mut state = self.inner.state.lock().await;
            self.ensure_started_locked(&mut state)?;
            if state.connected {
                return Ok(());
            }
        }

        self.send_request::<serde_json::Value>(
            DuckDbWorkerMethod::Connect,
            self.inner.connect_params.clone(),
            Some(self.inner.request_timeout),
        )
        .await?;

        let mut state = self.inner.state.lock().await;
        state.connected = true;
        Ok(())
    }

    fn ensure_started_locked(&self, state: &mut WorkerProcessState) -> Result<(), String> {
        let should_restart = match state.child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => {
                    log::warn!("[duckdb-worker:exit] status={status}");
                    true
                }
                Ok(None) => return Ok(()),
                Err(err) => {
                    log::warn!("[duckdb-worker:exit-check-failed] error={err}");
                    true
                }
            },
            None => true,
        };

        if should_restart {
            state.child = None;
            state.stdin = None;
            state.connected = false;
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;

        log::info!("[duckdb-worker:start] executable={}", self.inner.executable.display());
        let mut child = Command::new(&self.inner.executable)
            .arg("--duckdb-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Failed to start DuckDB worker: {e}"))?;

        let stdin = child.stdin.take().ok_or("DuckDB worker stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("DuckDB worker stdout unavailable")?;
        spawn_stdout_reader(stdout, self.inner.pending.clone(), generation);

        state.child = Some(child);
        state.stdin = Some(stdin);
        state.connected = false;
        Ok(())
    }

    async fn send_request<T>(
        &self,
        method: DuckDbWorkerMethod,
        params: impl serde::Serialize,
        timeout: Option<Duration>,
    ) -> Result<T, String>
    where
        T: serde::de::DeserializeOwned,
    {
        let id = format!("duckdb-worker-{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let request = DuckDbWorkerRequest::new(&id, method, params)?;
        let line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        let (tx, rx) = oneshot::channel();

        let write_result = async {
            let mut state = self.inner.state.lock().await;
            self.ensure_started_locked(&mut state)?;
            if method != DuckDbWorkerMethod::Connect && !state.connected {
                return Err("DuckDB worker is not connected".to_string());
            }
            let generation = state.generation;
            self.inner.pending.lock().await.insert(id.clone(), PendingRequest { generation, sender: tx });
            let stdin = state.stdin.as_mut().ok_or("DuckDB worker stdin unavailable")?;
            stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
            stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
            stdin.flush().await.map_err(|e| e.to_string())
        }
        .await;

        if let Err(err) = write_result {
            self.inner.pending.lock().await.remove(&id);
            return Err(err);
        }

        let response = match timeout {
            Some(timeout) => match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(response)) => response,
                Ok(Err(_)) => return Err("DuckDB worker response channel closed".to_string()),
                Err(_) => {
                    self.inner.pending.lock().await.remove(&id);
                    return Err(format!("DuckDB worker request timed out after {}s", timeout.as_secs()));
                }
            },
            None => rx.await.map_err(|_| "DuckDB worker response channel closed".to_string())?,
        };

        if !response.ok {
            let error = response
                .error
                .unwrap_or_else(|| DuckDbWorkerError::new("duckdb_worker_error", "DuckDB worker request failed"));
            return Err(error.message);
        }

        let result = response.result.unwrap_or(serde_json::Value::Null);
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    async fn send_notification(&self, method: DuckDbWorkerMethod, params: impl serde::Serialize) -> Result<(), String> {
        self.ensure_connected().await?;
        let id = format!("duckdb-worker-{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let request = DuckDbWorkerRequest::new(id, method, params)?;
        let line = serde_json::to_string(&request).map_err(|e| e.to_string())?;

        let mut state = self.inner.state.lock().await;
        self.ensure_started_locked(&mut state)?;
        if method != DuckDbWorkerMethod::Connect && !state.connected {
            return Err("DuckDB worker is not connected".to_string());
        }
        let stdin = state.stdin.as_mut().ok_or("DuckDB worker stdin unavailable")?;
        stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    async fn fail_pending_for_generation(&self, generation: u64, code: &'static str, message: &'static str) {
        let pending = {
            let mut pending = self.inner.pending.lock().await;
            let ids = pending
                .iter()
                .filter_map(|(id, request)| (request.generation == generation).then(|| id.clone()))
                .collect::<Vec<_>>();
            ids.into_iter().filter_map(|id| pending.remove(&id).map(|request| (id, request.sender))).collect::<Vec<_>>()
        };
        for (id, sender) in pending {
            let _ = sender.send(DuckDbWorkerResponse::err(id, DuckDbWorkerError::new(code, message)));
        }
    }
}

fn spawn_stdout_reader(stdout: tokio::process::ChildStdout, pending: PendingRequests, generation: u64) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<DuckDbWorkerResponse>(&line) {
                        Ok(response) => {
                            let sender = {
                                let mut pending = pending.lock().await;
                                let matches_generation = pending
                                    .get(&response.id)
                                    .map(|request| request.generation == generation)
                                    .unwrap_or(false);
                                if matches_generation {
                                    pending.remove(&response.id).map(|request| request.sender)
                                } else {
                                    None
                                }
                            };
                            if let Some(sender) = sender {
                                let _ = sender.send(response);
                            }
                        }
                        Err(err) => {
                            log::warn!("[duckdb-worker:invalid-response] error={err} line={line}");
                        }
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    log::warn!("[duckdb-worker:stdout-error] error={err}");
                    break;
                }
            }
        }

        let pending = {
            let mut pending = pending.lock().await;
            let ids = pending
                .iter()
                .filter_map(|(id, request)| (request.generation == generation).then(|| id.clone()))
                .collect::<Vec<_>>();
            ids.into_iter().filter_map(|id| pending.remove(&id).map(|request| (id, request.sender))).collect::<Vec<_>>()
        };
        for (id, sender) in pending {
            let _ = sender.send(DuckDbWorkerResponse::err(
                id,
                DuckDbWorkerError::new("duckdb_worker_exited", "DuckDB worker process exited"),
            ));
        }
    });
}
