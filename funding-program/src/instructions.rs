use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::pubkey::Pubkey;

use crate::state::Exchange;

#[derive(BorshSerialize, BorshDeserialize)]
pub enum InstructionData {
    InitializeFundingAccount {
        id: u16,
        exchange: Exchange,
        market_index: u16,
        update_frequency_secs: u64,
        staleness_threshold_secs: u64,
        period_length: u32,
        data_points_count: u16,
    },
    ConfigureFundingAccount {
        update_frequency_secs: Option<u64>,
        staleness_threshold_secs: Option<u64>,
        period_length: Option<u32>,
        data_points_count: Option<u16>,
    },
    ConfigureFundingAccountAuthority {
        authority: Pubkey,
    },
    UpdateFundingData {
        data_point: i64,
    },
    CloseFundingAccount,
}
