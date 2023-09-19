use std::borrow::BorrowMut;

use uint::construct_uint;

use crate::{DriftError, DriftResult};

construct_uint! {
    pub struct U192(3);
}

impl U192 {
    pub fn to_u64(self) -> DriftResult<u64> {
        self.try_into().map_err(|_| DriftError::MathError)
    }

    pub fn to_u128(self) -> DriftResult<u128> {
        self.try_into().map_err(|_| DriftError::MathError)
    }

    pub fn to_i128(self) -> DriftResult<i128> {
        let x = self.to_u128()?;
        x.try_into().map_err(|_| DriftError::MathError)
    }

    pub fn from_le_bytes(bytes: [u8; 24]) -> Self {
        U192::from_little_endian(&bytes)
    }

    pub fn to_le_bytes(self) -> [u8; 24] {
        let mut buf: Vec<u8> = Vec::with_capacity(std::mem::size_of::<Self>());
        self.to_little_endian(buf.borrow_mut());

        let mut bytes: [u8; 24] = [0u8; 24];
        bytes.copy_from_slice(buf.as_slice());
        bytes
    }
}
