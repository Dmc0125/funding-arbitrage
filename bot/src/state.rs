use std::sync::Arc;

use anchor_lang::{AccountDeserialize, Discriminator};
use drift::accounts::PerpMarket as DriftPerpMarket;
use fixed::types::I80F48;
use mango::accounts::{BookSide, PerpMarket as MangoPerpMarket};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use tokio::{sync::RwLock, time::Instant};

use crate::{addresses::StaticAddresses, error::Error, utils::deser::AccountData};

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
}

pub async fn fetch_markets<T: AccountDeserialize + Discriminator>(
    rpc_client: &Arc<RpcClient>,
    markets: &Vec<Pubkey>,
) -> Result<Vec<(Pubkey, T)>, Error> {
    let ais = rpc_client.get_multiple_accounts(&markets).await?;
    let mut parsed = vec![];

    for (i, ai) in ais.iter().enumerate() {
        let address = &markets[i];
        if let Some(ai) = ai {
            let market = AccountData::from(ai).parse().map_err(|e| {
                println!("Unable to deserialize market account: {}", address);
                e
            })?;
            parsed.push((*address, market));
        } else {
            println!("Perp market account does not exist: {}", address);
            return Err(Error::UnableToFetchAccount);
        }
    }

    Ok(parsed)
}

pub struct State {
    rpc_client: Arc<RpcClient>,
    pub static_addresses: StaticAddresses,

    pub drift_markets: RwLock<Vec<(Pubkey, DriftPerpMarket)>>,
    pub mango_markets: RwLock<Vec<(Pubkey, MangoPerpMarket)>>,
    pub oracles: RwLock<Vec<(Pubkey, OraclePriceData)>>,
    pub book_sides: RwLock<Vec<(Pubkey, BookSide)>>,
}

impl State {
    pub fn new(rpc_client: Arc<RpcClient>, static_addresses: StaticAddresses) -> Self {
        Self {
            rpc_client,
            static_addresses,
            drift_markets: RwLock::new(vec![]),
            mango_markets: RwLock::new(vec![]),
            oracles: RwLock::new(vec![]),
            book_sides: RwLock::new(vec![]),
        }
    }

    pub async fn update_drift_markets(state: Arc<State>) -> Result<(), Error> {
        *state.drift_markets.write().await = fetch_markets::<DriftPerpMarket>(
            &state.rpc_client,
            &state.static_addresses.drift_markets,
        )
        .await?;

        Ok(())
    }

    pub async fn update_mango_markets(state: Arc<State>) -> Result<(), Error> {
        *state.mango_markets.write().await = fetch_markets::<MangoPerpMarket>(
            &state.rpc_client,
            &state.static_addresses.mango_markets,
        )
        .await?;
        Ok(())
    }

    pub async fn update_oracles(state: Arc<State>) -> Result<(), Error> {
        let oracles = &state.static_addresses.oracles;
        let ais = state.rpc_client.get_multiple_accounts(oracles).await?;
        let mut parsed = vec![];

        for (i, ai) in ais.iter().enumerate() {
            let address = &oracles[i];
            if let Some(ai) = ai {
                match pyth_sdk_solana::state::load_price_account(&ai.data[..]) {
                    Ok(price_account) => {
                        parsed.push((*address, OraclePriceData::from(price_account)));
                    }
                    Err(e) => {
                        println!(
                            "Unable to parse oracle account {}: {}",
                            address,
                            e.to_string()
                        )
                    }
                };
            } else {
                println!("Perp market account does not exist: {}", address);
                return Err(Error::UnableToFetchAccount);
            }
        }

        *state.oracles.write().await = parsed;
        Ok(())
    }

    pub async fn update_mango_book_sides(state: Arc<State>) -> Result<(), Error> {
        let book_sides_addresses = &state.static_addresses.mango_book_sides;
        let ais = state
            .rpc_client
            .get_multiple_accounts(
                &book_sides_addresses
                    .iter()
                    .map(|(_, addr, _)| *addr)
                    .collect::<Vec<Pubkey>>(),
            )
            .await?;

        let mut book_sides = vec![];

        for (i, ai) in ais.iter().enumerate() {
            let address = book_sides_addresses[i].1;
            if let Some(ai) = ai {
                book_sides.push((
                    address,
                    AccountData::from(ai).parse().map_err(|e| {
                        println!("Unable to deserialize mango book side account: {}", address);
                        e
                    })?,
                ))
            } else {
                println!("Mango book side does not exist: {}", address);
                return Err(Error::UnableToFetchAccount);
            }
        }

        *state.book_sides.write().await = book_sides;

        Ok(())
    }

    pub async fn update_for_funding_snapshot(state: &Arc<State>) -> Result<(), Error> {
        let (r1, r2, r3, r4) = tokio::join!(
            State::update_drift_markets(state.clone()),
            State::update_mango_markets(state.clone()),
            State::update_mango_book_sides(state.clone()),
            State::update_oracles(state.clone()),
        );

        r1?;
        r2?;
        r3?;
        r4?;

        Ok(())
    }

    pub async fn get_drift_market_and_oracle(
        &self,
        market_address: Pubkey,
    ) -> Option<(DriftPerpMarket, OraclePriceData)> {
        let drift_markets = self.drift_markets.read().await;
        let market = drift_markets
            .iter()
            .find(|(addr, _)| addr == &market_address);

        if let Some((_, market)) = market {
            let oracles = self.oracles.read().await;
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
        let mango_markets = self.mango_markets.read().await;
        let Some((_, market)) = mango_markets
            .iter()
            .find(|(addr, _)| addr == &market_address)
        else {
            return None;
        };

        let oracles = self.oracles.read().await;
        let Some((_, oracle)) = oracles.iter().find(|(addr, _)| addr == &market.oracle) else {
            return None;
        };

        let book_sides = self.book_sides.read().await;
        let bids = book_sides.iter().find(|(addr, _)| addr == &market.bids);
        let asks = book_sides.iter().find(|(addr, _)| addr == &market.asks);

        match (bids, asks) {
            (Some((_, bids)), Some((_, asks))) => Some((market.clone(), *bids, *asks, *oracle)),
            _ => None,
        }
    }
}
