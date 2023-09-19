use borsh::BorshDeserialize;
use solana_program::pubkey::Pubkey;

use crate::state::{Exchange, FundingAccountConfig, FundingAccountFixed};

#[derive(Debug, Default)]
pub struct FundingAccount {
    pub bump: u8,
    pub id: u16,
    pub exchange: Exchange,
    pub market_index: u16,
    pub authority: Pubkey,

    pub last_updated_ts: i64,
    pub config: FundingAccountConfig,
    /// Percentage with 6 decimals
    /// ex: 1000000 = 10.000000%
    pub funding_ema: Option<i64>,
    pub data_points: Vec<Option<i64>>,
}

#[derive(Debug)]
pub struct DeserializeError;

pub fn load_funding_account(account_data: &Vec<u8>) -> Result<FundingAccount, DeserializeError> {
    let fixed_bytes = &mut &account_data[..FundingAccountFixed::SIZE];
    let fixed = FundingAccountFixed::deserialize(fixed_bytes).map_err(|_| DeserializeError)?;

    let mut funding_account = FundingAccount {
        bump: fixed.bump,
        id: fixed.id,
        exchange: fixed.exchange,
        market_index: fixed.market_index,
        authority: fixed.authority,
        last_updated_ts: fixed.last_updated_ts,
        config: fixed.config,
        funding_ema: fixed.funding_ema,
        data_points: vec![],
    };

    let dynamic_bytes = &account_data[FundingAccountFixed::SIZE..];
    let data_points_count = fixed.config.data_points_count as usize;
    if dynamic_bytes.len() != data_points_count * FundingAccountFixed::DATA_POINT_SIZE {
        return Err(DeserializeError);
    }

    for i in 0..data_points_count {
        let start = i * FundingAccountFixed::DATA_POINT_SIZE;
        let end = start + FundingAccountFixed::DATA_POINT_SIZE;
        let bytes = &mut &dynamic_bytes[start..end];

        funding_account
            .data_points
            .push(Option::<i64>::deserialize(bytes).map_err(|_| DeserializeError)?);
    }

    Ok(funding_account)
}
