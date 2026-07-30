use std::sync::Arc;

use tokio::sync::{Mutex, MutexGuard, TryLockError};

use super::agent_driver::AgentDriverClient;

/// Owns an Agent client and exposes intent-specific access lanes.
///
/// Recovery-capable workload calls use `AgentSessionHandle` and therefore do
/// not hold this guard while waiting for JDBC. Legacy agents remain serialized
/// behind the compatibility lane until their protocol is upgraded.
pub struct AgentConnectionPool {
    client: Mutex<AgentDriverClient>,
}

impl AgentConnectionPool {
    pub fn new(client: AgentDriverClient) -> Arc<Self> {
        Arc::new(Self { client: Mutex::new(client) })
    }

    pub async fn metadata(&self) -> MutexGuard<'_, AgentDriverClient> {
        self.client.lock().await
    }

    pub async fn workload(&self) -> MutexGuard<'_, AgentDriverClient> {
        self.client.lock().await
    }

    pub async fn control(&self) -> MutexGuard<'_, AgentDriverClient> {
        self.client.lock().await
    }

    pub async fn compatibility(&self) -> MutexGuard<'_, AgentDriverClient> {
        self.client.lock().await
    }

    pub fn try_control(&self) -> Result<MutexGuard<'_, AgentDriverClient>, TryLockError> {
        self.client.try_lock()
    }
}
