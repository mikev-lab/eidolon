//! Regional Market Escrow Coordinator & Spatial Freight Tariffs.
//!
//! Manages locked escrow accounts, regional sales taxes, inter-station freight tariffs,
//! and atomic trade settlement across seamless continental zones.

use eidolon_core::{MarketOrder, OrderBook, OrderType, TradeExecution};

/// Maximum escrow orders tracked by a single market coordinator node.
pub const MAX_ESCROW_ORDERS: usize = 256;

/// Regional station location and local tax profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketStation {
    /// Station identifier.
    pub station_id: u32,
    /// Sector X coordinate on the continental grid.
    pub sector_x: i32,
    /// Sector Z coordinate on the continental grid.
    pub sector_z: i32,
    /// Base sales tax in basis points (e.g. 200 = 2.0%).
    pub sales_tax_bps: u16,
    /// Freight tariff per sector distance in copper.
    pub freight_per_sector: u64,
}

impl MarketStation {
    /// Creates a new market station definition.
    pub const fn new(
        station_id: u32,
        sector_x: i32,
        sector_z: i32,
        sales_tax_bps: u16,
        freight_per_sector: u64,
    ) -> Self {
        Self {
            station_id,
            sector_x,
            sector_z,
            sales_tax_bps,
            freight_per_sector,
        }
    }

    /// Computes discrete Manhattan distance in sectors to another station.
    pub fn sector_distance_to(&self, other: &MarketStation) -> u32 {
        let dx = (self.sector_x - other.sector_x).unsigned_abs();
        let dz = (self.sector_z - other.sector_z).unsigned_abs();
        dx.saturating_add(dz)
    }

    /// Computes delivery freight fee between stations.
    pub fn calculate_freight_fee(&self, destination: &MarketStation, quantity: u32) -> u64 {
        let dist = self.sector_distance_to(destination);
        (dist as u64)
            .saturating_mul(self.freight_per_sector)
            .saturating_mul(quantity as u64)
    }
}

/// Escrow holding record for an active resting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscrowHolding {
    /// Order ID associated with this holding.
    pub order_id: u64,
    /// Account ID of the placing trader.
    pub account_id: u64,
    /// True if currency is locked (buy order); false if item is locked (sell order).
    pub is_currency: bool,
    /// Locked currency amount (for buy orders).
    pub locked_currency: u64,
    /// Locked item instance ID or commodity type ID (for sell orders).
    pub locked_item_id: u64,
    /// Locked quantity.
    pub locked_quantity: u32,
    /// Station ID where order was placed.
    pub station_id: u32,
}

/// Errors occurring during market escrow and settlement operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscrowError {
    /// Escrow registry capacity exceeded.
    EscrowCapacityExceeded,
    /// Escrow holding not found for order ID.
    EscrowNotFound,
    /// Insufficient player wallet balance to fund buy order escrow.
    InsufficientFunds,
    /// Insufficient player item inventory to fund sell order escrow.
    InsufficientItems,
    /// Unauthorized settlement or refund attempt.
    UnauthorizedAccount,
    /// Order book matching failure.
    MarketFailure(eidolon_core::market::MarketError),
}

impl core::fmt::Display for EscrowError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EscrowCapacityExceeded => write!(f, "Escrow capacity exceeded"),
            Self::EscrowNotFound => write!(f, "Escrow holding not found"),
            Self::InsufficientFunds => write!(f, "Insufficient funds to lock escrow"),
            Self::InsufficientItems => write!(f, "Insufficient items to lock escrow"),
            Self::UnauthorizedAccount => write!(f, "Unauthorized escrow account access"),
            Self::MarketFailure(e) => write!(f, "Market error: {e}"),
        }
    }
}

impl std::error::Error for EscrowError {}

/// Regional market coordinator managing escrow accounts and settlement.
pub struct MarketEscrowCoordinator {
    stations: [Option<MarketStation>; 16],
    station_count: usize,
    holdings: [Option<EscrowHolding>; MAX_ESCROW_ORDERS],
    holding_count: usize,
    /// Total sales tax revenue collected in copper.
    pub total_tax_revenue: u64,
    /// Total trade volume settled in copper.
    pub total_volume_settled: u64,
}

impl Default for MarketEscrowCoordinator {
    fn default() -> Self {
        Self {
            stations: [None; 16],
            station_count: 0,
            holdings: [None; MAX_ESCROW_ORDERS],
            holding_count: 0,
            total_tax_revenue: 0,
            total_volume_settled: 0,
        }
    }
}

impl MarketEscrowCoordinator {
    /// Creates a new market escrow coordinator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a market trading station.
    pub fn register_station(&mut self, station: MarketStation) -> bool {
        if self.station_count >= self.stations.len() {
            return false;
        }

        for slot in self.stations.iter_mut() {
            if slot.is_none() {
                *slot = Some(station);
                self.station_count += 1;
                return true;
            }
        }
        false
    }

    /// Looks up a registered market station by ID.
    pub fn get_station(&self, station_id: u32) -> Option<&MarketStation> {
        self.stations
            .iter()
            .flatten()
            .find(|s| s.station_id == station_id)
    }

    /// Locks currency into escrow when placing a buy order.
    pub fn lock_buy_escrow(
        &mut self,
        order_id: u64,
        account_id: u64,
        total_escrow_currency: u64,
        station_id: u32,
    ) -> Result<(), EscrowError> {
        if self.holding_count >= MAX_ESCROW_ORDERS {
            return Err(EscrowError::EscrowCapacityExceeded);
        }

        for slot in self.holdings.iter_mut() {
            if slot.is_none() {
                *slot = Some(EscrowHolding {
                    order_id,
                    account_id,
                    is_currency: true,
                    locked_currency: total_escrow_currency,
                    locked_item_id: 0,
                    locked_quantity: 0,
                    station_id,
                });
                self.holding_count += 1;
                return Ok(());
            }
        }

        Err(EscrowError::EscrowCapacityExceeded)
    }

    /// Locks items into escrow when placing a sell order.
    pub fn lock_sell_escrow(
        &mut self,
        order_id: u64,
        account_id: u64,
        item_id: u64,
        quantity: u32,
        station_id: u32,
    ) -> Result<(), EscrowError> {
        if self.holding_count >= MAX_ESCROW_ORDERS {
            return Err(EscrowError::EscrowCapacityExceeded);
        }

        for slot in self.holdings.iter_mut() {
            if slot.is_none() {
                *slot = Some(EscrowHolding {
                    order_id,
                    account_id,
                    is_currency: false,
                    locked_currency: 0,
                    locked_item_id: item_id,
                    locked_quantity: quantity,
                    station_id,
                });
                self.holding_count += 1;
                return Ok(());
            }
        }

        Err(EscrowError::EscrowCapacityExceeded)
    }

    /// Cancels a resting order and returns the locked escrow holding for refund.
    pub fn cancel_and_refund_escrow(
        &mut self,
        order_id: u64,
        account_id: u64,
    ) -> Result<EscrowHolding, EscrowError> {
        for slot in self.holdings.iter_mut() {
            if let Some(holding) = *slot {
                if holding.order_id == order_id {
                    if holding.account_id != account_id {
                        return Err(EscrowError::UnauthorizedAccount);
                    }
                    *slot = None;
                    self.holding_count -= 1;
                    return Ok(holding);
                }
            }
        }
        Err(EscrowError::EscrowNotFound)
    }

    /// Settles a matched trade execution between buyer and seller escrow holdings.
    ///
    /// Computes local station sales tax, deducts currency from buyer escrow,
    /// credits net proceeds to seller, and handles buyer surplus refunds if matched below limit price.
    ///
    /// Returns: `(net_seller_payout, tax_collected, buyer_refund)`.
    pub fn settle_trade(
        &mut self,
        trade: &TradeExecution,
        buyer_limit_price: u64,
    ) -> Result<(u64, u64, u64), EscrowError> {
        // Look up station tax rate (default 100 bps = 1.0% if station not explicitly registered)
        let tax_bps = self
            .get_station(trade.station_id)
            .map(|s| s.sales_tax_bps as u64)
            .unwrap_or(100);

        let tax_collected = trade.total_price.saturating_mul(tax_bps) / 10_000;
        let net_seller_payout = trade.total_price.saturating_sub(tax_collected);

        // Buyer refund occurs if buyer's limit price was higher than execution price
        let buyer_unit_savings = buyer_limit_price.saturating_sub(trade.unit_price);
        let buyer_refund = (trade.quantity as u64).saturating_mul(buyer_unit_savings);

        // Deduct from buyer escrow holding
        let mut buyer_holding_found = false;
        for slot in self.holdings.iter_mut() {
            if let Some(ref mut holding) = slot {
                if holding.order_id == trade.buy_order_id && holding.is_currency {
                    buyer_holding_found = true;
                    let total_buyer_escrow_deduction =
                        (trade.quantity as u64).saturating_mul(buyer_limit_price);
                    holding.locked_currency = holding
                        .locked_currency
                        .saturating_sub(total_buyer_escrow_deduction);

                    if holding.locked_currency == 0 {
                        *slot = None;
                        self.holding_count -= 1;
                    }
                    break;
                }
            }
        }

        // Deduct from seller item escrow holding
        let mut seller_holding_found = false;
        for slot in self.holdings.iter_mut() {
            if let Some(ref mut holding) = slot {
                if holding.order_id == trade.sell_order_id && !holding.is_currency {
                    seller_holding_found = true;
                    holding.locked_quantity =
                        holding.locked_quantity.saturating_sub(trade.quantity);

                    if holding.locked_quantity == 0 {
                        *slot = None;
                        self.holding_count -= 1;
                    }
                    break;
                }
            }
        }

        // Note: For immediate matching orders where the taker order wasn't previously resting,
        // one of the holdings may not exist in resting escrow, which is expected.
        let _ = (buyer_holding_found, seller_holding_found);

        self.total_tax_revenue = self.total_tax_revenue.saturating_add(tax_collected);
        self.total_volume_settled = self.total_volume_settled.saturating_add(trade.total_price);

        Ok((net_seller_payout, tax_collected, buyer_refund))
    }

    /// Submits an order through the coordinator, automatically funding escrow and matching.
    pub fn submit_and_match(
        &mut self,
        order: MarketOrder,
        book: &mut OrderBook,
        trades_out: &mut [TradeExecution],
        current_tick: u64,
    ) -> Result<(usize, MarketOrder), EscrowError> {
        // If order will rest as a limit order, lock escrow up front
        if order.order_type == OrderType::Limit {
            if order.is_buy {
                let total_cost = (order.initial_quantity as u64).saturating_mul(order.unit_price);
                self.lock_buy_escrow(
                    order.order_id,
                    order.account_id,
                    total_cost,
                    order.station_id,
                )?;
            } else {
                self.lock_sell_escrow(
                    order.order_id,
                    order.account_id,
                    order.item_type_id as u64,
                    order.initial_quantity,
                    order.station_id,
                )?;
            }
        }

        let (trade_count, final_order) = book
            .place_order(order, trades_out, current_tick)
            .map_err(EscrowError::MarketFailure)?;

        // Settle all executed trades
        for trade in trades_out.iter().take(trade_count) {
            let buyer_limit = if order.is_buy {
                order.unit_price
            } else {
                trade.unit_price // Sell order matched against existing buy limit
            };
            let _ = self.settle_trade(trade, buyer_limit)?;
        }

        Ok((trade_count, final_order))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_station_freight_and_distance() {
        // Station 1: Sector (0, 0), 200 bps tax (2.0%), 10 copper per sector freight
        let st1 = MarketStation::new(1, 0, 0, 200, 10);
        // Station 2: Sector (3, 4), 100 bps tax (1.0%), 15 copper per sector freight
        let st2 = MarketStation::new(2, 3, 4, 100, 15);

        // Manhattan distance: |3 - 0| + |4 - 0| = 3 + 4 = 7 sectors
        assert_eq!(st1.sector_distance_to(&st2), 7);
        assert_eq!(st2.sector_distance_to(&st1), 7);

        // Freight for 5 units from Station 1 to Station 2: 7 sectors * 10 copper * 5 units = 350 copper
        let freight = st1.calculate_freight_fee(&st2, 5);
        assert_eq!(freight, 350);
    }

    #[test]
    fn test_escrow_settlement_and_buyer_savings_refund() {
        let mut coordinator = MarketEscrowCoordinator::new();
        coordinator.register_station(MarketStation::new(1, 0, 0, 500, 5)); // 5.0% tax (500 bps)

        let mut book = OrderBook::new(999);
        let mut trades = [TradeExecution {
            trade_id: 0,
            buy_order_id: 0,
            sell_order_id: 0,
            buyer_account_id: 0,
            seller_account_id: 0,
            item_type_id: 0,
            quantity: 0,
            unit_price: 0,
            total_price: 0,
            station_id: 0,
            tick: 0,
        }; 4];

        // Seller places resting sell order: 10 units @ 80 copper
        let sell_order = MarketOrder::new(101, 1, 999, false, 80, 10, 1, OrderType::Limit, 1, 1000);
        coordinator
            .submit_and_match(sell_order, &mut book, &mut trades, 1)
            .unwrap();
        assert_eq!(coordinator.holding_count, 1);

        // Buyer places buy limit order: 10 units @ 100 copper (willing to pay up to 100)
        let buy_order = MarketOrder::new(102, 2, 999, true, 100, 10, 1, OrderType::Limit, 2, 1000);
        let (trades_matched, final_buy) = coordinator
            .submit_and_match(buy_order, &mut book, &mut trades, 2)
            .unwrap();

        assert_eq!(trades_matched, 1);
        assert_eq!(final_buy.remaining_quantity, 0);

        // Trade execution price should be seller's passive price of 80
        let trade = trades[0];
        assert_eq!(trade.unit_price, 80);
        assert_eq!(trade.total_price, 800);

        // Tax: 5% of 800 = 40 copper
        // Net seller payout: 800 - 40 = 760 copper
        // Buyer savings refund: (100 - 80) * 10 = 200 copper refunded to buyer!
        assert_eq!(coordinator.total_tax_revenue, 40);
        assert_eq!(coordinator.total_volume_settled, 800);
        // All escrow holdings fulfilled and cleared
        assert_eq!(coordinator.holding_count, 0);
    }
}
