use drift::accounts::PerpMarket as DriftPerpMarket;
use funding_program::state::Exchange;
use mango::{accounts::PerpMarket as MangoPerpMarket, types::Side};
use solana_sdk::pubkey::Pubkey;

use crate::constants;

pub struct StaticAddresses {
    pub drift_markets: Vec<Pubkey>,
    pub mango_markets: Vec<Pubkey>,
    /// (market address, book side, Side)
    pub mango_book_sides: Vec<(Pubkey, Pubkey, Side)>,
    pub oracles: Vec<Pubkey>,
    /// (market address, funding account)
    pub funding_accounts: Vec<(Pubkey, Pubkey)>,
}

impl StaticAddresses {
    pub fn new() -> Self {
        Self {
            drift_markets: vec![],
            mango_markets: vec![],
            mango_book_sides: vec![],
            oracles: vec![],
            funding_accounts: vec![],
        }
    }

    pub fn get_mango_markets_from_ids(markets_ids: Vec<u16>) -> Vec<Pubkey> {
        markets_ids
            .iter()
            .map(|id| {
                Pubkey::find_program_address(
                    &[
                        b"PerpMarket",
                        constants::mango::group::id().as_ref(),
                        id.to_le_bytes().as_ref(),
                    ],
                    &mango::id(),
                )
                .0
            })
            .collect()
    }

    pub fn get_drift_markets_from_ids(markets_ids: Vec<u16>) -> Vec<Pubkey> {
        markets_ids
            .iter()
            .map(|id| {
                Pubkey::find_program_address(
                    &[b"perp_market", id.to_le_bytes().as_ref()],
                    &drift::id(),
                )
                .0
            })
            .collect()
    }

    fn insert_unique_oracle(&mut self, oracle: Pubkey) {
        if !self.oracles.contains(&oracle) {
            self.oracles.push(oracle);
        }
    }

    pub fn set_mango_markets(&mut self, markets: &Vec<(Pubkey, MangoPerpMarket)>) {
        for (market_address, market) in markets.iter() {
            self.mango_markets.push(*market_address);
            self.insert_unique_oracle(market.oracle);

            self.mango_book_sides
                .push((*market_address, market.asks, Side::Ask));
            self.mango_book_sides
                .push((*market_address, market.bids, Side::Bid));

            let funding_account = funding_program::state::FundingAccountLoader::pda(
                0,
                market.perp_market_index,
                &Exchange::Mango,
            )
            .0;
            self.funding_accounts
                .push((*market_address, funding_account));
        }
    }

    pub fn set_drift_markets(&mut self, markets: &Vec<(Pubkey, DriftPerpMarket)>) {
        for (market_address, market) in markets.iter() {
            self.drift_markets.push(*market_address);
            self.insert_unique_oracle(market.amm.oracle);

            let funding_account = funding_program::state::FundingAccountLoader::pda(
                0,
                market.market_index,
                &Exchange::Drift,
            )
            .0;
            self.funding_accounts
                .push((*market_address, funding_account));
        }
    }
}
