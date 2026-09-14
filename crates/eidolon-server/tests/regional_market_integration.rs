//! Exhaustive integration test suite for Phase 25: Universal Regional Market & Auction Order Book.
//!
//! Validates:
//! 1. Double-auction order matching with price-time priority and maker passive pricing.
//! 2. Partial fills and multi-order book depth clearing.
//! 3. Execution policies: Limit, Immediate-or-Cancel (IOC), and Fill-or-Kill (FOK).
//! 4. Escrow currency and item locking with atomic trade settlement.
//! 5. Regional station tariffs, sales tax revenue, and distance-based freight fees.
//! 6. Order cancellation authorization and escrow holding refunds.

use eidolon_core::{MarketError, MarketOrder, OrderBook, OrderStatus, OrderType, TradeExecution};
use eidolon_world::{EscrowError, MarketEscrowCoordinator, MarketStation};

#[test]
fn test_order_book_price_time_priority_and_passive_pricing() {
    let mut book = OrderBook::new(101); // Commodity: Iron Ore
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

    // Asks:
    // Seller 1: 10 units @ 105 copper, placed at tick 10
    let ask1 = MarketOrder::new(1, 10, 101, false, 105, 10, 1, OrderType::Limit, 10, 1000);
    book.place_order(ask1, &mut trades, 10).unwrap();

    // Seller 2: 10 units @ 100 copper, placed at tick 20 (cheaper than ask1)
    let ask2 = MarketOrder::new(2, 20, 101, false, 100, 10, 1, OrderType::Limit, 20, 1000);
    book.place_order(ask2, &mut trades, 20).unwrap();

    // Seller 3: 10 units @ 100 copper, placed at tick 25 (same price as ask2, but later)
    let ask3 = MarketOrder::new(3, 30, 101, false, 100, 10, 1, OrderType::Limit, 25, 1000);
    book.place_order(ask3, &mut trades, 25).unwrap();

    assert_eq!(book.ask_count(), 3);
    assert_eq!(book.best_ask_price(), Some(100));

    // Buyer arrives at tick 30 with buy limit of 110 copper for 15 units.
    // Matching sequence should be:
    // 1. Ask 2: 10 units @ 100 (lowest price, placed earlier)
    // 2. Ask 3: 5 units @ 100 (same price, placed later)
    let buy = MarketOrder::new(4, 40, 101, true, 110, 15, 1, OrderType::Limit, 30, 1000);
    let (trade_count, final_buy) = book.place_order(buy, &mut trades, 30).unwrap();

    assert_eq!(trade_count, 2);
    assert_eq!(final_buy.status, OrderStatus::Filled);
    assert_eq!(final_buy.remaining_quantity, 0);

    // Trade 1 with Seller 2
    assert_eq!(trades[0].sell_order_id, 2);
    assert_eq!(trades[0].quantity, 10);
    assert_eq!(trades[0].unit_price, 100); // Passive maker price

    // Trade 2 with Seller 3
    assert_eq!(trades[1].sell_order_id, 3);
    assert_eq!(trades[1].quantity, 5);
    assert_eq!(trades[1].unit_price, 100);

    // Ask 3 should have 5 remaining units; Ask 1 remains untouched at 105
    assert_eq!(book.ask_count(), 2);
    assert_eq!(book.best_ask_price(), Some(100));
}

#[test]
fn test_immediate_or_cancel_policy() {
    let mut book = OrderBook::new(202); // Commodity: Healing Potion
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

    // Resting sell: 4 units @ 50 copper
    let sell = MarketOrder::new(1, 10, 202, false, 50, 4, 1, OrderType::Limit, 1, 100);
    book.place_order(sell, &mut trades, 1).unwrap();

    // Buyer places IOC for 10 units @ 50 copper
    let ioc_buy = MarketOrder::new(
        2,
        20,
        202,
        true,
        50,
        10,
        1,
        OrderType::ImmediateOrCancel,
        2,
        100,
    );
    let (trade_count, final_buy) = book.place_order(ioc_buy, &mut trades, 2).unwrap();

    // Must match the 4 available units, and cancel the remaining 6 without resting in book
    assert_eq!(trade_count, 1);
    assert_eq!(trades[0].quantity, 4);
    assert_eq!(final_buy.remaining_quantity, 6);
    assert_eq!(final_buy.status, OrderStatus::PartiallyFilled);
    assert_eq!(book.bid_count(), 0); // Did not rest in book!
    assert_eq!(book.ask_count(), 0); // Resting ask was consumed
}

#[test]
fn test_fill_or_kill_execution_and_rejection() {
    let mut book = OrderBook::new(303);
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

    // Resting asks: 3 units @ 20, 4 units @ 25 (total 7 units <= 25)
    let s1 = MarketOrder::new(1, 10, 303, false, 20, 3, 1, OrderType::Limit, 1, 100);
    let s2 = MarketOrder::new(2, 20, 303, false, 25, 4, 1, OrderType::Limit, 2, 100);
    book.place_order(s1, &mut trades, 1).unwrap();
    book.place_order(s2, &mut trades, 2).unwrap();

    // Case 1: FOK for 8 units @ 25 (fails because only 7 available)
    let fok_fail = MarketOrder::new(3, 30, 303, true, 25, 8, 1, OrderType::FillOrKill, 3, 100);
    assert_eq!(
        book.place_order(fok_fail, &mut trades, 3),
        Err(MarketError::FillOrKillUnfulfilled)
    );
    assert_eq!(book.ask_count(), 2); // Book unchanged

    // Case 2: FOK for 7 units @ 25 (succeeds completely across both asks)
    let fok_ok = MarketOrder::new(4, 40, 303, true, 25, 7, 1, OrderType::FillOrKill, 4, 100);
    let (trade_count, final_fok) = book.place_order(fok_ok, &mut trades, 4).unwrap();
    assert_eq!(trade_count, 2);
    assert_eq!(final_fok.status, OrderStatus::Filled);
    assert_eq!(final_fok.remaining_quantity, 0);
    assert_eq!(book.ask_count(), 0); // Both asks completely consumed
}

#[test]
fn test_market_coordinator_escrow_lifecycle_and_tariffs() {
    let mut coordinator = MarketEscrowCoordinator::new();

    // Station Alpha: Sector (0, 0), 2.5% sales tax (250 bps), 5 copper freight/sector
    let station_alpha = MarketStation::new(1, 0, 0, 250, 5);
    // Station Beta: Sector (10, 0), 1.0% sales tax (100 bps), 5 copper freight/sector
    let station_beta = MarketStation::new(2, 10, 0, 100, 5);

    coordinator.register_station(station_alpha);
    coordinator.register_station(station_beta);

    // Freight calculation between Alpha and Beta for 10 units:
    // Distance = 10 sectors * 5 copper/sector * 10 units = 500 copper
    assert_eq!(station_alpha.calculate_freight_fee(&station_beta, 10), 500);

    let mut book = OrderBook::new(404);
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

    // Seller places limit sell at Station Alpha: 20 units @ 50 copper
    // Total value = 1,000 copper
    let sell = MarketOrder::new(501, 100, 404, false, 50, 20, 1, OrderType::Limit, 1, 500);
    coordinator
        .submit_and_match(sell, &mut book, &mut trades, 1)
        .unwrap();

    // Buyer places limit buy at Station Alpha: 10 units @ 60 copper (willing to pay up to 60)
    let buy = MarketOrder::new(502, 200, 404, true, 60, 10, 1, OrderType::Limit, 2, 500);
    let (trade_count, final_buy) = coordinator
        .submit_and_match(buy, &mut book, &mut trades, 2)
        .unwrap();

    assert_eq!(trade_count, 1);
    assert_eq!(final_buy.status, OrderStatus::Filled);

    // Settlement verification:
    // Matched price = 50 copper (passive sell limit)
    // Gross volume = 10 * 50 = 500 copper
    // Tax at Station Alpha (2.5%) = 500 * 250 / 10000 = 12 copper
    // Net seller payout = 500 - 12 = 488 copper
    // Buyer savings refund: (60 - 50) * 10 = 100 copper
    assert_eq!(coordinator.total_tax_revenue, 12);
    assert_eq!(coordinator.total_volume_settled, 500);

    // Cancel remaining sell order (10 units remaining)
    let cancelled_order = book.cancel_order(501, 100).unwrap();
    assert_eq!(cancelled_order.remaining_quantity, 10);
    let refund_holding = coordinator.cancel_and_refund_escrow(501, 100).unwrap();
    assert_eq!(refund_holding.locked_quantity, 10);
}

#[test]
fn test_order_cancellation_fencing_and_security() {
    let mut coordinator = MarketEscrowCoordinator::new();
    let mut book = OrderBook::new(505);
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

    let buy = MarketOrder::new(601, 42, 505, true, 100, 5, 1, OrderType::Limit, 1, 100);
    coordinator
        .submit_and_match(buy, &mut book, &mut trades, 1)
        .unwrap();

    // Attacker (Account 999) tries to cancel Account 42's order in book
    assert_eq!(
        book.cancel_order(601, 999),
        Err(MarketError::UnauthorizedCancellation)
    );

    // Attacker tries to steal escrow refund
    assert_eq!(
        coordinator.cancel_and_refund_escrow(601, 999),
        Err(EscrowError::UnauthorizedAccount)
    );

    // Legitimate cancellation succeeds
    assert!(book.cancel_order(601, 42).is_ok());
    let holding = coordinator.cancel_and_refund_escrow(601, 42).unwrap();
    assert_eq!(holding.locked_currency, 500); // 5 * 100 copper refunded
}
