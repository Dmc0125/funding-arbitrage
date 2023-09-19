use anchor_lang::{AccountDeserialize, Discriminator};
use base64::{engine::general_purpose, Engine};
use solana_account_decoder::{UiAccount, UiAccountData, UiAccountEncoding};
use solana_sdk::account::Account;

use crate::error::Error;

pub fn decode_base64_data(encoded: &String) -> Option<Vec<u8>> {
    general_purpose::STANDARD.decode(encoded).ok()
}

pub enum AccountData<'a> {
    Serialized(&'a Vec<u8>),
    Encoded(&'a UiAccountData),
}

impl<'a> From<&'a Account> for AccountData<'a> {
    fn from(value: &'a Account) -> Self {
        Self::Serialized(&value.data)
    }
}

impl<'a> From<&'a UiAccount> for AccountData<'a> {
    fn from(value: &'a UiAccount) -> Self {
        Self::Encoded(&value.data)
    }
}

impl<'a> AccountData<'a> {
    pub fn decode(encoded_data: &UiAccountData) -> Result<Vec<u8>, Error> {
        let res = match encoded_data {
            UiAccountData::Binary(encoded_data, encoding) => match encoding {
                UiAccountEncoding::Base64 => decode_base64_data(encoded_data),
                _ => None,
            },
            _ => None,
        };
        res.ok_or(Error::UnableToDecode)
    }

    pub fn deserialize<T: AccountDeserialize + Discriminator>(data: &Vec<u8>) -> Result<T, Error> {
        T::try_deserialize(&mut &data[..]).map_err(|_| Error::UnableToDeserialize)
    }

    pub fn parse<T: AccountDeserialize + Discriminator>(&self) -> Result<T, Error> {
        match self {
            Self::Encoded(encoded) => {
                let bytes = Self::decode(encoded)?;
                Self::deserialize(&bytes)
            }
            Self::Serialized(bytes) => Self::deserialize(bytes),
        }
    }
}
