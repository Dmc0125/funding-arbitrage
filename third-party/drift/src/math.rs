use std::cmp::{max, min};

use crate::{
    constants::{self, AMM_TO_QUOTE_PRECISION_RATIO, FUNDING_RATE_BUFFER, PRICE_PRECISION},
    impl_helpers::{Cast, SafeMath},
    types::{Amm, Order, OrderType, PositionDirection},
    DriftError, DriftResult,
};

pub fn normalize_oracle_price(
    oracle_price: i64,
    oracle_conf: u64,
    reserve_price: u64,
) -> DriftResult<i64> {
    let reserve_price = reserve_price.cast::<i64>()?;
    // 2.5 bps of the mark price
    let reserve_price_2p5_bps = reserve_price.safe_div(4000)?;
    let conf_int = oracle_conf.cast::<i64>()?;

    //  normalises oracle toward mark price based on the oracle’s confidence interval
    //  if mark above oracle: use oracle+conf unless it exceeds .99975 * mark price
    //  if mark below oracle: use oracle-conf unless it less than 1.00025 * mark price
    //  (this guarantees more reasonable funding rates in volatile periods)
    let normalized_price = if reserve_price > oracle_price {
        min(
            max(reserve_price.safe_sub(reserve_price_2p5_bps)?, oracle_price),
            oracle_price.safe_add(conf_int)?,
        )
    } else {
        max(
            min(reserve_price.safe_add(reserve_price_2p5_bps)?, oracle_price),
            oracle_price.safe_sub(conf_int)?,
        )
    };

    Ok(normalized_price)
}

pub fn sanitize_new_price(
    new_price: i64,
    last_price_twap: i64,
    sanitize_clamp_denominator: Option<i64>,
) -> DriftResult<i64> {
    // when/if twap is 0, dont try to normalize new_price
    if last_price_twap == 0 {
        return Ok(new_price);
    }

    let new_price_spread = new_price.safe_sub(last_price_twap)?;

    // cap new oracle update to 100/MAX_TWAP_UPDATE_PRICE_BAND_DENOMINATOR% delta from twap
    let sanitize_clamp_denominator =
        if let Some(sanitize_clamp_denominator) = sanitize_clamp_denominator {
            sanitize_clamp_denominator
        } else {
            constants::DEFAULT_MAX_TWAP_UPDATE_PRICE_BAND_DENOMINATOR
        };

    if sanitize_clamp_denominator == 0 {
        // no need to use price band check
        return Ok(new_price);
    }

    let price_twap_price_band = last_price_twap.safe_div(sanitize_clamp_denominator)?;

    let capped_update_price =
        if new_price_spread.unsigned_abs() > price_twap_price_band.unsigned_abs() {
            if new_price > last_price_twap {
                last_price_twap.safe_add(price_twap_price_band)?
            } else {
                last_price_twap.safe_sub(price_twap_price_band)?
            }
        } else {
            new_price
        };

    Ok(capped_update_price)
}

pub fn calculate_weighted_average(
    data1: i64,
    data2: i64,
    weight1: i64,
    weight2: i64,
) -> DriftResult<i64> {
    let denominator = weight1.safe_add(weight2)?.cast::<i128>()?;
    let prev_twap_99 = data1.cast::<i128>()?.safe_mul(weight1.cast()?)?;
    let latest_price_01 = data2.cast::<i128>()?.safe_mul(weight2.cast()?)?;

    if weight1 == 0 {
        return Ok(data2);
    }

    if weight2 == 0 {
        return Ok(data1);
    }

    let bias: i64 = if weight2 > 1 {
        if latest_price_01 < prev_twap_99 {
            -1
        } else if latest_price_01 > prev_twap_99 {
            1
        } else {
            0
        }
    } else {
        0
    };

    let twap = prev_twap_99
        .safe_add(latest_price_01)?
        .safe_div(denominator)?
        .cast::<i64>()?;

    if twap == 0 && bias < 0 {
        return Ok(twap);
    }

    twap.safe_add(bias)
}

pub fn calculate_new_oracle_price_twap(amm: &Amm, now: i64, oracle_price: i64) -> DriftResult<i64> {
    let (last_mark_twap, last_oracle_twap) = (
        amm.last_mark_price_twap,
        amm.historical_oracle_data.last_oracle_price_twap,
    );
    let period: i64 = amm.funding_period;

    let since_last = max(
        if period == 0 { 1_i64 } else { 0_i64 },
        now.safe_sub(amm.historical_oracle_data.last_oracle_price_twap_ts)?,
    );
    let from_start = max(0_i64, period.safe_sub(since_last)?);

    // if an oracle delay impacted last oracle_twap, shrink toward mark_twap
    let interpolated_oracle_price =
        if amm.last_mark_price_twap_ts > amm.historical_oracle_data.last_oracle_price_twap_ts {
            let since_last_valid = amm
                .last_mark_price_twap_ts
                .safe_sub(amm.historical_oracle_data.last_oracle_price_twap_ts)?;

            let from_start_valid = max(1, period.safe_sub(since_last_valid)?);
            calculate_weighted_average(
                last_mark_twap.cast::<i64>()?,
                oracle_price,
                since_last_valid,
                from_start_valid,
            )?
        } else {
            oracle_price
        };

    calculate_weighted_average(
        interpolated_oracle_price,
        last_oracle_twap.cast()?,
        since_last,
        from_start,
    )
}

pub fn calculate_new_twap(
    current_price: i64,
    current_ts: i64,
    last_twap: i64,
    last_ts: i64,
    period: i64,
) -> DriftResult<i64> {
    let since_last = max(0_i64, current_ts.safe_sub(last_ts)?);
    let from_start = max(1_i64, period.safe_sub(since_last)?);

    calculate_weighted_average(current_price, last_twap, since_last, from_start)
}

fn _calculate_funding_payment(
    funding_rate_delta: i128,
    base_asset_amount: i128,
) -> DriftResult<i128> {
    let funding_rate_delta_sign: i128 = if funding_rate_delta > 0 { 1 } else { -1 };

    let funding_rate_payment_magnitude = crate::bn::U192::from(funding_rate_delta.unsigned_abs())
        .safe_mul(crate::bn::U192::from(base_asset_amount.unsigned_abs()))?
        .safe_div(crate::bn::U192::from(PRICE_PRECISION))?
        .safe_div(crate::bn::U192::from(FUNDING_RATE_BUFFER))?
        .to_i128()?;

    // funding_rate: longs pay shorts
    let funding_rate_payment_sign: i128 = if base_asset_amount > 0 { -1 } else { 1 };

    let funding_rate_payment = (funding_rate_payment_magnitude)
        .safe_mul(funding_rate_payment_sign)?
        .safe_mul(funding_rate_delta_sign)?;

    Ok(funding_rate_payment)
}

pub fn calculate_funding_payment_in_quote_precision(
    funding_rate_delta: i128,
    base_asset_amount: i128,
) -> DriftResult<i128> {
    let funding_payment = _calculate_funding_payment(funding_rate_delta, base_asset_amount)?;
    let funding_payment_collateral =
        funding_payment.safe_div(AMM_TO_QUOTE_PRECISION_RATIO.cast::<i128>()?)?;

    Ok(funding_payment_collateral)
}

pub fn calculate_funding_rate_from_pnl_limit(
    pnl_limit: i128,
    base_asset_amount: i128,
) -> DriftResult<i128> {
    if base_asset_amount == 0 {
        return Ok(0);
    }

    let pnl_limit_biased = if pnl_limit < 0 {
        pnl_limit.safe_add(1)?
    } else {
        pnl_limit
    };

    pnl_limit_biased
        .safe_mul(constants::QUOTE_TO_BASE_AMT_FUNDING_PRECISION)?
        .safe_div(base_asset_amount)
}

pub fn standardize_price(
    price: u64,
    tick_size: u64,
    direction: PositionDirection,
) -> DriftResult<u64> {
    if price == 0 {
        return Ok(0);
    }

    let remainder = price
        .checked_rem_euclid(tick_size)
        .ok_or(DriftError::MathError)?;

    if remainder == 0 {
        return Ok(price);
    }

    match direction {
        PositionDirection::Long => price.safe_sub(remainder),
        PositionDirection::Short => price.safe_add(tick_size)?.safe_sub(remainder),
    }
}

fn calculate_auction_price_for_fixed_auction(
    order: &Order,
    slot: u64,
    tick_size: u64,
) -> DriftResult<u64> {
    let slots_elapsed = slot.safe_sub(order.slot)?;

    let delta_numerator = min(slots_elapsed, order.auction_duration.cast()?);
    let delta_denominator = order.auction_duration;

    let auction_start_price = order.auction_start_price.cast::<u64>()?;
    let auction_end_price = order.auction_end_price.cast::<u64>()?;

    if delta_denominator == 0 {
        return standardize_price(auction_end_price, tick_size, order.direction);
    }

    let price_delta = match order.direction {
        PositionDirection::Long => auction_end_price
            .safe_sub(auction_start_price)?
            .safe_mul(delta_numerator.cast()?)?
            .safe_div(delta_denominator.cast()?)?,
        PositionDirection::Short => auction_start_price
            .safe_sub(auction_end_price)?
            .safe_mul(delta_numerator.cast()?)?
            .safe_div(delta_denominator.cast()?)?,
    };

    let price = match order.direction {
        PositionDirection::Long => auction_start_price.safe_add(price_delta)?,
        PositionDirection::Short => auction_start_price.safe_sub(price_delta)?,
    };

    standardize_price(price, tick_size, order.direction)
}

fn calculate_auction_price_for_oracle_offset_auction(
    order: &Order,
    slot: u64,
    tick_size: u64,
    oracle_price: i64,
) -> DriftResult<u64> {
    let slots_elapsed = slot.safe_sub(order.slot)?;

    let delta_numerator = min(slots_elapsed, order.auction_duration.cast()?);
    let delta_denominator = order.auction_duration;

    let auction_start_price_offset = order.auction_start_price;
    let auction_end_price_offset = order.auction_end_price;

    if delta_denominator == 0 {
        let price = oracle_price.safe_add(auction_end_price_offset)?;

        if price <= 0 {
            return Err(DriftError::InvalidOracleOffset);
        }

        return standardize_price(price.cast()?, tick_size, order.direction);
    }

    let price_offset_delta = match order.direction {
        PositionDirection::Long => auction_end_price_offset
            .safe_sub(auction_start_price_offset)?
            .safe_mul(delta_numerator.cast()?)?
            .safe_div(delta_denominator.cast()?)?,
        PositionDirection::Short => auction_start_price_offset
            .safe_sub(auction_end_price_offset)?
            .safe_mul(delta_numerator.cast()?)?
            .safe_div(delta_denominator.cast()?)?,
    };

    let price_offset = match order.direction {
        PositionDirection::Long => auction_start_price_offset.safe_add(price_offset_delta)?,
        PositionDirection::Short => auction_start_price_offset.safe_sub(price_offset_delta)?,
    };

    let price = standardize_price(
        oracle_price.safe_add(price_offset)?.max(0).cast()?,
        tick_size,
        order.direction,
    )?;

    if price == 0 {
        return Err(DriftError::InvalidOracleOffset);
    }

    Ok(price)
}

pub fn calculate_auction_price(
    order: &Order,
    slot: u64,
    tick_size: u64,
    oracle_price: i64,
) -> DriftResult<u64> {
    match order.order_type {
        OrderType::Market | OrderType::TriggerMarket | OrderType::Limit => {
            calculate_auction_price_for_fixed_auction(order, slot, tick_size)
        }
        OrderType::Oracle => {
            calculate_auction_price_for_oracle_offset_auction(order, slot, tick_size, oracle_price)
        }
        _ => unreachable!(),
    }
}
