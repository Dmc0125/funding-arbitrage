use anchor_lang::{AccountDeserialize, Discriminator};
use drift::accounts::PerpMarket as DriftPerpMarket;
use fixed::types::I80F48;
use futures_util::StreamExt;
use mango::accounts::{BookSide, PerpMarket as MangoPerpMarket};
use solana_account_decoder::UiAccountEncoding;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::{
    config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
    filter::{Memcmp, RpcFilterType},
};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use std::{str::FromStr, sync::Arc, time::Instant};
use tokio::{
    sync::{mpsc, Mutex},
    task::JoinHandle,
};

use crate::{
    addresses::StaticAddresses,
    constants,
    error::Error,
    utils::{deser::AccountData, websocket_client::WebsocketClient},
};

#[derive(Clone, Copy, Debug)]
pub struct OraclePriceData {
    pub expo: i32,
    pub price: i64,
    pub updated_at_slot: u64,
    pub updated_at_ts: Instant,
    pub confidence: u64,
}

impl From<&pyth_sdk_solana::state::PriceAccount> for OraclePriceData {
    fn from(value: &pyth_sdk_solana::state::PriceAccount) -> Self {
        Self {
            expo: value.expo,
            price: value.agg.price,
            updated_at_slot: value.last_slot,
            confidence: value.agg.conf,
            updated_at_ts: Instant::now(),
        }
    }
}

impl OraclePriceData {
    pub const STALENESS_THRESHOLD: u64 = 40;

    pub fn get_drift_price(&self) -> Result<i64, Error> {
        use drift::constants::PRICE_PRECISION;

        let oracle_precision = 10_u128.pow(self.expo.unsigned_abs());

        let mut oracle_scale_mult = 1;
        let mut oracle_scale_div = 1;

        if oracle_precision > PRICE_PRECISION {
            oracle_scale_div = oracle_precision
                .checked_div(PRICE_PRECISION)
                .ok_or(Error::InvalidOraclePriceData)?;
        } else {
            oracle_scale_mult = PRICE_PRECISION
                .checked_div(oracle_precision)
                .ok_or(Error::InvalidOraclePriceData)?;
        }

        let oracle_price_scaled = (self.price as i128)
            .checked_mul(oracle_scale_mult as i128)
            .ok_or(Error::InvalidOraclePriceData)?
            .checked_div(oracle_scale_div as i128)
            .ok_or(Error::InvalidOraclePriceData)?;

        if oracle_price_scaled > (i64::MAX as i128) || oracle_price_scaled < (i64::MIN as i128) {
            Err(Error::InvalidOraclePriceData)
        } else {
            Ok(oracle_price_scaled as i64)
        }
    }

    pub fn get_mango_price(&self, base_decimals: u8) -> I80F48 {
        use mango::oracle_math::{power_of_ten, QUOTE_DECIMALS};

        let decimals = (self.expo as i8) + (QUOTE_DECIMALS as i8) - (base_decimals as i8);
        let decimal_adj = power_of_ten(decimals);

        I80F48::from_num(self.price) * decimal_adj
    }

    //     pub fn validate_staleness(&self, address: &Pubkey, slot: u64) -> Result<(), SharedError> {
    //         if self.updated_at_slot + Self::STALENESS_THRESHOLD < slot {
    //             let unix_timestamp = self
    //                 .updated_at_ts
    //                 .duration_since(SystemTime::UNIX_EPOCH)
    //                 .unwrap();
    //             let timestamp: DateTime<Utc> = Utc
    //                 .timestamp_opt(
    //                     unix_timestamp.as_secs() as i64,
    //                     unix_timestamp.subsec_nanos(),
    //                 )
    //                 .unwrap();
    //             let formatted = timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
    //             println!(
    //                 "Oracle {} is stale - Last update: {}",
    //                 address.to_string(),
    //                 formatted
    //             );
    //             return Err(SharedError::OracleIsStale);
    //         }
    //         Ok(())
    //     }
}

macro_rules! update_state_account {
    ($state_mutex: expr, $address: expr, $new_account: expr) => {
        let mut accounts = $state_mutex.lock().await;

        if let Some((_, state)) = accounts.iter_mut().find(|(addr, _)| addr == &$address) {
            *state = $new_account;
        } else {
            accounts.push(($address, $new_account));
        }
    };
}

pub enum StateUpdateMessage {
    Oracle(Pubkey, OraclePriceData),
    DriftMarket(Pubkey, DriftPerpMarket),
    MangoMarket(Pubkey, MangoPerpMarket),
    MangoBookSide(Pubkey, BookSide),
}

pub type StateUpdateSender = mpsc::UnboundedSender<StateUpdateMessage>;
pub type StateUpdateReceiver = mpsc::UnboundedReceiver<StateUpdateMessage>;

#[derive(Debug)]
pub struct State {
    pub drift_markets: Mutex<Vec<(Pubkey, DriftPerpMarket)>>,
    pub mango_markets: Mutex<Vec<(Pubkey, MangoPerpMarket)>>,
    pub mango_book_sides: Mutex<Vec<(Pubkey, BookSide)>>,
    pub oracles: Mutex<Vec<(Pubkey, OraclePriceData)>>,
}

impl State {
    pub fn new() -> (State, StateUpdateSender, StateUpdateReceiver) {
        let state = Self {
            drift_markets: Mutex::new(vec![]),
            mango_markets: Mutex::new(vec![]),
            mango_book_sides: Mutex::new(vec![]),
            oracles: Mutex::new(vec![]),
        };
        let (update_sender, update_receiver) = mpsc::unbounded_channel();

        (state, update_sender, update_receiver)
    }

    pub async fn set_initial_drift_markets(&self, markets: Vec<(Pubkey, DriftPerpMarket)>) {
        *self.drift_markets.lock().await = markets;
    }

    pub async fn set_initial_mango_markets(&self, markets: Vec<(Pubkey, MangoPerpMarket)>) {
        *self.mango_markets.lock().await = markets;
    }

    pub async fn set_initial_mango_book_sides(&self, book_sides: Vec<(Pubkey, BookSide)>) {
        *self.mango_book_sides.lock().await = book_sides;
    }

    pub async fn get_drift_market_and_oracle(
        &self,
        market_address: Pubkey,
    ) -> Option<(DriftPerpMarket, OraclePriceData)> {
        let drift_markets = self.drift_markets.lock().await;
        let market = drift_markets
            .iter()
            .find(|(addr, _)| addr == &market_address);

        if let Some((_, market)) = market {
            let oracles = self.oracles.lock().await;

            if let Some((_, oracle)) = oracles.iter().find(|(addr, _)| addr == &market.amm.oracle) {
                return Some((market.clone(), *oracle));
            }
        }

        None
    }

    pub async fn get_mango_market_with_components(
        &self,
        market_address: Pubkey,
    ) -> Option<(MangoPerpMarket, BookSide, BookSide, OraclePriceData)> {
        let mango_markets = self.mango_markets.lock().await;
        let Some((_, market)) = mango_markets
            .iter()
            .find(|(addr, _)| addr == &market_address)
        else {
            return None;
        };

        let oracles = self.oracles.lock().await;
        let Some((_, oracle)) = oracles.iter().find(|(addr, _)| addr == &market.oracle) else {
            return None;
        };

        let book_sides = self.mango_book_sides.lock().await;
        let bids = book_sides.iter().find(|(addr, _)| addr == &market.bids);
        let asks = book_sides.iter().find(|(addr, _)| addr == &market.asks);

        match (bids, asks) {
            (Some((_, bids)), Some((_, asks))) => Some((market.clone(), *bids, *asks, *oracle)),
            _ => None,
        }
    }

    pub fn subscribe_to_state_updates(
        state: Arc<State>,
        mut receiver: StateUpdateReceiver,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(update_message) = receiver.recv().await {
                match update_message {
                    StateUpdateMessage::Oracle(address, new_oracle) => {
                        // println!("Updating oracle");
                        update_state_account!(state.oracles, address, new_oracle);
                    }
                    StateUpdateMessage::DriftMarket(address, new_market) => {
                        // println!("Updating drift market");
                        update_state_account!(state.drift_markets, address, new_market);
                    }
                    StateUpdateMessage::MangoMarket(address, new_market) => {
                        // println!("Updating mango market");
                        update_state_account!(state.mango_markets, address, new_market);
                    }
                    StateUpdateMessage::MangoBookSide(address, new_book_side) => {
                        // println!("Updating book side");
                        update_state_account!(state.mango_book_sides, address, new_book_side);
                    }
                }
            }
        })
    }
}

pub async fn fetch_markets<T: AccountDeserialize + Discriminator>(
    rpc_client: &Arc<RpcClient>,
    markets: Vec<Pubkey>,
) -> Result<Vec<(Pubkey, T)>, Error> {
    let ais = rpc_client.get_multiple_accounts(&markets).await?;
    let mut parsed = vec![];

    for (i, ai) in ais.iter().enumerate() {
        let address = &markets[i];
        if let Some(ai) = ai {
            let market = AccountData::from(ai).parse()?;
            parsed.push((*address, market));
        } else {
            println!("Mango perp market does not exist: {}", address);
            return Err(Error::UnableToFetchAccount);
        }
    }

    Ok(parsed)
}

pub async fn fetch_mango_book_sides(
    rpc_client: &Arc<RpcClient>,
    static_addresses: &StaticAddresses,
) -> Result<Vec<(Pubkey, BookSide)>, Error> {
    let book_sides_addresses = &static_addresses.mango_book_sides;
    let ais = rpc_client
        .get_multiple_accounts(
            &book_sides_addresses
                .iter()
                .map(|(_, addr, _)| *addr)
                .collect::<Vec<Pubkey>>(),
        )
        .await?;

    let mut book_sides = vec![];

    for (i, ai) in ais.iter().enumerate() {
        let address = book_sides_addresses[i].0;
        if let Some(ai) = ai {
            book_sides.push((address, AccountData::from(ai).parse()?))
        } else {
            println!("Mango book side does not exist: {}", address);
            return Err(Error::UnableToFetchAccount);
        }
    }

    Ok(book_sides)
}

fn get_program_subscribe_config(discriminator: Vec<u8>) -> RpcProgramAccountsConfig {
    RpcProgramAccountsConfig {
        filters: Some(vec![RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
            0,
            discriminator,
        ))]),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            commitment: Some(CommitmentConfig::confirmed()),
            data_slice: None,
            min_context_slot: None,
        },
        with_context: None,
    }
}

pub type SubscriptionHandle = JoinHandle<Result<(), Error>>;

// TODO: handle staleness
pub fn subscribe_to_oracles(
    ws_client: Arc<WebsocketClient>,
    static_addresses: &StaticAddresses,
    state_update_sender: StateUpdateSender,
) -> SubscriptionHandle {
    let oracles_addresses = static_addresses.oracles.clone();
    let pyth_magic = pyth_sdk_solana::state::MAGIC.to_le_bytes().to_vec();
    let config = get_program_subscribe_config(pyth_magic.clone());

    tokio::spawn(async move {
        loop {
            println!("Subscribing to oracles");
            let (_, mut stream) = ws_client
                .program_subscribe(constants::pyth::id(), config.clone())
                .await?;

            while let Some(payload) = stream.next().await {
                let address = Pubkey::from_str(&payload.value.pubkey).unwrap();

                if !oracles_addresses.contains(&address) {
                    continue;
                }

                let err = match AccountData::decode(&payload.value.account.data) {
                    Ok(bytes) => match pyth_sdk_solana::state::load_price_account(&bytes[..]) {
                        Ok(price_account) => {
                            state_update_sender
                                .send(StateUpdateMessage::Oracle(
                                    address,
                                    OraclePriceData::from(price_account),
                                ))
                                .ok();
                            continue;
                        }
                        Err(e) => e.to_string(),
                    },
                    Err(e) => e.to_string(),
                };

                println!(
                    "Unable to parse oracle {} - error: {}",
                    address,
                    err.to_string()
                );
            }

            println!("Oracles sub closed");
        }
    })
}

pub fn subscribe_to_drift_markets(
    ws_client: Arc<WebsocketClient>,
    static_addresses: &StaticAddresses,
    state_update_sender: StateUpdateSender,
) -> SubscriptionHandle {
    let addresses = static_addresses.drift_markets.clone();
    let config = get_program_subscribe_config(DriftPerpMarket::discriminator().to_vec());

    tokio::spawn(async move {
        loop {
            println!("Subscribing to drift markets");
            let (_, mut stream) = ws_client
                .program_subscribe(drift::id(), config.clone())
                .await?;

            while let Some(payload) = stream.next().await {
                let address = Pubkey::from_str(&payload.value.pubkey).unwrap();

                if !addresses.contains(&address) {
                    continue;
                }

                let Ok(parsed) = AccountData::from(&payload.value.account).parse() else {
                    println!("Unable to parse drift market account {}", address);
                    continue;
                };

                state_update_sender
                    .send(StateUpdateMessage::DriftMarket(address, parsed))
                    .ok();
            }

            println!("Drift markets sub closed");
        }
    })
}

pub fn subscribe_to_mango_markets(
    ws_client: Arc<WebsocketClient>,
    static_addresses: &StaticAddresses,
    state_update_sender: StateUpdateSender,
) -> SubscriptionHandle {
    let addresses = static_addresses.mango_markets.clone();
    let config = get_program_subscribe_config(MangoPerpMarket::discriminator().to_vec());

    tokio::spawn(async move {
        loop {
            println!("Subscribing to mango markets");
            let (_, mut stream) = ws_client
                .program_subscribe(mango::id(), config.clone())
                .await?;

            while let Some(payload) = stream.next().await {
                let address = Pubkey::from_str(&payload.value.pubkey).unwrap();

                if !addresses.contains(&address) {
                    continue;
                }

                let Ok(parsed) = AccountData::from(&payload.value.account).parse() else {
                    println!("Unable to parse mango market account {}", address);
                    continue;
                };

                state_update_sender
                    .send(StateUpdateMessage::MangoMarket(address, parsed))
                    .ok();
            }

            println!("Mango sub closed");
        }
    })
}

pub fn subscribe_to_mango_book_sides(
    ws_client: Arc<WebsocketClient>,
    static_addresses: &StaticAddresses,
    state_update_sender: StateUpdateSender,
) -> SubscriptionHandle {
    let addresses = static_addresses
        .mango_book_sides
        .iter()
        .map(|(_, address, _)| *address)
        .collect::<Vec<Pubkey>>();
    let config = get_program_subscribe_config(BookSide::discriminator().to_vec());

    tokio::spawn(async move {
        loop {
            println!("Subscribing to mango booksides");
            let (_, mut stream) = ws_client
                .program_subscribe(mango::id(), config.clone())
                .await?;

            while let Some(payload) = stream.next().await {
                let address = Pubkey::from_str(&payload.value.pubkey).unwrap();

                if !addresses.contains(&address) {
                    continue;
                }

                let Ok(parsed) = AccountData::from(&payload.value.account).parse() else {
                    println!("Unable to parse mango book side {}", address);
                    continue;
                };

                state_update_sender
                    .send(StateUpdateMessage::MangoBookSide(address, parsed))
                    .ok();
            }

            println!("Mango booksides sub closed");
        }
    })
}
