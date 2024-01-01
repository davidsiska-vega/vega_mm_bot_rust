use futures_util::lock::MutexGuard as FuturesUtilsMutexGuard;
use log::info;
use num_traits::ToPrimitive;
use vega_protobufs::vega::events::v1::ExpiredOrders;
use core::num;
use std::time::{SystemTime, UNIX_EPOCH};
use num_bigint::BigUint;
use num_traits::cast::FromPrimitive;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;


use vega_crypto::Transact;
use vega_protobufs::vega::{
    commands::v1::{
        input_data::Command, 
        BatchMarketInstructions, 
        OrderCancellation, 
        OrderSubmission,
        LiquidityProvisionSubmission,
        LiquidityProvisionAmendment,
        LiquidityProvisionCancellation,
    },
    instrument::Product,
    order::{TimeInForce, Type},
    Market, Side
};
use vega_protobufs::vega::{Asset, Position};
use vega_protobufs::vega::MarketData;

use crate::{binance_ws::RefPrice, vega_store2::VegaStore};
use crate::{Config, vega_store2};
use crate::opt_offsets;
use crate::estimate_params::{self, estimate_lambda2, estimate_kappa};



pub async fn start(
    mut w1: Transact,
    config: Config,
    store: Arc<Mutex<VegaStore>>,
    rp: Arc<Mutex<RefPrice>>,
) {
    // just loop forever, waiting for user interupt
    info!(
        "starting with submission rate of {} seconds",
        config.submission_rate
    );

    let mkt = store.lock().unwrap().get_market();
    let asset = store.lock().unwrap().get_asset(get_asset(&mkt));
    let d = Decimals::new(&mkt, &asset);

    if !config.dryrun {
        info!("closing all orders");
        match w1
            .send(Command::BatchMarketInstructions(get_close_batch(
                config.vega_market.clone(),
            )))
            .await
        {
            Ok(o) => info!("cancel orders batch result: {:?}", o),
            Err(e) => info!("cancel orders transaction error: {:?}", e),
        };

        if config.cancel_liquidity_commitment {
            match w1
                .send(Command::LiquidityProvisionCancellation(get_liquidity_cancellation_transaction(
                    config.vega_market.clone(),
                )))
                .await
            {
                Ok(o) => info!("cancel liquidity result: {:?}", o),
                Err(e) => info!("cancel liquidity  error: {:?}", e),
            };
        }

        if config.create_liquidity_commitment{
            match w1
                .send(Command::LiquidityProvisionSubmission(get_liquidity_submission_transaction(
                    config.vega_market.clone(),
                    config.bond_amount as f64,
                    config.lp_fee_bid,
                    "basic_mm_bot".to_string(),
                    &d,
                )))
                .await
            {
                Ok(o) => info!("submit new liquidity result: {:?}", o),
                Err(e) => info!("submit new liquidity  error: {:?}", e),
            };
        }

        if config.amend_liquidity_commitment {
            match w1
                .send(Command::LiquidityProvisionAmendment(get_liquidity_amendment_transaction(
                    config.vega_market.clone(),
                    config.bond_amount as f64,
                    config.lp_fee_bid,
                    "basic_mm_bot".to_string(),
                    &d,
                )))
                .await
            {
                Ok(o) => info!("submit new liquidity result: {:?}", o),
                Err(e) => info!("submit new liquidity  error: {:?}", e),
            };
        }
    }
    else {
        info!("drurun mode, at this stage would submit a close orders transaction and update / cancel liquidity"); 
    }



    let mut interval = time::interval(Duration::from_secs(config.submission_rate));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                interval.reset();
                // let extra_sleep = rand::random::<u64>() % submission_rate;
                // info!("adding extra sleep of {} seconds before starting", extra_sleep);
                // // add some extra time here jsut to look a little bit less scripted
                // time::sleep(Duration::from_secs(extra_sleep)).await;
                run_strategy(&mut w1, 
                    &config, 
                    store.clone(), 
                    rp.clone(), 
                ).await;
            }
        }
    }
}

async fn run_strategy(
    w1: &mut Transact,
    c: &Config,
    store: Arc<Mutex<VegaStore>>,
    rp: Arc<Mutex<RefPrice>>,
) {
    if c.q_lower >= c.q_upper {
        panic!("we need q_lower < q_upper");
    }
    
    info!("executing trading strategy...");
    let mkt = store.lock().unwrap().get_market();
    let asset = store.lock().unwrap().get_asset(get_asset(&mkt));

    info!(
        "updating quotes for {}",
        mkt.tradable_instrument
            .as_ref()
            .unwrap()
            .instrument
            .as_ref()
            .unwrap()
            .name
    );

    //default_trade_size = ((rand::random::<u64>() % default_trade_size as u64) + 1) as i64;
    //info!("selected trade size: {}", default_trade_size,);

    let d = Decimals::new(&mkt, &asset);

    let (binance_best_bid_f, binance_best_ask_f) = rp.lock().unwrap().get();
    if binance_best_ask_f <= 0.0 || binance_best_ask_f <= 0.0 {
        info!("At least one Binance price is NOT +ve! Either error or prices not updated yet.");
        return;
    }
    let binance_best_bid = (binance_best_bid_f * d.price_factor) as u64;
    let binance_best_ask = (binance_best_ask_f * d.price_factor) as u64;
    info!(
        "new Binance reference prices: bestBid({}), bestAsk({}))", binance_best_bid, binance_best_ask);

    let md = store.lock().unwrap().get_market_data();
    
    let vega_best_bid = BigUint::parse_bytes(md.best_bid_price.as_bytes(), 10).unwrap().to_u64().unwrap_or(0);
    let vega_best_ask = BigUint::parse_bytes(md.best_offer_price.as_bytes(), 10).unwrap().to_u64().unwrap_or(0);
    if vega_best_bid == 0 || vega_best_ask == 0 {
        info!("At least one vega price is NOT +ve! Either error or prices not updated yet.");
        return
    }
    info!(
        "new Vega reference prices: bestBid({}), bestAsk({})", vega_best_bid, vega_best_ask);

    // we now conservatively process the reference prices: 
    // best ask we take the bigger one
    let used_ask = std::cmp::max(binance_best_ask, vega_best_ask);
    // best bid we take the smaller one
    let used_bid = std::cmp::min(binance_best_bid, vega_best_bid);

    if used_ask <= 0 || used_bid <= 0 {
        info!("reference price are not up to date yet");
        return;
    }
    info!(
        "reference prices to use: bid: {}, ask: {}", used_bid, used_ask);
    

    let position_size = match store.lock().unwrap().get_position(&*w1.public_key()) {
        Some(p) => p.open_volume,
        None => 0,
    }; 

    info!("Position size: {}", position_size);

    let mut lambd = c.lambd;
    let mut kappa = c.kappa;
    if !store.lock().unwrap().get_trades().is_empty() {
        //let last_trade_timestamp = store.lock().unwrap().get_trades().last().unwrap().timestamp;
        //info!("Trying trade store stuff...{}", last_trade_timestamp);
        
        // Get the current time
        let current_time = SystemTime::now();

        // Get the duration since the UNIX epoch
        let duration_since_epoch = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");

        // Get the duration in nanoseconds
        let current_t = duration_since_epoch.as_nanos() as u64;

        let estimation_interval = Duration::from_secs(30*60).as_nanos() as u64;

        store.lock().unwrap().prune_trades_older_than(current_t - estimation_interval);

        let trades = store.lock().unwrap().get_trades().clone();
        lambd = estimate_lambda2(lambd, current_t, estimation_interval, &trades);
        kappa = estimate_kappa(kappa, current_t, estimation_interval, &trades, d.price_factor);
        info!("Lambda estimate: {}, Kappa estimate: {}", lambd, kappa);
    }

    let (buy_deltas, sell_deltas) = opt_offsets::calculate_offsets(c.q_lower, c.q_upper, kappa, lambd, c.phi, d.position_factor);
    let (ask_offset, submit_asks, bid_offset, submit_bids) =
            opt_offsets::offsets_from_position(buy_deltas, sell_deltas, c.q_lower, c.q_upper, position_size);


    let vega_best_bid = vega_best_bid as f64 / d.price_factor;
    let vega_best_ask = vega_best_ask as f64 / d.price_factor;
    let used_bid = used_bid as f64 / d.price_factor;
    let used_ask = used_ask as f64 / d.price_factor;
    let used_mid_price = ((used_ask + used_bid)/2.0) as f64;
    let worst_bid_offset = (used_mid_price as f64) * (c.price_range_factor - 0.005); // we remove 0.5 % from what's allowed to be more safely inside
    let worst_ask_offset = (used_mid_price as f64) * (c.price_range_factor - 0.005);        

    if submit_asks {
        info!("Submitting sells: {}, at offset: {} in % at offset: {}%", submit_asks, ask_offset, 100.0*ask_offset/used_mid_price);
    }
    else {
        info!("Submitting worst sells at offset: {}, i.e. price level: {}", worst_ask_offset, used_mid_price as f64 + worst_ask_offset);
    }

    if submit_bids {
        info!("Submitting buys: {} at offset: {} in % at offset: {}%", submit_bids, bid_offset, 100.0*bid_offset/used_mid_price);
    }
    else {
        info!("Submitting worst buys at offset: {}, i.e. price level: {}", worst_bid_offset, used_mid_price as f64 - worst_bid_offset);
    }
    
    let batch_w1 = Command::BatchMarketInstructions(
        get_batch(
            c.vega_market.clone(),
            used_mid_price,
            used_bid,
            vega_best_bid,
            bid_offset,
            submit_bids,
            worst_bid_offset,
            used_ask,
            vega_best_ask,
            ask_offset,
            submit_asks,
            worst_ask_offset,
            c.levels,
            c.step,
            c.volume_of_notional,
            &d,
            c.use_mid,
        )
    );
    if !c.dryrun {
        match w1.send(batch_w1).await {
            Ok(o) => info!("w1 result: {:?}", o),
            Err(e) => info!("w1 transaction error: {:?}", e),
        };
    }
    else {
        info!("dryrun mode, no transaction submitted")
    }
}


fn get_batch(
    market_id: String,
    mid_price: f64,
    best_bid: f64,
    vega_best_bid: f64, 
    bid_offset: f64,
    submit_bids: bool,
    worst_bid_offset: f64,
    best_ask: f64,
    vega_best_ask: f64,
    ask_offset: f64,
    submit_asks: bool,
    worst_ask_offset: f64,
    num_levels: u64,
    step: f64,
    volume_of_notional: u64,
    d: &Decimals,
    use_mid: bool,
) -> BatchMarketInstructions {
    
    
    let (tif, typ) = (TimeInForce::Gtt, Type::Limit);
    
    // Get the current time
    let current_time = SystemTime::now();

    // Get the duration since the UNIX epoch
    let duration_since_epoch = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");

    // Add some time to current time
    let duration_since_epoch_plus_extra = duration_since_epoch + Duration::from_secs(20);

    // Get the duration in nanoseconds
    let expires_at = duration_since_epoch_plus_extra.as_nanos() as i64;

    let mut orders: Vec<OrderSubmission> = vec![];
    
    // lets setup the buy side orders
    let side = Side::Buy;
    let mut ref_price = best_bid;
    if use_mid {
        ref_price = mid_price;
    }
    if submit_bids {
        let offset = bid_offset;
        for i in 0..=num_levels-1 {
            let price = (ref_price - offset - (i as f64 * step)).min(vega_best_ask - 1.0/d.price_factor);
            let size_f = get_order_size_mm_linear(i, volume_of_notional, num_levels, price);
            let size = (size_f * d.position_factor).ceil() as u64;
            let price_sub = (price * d.price_factor) as i64;

            info!("ref: {}, order: buy {:.3} @ {:.3}, at position and price decimals: {} @ {}", 
                        (ref_price.clone().to_f64().unwrap()), 
                        size_f, 
                        price, 
                        size, 
                        price_sub);

            orders.push(OrderSubmission {
                expires_at: expires_at,
                market_id: market_id.clone(),
                pegged_order: None,
                price: price_sub.to_string(),
                size: size,
                reference: "".to_string(),
                side: side.into(),
                time_in_force: tif.into(),
                r#type: typ.into(),
                reduce_only: false,
                post_only: true,
                iceberg_opts: None,
            });
        }
    }
    else {
        let offset = worst_bid_offset;
        let price = (ref_price - offset).min(vega_best_ask - 1.0/d.price_factor);
        let price_sub = (price * d.price_factor) as i64;
        let size_f = get_order_size_mm(volume_of_notional, 1, price);
        let size = (size_f * d.position_factor).ceil() as u64;
        info!("ref: {}, order: buy {:.3} @ {:.3}, at position and price decimals: {} @ {}", 
                        (ref_price.clone().to_f64().unwrap()), 
                        size_f, 
                        price, 
                        size, 
                        price_sub);

        orders.push(OrderSubmission {
            expires_at: expires_at,
            market_id: market_id.clone(),
            pegged_order: None,
            price: price_sub.to_string(),
            size: size,
            reference: "".to_string(),
            side: side.into(),
            time_in_force: tif.into(),
            r#type: typ.into(),
            reduce_only: false,
            post_only: true,
            iceberg_opts: None,
        });
    }
    
    let side = Side::Sell;
    ref_price = best_ask;
    if use_mid {
        ref_price = mid_price;
    }
    if submit_asks {
        let offset = ask_offset;
        for i in 0..=num_levels-1 {
            let price = (ref_price + offset + (i as f64 * step)).max(vega_best_bid + 1.0/d.price_factor);
            let price_sub = (price * d.price_factor) as i64;
            let size_f = get_order_size_mm_linear(i, volume_of_notional, num_levels, price);
            let size = (size_f * d.position_factor).ceil() as u64;
            info!("ref: {}, order: sell {:.3} @ {:.3}, at position and price decimals: {} @ {}", 
                        (ref_price.clone().to_f64().unwrap()), 
                        size_f, 
                        price, 
                        size, 
                        price_sub);

            orders.push(OrderSubmission {
                expires_at: expires_at,
                market_id: market_id.clone(),
                pegged_order: None,
                price: price_sub.to_string(),
                size: size as u64,
                reference: "".to_string(),
                side: side.into(),
                time_in_force: tif.into(),
                r#type: typ.into(),
                reduce_only: false,
                post_only: true,
                iceberg_opts: None,
            });
        }
    }
    else {
        let offset = worst_ask_offset;
        let price = (ref_price + offset).max(vega_best_bid + 1.0);
        let price_sub = (price * d.price_factor) as i64;
        let size_f = get_order_size_mm(volume_of_notional, 1, price);
        let size = (size_f * d.position_factor).ceil() as u64;
        info!("mid: {}, order: sell {:.3} @ {:.3}, at position and price decimals: {} @ {}", 
                        (ref_price.clone().to_f64().unwrap()), 
                        size_f, 
                        price, 
                        size, 
                        price_sub);

        orders.push(OrderSubmission {
            expires_at: expires_at,
            market_id: market_id.clone(),
            pegged_order: None,
            price: price_sub.to_string(),
            size: size,
            reference: "".to_string(),
            side: side.into(),
            time_in_force: tif.into(),
            r#type: typ.into(),
            reduce_only: false,
            post_only: true,
            iceberg_opts: None,
        });
    }



    return BatchMarketInstructions {
        cancellations: vec![OrderCancellation {
            order_id: "".to_string(),
            market_id: market_id.clone(),
        }],
        amendments: vec![],
        submissions: orders,
        stop_orders_cancellation: vec![],
        stop_orders_submission: vec![],
    };
}

fn get_close_batch(market_id: String) -> BatchMarketInstructions {
    return BatchMarketInstructions {
        cancellations: vec![OrderCancellation {
            order_id: "".to_string(),
            market_id: market_id.clone(),
        }],
        amendments: vec![],
        submissions: vec![],
        stop_orders_cancellation: vec![],
        stop_orders_submission: vec![],
    };
}

fn get_liquidity_submission_transaction(market_id: String, 
    commitment_amt: f64, 
    fee: f64, 
    reference: String,
    d: &Decimals,
) -> LiquidityProvisionSubmission {
    let commitment_amt_s = ((commitment_amt * d.asset_factor).floor() as i64).to_string();
    return LiquidityProvisionSubmission { market_id: market_id, 
        commitment_amount: commitment_amt_s,
        fee: fee.to_string(),
        reference: reference,
    };
}

fn get_liquidity_amendment_transaction(market_id: String, 
    commitment_amt: f64, 
    fee: f64, 
    reference: String,
    d: &Decimals,
) -> LiquidityProvisionAmendment {
    let commitment_amt_s = ((commitment_amt * d.asset_factor).floor() as i64).to_string();
    return LiquidityProvisionAmendment { market_id: market_id, 
        commitment_amount: commitment_amt_s,
        fee: fee.to_string(),
        reference: reference,
    };
}


fn get_liquidity_cancellation_transaction(market_id: String) -> LiquidityProvisionCancellation {
    return LiquidityProvisionCancellation { market_id: market_id };
}

fn get_order_size_mm(
    volume_of_notional: u64,
    num_levels: u64,
    mid_price: f64,
) -> f64 {
    // volume_of_notional = price x size x num_levels
    // so size = volume_of_notional / price / num_levels 
    
    //info!("asset_factor: {}, position_factor: {}", d.asset_factor, d.position_factor); 
    let size = (volume_of_notional as f64) / mid_price / (num_levels as f64);

    //info!("volume on notional: {}, mid price: {}, num_levels: {}", volume_of_notional, mid_price, num_levels);

    // we include a buffer to be sure we're meeting
    let buffer = 0.1;
    let size = size * (1.0+buffer);
    return size;
}

// the point is to return the correct size of each i so that with the given
// num_levels, price and step_size we hit the volume of notional. 
fn get_order_size_mm_linear(
    i: u64,
    volume_of_notional: u64,
    num_levels: u64,
    price: f64,
) -> f64 {
    let buffer = 0.1;
    // times two because the triangle needs 2x height to match size of rectangle
    let height = (1.0+buffer) * (volume_of_notional as f64) / price / (num_levels as f64) * 2.0; 
    let frac = (i+1) as f64 / num_levels as f64; 
    return frac * height;
}


fn get_asset(mkt: &Market) -> String {
    match mkt
        .clone()
        .tradable_instrument
        .unwrap()
        .instrument
        .unwrap()
        .product
        .unwrap()
    {
        Product::Future(f) => f.settlement_asset,
        Product::Spot(_) => unimplemented!("spot market not supported"),
        Product::Perpetual(f) => f.settlement_asset,
    }
}

struct Decimals {
    position_decimal_places: i64,
    price_decimal_places: u64,
    asset_decimal_places: u64,
    position_factor: f64,
    price_factor: f64,
    asset_factor: f64,
}

impl Decimals {
    fn new(mkt: &Market, asset: &Asset) -> Decimals {
        return Decimals {
            position_decimal_places: mkt.position_decimal_places,
            price_decimal_places: mkt.decimal_places,
            asset_decimal_places: asset.details.as_ref().unwrap().decimals,
            position_factor: (10_f64).powf(mkt.position_decimal_places as f64),
            price_factor: (10_f64).powf(mkt.decimal_places as f64),
            asset_factor: (10_f64).powf(asset.details.as_ref().unwrap().decimals as f64),
        };
    }

    fn from_asset_precision(&self, amount: f64) -> f64 {
        return amount / self.asset_factor;
    }

    fn from_market_price_precision(&self, price: f64) -> f64 {
        return price / self.price_factor;
    }

    fn from_market_position_precision(&self, position: f64) -> f64 {
        return position / self.position_factor;
    }

    fn to_market_price_precision(&self, price: f64) -> f64 {
        return price * self.price_factor;
    }

    fn to_market_position_precision(&self, position: f64) -> f64 {
        return position * self.position_factor;
    }
}

