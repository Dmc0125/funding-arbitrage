use anchor_lang::prelude::Pubkey;
use bytemuck::cast_ref;
use iter::{BookSideIter, OrderTreeIter};

pub mod iter;

anchor_client_gen::generate!(
    idl_path = "idl.json",
    program_id = "4MangoMjqJ2firMokCjjGgoK8d4MXcrgL7XJaL3w6fVg",
    zero_copy(
        BookSide,
        AnyNode,
        OrderTreeNodes,
        OrderTreeRoot,
        InnerNode,
        LeafNode
    ),
    repr_c(
        PerpMarket,
        BookSide,
        OrderTreeNodes,
        AnyNode,
        OracleConfig,
        StablePriceModel
    )
);

impl Default for types::OracleConfig {
    fn default() -> Self {
        Self {
            conf_filter: Default::default(),
            max_staleness_slots: Default::default(),
            reserved: [0; 72],
        }
    }
}

impl Default for types::StablePriceModel {
    fn default() -> Self {
        Self {
            stable_price: Default::default(),
            last_update_timestamp: Default::default(),
            delay_prices: Default::default(),
            delay_accumulator_price: Default::default(),
            delay_accumulator_time: Default::default(),
            delay_interval_seconds: Default::default(),
            delay_growth_limit: Default::default(),
            stable_growth_limit: Default::default(),
            last_delay_interval_index: Default::default(),
            reset_on_nonzero_price: Default::default(),
            padding: [0; 6],
            reserved: [0; 48],
        }
    }
}

impl Default for accounts::PerpMarket {
    fn default() -> Self {
        Self {
            group: Default::default(),
            settle_token_index: Default::default(),
            perp_market_index: Default::default(),
            blocked_1: Default::default(),
            group_insurance_fund: Default::default(),
            bump: Default::default(),
            base_decimals: Default::default(),
            name: Default::default(),
            bids: Default::default(),
            asks: Default::default(),
            event_queue: Default::default(),
            oracle: Default::default(),
            oracle_config: Default::default(),
            stable_price_model: Default::default(),
            quote_lot_size: Default::default(),
            base_lot_size: Default::default(),
            maint_base_asset_weight: Default::default(),
            init_base_asset_weight: Default::default(),
            maint_base_liab_weight: Default::default(),
            init_base_liab_weight: Default::default(),
            open_interest: Default::default(),
            seq_num: Default::default(),
            registration_time: Default::default(),
            min_funding: Default::default(),
            max_funding: Default::default(),
            impact_quantity: Default::default(),
            long_funding: Default::default(),
            short_funding: Default::default(),
            funding_last_updated: Default::default(),
            base_liquidation_fee: Default::default(),
            maker_fee: Default::default(),
            taker_fee: Default::default(),
            fees_accrued: Default::default(),
            fees_settled: Default::default(),
            fee_penalty: Default::default(),
            settle_fee_flat: Default::default(),
            settle_fee_amount_threshold: Default::default(),
            settle_fee_fraction_low_health: Default::default(),
            settle_pnl_limit_factor: Default::default(),
            padding_3: Default::default(),
            settle_pnl_limit_window_size_ts: Default::default(),
            reduce_only: Default::default(),
            force_close: Default::default(),
            padding_4: Default::default(),
            maint_overall_asset_weight: Default::default(),
            init_overall_asset_weight: Default::default(),
            positive_pnl_liquidation_fee: Default::default(),
            fees_withdrawn: Default::default(),
            reserved: [0; 1880],
        }
    }
}

impl accounts::PerpMarket {
    // native price * bĺs / qls = lot
    pub fn mango_native_price_to_lot(&self, price: fixed::types::I80F48) -> Option<i64> {
        price
            .checked_mul(fixed::types::I80F48::from_num(self.base_lot_size))
            .map(|p| p / fixed::types::I80F48::from_num(self.quote_lot_size))
            .map(|p| p.to_num())
    }

    // lot * qls / bls = native
    pub fn lot_to_mango_native(&self, price: i64) -> Option<fixed::types::I80F48> {
        use fixed::types::I80F48;

        I80F48::from_num(price)
            .checked_mul(I80F48::from_num(self.quote_lot_size))
            .map(|p| p / I80F48::from_num(self.base_lot_size))
            .map(|p| p.to_num())
    }

    pub fn lot_to_ui_price(&self, price: i64) -> Option<fixed::types::I80F48> {
        use fixed::types::I80F48;

        let expo = self.base_decimals - oracle_math::QUOTE_DECIMALS;
        let expo_pow = oracle_math::power_of_ten(expo as i8);

        I80F48::from_num(price)
            .checked_mul(expo_pow)
            .map(|p| p.checked_mul(I80F48::from_num(self.quote_lot_size)))
            .flatten()
            .map(|p| p / I80F48::from_num(self.base_lot_size))
    }

    pub fn ui_price_to_lot(&self, price: fixed::types::I80F48) -> Option<i64> {
        use fixed::types::I80F48;

        let expo = self.base_decimals as i8 - oracle_math::QUOTE_DECIMALS as i8;
        let expo_pow = oracle_math::power_of_ten(expo);

        let u = price.checked_mul(I80F48::from_num(self.base_lot_size));
        let l = I80F48::from_num(self.quote_lot_size).checked_mul(expo_pow);

        match (u, l) {
            (Some(u), Some(l)) => Some((u / l).to_num()),
            _ => None,
        }
    }

    pub fn ui_price_to_native_price(&self, price: fixed::types::I80F48) -> Option<u64> {
        use fixed::types::I80F48;

        let decimals = I80F48::from_num(10_u64.pow(oracle_math::QUOTE_DECIMALS as u32));
        price.checked_mul(decimals).map(|p| p.to_num())
    }

    pub fn native_price_to_ui_price(&self, price: u64) -> fixed::types::I80F48 {
        use fixed::types::I80F48;

        let decimals = I80F48::from_num(10_u64.pow(oracle_math::QUOTE_DECIMALS as u32));
        I80F48::from_num(price) / decimals
    }

    pub fn base_amount_to_base_lots(&self, amount: u64) -> i64 {
        use fixed::types::I80F48;

        (I80F48::from_num(amount) / I80F48::from_num(self.base_lot_size)).to_num()
    }

    pub fn calculate_funding_rate(
        &self,
        bids: &accounts::BookSide,
        asks: &accounts::BookSide,
        oracle_price: fixed::types::I80F48,
        now_ts: u64,
    ) -> Result<i64, ()> {
        use fixed::types::I80F48;

        let oracle_price_lots = self.mango_native_price_to_lot(oracle_price).ok_or(())?;
        let bid = bids.impact_price(self.impact_quantity, now_ts, oracle_price_lots);
        let ask = asks.impact_price(self.impact_quantity, now_ts, oracle_price_lots);

        let min_funding = oracle_math::mango_i80_f48_into_fixed(self.min_funding);
        let max_funding = oracle_math::mango_i80_f48_into_fixed(self.max_funding);

        let funding_rate = match (bid, ask) {
            (Some(bid), Some(ask)) => {
                // calculate mid-market rate
                let mid_price = (bid + ask) / 2;
                let book_price = self.lot_to_mango_native(mid_price).ok_or(())?;

                let diff = book_price / oracle_price - I80F48::ONE;
                diff.clamp(min_funding, max_funding)
            }
            (Some(_bid), None) => max_funding,
            (None, Some(_ask)) => min_funding,
            (None, None) => I80F48::ZERO,
        };

        // 1e6 precision
        let fr = funding_rate * 100000000 * 365;
        Ok(fr.to_num())
    }
}

impl Default for accounts::MangoAccount {
    fn default() -> Self {
        Self {
            group: Pubkey::default(),
            owner: Pubkey::default(),
            name: [0; 32],
            delegate: Pubkey::default(),
            account_num: 0,
            being_liquidated: 0,
            in_health_region: 0,
            bump: 0,
            padding: [0; 1],
            net_deposits: 0,
            perp_spot_transfers: 0,
            health_region_begin_init_health: 0,
            frozen_until: 0,
            buyback_fees_accrued_current: 0,
            buyback_fees_accrued_previous: 0,
            buyback_fees_expiry_timestamp: 0,
            next_token_conditional_swap_id: 0,
            reserved: [0; 200],
            header_version: 0,
            padding_3: [0; 7],
            padding_4: 0,
            tokens: vec![],
            padding_5: 0,
            serum_3: vec![],
            padding_6: 0,
            perps: vec![],
            padding_7: 0,
            perp_open_orders: vec![],
        }
    }
}

impl Default for types::AnyNode {
    fn default() -> Self {
        Self {
            tag: 0,
            data: [0; 119],
        }
    }
}

impl types::LeafNode {
    pub fn price_data(&self) -> u64 {
        (self.key >> 64) as u64
    }

    pub fn is_expired(&self, now_ts: u64) -> bool {
        self.time_in_force > 0 && now_ts >= self.timestamp + self.time_in_force as u64
    }
}

impl types::Side {
    pub fn is_price_better(self: &types::Side, lhs: i64, rhs: i64) -> bool {
        match self {
            types::Side::Bid => lhs > rhs,
            types::Side::Ask => lhs < rhs,
        }
    }
}

pub enum NodeRef<'a> {
    Inner(&'a types::InnerNode),
    Leaf(&'a types::LeafNode),
}

impl types::AnyNode {
    pub fn case(&self) -> Option<NodeRef> {
        use types::NodeTag;

        match NodeTag::try_from(self.tag) {
            Ok(NodeTag::InnerNode) => Some(NodeRef::Inner(cast_ref(self))),
            Ok(NodeTag::LeafNode) => Some(NodeRef::Leaf(cast_ref(self))),
            _ => None,
        }
    }
}

impl TryFrom<u8> for types::NodeTag {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Uninitialized,
            1 => Self::InnerNode,
            2 => Self::LeafNode,
            3 => Self::FreeNode,
            4 => Self::LastFreeNode,
            _ => return Err(()),
        })
    }
}

impl Default for types::OrderTreeNodes {
    fn default() -> Self {
        Self {
            order_tree_type: 0,
            padding: [0; 3],
            bump_index: 0,
            free_list_len: 0,
            free_list_head: 0,
            reserved: [0; 512],
            nodes: [types::AnyNode::default(); 1024],
        }
    }
}

impl types::OrderTreeNodes {
    pub fn node(&self, handle: u32) -> Option<&types::AnyNode> {
        use types::NodeTag;

        let node = &self.nodes[handle as usize];
        let tag = NodeTag::try_from(node.tag);
        match tag {
            Ok(NodeTag::InnerNode) | Ok(NodeTag::LeafNode) => Some(node),
            _ => None,
        }
    }

    pub fn order_tree_type(&self) -> types::OrderTreeType {
        match self.order_tree_type {
            0 => types::OrderTreeType::Bids,
            1 => types::OrderTreeType::Asks,
            _ => unreachable!(),
        }
    }

    pub fn iter(&self, root: &types::OrderTreeRoot) -> OrderTreeIter {
        OrderTreeIter::new(self, root)
    }
}

impl Default for accounts::BookSide {
    fn default() -> Self {
        Self {
            roots: [types::OrderTreeRoot::default(); 2],
            reserved_roots: [types::OrderTreeRoot::default(); 4],
            reserved: [0; 256],
            nodes: types::OrderTreeNodes::default(),
        }
    }
}

impl accounts::BookSide {
    pub fn root(&self, component: types::BookSideOrderTree) -> &types::OrderTreeRoot {
        &self.roots[component as usize]
    }

    pub fn impact_price(&self, quantity: i64, now_ts: u64, oracle_price_lots: i64) -> Option<i64> {
        let mut sum = 0_i64;
        let valid_iter =
            BookSideIter::new(self, now_ts, oracle_price_lots).filter(|item| item.is_valid());
        for order in valid_iter {
            sum += order.node.quantity;
            if sum >= quantity {
                return Some(order.price_lots);
            }
        }
        None
    }
}

impl types::OrderTreeRoot {
    pub fn node(&self) -> Option<u32> {
        if self.leaf_count == 0 {
            None
        } else {
            Some(self.maybe_node)
        }
    }
}

pub mod oracle_math {
    use fixed::types::I80F48;

    pub const QUOTE_DECIMALS: u8 = 6;

    const DECIMAL_CONSTANT_ZERO_INDEX: i8 = 12;
    pub const DECIMAL_CONSTANTS: [I80F48; 25] = [
        I80F48::from_bits((1 << 48) / 10i128.pow(12u32)),
        I80F48::from_bits((1 << 48) / 10i128.pow(11u32) + 1),
        I80F48::from_bits((1 << 48) / 10i128.pow(10u32)),
        I80F48::from_bits((1 << 48) / 10i128.pow(9u32) + 1),
        I80F48::from_bits((1 << 48) / 10i128.pow(8u32) + 1),
        I80F48::from_bits((1 << 48) / 10i128.pow(7u32) + 1),
        I80F48::from_bits((1 << 48) / 10i128.pow(6u32) + 1),
        I80F48::from_bits((1 << 48) / 10i128.pow(5u32)),
        I80F48::from_bits((1 << 48) / 10i128.pow(4u32)),
        I80F48::from_bits((1 << 48) / 10i128.pow(3u32) + 1), // 0.001
        I80F48::from_bits((1 << 48) / 10i128.pow(2u32) + 1), // 0.01
        I80F48::from_bits((1 << 48) / 10i128.pow(1u32) + 1), // 0.1
        I80F48::from_bits((1 << 48) * 10i128.pow(0u32)),     // 1, index 12
        I80F48::from_bits((1 << 48) * 10i128.pow(1u32)),     // 10
        I80F48::from_bits((1 << 48) * 10i128.pow(2u32)),     // 100
        I80F48::from_bits((1 << 48) * 10i128.pow(3u32)),     // 1000
        I80F48::from_bits((1 << 48) * 10i128.pow(4u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(5u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(6u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(7u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(8u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(9u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(10u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(11u32)),
        I80F48::from_bits((1 << 48) * 10i128.pow(12u32)),
    ];
    pub const fn power_of_ten(decimals: i8) -> I80F48 {
        DECIMAL_CONSTANTS[(decimals + DECIMAL_CONSTANT_ZERO_INDEX) as usize]
    }

    pub type MangoI80F48 = crate::types::I80F48;

    pub fn mango_i80_f48_into_fixed(mango_i80f48: MangoI80F48) -> fixed::types::I80F48 {
        fixed::types::I80F48::from_le_bytes(mango_i80f48.val.to_le_bytes())
    }
}
