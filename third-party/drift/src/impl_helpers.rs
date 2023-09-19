use crate::{DriftError, DriftResult};

pub trait SafeMath: Sized {
    fn safe_add(self, rhs: Self) -> DriftResult<Self>;
    fn safe_sub(self, rhs: Self) -> DriftResult<Self>;
    fn safe_mul(self, rhs: Self) -> DriftResult<Self>;
    fn safe_div(self, rhs: Self) -> DriftResult<Self>;
}

macro_rules! checked_impl {
    ($t:ty) => {
        impl SafeMath for $t {
            #[inline(always)]
            fn safe_add(self, v: $t) -> DriftResult<$t> {
                match self.checked_add(v) {
                    Some(result) => Ok(result),
                    None => Err(DriftError::MathError),
                }
            }

            #[inline(always)]
            fn safe_sub(self, v: $t) -> DriftResult<$t> {
                match self.checked_sub(v) {
                    Some(result) => Ok(result),
                    None => Err(DriftError::MathError),
                }
            }

            #[inline(always)]
            fn safe_mul(self, v: $t) -> DriftResult<$t> {
                match self.checked_mul(v) {
                    Some(result) => Ok(result),
                    None => Err(DriftError::MathError),
                }
            }

            #[inline(always)]
            fn safe_div(self, v: $t) -> DriftResult<$t> {
                match self.checked_div(v) {
                    Some(result) => Ok(result),
                    None => Err(DriftError::MathError),
                }
            }
        }
    };
}

checked_impl!(crate::bn::U192);
checked_impl!(u128);
checked_impl!(u64);
checked_impl!(u32);
checked_impl!(u16);
checked_impl!(u8);
checked_impl!(i128);
checked_impl!(i64);
checked_impl!(i32);
checked_impl!(i16);
checked_impl!(i8);

pub trait Cast: Sized {
    #[inline(always)]
    fn cast<T: std::convert::TryFrom<Self>>(self) -> DriftResult<T> {
        match self.try_into() {
            Ok(result) => Ok(result),
            Err(_) => Err(DriftError::MathError),
        }
    }
}

impl Cast for crate::bn::U192 {}
impl Cast for u128 {}
impl Cast for u64 {}
impl Cast for u32 {}
impl Cast for u16 {}
impl Cast for u8 {}
impl Cast for i128 {}
impl Cast for i64 {}
impl Cast for i32 {}
impl Cast for i16 {}
impl Cast for i8 {}
impl Cast for bool {}
