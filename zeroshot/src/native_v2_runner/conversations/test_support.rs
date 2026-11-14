//! An in-process provider: native Codex file names, identifiers and cleanup, with no child process.
use super::*;
use crate::native_v2_capsule::provider_process::ProviderSessionCore;
use crate::native_v2_contract::NodeInvocation;
use openengine_cluster_protocol::RunId;
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, NodeDriver, NodeRunnerError, NodeSession, ResolvedEnvironment,
    SessionFactory,
};
use async_trait::async_trait;
use openengine_cluster_protocol::WorkerOutcome;
use tokio::sync::Mutex;

pub(crate) const ID: &str = "checkpoint-provider-thread";
pub(crate) const CONTENT: &str = "first turn known only to the provider";

pub(crate) struct Provider {
    pub home: PathBuf,
    pub restored: Arc<Mutex<Vec<(String, PathBuf, String)>>>,
}

struct Session {
    core: ProviderSessionCore,
    home: PathBuf,
    id: Mutex<Option<String>>,
}

#[async_trait]
impl NodeSession for Session {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn is_live(&self) -> bool {
        self.core.is_live()
    }
    async fn close(&self) {
        self.core.close().await;
        let _ = fs::remove_dir_all(&self.home);
    }
    async fn conversation(&self) -> Result<ConversationSource, &'static str> {
        self.core.capture_conversation().await
    }
    async fn restore_conversation(&self, source: ConversationSource) -> bool {
        self.core.queue_conversation(source).await;
        true
    }
}

#[async_trait]
impl SessionFactory for Provider {
    async fn open(
        &self,
        _invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        Ok(Arc::new(Session {
            core: ProviderSessionCore::new(),
            home: self.home.clone(),
            id: Mutex::new(None),
        }))
    }
}

#[async_trait]
impl NodeDriver for Provider {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let session = invocation
            .session
            .as_any()
            .downcast_ref::<Session>()
            .unwrap();
        let layout = crate::native_v2_codex::conversation::layout(session.home.clone());
        let metadata = session
            .core
            .prepare_conversation(Some(layout), &invocation, &control)
            .await?;
        if let Some(metadata) = metadata {
            *session.id.lock().await = Some(metadata.id);
        }
        let mut id = session.id.lock().await;
        let file = session
            .home
            .join(format!("sessions/2026/11/14/rollout-{ID}.jsonl"));
        if id.is_some() {
            assert_eq!(id.as_deref(), Some(ID));
            let content = fs::read_to_string(&file).unwrap();
            assert_eq!(content, CONTENT);
            self.restored
                .lock()
                .await
                .push((ID.to_owned(), session.home.clone(), content));
        } else {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, CONTENT).unwrap();
            *id = Some(ID.to_owned());
        }
        session
            .core
            .record_conversation(id.clone(), super::layout_tests::usage())
            .await;
        Ok(WorkerOutcome::Verified {
            output: serde_json::Value::Null,
            artifacts: Vec::new(),
        })
    }
    async fn close_run(&self, _run_id: &RunId) {}
}
