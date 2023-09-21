use std::{
    cell::RefMut,
    cmp,
    io::{self, Write},
};

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::AccountInfo,
    msg,
    program_error::ProgramError,
    program_memory::{sol_memcpy, sol_memmove},
    pubkey::Pubkey,
};

use crate::error::{ErrorCode, FundingResult};

pub struct BpfWriter<T> {
    inner: T,
    pos: u64,
}

impl<T> BpfWriter<T> {
    pub fn new(inner: T) -> Self {
        Self { inner, pos: 0 }
    }
}

impl Write for BpfWriter<&mut [u8]> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.pos >= self.inner.len() as u64 {
            return Ok(0);
        }

        let amt = cmp::min(
            self.inner.len().saturating_sub(self.pos as usize),
            buf.len(),
        );

        sol_memcpy(&mut self.inner[(self.pos as usize)..], buf, amt);
        self.pos += amt as u64;
        Ok(amt)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.write(buf)? == buf.len() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write whole buffer",
            ))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Copy, Clone, BorshSerialize, BorshDeserialize, PartialEq, Debug)]
#[repr(C)]
pub enum Exchange {
    Drift,
    Mango,
}

impl Exchange {
    pub fn discriminator(&self) -> u8 {
        match self {
            Self::Drift => 0,
            Self::Mango => 1,
        }
    }
}

impl Default for Exchange {
    fn default() -> Self {
        Self::Drift
    }
}

#[derive(Copy, Clone, Default, BorshDeserialize, BorshSerialize, Debug)]
pub struct FundingAccountConfig {
    pub update_frequency_secs: u64,
    pub staleness_threshold_secs: u64,
    /// used in EMA computation
    /// (data_point - prev_ema) * 2 / (period + 1) + prev_ema
    pub period_length: u32,
    pub data_points_count: u16,
}

impl FundingAccountConfig {
    pub fn log(&self) {
        msg!("update_frequency_secs: {}", self.update_frequency_secs);
        msg!(
            "staleness_threshold_secs: {}",
            self.staleness_threshold_secs
        );
        msg!("period_length: {}", self.period_length);
        msg!("data_points_count: {}", self.data_points_count);
    }
}

#[derive(Copy, Clone, Default, BorshDeserialize, BorshSerialize)]
pub struct FundingAccountFixed {
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
}

impl FundingAccountFixed {
    pub const SIZE: usize = std::mem::size_of::<Self>();
    pub const DATA_POINT_SIZE: usize = std::mem::size_of::<Option<i64>>();
}

pub struct FundingAccountLoader<'a, 'info> {
    pub ai: &'a AccountInfo<'info>,
    pub fixed: FundingAccountFixed,
    pub dynamic: RefMut<'a, [u8]>,
}

impl<'a, 'info: 'a> FundingAccountLoader<'a, 'info> {
    pub const NAMESPACE: &'static [u8; 7] = b"funding";

    pub fn size(data_points_count: u16) -> usize {
        FundingAccountFixed::SIZE
            + FundingAccountFixed::DATA_POINT_SIZE * data_points_count as usize
    }

    pub fn pda(id: u16, market_index: u16, exchange: &Exchange) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                Self::NAMESPACE,
                id.to_le_bytes().as_ref(),
                market_index.to_le_bytes().as_ref(),
                exchange.discriminator().to_le_bytes().as_ref(),
            ],
            &crate::id(),
        )
    }

    fn get_start_and_end_index(i: usize) -> (usize, usize) {
        let start_index = i * FundingAccountFixed::DATA_POINT_SIZE;
        let end_index = start_index + FundingAccountFixed::DATA_POINT_SIZE;
        (start_index, end_index)
    }

    fn write_data_point(&mut self, data_point: Option<i64>, i: usize) -> FundingResult<()> {
        let (start_index, end_index) = Self::get_start_and_end_index(i);
        let dst = &mut self.dynamic[start_index..end_index];
        let mut writer = BpfWriter::new(dst);
        data_point
            .serialize(&mut writer)
            .map_err(|_| ProgramError::InvalidAccountData)?;

        Ok(())
    }

    fn load_data_point(&self, i: usize) -> Option<i64> {
        let (start_index, end_index) = Self::get_start_and_end_index(i);
        let bytes = &mut &self.dynamic[start_index..end_index];
        Option::<i64>::deserialize(bytes).unwrap()
    }

    pub fn update_ema(&mut self) {
        let mut ema = self.load_data_point(0).unwrap();
        let k = (self.fixed.config.period_length + 1) as i64;
        let n = self.fixed.config.data_points_count as usize;

        for i in 1..n {
            let data_point = self.load_data_point(i).unwrap();
            let diff = data_point - ema;
            ema = diff * 2 / k + ema;
        }

        self.fixed.funding_ema = Some(ema);
    }

    pub fn update_data_points(&mut self, new_data_point: i64) -> FundingResult<()> {
        let data_points_count = self.fixed.config.data_points_count as usize;

        for i in 0..data_points_count {
            let data_point = self.load_data_point(i);

            if data_point.is_none() {
                self.write_data_point(Some(new_data_point), i)?;
                return Ok(());
            }
        }

        unsafe {
            sol_memmove(
                &mut self.dynamic[0],
                &mut self.dynamic[FundingAccountFixed::DATA_POINT_SIZE],
                (data_points_count - 1) * FundingAccountFixed::DATA_POINT_SIZE,
            );
        }
        self.write_data_point(Some(new_data_point), data_points_count - 1)?;
        self.update_ema();

        Ok(())
    }

    pub fn reset_data_points_and_write_first(&mut self, data_point: i64) -> FundingResult<()> {
        self.fixed.funding_ema = None;
        self.write_data_point(Some(data_point), 0)?;

        for i in 1..self.fixed.config.data_points_count {
            self.write_data_point(None, i as usize)?;
        }

        Ok(())
    }

    pub fn load(account_info: &'a AccountInfo<'info>) -> FundingResult<Self> {
        let (fixed, dynamic) = RefMut::map_split(account_info.try_borrow_mut_data()?, |b| {
            b.split_at_mut(FundingAccountFixed::SIZE)
        });

        let fixed = FundingAccountFixed::deserialize(&mut &fixed[..])
            .map_err(|_| ProgramError::InvalidAccountData)?;

        Ok(Self {
            ai: account_info,
            fixed,
            dynamic,
        })
    }

    pub fn try_load(
        account_info: &'a AccountInfo<'info>,
        authority: &Pubkey,
    ) -> FundingResult<Self> {
        if !account_info.is_writable {
            Err(ErrorCode::AccountsNeedToBeWritable)?;
        }
        if account_info.owner != &crate::id() {
            Err(ErrorCode::InvalidAccount)?;
        }

        let data_len = account_info.data_len();
        if data_len < FundingAccountFixed::SIZE {
            Err(ProgramError::AccountDataTooSmall)?;
        }

        let loader = Self::load(account_info)?;
        let fixed = &loader.fixed;

        if loader.dynamic.len()
            != FundingAccountFixed::DATA_POINT_SIZE * fixed.config.data_points_count as usize
        {
            Err(ProgramError::InvalidAccountData)?;
        }

        let (address, bump) = Self::pda(fixed.id, fixed.market_index, &fixed.exchange);
        if account_info.key != &address || fixed.bump != bump {
            Err(ProgramError::InvalidAccountData)?;
        }
        if &fixed.authority != authority {
            Err(ErrorCode::MissingOrInvalidAuthority)?;
        }

        Ok(loader)
    }

    pub fn save(self) -> FundingResult<()> {
        drop(self.dynamic);

        let data = &mut self.ai.try_borrow_mut_data()?[..FundingAccountFixed::SIZE];
        let mut writer = BpfWriter::new(data);
        self.fixed
            .serialize(&mut writer)
            .map_err(|_| ProgramError::InvalidAccountData.into())
    }

    pub fn log(&self) {
        msg!("bump: {}", self.fixed.bump);
        msg!("id: {}", self.fixed.id);
        msg!("exchange: {:?}", self.fixed.exchange);
        msg!("market_index: {}", self.fixed.market_index);
        msg!("authority: {}", self.fixed.authority);
        msg!("last_updated_ts: {}", self.fixed.last_updated_ts);
        msg!("funding_ema: {:?}", self.fixed.funding_ema);

        self.fixed.config.log();
    }
}

#[cfg(test)]
pub mod tests {
    use std::cell::{RefCell, RefMut};

    use crate::state::{
        BpfWriter, FundingAccountConfig, FundingAccountFixed, FundingAccountLoader,
    };
    use borsh::BorshSerialize;
    use solana_program::{account_info::AccountInfo, pubkey::Pubkey};

    #[test]
    fn ema() {
        // 1
        // (2 - 1) * 2 / 3 + 1 = 1,66
        // (3 - 1,66) * 2 / 3 + 1,66 = 2,553
        // (4 - 2,553) * 2 / 3 + 2,553 = ...
        // ...
        let mut data_points_bytes = [0u8; 192];
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
            .iter()
            .enumerate()
            .for_each(|(i, x)| {
                let x = Some(x * 1000_000);
                let offset = i * 16;
                let dst = &mut data_points_bytes[offset..offset + 16];
                let mut writer = BpfWriter::new(dst);
                x.serialize(&mut writer).ok();
            });
        let dynamic = RefCell::new(data_points_bytes);

        let def_pk = Pubkey::default();
        let mut l = 0u64;
        let mut funding_account = FundingAccountLoader {
            ai: &AccountInfo::new(&def_pk, false, false, &mut l, &mut [], &def_pk, false, 0),
            fixed: FundingAccountFixed {
                config: FundingAccountConfig {
                    period_length: 5,
                    data_points_count: 12,
                    ..Default::default()
                },
                ..Default::default()
            },
            dynamic: RefMut::from(dynamic.borrow_mut()),
        };

        funding_account.update_ema();
        assert_eq!(funding_account.fixed.funding_ema, Some(10023121));
    }
}
