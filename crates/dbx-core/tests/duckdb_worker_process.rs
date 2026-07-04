#![cfg(feature = "duckdb-bundled")]

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dbx_core::db::duckdb_worker_process::DuckDbWorkerClient;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_process_recovers_immediately_after_cancelled_long_query() {
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_duckdb-worker-test-host"));
    let db_path = temp_duckdb_path();
    let _ = std::fs::remove_file(&db_path);

    let client =
        DuckDbWorkerClient::open_with_executable(executable, db_path.to_string_lossy().to_string(), Vec::new())
            .await
            .expect("worker process connects");

    let token = CancellationToken::new();
    let long_query = client.execute(
        None,
        "SELECT sum(sin(i::DOUBLE) * cos(i::DOUBLE / 3.0)) FROM range(100000000000) AS t(i)".to_string(),
        Some(10),
        Some(token.clone()),
        Some(Duration::from_secs(30)),
    );
    tokio::pin!(long_query);

    tokio::time::sleep(Duration::from_millis(200)).await;
    token.cancel();

    let cancelled = tokio::time::timeout(Duration::from_secs(5), &mut long_query)
        .await
        .expect("cancelled query should return promptly");
    assert_eq!(cancelled.expect_err("long query should be cancelled"), dbx_core::query::canceled_error());

    let probe = tokio::time::timeout(
        Duration::from_secs(5),
        client.execute(
            None,
            "SELECT 1 AS after_cancel_probe".to_string(),
            Some(10),
            None,
            Some(Duration::from_secs(5)),
        ),
    )
    .await
    .expect("probe query should not hang")
    .expect("probe query should succeed");

    assert_eq!(probe.columns, vec!["after_cancel_probe".to_string()]);
    assert_eq!(probe.rows, vec![vec![serde_json::json!(1)]]);

    client.shutdown().await;
    let _ = std::fs::remove_file(&db_path);
}

fn temp_duckdb_path() -> PathBuf {
    let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("dbx-duckdb-worker-process-{suffix}.duckdb"))
}
