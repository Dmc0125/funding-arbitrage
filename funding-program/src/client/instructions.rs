use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};

use crate::{instructions::InstructionData, state::Exchange};

pub struct InitializeFundingAccountAccounts {
    pub authority: Pubkey,
    pub funding_account: Pubkey,
}

pub fn initialize_funding_account(
    accounts: InitializeFundingAccountAccounts,
    id: u16,
    exchange: Exchange,
    market_index: u16,
    update_frequency_secs: u64,
    staleness_threshold_secs: u64,
    period_length: u32,
    data_points_count: u16,
) -> Instruction {
    let data = InstructionData::InitializeFundingAccount {
        id,
        exchange,
        market_index,
        update_frequency_secs,
        staleness_threshold_secs,
        period_length,
        data_points_count,
    };
    let accounts = vec![
        AccountMeta {
            pubkey: accounts.authority,
            is_signer: true,
            is_writable: false,
        },
        AccountMeta {
            pubkey: accounts.funding_account,
            is_signer: false,
            is_writable: true,
        },
        AccountMeta {
            pubkey: system_program::id(),
            is_signer: false,
            is_writable: false,
        },
    ];
    Instruction::new_with_borsh(crate::id(), &data, accounts)
}

pub struct ConfigureFundingAccountAccounts {
    pub authority: Pubkey,
    pub funding_account: Pubkey,
}

pub fn configure_funding_account(
    accounts: ConfigureFundingAccountAccounts,
    update_frequency_secs: Option<u64>,
    staleness_threshold_secs: Option<u64>,
    period_length: Option<u32>,
    data_points_count: Option<u16>,
) -> Instruction {
    let data = InstructionData::ConfigureFundingAccount {
        update_frequency_secs,
        staleness_threshold_secs,
        period_length,
        data_points_count,
    };
    let accounts = vec![
        AccountMeta {
            pubkey: accounts.authority,
            is_signer: true,
            is_writable: false,
        },
        AccountMeta {
            pubkey: accounts.funding_account,
            is_signer: false,
            is_writable: true,
        },
        AccountMeta {
            pubkey: system_program::id(),
            is_signer: false,
            is_writable: false,
        },
    ];
    Instruction::new_with_borsh(crate::id(), &data, accounts)
}

pub struct ConfigureFundingAccountAuthorityAccounts {
    pub authority: Pubkey,
    pub funding_account: Pubkey,
}

pub fn configure_funding_account_authority(
    accounts: ConfigureFundingAccountAuthorityAccounts,
    authority: Pubkey,
) -> Instruction {
    let data = InstructionData::ConfigureFundingAccountAuthority { authority };
    let accounts = vec![
        AccountMeta {
            pubkey: accounts.authority,
            is_signer: true,
            is_writable: false,
        },
        AccountMeta {
            pubkey: accounts.funding_account,
            is_signer: false,
            is_writable: true,
        },
    ];
    Instruction::new_with_borsh(crate::id(), &data, accounts)
}

pub struct UpdateFundingAccountAccounts {
    pub authority: Pubkey,
    pub funding_account: Pubkey,
}

pub fn update_funding_accounts(
    accounts: UpdateFundingAccountAccounts,
    data_point_5m: i64,
) -> Instruction {
    let data = InstructionData::UpdateFundingData { data_point_5m };
    let accounts = vec![
        AccountMeta {
            pubkey: accounts.authority,
            is_signer: true,
            is_writable: false,
        },
        AccountMeta {
            pubkey: accounts.funding_account,
            is_signer: false,
            is_writable: true,
        },
    ];
    Instruction::new_with_borsh(crate::id(), &data, accounts)
}

pub struct CloseFundingAccountAccounts {
    pub authority: Pubkey,
    pub funding_account: Pubkey,
    pub receiver: Pubkey,
}

pub fn close_funding_account(accounts: CloseFundingAccountAccounts) -> Instruction {
    let data = InstructionData::CloseFundingAccount;
    let accounts = vec![
        AccountMeta {
            pubkey: accounts.authority,
            is_signer: true,
            is_writable: true,
        },
        AccountMeta {
            pubkey: accounts.funding_account,
            is_signer: false,
            is_writable: true,
        },
        AccountMeta {
            pubkey: accounts.receiver,
            is_signer: false,
            is_writable: true,
        },
    ];
    Instruction::new_with_borsh(crate::id(), &data, accounts)
}
