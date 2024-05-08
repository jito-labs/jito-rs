use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use jito_protos::searcher::{
    searcher_service_client::SearcherServiceClient, ConnectedLeadersRequest,
    ConnectedLeadersResponse,
};
use log::error;
use solana_sdk::pubkey::Pubkey;
use tokio::{
    sync::{
        mpsc::{channel, Receiver},
        Mutex,
    },
    time::{interval, timeout, Interval},
};
use tonic::codegen::{Body, Bytes, StdError};

use crate::{ClusterData, Slot};

/// Convenient types.
type JitoLeaderScheduleCache = Arc<Mutex<BTreeMap<Slot, LeaderScheduleCacheEntry>>>;

struct LeaderScheduleCacheEntry {
    validator_identity: Arc<Pubkey>,
}

/// Main object keeping track of cluster data.
#[derive(Clone)]
pub struct ClusterDataImpl {
    /// Tracks the current slot.
    current_slot: Arc<AtomicU64>,

    /// Keeps track of Jito-Solana slot schedules.
    jito_leader_schedule_cache: JitoLeaderScheduleCache,
}

impl ClusterDataImpl {
    /// Creates a new instance of this object; also spawns a few background tokio tasks.
    pub async fn new<T>(
        rpc_pubsub_addr: String,
        searcher_service_client: SearcherServiceClient<T>,
        exit: Arc<AtomicBool>,
    ) -> Self
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody> + Send + 'static,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
        <T as tonic::client::GrpcService<tonic::body::BoxBody>>::Future: std::marker::Send,
    {
        let current_slot = Arc::new(AtomicU64::new(0));
        let jito_leader_schedule_cache = Arc::new(Mutex::new(BTreeMap::new()));

        let (slot_sender, slot_receiver) = channel(1_000);
        tokio::spawn(rpc_utils::slot_subscribe_loop(
            rpc_pubsub_addr,
            slot_sender,
            exit.clone(),
        ));
        tokio::spawn(Self::current_slot_updater(
            current_slot.clone(),
            slot_receiver,
            exit.clone(),
        ));

        let fetch_interval = interval(Duration::from_secs(30));
        tokio::spawn(Self::jito_leader_schedule_cache_updater(
            jito_leader_schedule_cache.clone(),
            current_slot.clone(),
            searcher_service_client,
            fetch_interval,
            exit,
        ));

        Self {
            current_slot,
            jito_leader_schedule_cache,
        }
    }

    async fn current_slot_updater(
        current_slot: Arc<AtomicU64>,
        mut slot_receiver: Receiver<Slot>,
        exit: Arc<AtomicBool>,
    ) {
        let timeout_duration = Duration::from_secs(3);
        while !exit.load(Ordering::Relaxed) {
            match timeout(timeout_duration, slot_receiver.recv()).await {
                Err(_) => continue,
                Ok(Some(slot)) => {
                    current_slot.store(slot, Ordering::Relaxed);
                }
                Ok(None) => {
                    break;
                }
            }
        }
    }

    async fn jito_leader_schedule_cache_updater<T>(
        jito_leader_schedule_cache: JitoLeaderScheduleCache,
        current_slot: Arc<AtomicU64>,
        mut searcher_service_client: SearcherServiceClient<T>,
        mut fetch_interval: Interval,
        exit: Arc<AtomicBool>,
    ) where
        T: tonic::client::GrpcService<tonic::body::BoxBody> + Send,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
        <T as tonic::client::GrpcService<tonic::body::BoxBody>>::Future: std::marker::Send,
    {
        const MAX_RETRIES: usize = 5;
        while !exit.load(Ordering::Relaxed) {
            let _ = fetch_interval.tick().await;
            if let Some(connected_leaders_resp) = Self::fetch_connected_leaders_with_retries(
                &mut searcher_service_client,
                MAX_RETRIES,
            )
            .await
            {
                let mut leader_schedule = HashMap::with_capacity(
                    connected_leaders_resp.connected_validators.values().len(),
                );

                let current_slot = current_slot.load(Ordering::Relaxed);
                for (validator_identity, slots) in connected_leaders_resp.connected_validators {
                    if let Ok(validator_identity) = Pubkey::from_str(&validator_identity) {
                        let validator_identity = Arc::new(validator_identity);

                        slots
                            .slots
                            .iter()
                            .filter(|&&slot| slot >= current_slot)
                            .for_each(|&slot| {
                                leader_schedule.insert(
                                    slot,
                                    LeaderScheduleCacheEntry {
                                        validator_identity: validator_identity.clone(),
                                    },
                                );
                            });
                    } else {
                        error!("error parsing validator identity: {validator_identity}");
                    }
                }

                *jito_leader_schedule_cache.lock().await =
                    BTreeMap::from_iter(leader_schedule.into_iter());
            } else {
                exit.store(true, Ordering::Relaxed);
                return;
            }
        }
    }

    async fn fetch_connected_leaders_with_retries<T>(
        searcher_service_client: &mut SearcherServiceClient<T>,
        max_retries: usize,
    ) -> Option<ConnectedLeadersResponse>
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody> + Send,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
        <T as tonic::client::GrpcService<tonic::body::BoxBody>>::Future: std::marker::Send,
    {
        for _ in 0..max_retries {
            if let Ok(resp) = searcher_service_client
                .get_connected_leaders(ConnectedLeadersRequest {})
                .await
            {
                return Some(resp.into_inner());
            }
        }
        None
    }
}

#[tonic::async_trait]
impl ClusterData for ClusterDataImpl {
    async fn current_slot(&self) -> Slot {
        self.current_slot.load(Ordering::Relaxed) as Slot
    }

    async fn next_jito_validator(&self) -> Option<(Pubkey, Slot)> {
        let l_jito_leader_schedule_cache = self.jito_leader_schedule_cache.lock().await;
        let (slot, entry) = l_jito_leader_schedule_cache.first_key_value()?;
        Some((*entry.validator_identity, *slot))
    }
}

mod rpc_utils {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };

    use futures_util::StreamExt;
    use solana_client::{nonblocking::pubsub_client::PubsubClient, rpc_response::SlotUpdate};
    use solana_metrics::{datapoint_error, datapoint_info};
    use solana_sdk::clock::Slot;
    use tokio::{sync::mpsc::Sender, time::sleep};

    // Maintains a RPC subscription to keep track of the current slot.
    pub async fn slot_subscribe_loop(
        pubsub_addr: String,
        slot_sender: Sender<Slot>,
        exit: Arc<AtomicBool>,
    ) {
        let mut connect_errors: u64 = 0;
        let mut slot_subscribe_errors: u64 = 0;
        let mut slot_subscribe_disconnect_errors: u64 = 0;

        while !exit.load(Ordering::Relaxed) {
            sleep(Duration::from_secs(1)).await;

            match PubsubClient::new(&pubsub_addr).await {
                Ok(pubsub_client) => match pubsub_client.slot_updates_subscribe().await {
                    Ok((mut slot_update_subscription, _unsubscribe_fn)) => {
                        while let Some(slot_update) = slot_update_subscription.next().await {
                            if let SlotUpdate::FirstShredReceived { slot, timestamp: _ } =
                                slot_update
                            {
                                datapoint_info!("slot_subscribe_slot", ("slot", slot, i64));
                                if slot_sender.send(slot).await.is_err() {
                                    datapoint_error!(
                                        "slot_subscribe_send_error",
                                        ("errors", 1, i64)
                                    );
                                    exit.store(true, Ordering::Relaxed);
                                    return;
                                }
                            }

                            if exit.load(Ordering::Relaxed) {
                                return;
                            }
                        }
                        slot_subscribe_disconnect_errors += 1;
                        datapoint_error!(
                            "slot_subscribe_disconnect_error",
                            ("errors", slot_subscribe_disconnect_errors, i64)
                        );
                    }
                    Err(e) => {
                        slot_subscribe_errors += 1;
                        datapoint_error!(
                            "slot_subscribe_error",
                            ("errors", slot_subscribe_errors, i64),
                            ("error_str", e.to_string(), String),
                        );
                    }
                },
                Err(e) => {
                    connect_errors += 1;
                    datapoint_error!(
                        "slot_subscribe_pubsub_connect_error",
                        ("errors", connect_errors, i64),
                        ("error_str", e.to_string(), String)
                    );
                }
            }
        }
    }
}
