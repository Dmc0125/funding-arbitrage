use std::ops::Sub;

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_memory::sol_memset,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction, system_program,
    sysvar::Sysvar,
};

use crate::{
    error::{ErrorCode, FundingResult},
    state::{BpfWriter, Exchange, FundingAccountConfig, FundingAccountLoader},
};

pub fn deserialize_account_data<T: BorshDeserialize>(data: &mut &[u8]) -> FundingResult<T> {
    T::deserialize(data).map_err(|_| ProgramError::InvalidAccountData.into())
}

pub fn serialize_account_data<T: BorshSerialize>(
    account: &AccountInfo,
    data: T,
) -> FundingResult<()> {
    let dst = &mut account.try_borrow_mut_data()?[..];

    if dst.len() != std::mem::size_of::<T>() {
        msg!("Serialization error: {}", std::any::type_name::<T>());
        Err(ErrorCode::CouldNotSerializeAccount)?;
    }

    let mut writer = BpfWriter::new(dst);
    data.serialize(&mut writer)
        .map_err(|_| ProgramError::InvalidAccountData.into())
}

fn load_signer_ai<'a, 'info>(
    account: &'a AccountInfo<'info>,
) -> FundingResult<&'a AccountInfo<'info>> {
    if !account.is_signer {
        Err(ErrorCode::MissingOrInvalidAuthority)?;
    }

    Ok(account)
}

pub fn initialize_funding_account(
    accounts: &[AccountInfo],
    id: u16,
    exchange: Exchange,
    market_index: u16,
    update_frequency_secs: u64,
    staleness_threshold_secs: u64,
    period_length: u32,
    data_points_count: u16,
) -> FundingResult<()> {
    let mut accounts_iter = accounts.iter();

    let signer_ai = load_signer_ai(next_account_info(&mut accounts_iter)?)?;
    let funding_ai = next_account_info(&mut accounts_iter)?;

    if !funding_ai.is_writable {
        Err(ErrorCode::AccountsNeedToBeWritable)?;
    }

    let (address, bump) = FundingAccountLoader::pda(id, market_index, &exchange);
    if funding_ai.key != &address {
        Err(ErrorCode::InvalidAccount)?;
    }

    if data_points_count <= 1 {
        Err(ProgramError::InvalidInstructionData)?;
    }

    if update_frequency_secs >= staleness_threshold_secs {
        Err(ProgramError::InvalidInstructionData)?;
    }

    if period_length == 0 {
        Err(ProgramError::InvalidInstructionData)?;
    }

    let rent = Rent::get()?;
    let size = FundingAccountLoader::size(data_points_count);
    let lamports = rent.minimum_balance(size);

    invoke_signed(
        &system_instruction::create_account(
            signer_ai.key,
            funding_ai.key,
            lamports,
            size as u64,
            &crate::id(),
        ),
        &[signer_ai.clone(), funding_ai.clone()],
        &[&[
            FundingAccountLoader::NAMESPACE,
            id.to_le_bytes().as_ref(),
            market_index.to_le_bytes().as_ref(),
            exchange.discriminator().to_le_bytes().as_ref(),
            &[bump],
        ]],
    )?;

    let mut funding_account = FundingAccountLoader::load(funding_ai)?;

    funding_account.fixed.bump = bump;
    funding_account.fixed.id = id;
    funding_account.fixed.authority = signer_ai.key.clone();
    funding_account.fixed.market_index = market_index;
    funding_account.fixed.exchange = exchange;
    funding_account.fixed.config = FundingAccountConfig {
        update_frequency_secs,
        staleness_threshold_secs,
        period_length,
        data_points_count,
    };

    funding_account.save()?;

    Ok(())
}

pub fn configure_funding_account<'a, 'info>(
    accounts: &'a [AccountInfo<'info>],
    update_frequency_secs: Option<u64>,
    staleness_threshold_secs: Option<u64>,
    period_length: Option<u32>,
    data_points_count: Option<u16>,
) -> FundingResult<()> {
    let mut accounts_iter = accounts.iter();

    let signer_ai = load_signer_ai(next_account_info(&mut accounts_iter)?)?;
    let funding_ai = next_account_info(&mut accounts_iter)?;
    let mut funding_account = FundingAccountLoader::try_load(funding_ai, signer_ai.key)?;
    let config = &mut funding_account.fixed.config;

    let new_update_freq = update_frequency_secs.unwrap_or(config.update_frequency_secs);
    let new_staleness_threshold =
        staleness_threshold_secs.unwrap_or(config.staleness_threshold_secs);

    if new_update_freq >= new_staleness_threshold {
        Err(ProgramError::InvalidInstructionData)?;
    }

    if period_length == Some(0) {
        Err(ProgramError::InvalidInstructionData)?;
    }

    let new_period_length = period_length.unwrap_or(config.period_length);

    match data_points_count {
        None => {
            config.update_frequency_secs = new_update_freq;
            config.staleness_threshold_secs = new_staleness_threshold;
            config.period_length = new_period_length;

            funding_account.save()?;

            Ok(())
        }
        Some(new_count) => {
            if new_count <= 1 {
                Err(ProgramError::InvalidInstructionData)?;
            }

            let prev_count = config.data_points_count;
            let new_size = FundingAccountLoader::size(new_count);

            let mut new_fixed = funding_account.fixed.clone();
            drop(funding_account);

            new_fixed.funding_ema = None;
            new_fixed.config = FundingAccountConfig {
                update_frequency_secs: new_update_freq,
                staleness_threshold_secs: new_staleness_threshold,
                period_length: new_period_length,
                data_points_count: new_count,
            };

            let zero_init = if new_count < prev_count {
                new_fixed.last_updated_ts = 0;

                let rent = Rent::get()?;
                let new_lamports = rent.minimum_balance(new_size);
                let funding_lamports = funding_ai.lamports();
                let remaining_lamports = funding_lamports - new_lamports;

                if remaining_lamports > 0 {
                    if !signer_ai.is_writable {
                        Err(ErrorCode::AccountsNeedToBeWritable)?;
                    }

                    let receiver_lamports = signer_ai.lamports();

                    **signer_ai.try_borrow_mut_lamports()? = receiver_lamports
                        .checked_add(remaining_lamports)
                        .ok_or(ErrorCode::LamportsOverflow)?;
                    **funding_ai.try_borrow_mut_lamports()? =
                        funding_lamports.sub(remaining_lamports);
                }

                true
            } else {
                let rent = Rent::get()?;
                let new_lamports = rent.minimum_balance(new_size);
                let additional_lamports = new_lamports - funding_ai.lamports();

                if additional_lamports > 0 {
                    if !signer_ai.is_writable {
                        Err(ErrorCode::AccountsNeedToBeWritable)?;
                    }

                    invoke(
                        &system_instruction::transfer(
                            signer_ai.key,
                            funding_ai.key,
                            additional_lamports,
                        ),
                        &[signer_ai.clone(), funding_ai.clone()],
                    )?;
                }

                false
            };

            funding_ai.realloc(new_size, false)?;

            let mut funding_account = FundingAccountLoader::load(funding_ai)?;
            funding_account.fixed = new_fixed;

            if zero_init {
                let dynamic_size = funding_account.dynamic.len();
                sol_memset(
                    &mut funding_account.dynamic[0..dynamic_size],
                    0,
                    dynamic_size,
                );
            }

            funding_account.save()?;

            Ok(())
        }
    }
}

pub fn configure_funding_account_authority<'a, 'info>(
    accounts: &'a [AccountInfo<'info>],
    authority: Pubkey,
) -> FundingResult<()> {
    let mut accounts_iter = accounts.iter();

    let signer_ai = load_signer_ai(next_account_info(&mut accounts_iter)?)?;
    let mut funding_account =
        FundingAccountLoader::try_load(next_account_info(&mut accounts_iter)?, signer_ai.key)?;

    funding_account.fixed.authority = authority;

    funding_account.save()?;
    Ok(())
}

pub fn update_funding<'a, 'info>(
    accounts: &'a [AccountInfo<'info>],
    data_point: i64,
) -> FundingResult<()> {
    let mut accounts_iter = accounts.iter();

    let signer_ai = load_signer_ai(next_account_info(&mut accounts_iter)?)?;
    let mut funding_account =
        FundingAccountLoader::try_load(next_account_info(&mut accounts_iter)?, signer_ai.key)?;

    let clock = Clock::get()?;
    let now_ts = clock.unix_timestamp;

    let stale_ts = funding_account.fixed.last_updated_ts
        + funding_account.fixed.config.staleness_threshold_secs as i64;

    if now_ts > stale_ts {
        funding_account.reset_data_points_and_write_first(data_point)?;
        funding_account.fixed.last_updated_ts = now_ts;

        funding_account.save()?;
        return Ok(());
    }

    let update_ts = funding_account.fixed.last_updated_ts
        + funding_account.fixed.config.update_frequency_secs as i64;
    if now_ts < update_ts {
        Err(ErrorCode::UpdateTooSoon)?;
    }

    funding_account.update_data_points(data_point)?;
    funding_account.fixed.last_updated_ts = now_ts;

    funding_account.save()?;
    Ok(())
}

pub fn close_funding_account<'a, 'info>(accounts: &'a [AccountInfo<'info>]) -> FundingResult<()> {
    let mut accounts_iter = accounts.iter();

    let signer_ai = load_signer_ai(next_account_info(&mut accounts_iter)?)?;
    let funding_ai = next_account_info(&mut accounts_iter)?;
    let _ = FundingAccountLoader::try_load(funding_ai, signer_ai.key)?;
    let receiver = next_account_info(&mut accounts_iter)?;

    if !receiver.is_writable {
        Err(ErrorCode::AccountsNeedToBeWritable)?;
    }
    if receiver.key == signer_ai.key && !signer_ai.is_writable {
        Err(ErrorCode::AccountsNeedToBeWritable)?;
    }

    let funding_lamports = funding_ai.lamports();
    let receiver_lamports = receiver.lamports();

    **receiver.try_borrow_mut_lamports()? = receiver_lamports
        .checked_add(funding_lamports)
        .ok_or(ErrorCode::LamportsOverflow)?;
    **funding_ai.try_borrow_mut_lamports()? = 0;

    funding_ai.realloc(0, false)?;
    funding_ai.assign(&system_program::id());

    Ok(())
}
