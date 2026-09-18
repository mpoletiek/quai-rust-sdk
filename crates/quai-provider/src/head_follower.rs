//! Reconnect only read subscriptions; replay numbered headers through a trusted provider.
use crate::{HeadTracker, HeadUpdate, Provider, ProviderError};
use quai_rpc::{
    Endpoint, RpcError, Transport, WsConfig, WsSubscription, WsSubscriptionKind, WsTransport,
};
use std::time::Duration;

/// Bounded reconnection and missed-notification polling policy.
#[derive(Clone, Copy, Debug)]
pub struct HeadFollowPolicy {
    /// Maximum connection/subscription attempts within one `next` call, 1..=8.
    pub max_connect_attempts: u8,
    /// Fixed delay between attempts; 1 millisecond through 60 seconds.
    pub retry_delay: Duration,
    /// Poll canonical heads after this quiet interval; 1 millisecond through 60 seconds.
    pub idle_poll_interval: Duration,
}
/// A new-head subscription paired with bounded canonical replay. Raw notifications
/// are wake-up hints only. No transaction submission or pending-event replay occurs.
pub struct WsHeadFollower<T> {
    provider: Provider<T>,
    tracker: HeadTracker,
    endpoint: Endpoint,
    config: WsConfig,
    policy: HeadFollowPolicy,
    subscription: Option<WsSubscription>,
    connection: Option<WsTransport>,
    needs_poll: bool,
}
impl<T: Transport> WsHeadFollower<T> {
    /// Bind explicit endpoints, tracker and resource policies without network I/O.
    pub fn new(
        provider: Provider<T>,
        tracker: HeadTracker,
        endpoint: Endpoint,
        config: WsConfig,
        policy: HeadFollowPolicy,
    ) -> Result<Self, ProviderError> {
        if !matches!(endpoint.scheme(), "ws" | "wss")
            || !(1..=8).contains(&policy.max_connect_attempts)
            || policy.retry_delay < Duration::from_millis(1)
            || policy.retry_delay > Duration::from_secs(60)
            || policy.idle_poll_interval < Duration::from_millis(1)
            || policy.idle_poll_interval > Duration::from_secs(60)
        {
            return Err(ProviderError::InvalidRequest("head follower policy"));
        }
        Ok(Self {
            provider,
            tracker,
            endpoint,
            config,
            policy,
            subscription: None,
            connection: None,
            needs_poll: true,
        })
    }
    /// Current memory cursor. Persist application changes before requesting another page.
    pub fn tracker(&self) -> &HeadTracker {
        &self.tracker
    }
    /// Wait for the next nonempty canonical update, reconnecting only the read
    /// subscription within the explicit attempt budget. Missing/pruned/deep-reorg
    /// history fails explicitly. Cancellation never submits a transaction.
    pub async fn next(&mut self) -> Result<HeadUpdate, ProviderError> {
        let mut attempts = 0;
        loop {
            if self.subscription.is_none() {
                if attempts >= self.policy.max_connect_attempts {
                    return Err(RpcError::Transport.into());
                }
                if attempts > 0 {
                    tokio::time::sleep(self.policy.retry_delay).await;
                }
                attempts += 1;
                let connection =
                    match WsTransport::connect(self.endpoint.clone(), self.config.clone()).await {
                        Ok(c) => c,
                        Err(error) => {
                            if attempts == self.policy.max_connect_attempts {
                                return Err(error.into());
                            }
                            continue;
                        }
                    };
                match connection.subscribe(WsSubscriptionKind::NewHeads).await {
                    Ok(subscription) => {
                        self.connection = Some(connection);
                        self.subscription = Some(subscription);
                        self.needs_poll = true;
                        // The budget counts consecutive failures to establish a read
                        // subscription, not disconnects survived. Without this reset a
                        // single call that reconnects successfully still exhausts it and
                        // reports Transport, attributing a remote disconnect to a local
                        // connect failure that never happened.
                        attempts = 0;
                    }
                    Err(error) => {
                        if attempts == self.policy.max_connect_attempts {
                            return Err(error.into());
                        }
                        continue;
                    }
                }
            }
            if self.needs_poll {
                let update = self.tracker.poll(&self.provider).await?;
                self.needs_poll = !update.caught_up;
                if !update.added.is_empty() || !update.removed.is_empty() {
                    return Ok(update);
                }
            }
            let subscription = self.subscription.as_mut().expect("connected subscription");
            tokio::select! {
                event=subscription.recv()=>match event {
                    Ok(Some(_))=>self.needs_poll=true,
                    Ok(None)|Err(_)=>{self.subscription=None;self.connection=None;self.needs_poll=true;}
                },
                _=tokio::time::sleep(self.policy.idle_poll_interval)=>self.needs_poll=true,
            }
        }
    }
}
