use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use funding_program::{
    client::{
        instructions::{
            initialize_funding_account, update_funding_account, InitializeFundingAccountAccounts,
            UpdateFundingAccountAccounts,
        },
        state::load_funding_account,
    },
    state::Exchange,
};
use futures_util::lock::Mutex;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use tokio::{task::JoinHandle, time::sleep};

use crate::{
    addresses::FundingAccountMeta,
    args::Wallet,
    error::Error,
    state::State,
    utils::transaction::{
        build_signed_transaction, force_send_transaction, send_and_confirm_transaction,
        TransactionResult,
    },
};

pub async fn initialize_funding_accounts_if_needed(
    rpc_client: &Arc<RpcClient>,
    wallet: &Arc<Wallet>,
    funding_accounts: &Vec<FundingAccountMeta>,
) -> Result<(), Error> {
    let ais = rpc_client
        .get_multiple_accounts(
            &funding_accounts
                .iter()
                .map(|meta| meta.address)
                .collect::<Vec<Pubkey>>(),
        )
        .await?;
    let mut uninitialized = vec![];

    for (i, ai) in ais.iter().enumerate() {
        let meta = &funding_accounts[i];

        match ai {
            None => uninitialized.push(meta),
            Some(ai) => {
                if ai.data.len() == 0 {
                    uninitialized.push(meta);
                }
            }
        }
    }

    if uninitialized.len() > 0 {
        for metas in uninitialized.chunks(10) {
            let ixs = metas
                .iter()
                .map(|meta| {
                    initialize_funding_account(
                        InitializeFundingAccountAccounts {
                            authority: wallet.pubkey,
                            funding_account: meta.address,
                        },
                        0,
                        meta.exchange,
                        meta.market_index,
                        120,
                        600,
                        5,
                        30,
                    )
                })
                .collect::<Vec<Instruction>>();

            force_send_transaction(rpc_client, wallet, ixs, &vec![]).await?;
        }
    }

    Ok(())
}

const SNAPSHOT_TIMEOUT_SECS: u64 = 30;
const RELAYER_SEND_FREQUENCY_SECS: u64 = 10;

struct MarketFundingCache {
    pub address: Pubkey,
    pub market: Pubkey,
    pub market_index: u16,
    pub exchange: Exchange,
    pub update_frequency_secs: u64,

    pub funding_snapshots: Vec<i64>,

    pub last_account_update_at: Instant,
}

impl MarketFundingCache {
    fn cache_funding_rates(&self) -> usize {
        (self.update_frequency_secs / SNAPSHOT_TIMEOUT_SECS) as usize
    }

    pub fn insert_funding_rate(&mut self, funding_rate: i64) {
        if self.funding_snapshots.len() == self.cache_funding_rates() {
            self.funding_snapshots.remove(0);
        }

        self.funding_snapshots.push(funding_rate);
    }

    pub fn get_average_funding_rate(&self) -> Option<i64> {
        let len = self.cache_funding_rates();
        if self.funding_snapshots.len() == len {
            let sum = self.funding_snapshots.iter().sum::<i64>();
            Some(sum / (len as i64))
        } else {
            None
        }
    }
}

pub async fn start_funding_relayer(
    rpc_client: Arc<RpcClient>,
    wallet: Arc<Wallet>,
    state: Arc<State>,
) -> Result<(JoinHandle<Result<(), Error>>, JoinHandle<Result<(), Error>>), Error> {
    sleep(Duration::from_secs(5)).await;
    let cache: Arc<Mutex<Vec<MarketFundingCache>>> = Default::default();

    {
        let funding_accounts_metas = &state.static_addresses.funding_accounts;
        let ais = rpc_client
            .get_multiple_accounts(
                &funding_accounts_metas
                    .iter()
                    .map(|m| m.address)
                    .collect::<Vec<Pubkey>>(),
            )
            .await?;
        let mut cache = cache.lock().await;

        for (i, ai) in ais.iter().enumerate() {
            let meta = &funding_accounts_metas[i];

            if let Some(ai) = ai {
                if let Ok(funding_account) = load_funding_account(&ai.data) {
                    cache.push(MarketFundingCache {
                        address: meta.address,
                        market: meta.market,
                        market_index: meta.market_index,
                        exchange: funding_account.exchange,
                        update_frequency_secs: funding_account.config.update_frequency_secs,
                        funding_snapshots: vec![],
                        last_account_update_at: Instant::now(),
                    });
                    continue;
                }
            }

            // Should be unreachable since accounts get initialized
            println!("Funding account does not exist: {}", meta.address);
            return Err(Error::UnableToFetchAccount);
        }
    }

    let cache_handle: JoinHandle<Result<(), Error>> = tokio::spawn({
        let cache = cache.clone();

        async move {
            loop {
                println!("Taking snapshot");

                State::update_for_funding_snapshot(&state).await?;

                let mut cache = cache.lock().await;

                for market_cache in cache.iter_mut() {
                    match market_cache.exchange {
                        Exchange::Drift => {
                            let Some((perp_market, oracle)) =
                                state.get_drift_market_and_oracle(market_cache.market).await
                            else {
                                println!(
                                    "Perp market for drift market {} does not exist",
                                    market_cache.market
                                );
                                continue;
                            };

                            let Ok(price) = oracle.get_drift_price() else {
                                println!(
                                    "Invalid oracle price data for drift market: {} - {}",
                                    perp_market.market_index, perp_market.amm.oracle
                                );
                                continue;
                            };

                            let funding_rate =
                                perp_market.calculate_funding_rate(price, oracle.confidence, 0);

                            match funding_rate {
                                Ok(fr) => {
                                    market_cache.insert_funding_rate(fr);
                                }
                                Err(e) => {
                                    println!(
                                        "Unable to calculate drift funding rate for market: {}, oracle: {:?} - error: {}",
                                        perp_market.market_index,
                                        oracle,
                                        e.to_string(),
                                    );
                                }
                            }
                        }
                        Exchange::Mango => {
                            let Some((perp_market, bids, asks, oracle)) = state
                                .get_mango_market_with_components(market_cache.market)
                                .await
                            else {
                                println!(
                                    "Perp market for mango market {} does not exist",
                                    market_cache.market
                                );
                                continue;
                            };

                            let price = oracle.get_mango_price(perp_market.base_decimals);
                            let now_ts = SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap()
                                .as_secs();
                            let funding_rate =
                                perp_market.calculate_funding_rate(&bids, &asks, price, now_ts);

                            match funding_rate {
                                Ok(fr) => {
                                    market_cache.insert_funding_rate(fr);
                                }
                                Err(_) => {
                                    println!(
                                        "Unable to calculate mango funding rate for market: {}, oracle: {:?}",
                                        perp_market.perp_market_index,
                                        oracle,
                                    );
                                }
                            }
                        }
                    }
                }

                drop(cache);
                sleep(Duration::from_secs(SNAPSHOT_TIMEOUT_SECS)).await;
            }
        }
    });

    let relayer_handle: JoinHandle<Result<(), Error>> = tokio::spawn({
        let cache = cache.clone();

        async move {
            loop {
                let cache_lock = cache.lock().await;
                let mut markets_with_instructions = vec![];

                for market_cache in cache_lock.iter() {
                    let exchange_str = match market_cache.exchange {
                        Exchange::Drift => "drift",
                        Exchange::Mango => "mango",
                    };

                    if market_cache.last_account_update_at.elapsed().as_secs()
                        < market_cache.update_frequency_secs
                    {
                        continue;
                    }

                    let Some(funding_rate) = market_cache.get_average_funding_rate() else {
                        continue;
                    };

                    println!(
                        "{} - {}: {}",
                        exchange_str, market_cache.market_index, funding_rate
                    );
                    markets_with_instructions.push((
                        market_cache.market,
                        update_funding_account(
                            UpdateFundingAccountAccounts {
                                authority: wallet.pubkey,
                                funding_account: market_cache.address,
                            },
                            funding_rate,
                        ),
                    ))
                }
                drop(cache_lock);

                if markets_with_instructions.len() > 0 {
                    for markets_with_instructions in markets_with_instructions.chunks(10) {
                        let ixs = markets_with_instructions
                            .iter()
                            .map(|(_, ix)| ix.clone())
                            .collect::<Vec<Instruction>>();
                        let tx = build_signed_transaction(&rpc_client, &wallet, &ixs[..], &vec![])
                            .await?;

                        let mut retries = 0;
                        loop {
                            if retries == 2 {
                                println!("Unable to update chunk");
                                break;
                            }

                            match send_and_confirm_transaction(&rpc_client, &tx).await? {
                                TransactionResult::Timeout(_) => {
                                    retries += 1;
                                    continue;
                                }
                                TransactionResult::Error(sig, e) => {
                                    println!(
                                        "TransactionError: {} Funding accounts update error: {}",
                                        sig, e
                                    );
                                    break;
                                }
                                TransactionResult::Success(sig, _) => {
                                    let mut cache = cache.lock().await;

                                    for (market, _) in markets_with_instructions.iter() {
                                        cache.iter_mut().find(|c| &c.market == market).map(
                                            |market_cache| {
                                                market_cache.last_account_update_at =
                                                    Instant::now();
                                            },
                                        );
                                    }

                                    println!("Successfully updated funding accounts {}", sig);
                                    break;
                                }
                            }
                        }
                    }
                }

                sleep(Duration::from_secs(RELAYER_SEND_FREQUENCY_SECS)).await;
            }
        }
    });

    Ok((cache_handle, relayer_handle))
}
