use std::time::Duration;

use dbx_core::db::agent_driver::{AgentDriverClient, AgentLane, AgentLaunchSpec, AgentMethod, AgentRuntimeClient};

#[tokio::test]
async fn repeated_quarantines_replace_the_shared_agent_process() {
    let script_path = std::env::temp_dir().join(format!("dbx-agent-replace-test-{}.py", uuid::Uuid::new_v4()));
    std::fs::write(
        &script_path,
        r#"import json, sys, threading, time
print(json.dumps({'ready': True}), flush=True)
def respond(req):
    method = req['method']
    if method == 'handshake':
        result = {'protocolVersion': 3, 'agentProtocolVersion': 3, 'capabilities': ['multi_session', 'operation_control', 'session_generation', 'lane_isolation', 'structured_agent_errors']}
    elif method == 'execute_query' and req.get('params', {}).get('sql') == 'blocked':
        time.sleep(10)
        result = {'rows': []}
    elif method == 'execute_query':
        result = {'rows': [[1]]}
    else:
        result = {'ok': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
for line in sys.stdin:
    threading.Thread(target=respond, args=(json.loads(line),), daemon=True).start()
"#,
    )
    .unwrap();
    let python = if cfg!(windows) { "python" } else { "python3" };
    let runtime = AgentRuntimeClient::spawn(
        AgentLaunchSpec::new(python).with_args([script_path.to_string_lossy().to_string()]),
        "test",
    )
    .await
    .unwrap();
    let mut client = AgentDriverClient::shared_session_with_context(
        runtime.clone(),
        "replace-session".to_string(),
        1,
        AgentLane::Workload,
    );

    for _ in 0..2 {
        let result = client
            .execute_query_with_timeout::<serde_json::Value>(
                serde_json::json!({"sql": "blocked"}),
                Some(Duration::from_millis(20)),
            )
            .await;
        assert!(result.is_err());
    }

    assert!(runtime.is_failed());
    let replacement_runtime = AgentRuntimeClient::spawn(
        AgentLaunchSpec::new(python).with_args([script_path.to_string_lossy().to_string()]),
        "test",
    )
    .await
    .unwrap();
    let mut replacement = AgentDriverClient::shared_session_with_context(
        replacement_runtime.clone(),
        "replacement-session".to_string(),
        2,
        AgentLane::Workload,
    );
    let probe = replacement
        .execute_query_with_timeout::<serde_json::Value>(
            serde_json::json!({"sql": "SELECT 1"}),
            Some(Duration::from_secs(1)),
        )
        .await
        .unwrap();
    assert_eq!(probe["rows"][0][0], 1);
    replacement_runtime.kill_and_wait().await;
    let _ = std::fs::remove_file(script_path);
}

#[tokio::test]
async fn completed_late_responses_do_not_accumulate_toward_process_replacement() {
    let script_path = std::env::temp_dir().join(format!("dbx-agent-late-response-test-{}.py", uuid::Uuid::new_v4()));
    std::fs::write(
        &script_path,
        r#"import json, sys, threading, time
print(json.dumps({'ready': True}), flush=True)
def respond(req):
    method = req['method']
    if method == 'handshake':
        result = {'protocolVersion': 3, 'agentProtocolVersion': 3, 'capabilities': ['multi_session', 'operation_control', 'session_generation', 'lane_isolation', 'structured_agent_errors']}
    elif method == 'execute_query':
        time.sleep(0.05)
        result = {'rows': []}
    else:
        result = {'ok': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
for line in sys.stdin:
    threading.Thread(target=respond, args=(json.loads(line),), daemon=True).start()
"#,
    )
    .unwrap();
    let python = if cfg!(windows) { "python" } else { "python3" };
    let runtime = AgentRuntimeClient::spawn(
        AgentLaunchSpec::new(python).with_args([script_path.to_string_lossy().to_string()]),
        "test",
    )
    .await
    .unwrap();
    let mut client = AgentDriverClient::shared_session_with_context(
        runtime.clone(),
        "late-response-session".to_string(),
        1,
        AgentLane::Workload,
    );

    for _ in 0..2 {
        let result = client
            .execute_query_with_timeout::<serde_json::Value>(
                serde_json::json!({"sql": "briefly-blocked"}),
                Some(Duration::from_millis(20)),
            )
            .await;
        assert!(result.is_err());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(!runtime.is_failed());
    runtime.kill_and_wait().await;
    let _ = std::fs::remove_file(script_path);
}

#[tokio::test]
async fn blocked_metadata_lane_does_not_block_workload_lane() {
    let script_path = std::env::temp_dir().join(format!("dbx-agent-lane-test-{}.py", uuid::Uuid::new_v4()));
    std::fs::write(
        &script_path,
        r#"import json, sys, threading, time
print(json.dumps({'ready': True}), flush=True)
def respond(req):
    method = req['method']
    if method == 'handshake':
        result = {'protocolVersion': 3, 'agentProtocolVersion': 3, 'capabilities': ['multi_session', 'operation_control', 'session_generation', 'lane_isolation', 'structured_agent_errors']}
    elif method == 'list_databases':
        time.sleep(0.3)
        result = []
    elif method == 'execute_query':
        result = {'rows': [[1]]}
    else:
        result = {'ok': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
for line in sys.stdin:
    threading.Thread(target=respond, args=(json.loads(line),), daemon=True).start()
"#,
    )
    .unwrap();
    let python = if cfg!(windows) { "python" } else { "python3" };
    let runtime = AgentRuntimeClient::spawn(
        AgentLaunchSpec::new(python).with_args([script_path.to_string_lossy().to_string()]),
        "test",
    )
    .await
    .unwrap();
    let metadata = AgentDriverClient::shared_session_with_context(
        runtime.clone(),
        "metadata-session".to_string(),
        1,
        AgentLane::Metadata,
    )
    .concurrent_session_handle()
    .unwrap();
    let workload = AgentDriverClient::shared_session_with_context(
        runtime.clone(),
        "workload-session".to_string(),
        1,
        AgentLane::Workload,
    )
    .concurrent_session_handle()
    .unwrap();

    let metadata_call = tokio::spawn(async move {
        metadata
            .call::<serde_json::Value>(
                AgentMethod::ListDatabases,
                serde_json::json!({}),
                Some(Duration::from_secs(1)),
                None,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let started = std::time::Instant::now();
    let workload_result = workload
        .call::<serde_json::Value>(
            AgentMethod::ExecuteQuery,
            serde_json::json!({"sql": "SELECT 1"}),
            Some(Duration::from_millis(200)),
            None,
        )
        .await
        .unwrap();

    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(workload_result["rows"][0][0], 1);
    metadata_call.await.unwrap().unwrap();
    runtime.kill_and_wait().await;
    let _ = std::fs::remove_file(script_path);
}

#[tokio::test]
async fn twenty_unrecoverable_cycles_leave_no_running_agent_processes() {
    let script_path = std::env::temp_dir().join(format!("dbx-agent-resource-bound-test-{}.py", uuid::Uuid::new_v4()));
    std::fs::write(
        &script_path,
        r#"import json, sys, threading, time
print(json.dumps({'ready': True}), flush=True)
def respond(req):
    if req['method'] == 'handshake':
        result = {'protocolVersion': 3, 'agentProtocolVersion': 3, 'capabilities': ['multi_session', 'operation_control', 'session_generation', 'lane_isolation', 'structured_agent_errors']}
    elif req['method'] == 'execute_query':
        time.sleep(10)
        result = {'rows': []}
    else:
        result = {'ok': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
for line in sys.stdin:
    threading.Thread(target=respond, args=(json.loads(line),), daemon=True).start()
"#,
    )
    .unwrap();
    let python = if cfg!(windows) { "python" } else { "python3" };

    for cycle in 0..20 {
        let runtime = AgentRuntimeClient::spawn(
            AgentLaunchSpec::new(python).with_args([script_path.to_string_lossy().to_string()]),
            "test",
        )
        .await
        .unwrap();
        let mut client = AgentDriverClient::shared_session_with_context(
            runtime.clone(),
            format!("resource-bound-session-{cycle}"),
            cycle + 1,
            AgentLane::Workload,
        );
        for _ in 0..2 {
            assert!(client
                .execute_query_with_timeout::<serde_json::Value>(
                    serde_json::json!({"sql": "blocked"}),
                    Some(Duration::from_millis(10)),
                )
                .await
                .is_err());
        }
        assert!(runtime.is_failed(), "cycle {cycle} did not cross the replacement threshold");
        runtime.kill_and_wait().await;
    }

    let _ = std::fs::remove_file(script_path);
}
