use std::{
    panic,
    panic::PanicInfo,
    path::Path,
    process, result,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use clap::Parser;
use env_logger::TimestampPrecision;
use jito_protos::{
    auth::auth_service_client::AuthServiceClient, bundle::BundleResult,
    searcher::searcher_service_client::SearcherServiceClient,
};
use jito_searcher_client::{
    client_interceptor::ClientInterceptor, cluster_data_impl::ClusterDataImpl, grpc_connect,
    utils::derive_tip_accounts, ClusterData, SearcherClient, SearcherClientError,
    SearcherClientResult,
};
use log::*;
use rand::{thread_rng, Rng};
use solana_client::{
    client_error::ClientError,
    nonblocking::{pubsub_client::PubsubClientError, rpc_client::RpcClient},
};
use solana_metrics::{datapoint_error, set_host_id};
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction::transfer,
    transaction::{Transaction, VersionedTransaction},
};
use thiserror::Error;
use tokio::{
    sync::mpsc::Receiver,
    time::{interval, sleep},
};
use tonic::{codegen::InterceptedService, transport::Channel, Status};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Address for auth service
    #[clap(long, env)]
    block_engine_addr: String,

    /// Accounts to backrun
    #[clap(long, env)]
    accounts_to_backrun: Vec<String>,

    /// Path to keypair file used to sign and pay for transactions
    #[clap(long, env)]
    payer_keypair: String,

    /// Path to keypair file used to authenticate with the backend
    #[clap(long, env)]
    auth_keypair: String,

    /// Pubsub URL.
    #[clap(long, env)]
    pubsub_url: String,

    /// RPC URL to get block hashes from
    #[clap(long, env)]
    rpc_url: String,

    /// Memo program message
    #[clap(long, env, default_value_t = String::from("jito backrun"))]
    message: String,

    /// Tip program public key
    #[clap(long, env)]
    tip_program_id: String,

    /// Subscribe and print bundle results.
    #[clap(long, env)]
    subscribe_bundle_results: bool,
}

#[derive(Debug, Error)]
enum BackrunError {
    #[error("TonicError {0}")]
    Tonic(#[from] tonic::transport::Error),
    #[error("GrpcError {0}")]
    Grpc(#[from] Status),
    #[error("RpcError {0}")]
    Rpc(#[from] ClientError),
    #[error("PubSubError {0}")]
    PubSub(#[from] PubsubClientError),
}

#[derive(Clone, Debug)]
struct BundledTransactions {
    mempool_txs: Vec<VersionedTransaction>,
    backrun_txs: Vec<VersionedTransaction>,
}

type Result<T> = result::Result<T, BackrunError>;

async fn send_bundles(
    searcher_client: &mut SearcherClient<
        ClusterDataImpl,
        InterceptedService<Channel, ClientInterceptor>,
    >,
    bundles: &[BundledTransactions],
) -> Result<Vec<result::Result<Uuid, SearcherClientError>>> {
    let mut futs = vec![];
    for b in bundles {
        let txs = b
            .mempool_txs
            .clone()
            .into_iter()
            .chain(b.backrun_txs.clone().into_iter())
            .collect::<Vec<_>>();

        let searcher_client = searcher_client.clone();

        let task = tokio::spawn(async move { searcher_client.send_bundle(txs, 3).await });
        futs.push(task);
    }

    let responses = futures::future::join_all(futs).await;
    let send_bundle_responses = responses.into_iter().map(|r| r.unwrap()).collect();
    Ok(send_bundle_responses)
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .format_timestamp(Some(TimestampPrecision::Micros))
        .init();
    let args: Args = Args::parse();

    let payer_keypair =
        Arc::new(read_keypair_file(Path::new(&args.payer_keypair)).expect("parse kp file"));
    let auth_keypair =
        Arc::new(read_keypair_file(Path::new(&args.auth_keypair)).expect("parse kp file"));

    set_host_id(auth_keypair.pubkey().to_string());

    let accounts_to_backrun: Vec<Pubkey> = args
        .accounts_to_backrun
        .iter()
        .map(|a| Pubkey::from_str(a).unwrap())
        .collect();
    let tip_program_pubkey = Pubkey::from_str(&args.tip_program_id).unwrap();

    tokio::spawn(async move {
        backrun_loop(
            auth_keypair,
            payer_keypair,
            args.block_engine_addr,
            args.rpc_url,
            args.pubsub_url,
            args.message,
            tip_program_pubkey,
            accounts_to_backrun,
            graceful_panic(None),
        )
        .await
        .unwrap()
    })
    .await
    .unwrap();
}

async fn get_searcher_client(
    auth_keypair: &Arc<Keypair>,
    exit: &Arc<AtomicBool>,
    block_engine_url: &str,
    rpc_pubsub_addr: &str,
) -> SearcherClientResult<(
    SearcherClient<ClusterDataImpl, InterceptedService<Channel, ClientInterceptor>>,
    ClusterDataImpl,
)> {
    let auth_channel = grpc_connect(block_engine_url).await?;
    let client_interceptor =
        ClientInterceptor::new(AuthServiceClient::new(auth_channel), auth_keypair).await?;

    let searcher_channel = grpc_connect(block_engine_url).await?;
    let searcher_service_client =
        SearcherServiceClient::with_interceptor(searcher_channel, client_interceptor);

    let cluster_data_impl = ClusterDataImpl::new(
        rpc_pubsub_addr.to_string(),
        searcher_service_client.clone(),
        exit.clone(),
    )
    .await;

    Ok((
        SearcherClient::new(
            cluster_data_impl.clone(),
            searcher_service_client,
            exit.clone(),
        ),
        cluster_data_impl,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn backrun_loop(
    auth_keypair: Arc<Keypair>,
    payer_keypair: Arc<Keypair>,
    block_engine_addr: String,
    rpc_addr: String,
    rpc_pubsub_addr: String,
    backrun_message: String,
    tip_program_pubkey: Pubkey,
    accounts_to_backrun: Vec<Pubkey>,
    exit: Arc<AtomicBool>,
) -> Result<()> {
    let mut conns_errs: usize = 0;
    let mut mempool_subscription_conns_errs: usize = 0;
    let mut bundle_results_subscription_conns_errs: usize = 0;
    let mut mempool_subscription_errs: usize = 0;
    let mut bundle_results_subscription_errs: usize = 0;

    let tip_accounts = derive_tip_accounts(&tip_program_pubkey);
    info!("tip accounts: {:?}", tip_accounts);

    let mut blockhash_tick = interval(Duration::from_secs(5));

    while !exit.load(Ordering::Relaxed) {
        sleep(Duration::from_millis(1000)).await;

        let rpc_client = Arc::new(RpcClient::new(rpc_addr.clone()));
        let mut blockhash = rpc_client
            .get_latest_blockhash_with_commitment(CommitmentConfig {
                commitment: CommitmentLevel::Confirmed,
            })
            .await?
            .0;

        match get_searcher_client(
            &auth_keypair,
            &exit,
            block_engine_addr.as_str(),
            rpc_pubsub_addr.as_str(),
        )
        .await
        {
            Ok((mut searcher_client, cluster_data)) => {
                let mempool_receiver = searcher_client
                    .subscribe_mempool_accounts(&accounts_to_backrun[..], 1_000)
                    .await;
                if let Err(e) = mempool_receiver {
                    mempool_subscription_conns_errs += 1;
                    datapoint_error!(
                        "subscribe_mempool_accounts_error",
                        ("errors", mempool_subscription_conns_errs, i64),
                        ("msg", e.to_string(), String)
                    );
                    continue;
                }

                let bundle_results_receiver = searcher_client.subscribe_bundle_results(1_000).await;
                if let Err(e) = bundle_results_receiver {
                    bundle_results_subscription_conns_errs += 1;
                    datapoint_error!(
                        "subscribe_bundle_results_error",
                        ("errors", bundle_results_subscription_conns_errs, i64),
                        ("msg", e.to_string(), String)
                    );
                    continue;
                }

                let mut mempool_receiver: Receiver<Vec<VersionedTransaction>> =
                    mempool_receiver.unwrap();
                let mut bundle_results_receiver: Receiver<BundleResult> =
                    bundle_results_receiver.unwrap();

                while !exit.load(Ordering::Relaxed) {
                    tokio::select! {
                        _ = blockhash_tick.tick() => {
                            blockhash = rpc_client
                            .get_latest_blockhash_with_commitment(CommitmentConfig {
                                commitment: CommitmentLevel::Confirmed,
                            })
                            .await?
                            .0;

                            if let Some((next_jito_validator, next_slot)) = cluster_data.next_jito_validator().await {
                                let current_slot = cluster_data.current_slot().await;

                                if next_slot >= current_slot {
                                    info!("validator {next_jito_validator} upcoming in {} slots", next_slot - current_slot);
                                } else {
                                    warn!("no upcoming jito validator");
                                }
                            } else {
                                warn!("no connected jito validators");
                            }

                        }
                        maybe_transactions = mempool_receiver.recv() => {
                            if maybe_transactions.is_none() {
                                datapoint_error!(
                                    "mempool_subscription_error",
                                    ("errors", mempool_subscription_errs, i64),
                                    ("msg", "channel closed", String)
                                );
                                mempool_subscription_errs += 1;
                                break;
                            }
                            let transactions = maybe_transactions.unwrap();
                            info!("received mempool {} transactions", transactions.len());

                            let bundles = build_backrun_bundles(transactions, &payer_keypair, &blockhash, &tip_accounts, &backrun_message);
                            if !bundles.is_empty() {
                                let results = send_bundles(&mut searcher_client, &bundles).await?;
                                let successful_sends = results.iter().filter(|res| res.is_ok()).collect::<Vec<_>>();
                                let failed_sends = results.iter().filter(|res| res.is_err()).collect::<Vec<_>>();
                                info!("successful_sends={}, failed_send={}", successful_sends.len(), failed_sends.len());
                            }
                        }
                        maybe_bundle_result = bundle_results_receiver.recv() => {
                            if maybe_bundle_result.is_none() {
                                datapoint_error!(
                                    "bundle_results_subscription_error",
                                    ("errors", bundle_results_subscription_errs, i64),
                                    ("msg", "channel closed", String)
                                );
                                bundle_results_subscription_errs += 1;
                                break;
                            }
                            let bundle_result = maybe_bundle_result.unwrap();
                            info!("received bundle_result: [bundle_id={:?}, result={:?}]", bundle_result.bundle_id, bundle_result.result);
                        }
                    }
                }
            }
            Err(e) => {
                conns_errs += 1;
                datapoint_error!(
                    "searcher_connection_error",
                    ("errors", conns_errs, i64),
                    ("msg", e.to_string(), String)
                );
            }
        }
    }

    Ok(())
}

fn build_backrun_bundles(
    transactions: Vec<VersionedTransaction>,
    keypair: &Keypair,
    blockhash: &Hash,
    tip_accounts: &[Pubkey],
    message: &str,
) -> Vec<BundledTransactions> {
    let mut rng = thread_rng();
    transactions
        .into_iter()
        .map(|mempool_tx| {
            let tip_account = tip_accounts[rng.gen_range(0..tip_accounts.len())];

            let backrun_tx = Transaction::new_signed_with_payer(
                &[
                    build_memo(
                        format!("{}: {:?}", message, mempool_tx.signatures[0]).as_bytes(),
                        &[],
                    ),
                    transfer(&keypair.pubkey(), &tip_account, 10_000),
                ],
                Some(&keypair.pubkey()),
                &[keypair],
                *blockhash,
            );
            BundledTransactions {
                mempool_txs: vec![mempool_tx],
                backrun_txs: vec![backrun_tx.into()],
            }
        })
        .collect()
}

/// Returns an exit boolean to let other threads gracefully shut down
pub fn graceful_panic(callback: Option<fn(&PanicInfo)>) -> Arc<AtomicBool> {
    let exit = Arc::new(AtomicBool::new(false));
    // Fail fast!
    let panic_hook = panic::take_hook();
    {
        let exit = exit.clone();
        panic::set_hook(Box::new(move |panic_info| {
            if let Some(f) = callback {
                f(panic_info);
            }
            exit.store(true, Ordering::Relaxed);
            error!("exiting process");
            // let other loops finish up
            std::thread::sleep(Duration::from_secs(5));
            // invoke the default handler and exit the process
            panic_hook(panic_info); // print the panic backtrace. default exit code is 101

            process::exit(1); // bail us out if thread blocks/refuses to join main thread
        }));
    }
    exit
}

mod spl_memo_3_0 {
    solana_sdk::declare_id!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
}

fn build_memo(memo: &[u8], signer_pubkeys: &[&Pubkey]) -> Instruction {
    Instruction {
        program_id: spl_memo_3_0::id(),
        accounts: signer_pubkeys
            .iter()
            .map(|&pubkey| AccountMeta::new_readonly(*pubkey, true))
            .collect(),
        data: memo.to_vec(),
    }
}
