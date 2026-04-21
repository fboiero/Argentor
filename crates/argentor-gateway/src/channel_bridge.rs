//! Bridge between the channel subsystem and the gateway.
//!
//! [`ChannelBridge`] connects the [`ChannelManager`] from `argentor-channels`
//! into the gateway pipeline, allowing the gateway to forward agent responses
//! to platform channels (Slack, Discord, Telegram) and receive messages from
//! them via a shared event loop.

use crate::router::{InboundMessage, MessageRouter};
use argentor_channels::{ChannelEvent, ChannelManager, ChannelMessage};
use argentor_core::{ArgentorError, ArgentorResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Maps a (channel_name, sender_id) pair to a persistent session.
#[derive(Debug, Clone)]
pub struct ChannelSession {
    /// Name of the originating channel (e.g. "slack", "discord").
    pub channel_name: String,
    /// Unique identifier of the sender within that channel.
    pub sender_id: String,
    /// Session identifier assigned by the bridge.
    pub session_id: Uuid,
}

/// Bridges the [`ChannelManager`] into the gateway, forwarding messages
/// between platform channels and the agent pipeline.
pub struct ChannelBridge {
    manager: Arc<RwLock<ChannelManager>>,
    router: Arc<MessageRouter>,
    /// Session affinity: maps (channel_name, sender_id) -> session_id.
    sessions: Arc<RwLock<HashMap<(String, String), Uuid>>>,
}

impl ChannelBridge {
    /// Create a new bridge connecting a [`ChannelManager`] to a [`MessageRouter`].
    pub fn new(manager: ChannelManager, router: Arc<MessageRouter>) -> Self {
        Self {
            manager: Arc::new(RwLock::new(manager)),
            router,
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn a background task that processes [`ChannelEvent`]s from the
    /// given receiver.
    ///
    /// For each `MessageReceived` event the task will:
    /// 1. Look up (or create) a session for the channel+sender combination.
    /// 2. Route the message through the [`MessageRouter`] to the agent.
    /// 3. Send the agent's response back through the originating channel.
    pub fn start_event_loop(&self, mut event_rx: mpsc::Receiver<ChannelEvent>) -> JoinHandle<()> {
        let manager = Arc::clone(&self.manager);
        let router = Arc::clone(&self.router);
        let sessions = Arc::clone(&self.sessions);

        tokio::spawn(async move {
            info!("ChannelBridge event loop started");

            while let Some(event) = event_rx.recv().await {
                match event {
                    ChannelEvent::MessageReceived(msg) => {
                        let channel_name = msg.channel_id.clone();
                        let sender_id = msg.sender_id.clone();
                        let content = msg.content.clone();

                        // Resolve or create session
                        let session_id = {
                            let key = (channel_name.clone(), sender_id.clone());
                            let mut map = sessions.write().await;
                            *map.entry(key).or_insert_with(Uuid::new_v4)
                        };

                        info!(
                            channel = %channel_name,
                            sender = %sender_id,
                            session_id = %session_id,
                            "Routing channel message to agent"
                        );

                        // Build an InboundMessage for the router
                        let inbound = InboundMessage {
                            session_id: Some(session_id),
                            content,
                        };

                        // A synthetic connection_id for channel-originated messages
                        let connection_id = Uuid::new_v4();

                        // Route through the agent pipeline
                        match router.handle_message(inbound, connection_id).await {
                            Ok(()) => {
                                info!(
                                    channel = %channel_name,
                                    session_id = %session_id,
                                    "Message routed successfully"
                                );
                            }
                            Err(e) => {
                                error!(
                                    channel = %channel_name,
                                    error = %e,
                                    "Failed to route channel message"
                                );
                                // Attempt to send an error notification back to the channel
                                let error_msg = ChannelMessage {
                                    channel_id: channel_name.clone(),
                                    sender_id: "system".to_string(),
                                    content: format!("Error processing message: {e}"),
                                    session_id: Some(session_id),
                                };
                                let mgr = manager.read().await;
                                if let Err(send_err) = mgr.send_to(&channel_name, error_msg).await {
                                    warn!(
                                        error = %send_err,
                                        "Failed to send error notification to channel"
                                    );
                                }
                            }
                        }
                    }
                    ChannelEvent::Connected(name) => {
                        info!(channel = %name, "Channel connected");
                    }
                    ChannelEvent::Disconnected(name) => {
                        warn!(channel = %name, "Channel disconnected");
                    }
                }
            }

            info!("ChannelBridge event loop stopped");
        })
    }

    /// Send a message through a specific channel by name.
    pub async fn send_to_channel(
        &self,
        channel_name: &str,
        message: ChannelMessage,
    ) -> ArgentorResult<()> {
        let mgr = self.manager.read().await;
        mgr.send_to(channel_name, message).await
    }

    /// Broadcast a message to all registered channels.
    ///
    /// Returns a vector of errors from channels that failed to send.
    pub async fn broadcast(&self, message: ChannelMessage) -> Vec<ArgentorError> {
        let mgr = self.manager.read().await;
        mgr.broadcast(message).await
    }

    /// Return the number of channels registered in the underlying manager.
    pub async fn channel_count(&self) -> usize {
        let mgr = self.manager.read().await;
        mgr.channel_count()
    }

    /// Look up the session id for a given channel + sender combination,
    /// if one has already been established.
    pub async fn get_session(&self, channel_name: &str, sender_id: &str) -> Option<ChannelSession> {
        let map = self.sessions.read().await;
        let key = (channel_name.to_string(), sender_id.to_string());
        map.get(&key).map(|&session_id| ChannelSession {
            channel_name: channel_name.to_string(),
            sender_id: sender_id.to_string(),
            session_id,
        })
    }

    /// Return a snapshot of all active channel sessions.
    pub async fn active_sessions(&self) -> Vec<ChannelSession> {
        let map = self.sessions.read().await;
        map.iter()
            .map(|((channel_name, sender_id), &session_id)| ChannelSession {
                channel_name: channel_name.clone(),
                sender_id: sender_id.clone(),
                session_id,
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use argentor_channels::Channel;
    use argentor_core::ArgentorResult;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // ---- Mock Channel ----

    struct MockChannel {
        name: String,
        send_count: Arc<AtomicUsize>,
        fail_sends: bool,
    }

    impl MockChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                send_count: Arc::new(AtomicUsize::new(0)),
                fail_sends: false,
            }
        }

        fn failing(name: &str) -> Self {
            Self {
                name: name.to_string(),
                send_count: Arc::new(AtomicUsize::new(0)),
                fail_sends: true,
            }
        }

        fn count(&self) -> Arc<AtomicUsize> {
            Arc::clone(&self.send_count)
        }
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            &self.name
        }

        async fn send(&self, _message: ChannelMessage) -> ArgentorResult<()> {
            if self.fail_sends {
                return Err(ArgentorError::Channel("mock send failure".to_string()));
            }
            self.send_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // ---- Helpers ----

    fn test_message(channel: &str, sender: &str) -> ChannelMessage {
        ChannelMessage {
            channel_id: channel.to_string(),
            sender_id: sender.to_string(),
            content: "Hello from test".to_string(),
            session_id: None,
        }
    }

    /// Build a minimal ChannelBridge with the given mock channels.
    async fn build_bridge(channels: Vec<Box<dyn Channel>>) -> (ChannelBridge, Arc<MessageRouter>) {
        use crate::connection::ConnectionManager;
        use argentor_agent::{AgentRunner, LlmProvider, ModelConfig};
        use argentor_security::{AuditLog, PermissionSet};
        use argentor_session::FileSessionStore;
        use argentor_skills::SkillRegistry;

        let mut mgr = ChannelManager::new();
        for ch in channels {
            mgr.add_channel(ch);
        }

        let tmp = std::env::temp_dir().join(format!("argentor-bridge-test-{}", Uuid::new_v4()));
        let audit = Arc::new(AuditLog::new(tmp.join("audit")));
        let session_store: Arc<dyn argentor_session::SessionStore> = Arc::new(
            FileSessionStore::new(tmp.join("sessions"))
                .await
                .expect("failed to create session store"),
        );
        let config = ModelConfig {
            provider: LlmProvider::Claude,
            model_id: "test-model".to_string(),
            api_key: "test-key".to_string(),
            api_base_url: Some("http://127.0.0.1:1".to_string()),
            temperature: 0.0,
            max_tokens: 100,
            max_turns: 1,
            max_context_tokens: 200_000,
            fallback_models: vec![],
            retry_policy: None,
        };
        let skills = Arc::new(SkillRegistry::new());
        let permissions = PermissionSet::new();
        let agent = Arc::new(AgentRunner::new(config, skills, permissions, audit));

        let conn_mgr = ConnectionManager::new();
        let router = Arc::new(MessageRouter::new(agent, session_store, conn_mgr));

        let bridge = ChannelBridge::new(mgr, Arc::clone(&router));
        (bridge, router)
    }

    // ---- Tests ----

    #[tokio::test]
    async fn test_channel_count() {
        let mock1 = MockChannel::new("slack");
        let mock2 = MockChannel::new("discord");
        let (bridge, _) = build_bridge(vec![Box::new(mock1), Box::new(mock2)]).await;

        assert_eq!(bridge.channel_count().await, 2);
    }

    #[tokio::test]
    async fn test_send_to_channel() {
        let mock = MockChannel::new("slack");
        let count = mock.count();
        let (bridge, _) = build_bridge(vec![Box::new(mock)]).await;

        bridge
            .send_to_channel("slack", test_message("slack", "user1"))
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_send_to_unknown_channel() {
        let (bridge, _) = build_bridge(vec![]).await;

        let result = bridge
            .send_to_channel("nonexistent", test_message("nonexistent", "user1"))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_broadcast_sends_to_all() {
        let mock1 = MockChannel::new("slack");
        let c1 = mock1.count();
        let mock2 = MockChannel::new("discord");
        let c2 = mock2.count();
        let (bridge, _) = build_bridge(vec![Box::new(mock1), Box::new(mock2)]).await;

        let errors = bridge.broadcast(test_message("all", "system")).await;
        assert!(errors.is_empty());
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_broadcast_collects_errors() {
        let good = MockChannel::new("slack");
        let good_count = good.count();
        let bad = MockChannel::failing("discord");
        let (bridge, _) = build_bridge(vec![Box::new(good), Box::new(bad)]).await;

        let errors = bridge.broadcast(test_message("all", "system")).await;
        assert_eq!(errors.len(), 1);
        // The good channel should still have sent successfully
        assert_eq!(good_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_session_affinity() {
        let mock = MockChannel::new("slack");
        let (bridge, _) = build_bridge(vec![Box::new(mock)]).await;

        // No session yet
        assert!(bridge.get_session("slack", "user-A").await.is_none());

        // Simulate session creation by writing directly
        let session_id = Uuid::new_v4();
        {
            let mut map = bridge.sessions.write().await;
            map.insert(("slack".to_string(), "user-A".to_string()), session_id);
        }

        let cs = bridge.get_session("slack", "user-A").await.unwrap();
        assert_eq!(cs.channel_name, "slack");
        assert_eq!(cs.sender_id, "user-A");
        assert_eq!(cs.session_id, session_id);

        // Different sender has no session
        assert!(bridge.get_session("slack", "user-B").await.is_none());
    }

    #[tokio::test]
    async fn test_event_loop_creates_sessions() {
        let mock = MockChannel::new("telegram");
        let (bridge, _) = build_bridge(vec![Box::new(mock)]).await;
        let (tx, rx) = mpsc::channel::<ChannelEvent>(16);

        let handle = bridge.start_event_loop(rx);

        // Send a message event
        tx.send(ChannelEvent::MessageReceived(test_message(
            "telegram", "user-42",
        )))
        .await
        .unwrap();

        // Drop the sender to let the loop finish
        drop(tx);
        // Wait for the event loop to process and exit
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;

        // The event loop should have created a session for this sender
        let cs = bridge.get_session("telegram", "user-42").await;
        assert!(cs.is_some(), "session should have been created");
        assert_eq!(cs.unwrap().channel_name, "telegram");
    }
}
