use num_derive::FromPrimitive;
use solana_program::{msg, program_error::ProgramError};
use thiserror::Error;

#[derive(FromPrimitive, Error, Debug)]
pub enum ErrorCode {
    #[error("Accounts need to be writable")]
    AccountsNeedToBeWritable,

    #[error("Invalid account")]
    InvalidAccount,

    #[error("Missing or invalid authority")]
    MissingOrInvalidAuthority,

    #[error("Could not serialize account")]
    CouldNotSerializeAccount,

    #[error("Market state already exists")]
    MarketStateAlreadyExists,

    #[error("Market state does not exist")]
    MarketStateDoesNotExist,

    #[error("Can not update more than once per interval")]
    UpdateTooSoon,

    #[error("Receiver account lamports overflow")]
    LamportsOverflow,
}

pub enum Error {
    FundingError(ErrorCode),
    ProgramError(ProgramError),
}

impl Error {
    pub fn print(&self) {
        match self {
            Self::FundingError(e) => {
                msg!("{}", e.to_string());
            }
            _ => {}
        }
    }
}

impl From<ErrorCode> for Error {
    fn from(value: ErrorCode) -> Self {
        Self::FundingError(value)
    }
}

impl From<ProgramError> for Error {
    fn from(value: ProgramError) -> Self {
        Self::ProgramError(value)
    }
}

impl From<Error> for ProgramError {
    fn from(value: Error) -> Self {
        value.print();

        match value {
            Error::FundingError(e) => ProgramError::Custom(e as u32),
            Error::ProgramError(e) => e,
        }
    }
}

pub type FundingResult<T> = Result<T, Error>;
