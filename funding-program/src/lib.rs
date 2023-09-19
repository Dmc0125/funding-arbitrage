use solana_program::{
    account_info::AccountInfo, declare_id, entrypoint::ProgramResult, msg, pubkey::Pubkey,
};

#[cfg(any(test, feature = "cpi"))]
pub mod client;
pub mod error;
pub mod instructions;
pub mod processor;
pub mod state;
#[cfg(all(test, feature = "integration"))]
pub mod tests;

declare_id!("Fnd1yWeU4ajtCbzuDLsZq3cuoUiroJCYRoUi2y6PVZfy");

#[cfg(not(feature = "cpi"))]
solana_program::entrypoint!(process_instruction);

fn log_instruction(msg: &str) {
    msg!("Funding program: {}", msg);
}

pub fn process_instruction(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    use borsh::BorshDeserialize;
    use instructions::InstructionData;
    use solana_program::program_error::ProgramError;

    if instruction_data.len() < 1 {
        Err(ProgramError::InvalidInstructionData)?;
    }

    let ix_data = InstructionData::deserialize(&mut &instruction_data[..])
        .map_err(|_| ProgramError::InvalidInstructionData)?;

    match ix_data {
        InstructionData::InitializeFundingAccount {
            id,
            exchange,
            market_index,
            update_frequency_secs,
            staleness_threshold_secs,
            period_length,
            data_points_count,
        } => {
            log_instruction("InitializeFundingAccount");
            processor::initialize_funding_account(
                accounts,
                id,
                exchange,
                market_index,
                update_frequency_secs,
                staleness_threshold_secs,
                period_length,
                data_points_count,
            )?;
            Ok(())
        }
        InstructionData::ConfigureFundingAccount {
            update_frequency_secs,
            staleness_threshold_secs,
            period_length,
            data_points_count,
        } => {
            log_instruction("ConfigureFundingAccount");
            processor::configure_funding_account(
                accounts,
                update_frequency_secs,
                staleness_threshold_secs,
                period_length,
                data_points_count,
            )?;
            Ok(())
        }
        InstructionData::ConfigureFundingAccountAuthority { authority } => {
            log_instruction("ConfigureFundingAccountAuthority");
            processor::configure_funding_account_authority(accounts, authority)?;
            Ok(())
        }
        InstructionData::UpdateFundingData { data_point_5m } => {
            log_instruction("UpdateFundingAccount");
            processor::update_funding(accounts, data_point_5m)?;
            Ok(())
        }
        InstructionData::CloseFundingAccount => {
            log_instruction("CloseFundingAccount");
            processor::close_funding_account(accounts)?;
            Ok(())
        }
    }
}
