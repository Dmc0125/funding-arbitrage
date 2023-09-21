use impl_helpers::{Cast, SafeMath};
use math::{
    calculate_funding_payment_in_quote_precision, calculate_new_oracle_price_twap,
    normalize_oracle_price, sanitize_new_price,
};

use crate::math::calculate_funding_rate_from_pnl_limit;

anchor_client_gen::generate!(
    idl_path = "idl.json",
    program_id = "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH",
    repr_c(
        State,
        PerpMarket,
        SpotMarket,
        Amm,
        PoolBalance,
        InsuranceClaim,
        User,
        PerpPosition,
        SpotPosition,
        Order
    )
);

pub mod bn;
pub mod impl_helpers;
pub mod math;

pub mod constants {
    pub const PERCENTAGE_PRECISION: u128 = 1_000_000;
    pub const PRICE_PRECISION: u128 = 1_000_000; //expo = -6;
    pub const PEG_PRECISION: u128 = 1_000_000; //expo = -6
    pub const PRICE_TO_PEG_PRECISION_RATIO: u128 = PRICE_PRECISION / PEG_PRECISION;
    pub const DEFAULT_MAX_TWAP_UPDATE_PRICE_BAND_DENOMINATOR: i64 = 3;
    pub const FIVE_MINUTE: i128 = (60 * 5) as i128;
    pub const ONE_HOUR_I128: i128 = 3600;
    pub const ONE_MINUTE: i128 = 60;
    pub const FUNDING_RATE_BUFFER: i128 = 1000;
    pub const FUNDING_RATE_BUFFER_U128: u128 = 1000;
    pub const AMM_RESERVE_PRECISION: u128 = 1_000_000_000;
    pub const AMM_TO_QUOTE_PRECISION_RATIO: u128 = AMM_RESERVE_PRECISION / PERCENTAGE_PRECISION;

    pub const FUNDING_RATE_PRECISION: u128 = PRICE_PRECISION * FUNDING_RATE_BUFFER_U128;
    pub const QUOTE_TO_BASE_AMT_FUNDING_PRECISION: i128 =
        (AMM_RESERVE_PRECISION * FUNDING_RATE_PRECISION / PERCENTAGE_PRECISION) as i128;
}

pub enum DriftError {
    MathError,
    InvalidOracle,
    InvalidMarkTwapUpdateDetected,
    InvalidFundingProfitability,
}

impl ToString for DriftError {
    fn to_string(&self) -> String {
        match self {
            Self::MathError => "MathError".to_string(),
            Self::InvalidOracle => "InvalidOracle".to_string(),
            Self::InvalidMarkTwapUpdateDetected => "InvalidMarkTwapUpdateDetected".to_string(),
            Self::InvalidFundingProfitability => "InvalidFundingProfitability".to_string(),
        }
    }
}

pub type DriftResult<T> = Result<T, DriftError>;

impl types::Amm {
    fn calculate_price(
        quote_asset_reserve: u128,
        base_asset_reserve: u128,
        peg_multiplier: u128,
    ) -> DriftResult<u64> {
        let peg_quote_asset_amount = quote_asset_reserve.safe_mul(peg_multiplier)?;

        bn::U192::from(peg_quote_asset_amount)
            .safe_mul(bn::U192::from(constants::PRICE_TO_PEG_PRECISION_RATIO))?
            .safe_div(bn::U192::from(base_asset_reserve))?
            .to_u64()
    }

    fn reserve_price(&self) -> DriftResult<u64> {
        Self::calculate_price(
            self.quote_asset_reserve,
            self.base_asset_reserve,
            self.peg_multiplier,
        )
    }

    pub fn bid_price(&self, reserve_price: u64) -> DriftResult<u64> {
        reserve_price
            .cast::<u128>()?
            .safe_mul(constants::PERCENTAGE_PRECISION.safe_sub(self.short_spread.cast()?)?)?
            .safe_div(constants::PERCENTAGE_PRECISION)?
            .cast()
    }

    pub fn ask_price(&self, reserve_price: u64) -> DriftResult<u64> {
        reserve_price
            .cast::<u128>()?
            .safe_mul(constants::PERCENTAGE_PRECISION.safe_add(self.long_spread.cast()?)?)?
            .safe_div(constants::PERCENTAGE_PRECISION)?
            .cast::<u64>()
    }

    pub fn calculate_oracle_price_twap(
        &self,
        reserve_price: u64,
        now: i64,
        oracle_price: i64,
        oracle_confidence: u64,
        sanitize_clamp_denominator: Option<i64>,
    ) -> DriftResult<i64> {
        let oracle_price = normalize_oracle_price(oracle_price, oracle_confidence, reserve_price)?;

        let capped_oracle_update_price = sanitize_new_price(
            oracle_price,
            self.historical_oracle_data.last_oracle_price_twap,
            sanitize_clamp_denominator,
        )?;

        if capped_oracle_update_price > 0 && oracle_price > 0 {
            calculate_new_oracle_price_twap(self, now, capped_oracle_update_price)
        } else {
            Ok(self.historical_oracle_data.last_oracle_price_twap)
        }
    }

    fn estimate_best_bid_ask_price(
        &self,
        amm_reserve_price: u64,
        precomputed_trade_price: Option<u64>,
        direction: Option<types::PositionDirection>,
    ) -> DriftResult<(u64, u64)> {
        use std::cmp::min;

        if self.historical_oracle_data.last_oracle_price <= 0 {
            return Err(DriftError::InvalidOracle);
        }

        let base_spread_u64 = self.base_spread.cast::<u64>()?;
        let last_oracle_price_u64 = self
            .historical_oracle_data
            .last_oracle_price
            .cast::<u64>()?;

        let trade_price: u64 = match precomputed_trade_price {
            Some(trade_price) => trade_price,
            None => last_oracle_price_u64,
        };

        let trade_premium: i64 = trade_price
            .cast::<i64>()?
            .safe_sub(self.historical_oracle_data.last_oracle_price)?;

        let amm_bid_price = self.bid_price(amm_reserve_price)?;
        let amm_ask_price = self.ask_price(amm_reserve_price)?;

        let best_bid_estimate = if trade_premium > 0 {
            let discount = min(base_spread_u64, self.short_spread.cast::<u64>()? / 2);
            last_oracle_price_u64.safe_sub(discount.min(trade_premium.unsigned_abs()))?
        } else {
            trade_price
        }
        .max(amm_bid_price);

        // trade is a short
        let best_ask_estimate = if trade_premium < 0 {
            let premium = min(base_spread_u64, self.long_spread.cast::<u64>()? / 2);
            last_oracle_price_u64.safe_add(premium.min(trade_premium.unsigned_abs()))?
        } else {
            trade_price
        }
        .min(amm_ask_price);

        let (bid_price, ask_price) = match direction {
            Some(direction) => match direction {
                types::PositionDirection::Long => {
                    (best_bid_estimate, trade_price.max(best_bid_estimate))
                }
                types::PositionDirection::Short => {
                    (trade_price.min(best_ask_estimate), best_ask_estimate)
                }
            },
            None => (
                trade_price.max(amm_bid_price).min(amm_ask_price),
                trade_price.max(amm_bid_price).min(amm_ask_price),
            ),
        };

        if bid_price > ask_price {
            return Err(DriftError::InvalidMarkTwapUpdateDetected);
        }

        Ok((bid_price, ask_price))
    }

    pub fn calculate_mark_twap(
        &self,
        now: i64,
        reserve_price: u64,
        precomputed_trade_price: u64,
        direction: Option<types::PositionDirection>,
        sanitize_clamp: Option<i64>,
    ) -> DriftResult<u64> {
        use std::cmp::max;

        let (bid_price, ask_price) = self.estimate_best_bid_ask_price(
            reserve_price,
            Some(precomputed_trade_price),
            direction,
        )?;

        let (bid_price_capped_update, ask_price_capped_update) = (
            sanitize_new_price(
                bid_price.cast()?,
                self.last_bid_price_twap.cast()?,
                sanitize_clamp,
            )?,
            sanitize_new_price(
                ask_price.cast()?,
                self.last_ask_price_twap.cast()?,
                sanitize_clamp,
            )?,
        );

        if bid_price_capped_update > ask_price_capped_update {
            return Err(DriftError::InvalidMarkTwapUpdateDetected);
        }

        let last_valid_trade_since_oracle_twap_update = self
            .historical_oracle_data
            .last_oracle_price_twap_ts
            .safe_sub(self.last_mark_price_twap_ts)?;

        let (last_bid_price_twap, last_ask_price_twap) =
            if last_valid_trade_since_oracle_twap_update
                > self
                    .funding_period
                    .safe_div(60)?
                    .max(constants::ONE_MINUTE.cast()?)
            {
                let from_start_valid = max(
                    0,
                    self.funding_period
                        .safe_sub(last_valid_trade_since_oracle_twap_update)?,
                );
                (
                    math::calculate_weighted_average(
                        self.historical_oracle_data
                            .last_oracle_price_twap
                            .cast::<i64>()?,
                        self.last_bid_price_twap.cast()?,
                        last_valid_trade_since_oracle_twap_update,
                        from_start_valid,
                    )?,
                    math::calculate_weighted_average(
                        self.historical_oracle_data
                            .last_oracle_price_twap
                            .cast::<i64>()?,
                        self.last_ask_price_twap.cast()?,
                        last_valid_trade_since_oracle_twap_update,
                        from_start_valid,
                    )?,
                )
            } else {
                (
                    self.last_bid_price_twap.cast()?,
                    self.last_ask_price_twap.cast()?,
                )
            };

        // update bid and ask twaps
        let bid_twap = math::calculate_new_twap(
            bid_price_capped_update,
            now,
            last_bid_price_twap,
            self.last_mark_price_twap_ts,
            self.funding_period,
        )?;
        let ask_twap = math::calculate_new_twap(
            ask_price_capped_update,
            now,
            last_ask_price_twap,
            self.last_mark_price_twap_ts,
            self.funding_period,
        )?;

        let mid_twap = bid_twap.safe_add(ask_twap)? / 2;

        mid_twap.cast()
    }
}

impl accounts::PerpMarket {
    fn get_sanitize_clamp_denominator(&self) -> Option<i64> {
        match self.contract_tier {
            types::ContractTier::A => Some(10_i64),   // 10%
            types::ContractTier::B => Some(5_i64),    // 20%
            types::ContractTier::C => Some(2_i64),    // 50%
            types::ContractTier::Speculative => None, // DEFAULT_MAX_TWAP_UPDATE_PRICE_BAND_DENOMINATOR
            types::ContractTier::Isolated => None, // DEFAULT_MAX_TWAP_UPDATE_PRICE_BAND_DENOMINATOR
        }
    }

    fn get_total_fee_lower_bound(&self) -> DriftResult<u128> {
        self.amm.total_exchange_fee.safe_div(2)
    }

    fn calculate_fee_pool(&self) -> DriftResult<u128> {
        let total_fee_minus_distributions_lower_bound = self.get_total_fee_lower_bound()?.cast()?;

        let fee_pool =
            if self.amm.total_fee_minus_distributions > total_fee_minus_distributions_lower_bound {
                self.amm
                    .total_fee_minus_distributions
                    .safe_sub(total_fee_minus_distributions_lower_bound)?
                    .cast()?
            } else {
                0
            };

        Ok(fee_pool)
    }

    fn calculate_capped_funding_rate(
        &self,
        uncapped_funding_pnl: i128, // if negative, users would net receive from protocol
        funding_rate: i128,
    ) -> DriftResult<(i128, i128)> {
        use std::cmp::max;

        // The funding_rate_pnl_limit is the amount of fees the protocol can use before it hits it's lower bound
        let fee_pool = self.calculate_fee_pool()?;

        // limit to 1/3 of current fee pool per funding period
        let funding_rate_pnl_limit = -fee_pool.cast::<i128>()?.safe_div(3)?;

        // if theres enough in fees, give user's uncapped funding
        // if theres a little/nothing in fees, give the user's capped outflow funding
        let capped_funding_pnl = max(uncapped_funding_pnl, funding_rate_pnl_limit);
        let capped_funding_rate = if uncapped_funding_pnl < funding_rate_pnl_limit {
            // Calculate how much funding payment is already available from users
            let funding_payment_from_users = calculate_funding_payment_in_quote_precision(
                funding_rate,
                if funding_rate > 0 {
                    self.amm.base_asset_amount_long
                } else {
                    self.amm.base_asset_amount_short
                },
            )?;

            // increase the funding_rate_pnl_limit by accounting for the funding payment already being made by users
            // this makes it so that the capped rate includes funding payments from users and protocol collected fees
            let funding_rate_pnl_limit =
                funding_rate_pnl_limit.safe_sub(funding_payment_from_users.abs())?;

            if funding_rate < 0 {
                // longs receive
                calculate_funding_rate_from_pnl_limit(
                    funding_rate_pnl_limit,
                    self.amm.base_asset_amount_long,
                )?
            } else {
                // shorts receive
                calculate_funding_rate_from_pnl_limit(
                    funding_rate_pnl_limit,
                    self.amm.base_asset_amount_short,
                )?
            }
        } else {
            funding_rate
        };

        Ok((capped_funding_rate, capped_funding_pnl))
    }

    fn calculate_funding_rate_long_short(
        &self,
        funding_rate: i128,
    ) -> DriftResult<(i128, i128, i128)> {
        // Calculate the funding payment owed by the net_market_position if funding is not capped
        // If the net market position owes funding payment, the protocol receives payment
        let settled_net_market_position = self
            .amm
            .base_asset_amount_with_amm
            .safe_add(self.amm.base_asset_amount_with_unsettled_lp)?;

        let net_market_position_funding_payment = calculate_funding_payment_in_quote_precision(
            funding_rate,
            settled_net_market_position,
        )?;
        let uncapped_funding_pnl = -net_market_position_funding_payment;

        // If the uncapped_funding_pnl is positive, the protocol receives money.
        if uncapped_funding_pnl >= 0 {
            return Ok((funding_rate, funding_rate, uncapped_funding_pnl));
        }

        let (capped_funding_rate, capped_funding_pnl) =
            self.calculate_capped_funding_rate(uncapped_funding_pnl, funding_rate)?;

        let new_total_fee_minus_distributions = self
            .amm
            .total_fee_minus_distributions
            .safe_add(capped_funding_pnl)?;

        // protocol is paying part of funding imbalance
        if capped_funding_pnl != 0 {
            let total_fee_minus_distributions_lower_bound =
                self.get_total_fee_lower_bound()?.cast::<i128>()?;

            // makes sure the protocol doesn't pay more than the share of fees allocated to `distributions`
            if new_total_fee_minus_distributions < total_fee_minus_distributions_lower_bound {
                return Err(DriftError::InvalidFundingProfitability);
            }
        }

        let funding_rate_long = if funding_rate < 0 {
            capped_funding_rate
        } else {
            funding_rate
        };

        let funding_rate_short = if funding_rate > 0 {
            capped_funding_rate
        } else {
            funding_rate
        };

        Ok((funding_rate_long, funding_rate_short, uncapped_funding_pnl))
    }

    pub fn calculate_funding_rate(
        &self,
        oracle_price: i64,
        oracle_confidence: u64,
        now_ts: i64,
    ) -> DriftResult<i64> {
        use std::cmp::{max, min};

        let reserve_price = self.amm.reserve_price()?;
        let sanitize_clamp_denominator = self.get_sanitize_clamp_denominator();

        let oracle_price_twap = self.amm.calculate_oracle_price_twap(
            reserve_price,
            now_ts,
            oracle_price,
            oracle_confidence,
            sanitize_clamp_denominator,
        )?;

        let (execution_premium_price, execution_premium_direction) =
            if self.amm.long_spread > self.amm.short_spread {
                (
                    self.amm.ask_price(reserve_price)?,
                    Some(types::PositionDirection::Long),
                )
            } else if self.amm.long_spread < self.amm.short_spread {
                (
                    self.amm.bid_price(reserve_price)?,
                    Some(types::PositionDirection::Short),
                )
            } else {
                (reserve_price, None)
            };

        let mid_price_twap = self.amm.calculate_mark_twap(
            now_ts,
            reserve_price,
            execution_premium_price,
            execution_premium_direction,
            sanitize_clamp_denominator,
        )?;

        let period_adjustment = (24_i128).safe_mul(constants::ONE_HOUR_I128)?.safe_div(max(
            constants::ONE_HOUR_I128,
            self.amm.funding_period as i128,
        ))?;
        // funding period = 1 hour, window = 1 day
        // low periodicity => quickly updating/settled funding rates => lower funding rate payment per interval
        let price_spread = mid_price_twap.cast::<i64>()?.safe_sub(oracle_price_twap)?;

        // clamp price divergence to 3% for funding rate calculation
        let max_price_spread = oracle_price_twap.safe_div(33)?; // 3%
        let clamped_price_spread = max(-max_price_spread, min(price_spread, max_price_spread));

        let funding_rate = clamped_price_spread
            .cast::<i128>()?
            .safe_mul(constants::FUNDING_RATE_BUFFER.cast()?)?
            .safe_div(period_adjustment.cast()?)?
            .cast::<i64>()?;

        let (funding_rate_long, funding_rate_short, _) =
            self.calculate_funding_rate_long_short(funding_rate.cast()?)?;

        let (funding_delta, funding_direction) =
            if mid_price_twap.cast::<i64>()? > oracle_price_twap {
                (funding_rate_short, types::PositionDirection::Short)
            } else {
                (funding_rate_long, types::PositionDirection::Long)
            };

        // 1e6 precision
        let funding_rate = funding_delta
            .safe_mul(1000)?
            .safe_div(oracle_price_twap.cast()?)?
            .unsigned_abs();

        let funding_apr: i64 = funding_rate
            .safe_mul(100)?
            .safe_mul(24)?
            .safe_mul(365)?
            .cast()?;

        Ok(match funding_direction {
            types::PositionDirection::Long => -funding_apr,
            types::PositionDirection::Short => funding_apr,
        })
    }
}

impl Default for types::PoolBalance {
    fn default() -> Self {
        Self {
            scaled_balance: 0,
            market_index: 0,
            padding: [0; 6],
        }
    }
}

impl Default for types::OracleSource {
    fn default() -> Self {
        Self::Pyth
    }
}

impl Default for types::Amm {
    fn default() -> Self {
        Self {
            oracle: Default::default(),
            historical_oracle_data: Default::default(),
            base_asset_amount_per_lp: Default::default(),
            quote_asset_amount_per_lp: Default::default(),
            fee_pool: Default::default(),
            base_asset_reserve: Default::default(),
            quote_asset_reserve: Default::default(),
            concentration_coef: Default::default(),
            min_base_asset_reserve: Default::default(),
            max_base_asset_reserve: Default::default(),
            sqrt_k: Default::default(),
            peg_multiplier: Default::default(),
            terminal_quote_asset_reserve: Default::default(),
            base_asset_amount_long: Default::default(),
            base_asset_amount_short: Default::default(),
            base_asset_amount_with_amm: Default::default(),
            base_asset_amount_with_unsettled_lp: Default::default(),
            max_open_interest: Default::default(),
            quote_asset_amount: Default::default(),
            quote_entry_amount_long: Default::default(),
            quote_entry_amount_short: Default::default(),
            quote_break_even_amount_long: Default::default(),
            quote_break_even_amount_short: Default::default(),
            user_lp_shares: Default::default(),
            last_funding_rate: Default::default(),
            last_funding_rate_long: Default::default(),
            last_funding_rate_short: Default::default(),
            last_2_4h_avg_funding_rate: Default::default(),
            total_fee: Default::default(),
            total_mm_fee: Default::default(),
            total_exchange_fee: Default::default(),
            total_fee_minus_distributions: Default::default(),
            total_fee_withdrawn: Default::default(),
            total_liquidation_fee: Default::default(),
            cumulative_funding_rate_long: Default::default(),
            cumulative_funding_rate_short: Default::default(),
            total_social_loss: Default::default(),
            ask_base_asset_reserve: Default::default(),
            ask_quote_asset_reserve: Default::default(),
            bid_base_asset_reserve: Default::default(),
            bid_quote_asset_reserve: Default::default(),
            last_oracle_normalised_price: Default::default(),
            last_oracle_reserve_price_spread_pct: Default::default(),
            last_bid_price_twap: Default::default(),
            last_ask_price_twap: Default::default(),
            last_mark_price_twap: Default::default(),
            last_mark_price_twap_5min: Default::default(),
            last_update_slot: Default::default(),
            last_oracle_conf_pct: Default::default(),
            net_revenue_since_last_funding: Default::default(),
            last_funding_rate_ts: Default::default(),
            funding_period: Default::default(),
            order_step_size: Default::default(),
            order_tick_size: Default::default(),
            min_order_size: Default::default(),
            max_position_size: Default::default(),
            volume_2_4h: Default::default(),
            long_intensity_volume: Default::default(),
            short_intensity_volume: Default::default(),
            last_trade_ts: Default::default(),
            mark_std: Default::default(),
            oracle_std: Default::default(),
            last_mark_price_twap_ts: Default::default(),
            base_spread: Default::default(),
            max_spread: Default::default(),
            long_spread: Default::default(),
            short_spread: Default::default(),
            long_intensity_count: Default::default(),
            short_intensity_count: Default::default(),
            max_fill_reserve_fraction: Default::default(),
            max_slippage_ratio: Default::default(),
            curve_update_intensity: Default::default(),
            amm_jit_intensity: Default::default(),
            oracle_source: Default::default(),
            last_oracle_valid: Default::default(),
            target_base_asset_amount_per_lp: Default::default(),
            padding_1: Default::default(),
            total_fee_earned_per_lp: Default::default(),
            padding: Default::default(),
        }
    }
}

impl Default for types::MarketStatus {
    fn default() -> Self {
        Self::Active
    }
}

impl Default for types::ContractType {
    fn default() -> Self {
        Self::Future
    }
}

impl Default for types::ContractTier {
    fn default() -> Self {
        Self::A
    }
}

impl Default for accounts::PerpMarket {
    fn default() -> Self {
        Self {
            pubkey: Default::default(),
            amm: Default::default(),
            pnl_pool: Default::default(),
            name: Default::default(),
            insurance_claim: Default::default(),
            unrealized_pnl_max_imbalance: Default::default(),
            expiry_ts: Default::default(),
            expiry_price: Default::default(),
            next_fill_record_id: Default::default(),
            next_funding_rate_record_id: Default::default(),
            next_curve_record_id: Default::default(),
            imf_factor: Default::default(),
            unrealized_pnl_imf_factor: Default::default(),
            liquidator_fee: Default::default(),
            if_liquidation_fee: Default::default(),
            margin_ratio_initial: Default::default(),
            margin_ratio_maintenance: Default::default(),
            unrealized_pnl_initial_asset_weight: Default::default(),
            unrealized_pnl_maintenance_asset_weight: Default::default(),
            number_of_users_with_base: Default::default(),
            number_of_users: Default::default(),
            market_index: Default::default(),
            status: Default::default(),
            contract_type: Default::default(),
            contract_tier: Default::default(),
            padding_1: Default::default(),
            quote_spot_market_index: Default::default(),
            padding: [0; 48],
        }
    }
}
