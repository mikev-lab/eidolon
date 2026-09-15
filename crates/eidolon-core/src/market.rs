//! Universal Regional Market & Auction Order Book (The EVE Economic Exchange).
//!
//! Provides a deterministic, zero-allocation double-auction order matching engine
//! supporting price-time priority, partial fills, Fill-or-Kill (FOK), Immediate-or-Cancel (IOC),
//! and multi-station regional settlement.

/// Maximum orders stored per side (bids or asks) in a single commodity order book.
pub const MAX_ORDERS_PER_SIDE: usize = 64;

/// Order execution time-in-force classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    /// Limit order: rests on the book until filled or cancelled.
    Limit,
    /// Immediate-or-Cancel: matches available depth immediately; remaining quantity is cancelled.
    ImmediateOrCancel,
    /// Fill-or-Kill: must be filled completely immediately, or the entire order is rejected.
    FillOrKill,
}

/// Lifecycle status of a market order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    /// Order is active and resting in the order book.
    Active,
    /// Order has been partially matched and remains active for remaining quantity.
    PartiallyFilled,
    /// Order has been completely fulfilled.
    Filled,
    /// Order was cancelled by owner or expired.
    Cancelled,
    /// Order was rejected (e.g. FOK that could not be completely filled).
    Rejected,
}

/// A commodity order placed on the regional market.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketOrder {
    /// Unique monotonic order ID.
    pub order_id: u64,
    /// Account ID of the trader placing the order.
    pub account_id: u64,
    /// Commodity / item type ID being traded.
    pub item_type_id: u32,
    /// True if buying; false if selling.
    pub is_buy: bool,
    /// Unit price in base currency units (copper/cents).
    pub unit_price: u64,
    /// Initial requested quantity.
    pub initial_quantity: u32,
    /// Remaining unfilled quantity.
    pub remaining_quantity: u32,
    /// Station / hub identifier where the goods are located or requested.
    pub station_id: u32,
    /// Order execution policy.
    pub order_type: OrderType,
    /// Current order status.
    pub status: OrderStatus,
    /// Server tick when placed.
    pub placed_tick: u64,
    /// Server tick when the order expires.
    pub expiration_tick: u64,
}

impl MarketOrder {
    /// Creates a new active market order.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        order_id: u64,
        account_id: u64,
        item_type_id: u32,
        is_buy: bool,
        unit_price: u64,
        quantity: u32,
        station_id: u32,
        order_type: OrderType,
        placed_tick: u64,
        duration_ticks: u64,
    ) -> Self {
        Self {
            order_id,
            account_id,
            item_type_id,
            is_buy,
            unit_price,
            initial_quantity: quantity,
            remaining_quantity: quantity,
            station_id,
            order_type,
            status: OrderStatus::Active,
            placed_tick,
            expiration_tick: placed_tick.saturating_add(duration_ticks),
        }
    }
}

/// Record of an executed trade match between a buyer and seller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeExecution {
    /// Monotonic trade execution identifier.
    pub trade_id: u64,
    /// Buy order identifier.
    pub buy_order_id: u64,
    /// Sell order identifier.
    pub sell_order_id: u64,
    /// Buyer account identifier.
    pub buyer_account_id: u64,
    /// Seller account identifier.
    pub seller_account_id: u64,
    /// Traded commodity item type identifier.
    pub item_type_id: u32,
    /// Number of units transferred.
    pub quantity: u32,
    /// Execution price per unit.
    pub unit_price: u64,
    /// Total settlement price (`quantity * unit_price`).
    pub total_price: u64,
    /// Station location of the filled goods.
    pub station_id: u32,
    /// Server tick when executed.
    pub tick: u64,
}

/// Errors occurring during market operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketError {
    /// Order book reached maximum capacity.
    OrderBookFull,
    /// Order with this ID already exists.
    DuplicateOrderId,
    /// Order was not found in the book.
    OrderNotFound,
    /// Caller does not have authorization to cancel this order.
    UnauthorizedCancellation,
    /// Zero quantity or zero price specified.
    InvalidOrderParameters,
    /// Fill-or-Kill order could not be completely matched.
    FillOrKillUnfulfilled,
    /// Order has already expired or completed.
    OrderNotActive,
}

impl core::fmt::Display for MarketError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OrderBookFull => write!(f, "Order book capacity exceeded"),
            Self::DuplicateOrderId => write!(f, "Duplicate order identifier"),
            Self::OrderNotFound => write!(f, "Order not found in book"),
            Self::UnauthorizedCancellation => write!(f, "Unauthorized cancellation attempt"),
            Self::InvalidOrderParameters => write!(f, "Invalid order price or quantity"),
            Self::FillOrKillUnfulfilled => write!(f, "Fill-or-Kill order could not be fulfilled"),
            Self::OrderNotActive => write!(f, "Order is no longer active"),
        }
    }
}

impl std::error::Error for MarketError {}

/// In-place price-time priority order book for a single commodity type.
#[derive(Debug)]
pub struct OrderBook {
    /// Commodity item type ID tracked by this order book.
    pub item_type_id: u32,
    bids: [Option<MarketOrder>; MAX_ORDERS_PER_SIDE],
    bid_count: usize,
    asks: [Option<MarketOrder>; MAX_ORDERS_PER_SIDE],
    ask_count: usize,
    next_trade_id: u64,
}

impl OrderBook {
    /// Creates an empty order book for the given commodity type.
    pub fn new(item_type_id: u32) -> Self {
        Self {
            item_type_id,
            bids: [None; MAX_ORDERS_PER_SIDE],
            bid_count: 0,
            asks: [None; MAX_ORDERS_PER_SIDE],
            ask_count: 0,
            next_trade_id: 1,
        }
    }

    /// Number of active bids.
    pub fn bid_count(&self) -> usize {
        self.bid_count
    }

    /// Number of active asks.
    pub fn ask_count(&self) -> usize {
        self.ask_count
    }

    /// Returns the best (highest) bid price if any exist.
    pub fn best_bid_price(&self) -> Option<u64> {
        self.bids.iter().flatten().map(|o| o.unit_price).max()
    }

    /// Returns the best (lowest) ask price if any exist.
    pub fn best_ask_price(&self) -> Option<u64> {
        self.asks.iter().flatten().map(|o| o.unit_price).min()
    }

    /// Inserts a resting order into the book maintaining price-time priority.
    fn insert_resting_order(&mut self, order: MarketOrder) -> Result<(), MarketError> {
        let (side, count) = if order.is_buy {
            (&mut self.bids, &mut self.bid_count)
        } else {
            (&mut self.asks, &mut self.ask_count)
        };

        if *count >= MAX_ORDERS_PER_SIDE {
            return Err(MarketError::OrderBookFull);
        }

        // Check for duplicate order ID
        for slot in side.iter().flatten() {
            if slot.order_id == order.order_id {
                return Err(MarketError::DuplicateOrderId);
            }
        }

        // Find an empty slot
        for slot in side.iter_mut() {
            if slot.is_none() {
                *slot = Some(order);
                *count += 1;
                return Ok(());
            }
        }

        Err(MarketError::OrderBookFull)
    }

    /// Cancels an active order by ID.
    pub fn cancel_order(
        &mut self,
        order_id: u64,
        account_id: u64,
    ) -> Result<MarketOrder, MarketError> {
        // Search bids
        for slot in self.bids.iter_mut() {
            if let Some(mut o) = *slot {
                if o.order_id == order_id {
                    if o.account_id != account_id {
                        return Err(MarketError::UnauthorizedCancellation);
                    }
                    *slot = None;
                    self.bid_count -= 1;
                    o.status = OrderStatus::Cancelled;
                    return Ok(o);
                }
            }
        }

        // Search asks
        for slot in self.asks.iter_mut() {
            if let Some(mut o) = *slot {
                if o.order_id == order_id {
                    if o.account_id != account_id {
                        return Err(MarketError::UnauthorizedCancellation);
                    }
                    *slot = None;
                    self.ask_count -= 1;
                    o.status = OrderStatus::Cancelled;
                    return Ok(o);
                }
            }
        }

        Err(MarketError::OrderNotFound)
    }

    /// Executes double-auction matching against the resting book.
    ///
    /// If the order is a Limit order with remaining quantity, it rests in the book.
    /// Returns the number of trades generated in the output slice.
    pub fn place_order(
        &mut self,
        mut incoming: MarketOrder,
        trades_out: &mut [TradeExecution],
        current_tick: u64,
    ) -> Result<(usize, MarketOrder), MarketError> {
        if incoming.unit_price == 0 || incoming.initial_quantity == 0 {
            return Err(MarketError::InvalidOrderParameters);
        }

        // For Fill-or-Kill, verify total available volume at or better than limit price before executing
        if incoming.order_type == OrderType::FillOrKill {
            let mut available_qty = 0;
            if incoming.is_buy {
                for ask in self.asks.iter().flatten() {
                    if ask.unit_price <= incoming.unit_price {
                        available_qty += ask.remaining_quantity;
                    }
                }
            } else {
                for bid in self.bids.iter().flatten() {
                    if bid.unit_price >= incoming.unit_price {
                        available_qty += bid.remaining_quantity;
                    }
                }
            }

            if available_qty < incoming.initial_quantity {
                return Err(MarketError::FillOrKillUnfulfilled);
            }
        }

        let mut trade_count = 0;

        // Match against opposite side while incoming order has remaining quantity
        while incoming.remaining_quantity > 0 && trade_count < trades_out.len() {
            // Find best matching resting order (price-time priority)
            let best_match_idx = if incoming.is_buy {
                // For incoming buy: find ask with lowest price, then lowest placed_tick
                let mut best: Option<(usize, u64, u64)> = None;
                for (i, slot) in self.asks.iter().enumerate() {
                    if let Some(ask) = slot {
                        if ask.unit_price <= incoming.unit_price {
                            match best {
                                None => best = Some((i, ask.unit_price, ask.placed_tick)),
                                Some((_, best_p, best_t)) => {
                                    if ask.unit_price < best_p
                                        || (ask.unit_price == best_p && ask.placed_tick < best_t)
                                    {
                                        best = Some((i, ask.unit_price, ask.placed_tick));
                                    }
                                }
                            }
                        }
                    }
                }
                best.map(|(i, _, _)| i)
            } else {
                // For incoming sell: find bid with highest price, then lowest placed_tick
                let mut best: Option<(usize, u64, u64)> = None;
                for (i, slot) in self.bids.iter().enumerate() {
                    if let Some(bid) = slot {
                        if bid.unit_price >= incoming.unit_price {
                            match best {
                                None => best = Some((i, bid.unit_price, bid.placed_tick)),
                                Some((_, best_p, best_t)) => {
                                    if bid.unit_price > best_p
                                        || (bid.unit_price == best_p && bid.placed_tick < best_t)
                                    {
                                        best = Some((i, bid.unit_price, bid.placed_tick));
                                    }
                                }
                            }
                        }
                    }
                }
                best.map(|(i, _, _)| i)
            };

            let Some(idx) = best_match_idx else {
                break; // No further crosses available
            };

            // Execute partial or complete trade with best match
            if incoming.is_buy {
                let Some(ask) = self.asks.get_mut(idx).and_then(Option::as_mut) else {
                    break;
                };
                let matched_qty = incoming.remaining_quantity.min(ask.remaining_quantity);
                let exec_price = ask.unit_price; // Maker gets passive limit price
                let total_price = (matched_qty as u64).saturating_mul(exec_price);

                trades_out[trade_count] = TradeExecution {
                    trade_id: self.next_trade_id,
                    buy_order_id: incoming.order_id,
                    sell_order_id: ask.order_id,
                    buyer_account_id: incoming.account_id,
                    seller_account_id: ask.account_id,
                    item_type_id: incoming.item_type_id,
                    quantity: matched_qty,
                    unit_price: exec_price,
                    total_price,
                    station_id: ask.station_id,
                    tick: current_tick,
                };
                self.next_trade_id += 1;
                trade_count += 1;

                incoming.remaining_quantity -= matched_qty;
                ask.remaining_quantity -= matched_qty;

                if ask.remaining_quantity == 0 {
                    ask.status = OrderStatus::Filled;
                    self.asks[idx] = None;
                    self.ask_count -= 1;
                } else {
                    ask.status = OrderStatus::PartiallyFilled;
                }
            } else {
                let Some(bid) = self.bids.get_mut(idx).and_then(Option::as_mut) else {
                    break;
                };
                let matched_qty = incoming.remaining_quantity.min(bid.remaining_quantity);
                let exec_price = bid.unit_price; // Maker gets passive limit price
                let total_price = (matched_qty as u64).saturating_mul(exec_price);

                trades_out[trade_count] = TradeExecution {
                    trade_id: self.next_trade_id,
                    buy_order_id: bid.order_id,
                    sell_order_id: incoming.order_id,
                    buyer_account_id: bid.account_id,
                    seller_account_id: incoming.account_id,
                    item_type_id: incoming.item_type_id,
                    quantity: matched_qty,
                    unit_price: exec_price,
                    total_price,
                    station_id: incoming.station_id,
                    tick: current_tick,
                };
                self.next_trade_id += 1;
                trade_count += 1;

                incoming.remaining_quantity -= matched_qty;
                bid.remaining_quantity -= matched_qty;

                if bid.remaining_quantity == 0 {
                    bid.status = OrderStatus::Filled;
                    self.bids[idx] = None;
                    self.bid_count -= 1;
                } else {
                    bid.status = OrderStatus::PartiallyFilled;
                }
            }
        }

        // Finalize incoming order status
        if incoming.remaining_quantity == 0 {
            incoming.status = OrderStatus::Filled;
        } else if incoming.order_type == OrderType::ImmediateOrCancel {
            incoming.status = if trade_count > 0 {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::Cancelled
            };
        } else if incoming.order_type == OrderType::Limit {
            incoming.status = if trade_count > 0 {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::Active
            };
            self.insert_resting_order(incoming)?;
        }

        Ok((trade_count, incoming))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_book_limit_matching_and_price_time_priority() {
        let mut book = OrderBook::new(1001);
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

        // Sell order 1: 10 units @ 100 copper, placed at tick 10
        let sell1 = MarketOrder::new(1, 10, 1001, false, 100, 10, 1, OrderType::Limit, 10, 1000);
        let (t1, o1) = book.place_order(sell1, &mut trades, 10).unwrap();
        assert_eq!(t1, 0);
        assert_eq!(o1.status, OrderStatus::Active);
        assert_eq!(book.ask_count(), 1);

        // Sell order 2: 10 units @ 90 copper, placed at tick 20 (better price than sell1)
        let sell2 = MarketOrder::new(2, 20, 1001, false, 90, 10, 1, OrderType::Limit, 20, 1000);
        let (t2, o2) = book.place_order(sell2, &mut trades, 20).unwrap();
        assert_eq!(t2, 0);
        assert_eq!(o2.status, OrderStatus::Active);
        assert_eq!(book.ask_count(), 2);

        // Buy order: 15 units @ 100 copper, placed at tick 30
        // Should match 10 units from sell2 @ 90 first, then 5 units from sell1 @ 100
        let buy = MarketOrder::new(3, 30, 1001, true, 100, 15, 1, OrderType::Limit, 30, 1000);
        let (t3, o3) = book.place_order(buy, &mut trades, 30).unwrap();
        assert_eq!(t3, 2);
        assert_eq!(o3.remaining_quantity, 0);
        assert_eq!(o3.status, OrderStatus::Filled);

        // Verify trade 1 (matched against sell2)
        assert_eq!(trades[0].sell_order_id, 2);
        assert_eq!(trades[0].quantity, 10);
        assert_eq!(trades[0].unit_price, 90);
        assert_eq!(trades[0].total_price, 900);

        // Verify trade 2 (matched against sell1)
        assert_eq!(trades[1].sell_order_id, 1);
        assert_eq!(trades[1].quantity, 5);
        assert_eq!(trades[1].unit_price, 100);
        assert_eq!(trades[1].total_price, 500);

        // Sell 1 should still have 5 remaining units
        assert_eq!(book.ask_count(), 1);
        assert_eq!(book.best_ask_price(), Some(100));
    }

    #[test]
    fn test_order_book_fill_or_kill_rejected_when_insufficient_depth() {
        let mut book = OrderBook::new(2001);
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
        }; 2];

        // Sell 5 units @ 50
        let sell = MarketOrder::new(1, 10, 2001, false, 50, 5, 1, OrderType::Limit, 1, 100);
        book.place_order(sell, &mut trades, 1).unwrap();

        // Buy 10 units FOK @ 50 (only 5 available -> should reject completely)
        let fok_buy = MarketOrder::new(2, 20, 2001, true, 50, 10, 1, OrderType::FillOrKill, 2, 100);
        let res = book.place_order(fok_buy, &mut trades, 2);
        assert_eq!(res, Err(MarketError::FillOrKillUnfulfilled));

        // Resting sell order remains completely intact
        assert_eq!(book.ask_count(), 1);
    }

    #[test]
    fn test_order_cancellation_and_unauthorized_rejection() {
        let mut book = OrderBook::new(3001);
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
        }; 1];

        let sell = MarketOrder::new(100, 42, 3001, false, 10, 1, 1, OrderType::Limit, 1, 100);
        book.place_order(sell, &mut trades, 1).unwrap();

        // Unauthorized cancellation (wrong account ID)
        assert_eq!(
            book.cancel_order(100, 999),
            Err(MarketError::UnauthorizedCancellation)
        );

        // Authorized cancellation
        let cancelled = book.cancel_order(100, 42).unwrap();
        assert_eq!(cancelled.status, OrderStatus::Cancelled);
        assert_eq!(book.ask_count(), 0);
    }
}
