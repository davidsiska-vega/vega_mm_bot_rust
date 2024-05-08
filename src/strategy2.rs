use futures_util::lock::MutexGuard as FuturesUtilsMutexGuard;
use log::info;
use num_traits::ToPrimitive;
use vega_protobufs::vega::events::v1::ExpiredOrders;
use core::num;
use std::thread::Thread;
use std::time::{SystemTime, UNIX_EPOCH};
use num_bigint::BigUint;
use num_traits::cast::FromPrimitive;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;
use rand::prelude::*;


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

#[derive(Debug, PartialEq)]
pub enum PositionSituation {
    Normal, 
    OnTheEdge,
    HardStop,
}

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
    }
    else {
        info!("drurun mode, at this stage would submit a close orders transaction"); 
    }

    let mut interval = time::interval(Duration::from_secs_f64(config.submission_rate));

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

    let mut binance_best_bid = 0 as u64;
    let mut binance_best_ask = 0 as u64; 

    if c.use_binance_bidask {
        let (binance_best_bid_f, binance_best_ask_f) = rp.lock().unwrap().get();
        if binance_best_ask_f <= 0.0 || binance_best_ask_f <= 0.0 {
            info!("At least one Binance price is NOT +ve! Either error or prices not updated yet.");
            return;
        }
        binance_best_bid = (binance_best_bid_f * d.price_factor) as u64;
        binance_best_ask = (binance_best_ask_f * d.price_factor) as u64;
        info!(
            "new Binance reference prices: bestBid({}), bestAsk({}))", binance_best_bid, binance_best_ask);
    }
    
    let md = store.lock().unwrap().get_market_data();
    
    let vega_best_bid = BigUint::parse_bytes(md.best_bid_price.as_bytes(), 10).unwrap().to_u64().unwrap_or(0);
    let vega_best_ask = BigUint::parse_bytes(md.best_offer_price.as_bytes(), 10).unwrap().to_u64().unwrap_or(0);
    if c.use_vega_bidask && (vega_best_bid == 0 || vega_best_ask == 0) {
        info!("At least one vega price is NOT +ve! Either error or prices not updated yet.");
        return
    }
    info!(
        "new Vega reference prices: bestBid({}), bestAsk({})", vega_best_bid, vega_best_ask);

    let used_ask: u64;
    let used_bid: u64;
    if c.use_vega_bidask && c.use_binance_bidask {
        // best ask we take the bigger one
        used_ask = std::cmp::max(binance_best_ask, vega_best_ask);
        // best bid we take the smaller one
        used_bid = std::cmp::min(binance_best_bid, vega_best_bid);    
    }
    else if c.use_vega_bidask && !c.use_binance_bidask {
        used_ask = vega_best_ask;
        used_bid = vega_best_bid;
    }
    else if !c.use_vega_bidask && c.use_binance_bidask {
        used_ask = binance_best_ask; 
        used_bid = binance_best_bid; 
    }
    else {
        info!("We must use one of Binance OR Vega OR max on ask side, min on bid side.");
        return;
    }

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
        kappa = estimate_kappa(kappa, c.kappa_weight, current_t, estimation_interval, &trades, d.price_factor);
        info!("Lambda estimate: {}, Kappa estimate: {}", lambd, kappa);
    }

    let (buy_deltas, sell_deltas) = opt_offsets::calculate_offsets(c.q_lower, c.q_upper, kappa, lambd, c.phi, d.position_factor);
    let (mut ask_offset, submit_asks, mut bid_offset, submit_bids) =
            opt_offsets::offsets_from_position(buy_deltas, sell_deltas, c.q_lower, c.q_upper, position_size);


    let vega_best_bid = vega_best_bid as f64 / d.price_factor;
    let vega_best_ask = vega_best_ask as f64 / d.price_factor;
    let used_bid = used_bid as f64 / d.price_factor;
    let used_ask = used_ask as f64 / d.price_factor;
    let used_mid_price = ((used_ask + used_bid)/2.0) as f64;
    let worst_bid_offset = (used_mid_price as f64) * (c.price_range_factor - 0.001); // we remove 0.5 % from what's allowed to be more safely inside
    bid_offset = bid_offset.min(worst_bid_offset);
    let worst_ask_offset = (used_mid_price as f64) * (c.price_range_factor - 0.001);        
    ask_offset = ask_offset.min(worst_ask_offset);
    
    let mut ask_side_situation: PositionSituation = PositionSituation::Normal; 
    
    info!("Position size: {}", position_size);
    if !submit_asks {
        // if the position is too negative we don't want to sell any more, so hard stop on ask side
        if (position_size as f64) < (c.q_lower as f64) * c.pos_lim_scaling {
            ask_side_situation = PositionSituation::HardStop;
            info!("Position: {} too negaitve, not submitting anything on ask side", position_size);
        }
        else {
            ask_side_situation = PositionSituation::OnTheEdge;
            info!("Submitting SLA range worst sells at offset: {}, i.e. price level: {}", worst_ask_offset, used_mid_price as f64 + worst_ask_offset);
        }
    }

    let mut bid_side_situation = PositionSituation::Normal;
    if !submit_bids {
        // if the position is too positive we don't want to buy anyone, so hard stop on bid side
        if (position_size as f64) > (c.q_upper as f64) * c.pos_lim_scaling {
            bid_side_situation = PositionSituation::HardStop;
            info!("Position: {} too positive, not submitting anything on bid side", position_size);
        }
        else {
            bid_side_situation = PositionSituation::OnTheEdge;
            info!("Submitting worst buys at offset: {}, i.e. price level: {}", worst_bid_offset, used_mid_price as f64 - worst_bid_offset);
        }    
    }
    

    
    let mut dispose_of_short_pos = false;
    if position_size <= c.dispose_q_lower {
        let random_number: f64 = rand::random();
        dispose_of_short_pos = random_number <= c.dispose_prob;
        info!("position too short, will try to buy: {}", dispose_of_short_pos);
    }
    
    let mut dispose_of_long_pos = false;
    if position_size >= c.dispose_q_upper {
        let random_number: f64 = rand::random();
        dispose_of_long_pos = random_number <= c.dispose_prob;
        info!("position too long, will try to sell: {}", dispose_of_long_pos);
    }
    


    let batch_w1 = Command::BatchMarketInstructions(
        get_batch(
            c.vega_market.clone(),
            used_mid_price,
            used_bid,
            vega_best_bid,
            bid_offset,
            bid_side_situation,
            worst_bid_offset,
            used_ask,
            vega_best_ask,
            ask_offset,
            ask_side_situation,
            worst_ask_offset,
            c.levels,
            c.step,
            c.tick_size,
            c.volume_of_notional,
            &d,
            c.use_mid,
            c.allow_negative_offset,
            c.gtt_length,
            dispose_of_short_pos,
            dispose_of_long_pos,
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
    bid_side_situation: PositionSituation,
    worst_bid_offset: f64,
    best_ask: f64,
    vega_best_ask: f64,
    ask_offset: f64,
    ask_side_situation: PositionSituation,
    worst_ask_offset: f64,
    num_levels: u64,
    step: f64,
    tick: f64,
    volume_of_notional: u64,
    d: &Decimals,
    use_mid: bool,
    allow_negative_offset: bool,
    gtt_length: u64,
    dispose_of_short_pos: bool,
    dispose_of_long_pos: bool,
) -> BatchMarketInstructions {
    
    
    let (tif, typ) = (TimeInForce::Gtt, Type::Limit);
    
    // Get the current time
    let current_time = SystemTime::now();

    // Get the duration since the UNIX epoch
    let duration_since_epoch = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");

    // Add some time to current time
    let duration_since_epoch_plus_extra = duration_since_epoch + Duration::from_secs(gtt_length);

    // Get the duration in nanoseconds
    let expires_at = duration_since_epoch_plus_extra.as_nanos() as i64;

    let mut orders: Vec<OrderSubmission> = vec![];
    
    let mut buy_side_ref_price = best_bid;
    let mut sell_side_ref_price = best_ask;
    if use_mid {
        buy_side_ref_price = mid_price;
        sell_side_ref_price = mid_price;
    }
    
    // lets setup the buy side orders
    let side = Side::Buy;
    if bid_side_situation == PositionSituation::Normal {
        let mut offset = bid_offset;
        if !allow_negative_offset && offset < 0.0 {
            offset = 0.0;
        }
        info!("Submitting buys at offset: {:.3} in % at offset: {:.3}%", offset, 100.0*offset/mid_price);
        for i in 0..=num_levels-1 {
            let mut price = buy_side_ref_price - offset - (i as f64 * step);
            if vega_best_ask > 0.0 {
                price = price.min(vega_best_ask - 1.0/d.price_factor)
            }
            
            //let size_f = get_order_size_mm_linear(i, volume_of_notional, num_levels, price);
            let size_f = get_order_size_mm_quadratic(i, volume_of_notional, num_levels, step,  - ((i as f64 + 1.0) * step), buy_side_ref_price);
            let size = (size_f * d.position_factor).ceil() as u64;
            let mut price_sub = (price * d.price_factor) as i64;
            price_sub -= price_sub % ((tick * d.price_factor) as i64);

            info!("ref: {}, order: buy {:.4} @ {:.3}, at position and price decimals: {} @ {}", 
                        (buy_side_ref_price.clone().to_f64().unwrap()), 
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
    else if bid_side_situation == PositionSituation::OnTheEdge {
        let offset = worst_bid_offset;
        let mut price = buy_side_ref_price - offset;
        if vega_best_bid > 0.0 {
            price = price.min(vega_best_ask - 1.0/d.price_factor);
        }
        
        let mut price_sub = (price * d.price_factor) as i64;
        price_sub -= price_sub % ((tick * d.price_factor) as i64);
        let size_f = get_order_size_mm(volume_of_notional, 1, price);
        let size = (size_f * d.position_factor).ceil() as u64;
        info!("ref: {}, order: buy {:.4} @ {:.3}, at position and price decimals: {} @ {}", 
                        (buy_side_ref_price.clone().to_f64().unwrap()), 
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
    if ask_side_situation == PositionSituation::Normal {
        let mut offset = ask_offset;
        if !allow_negative_offset && offset < 0.0 {
            offset = 0.0;
        }
        info!("Submitting sells at offset: {:.3} in % at offset: {:.3}%", offset, 100.0*offset/mid_price);
        for i in 0..=num_levels-1 {
            let mut price = sell_side_ref_price + offset + (i as f64 * step);
            if vega_best_ask > 0.0 {
                price = price.max(vega_best_bid + 1.0/d.price_factor);
            }
            
            let mut price_sub = (price * d.price_factor) as i64;
            price_sub -= price_sub % ((tick * d.price_factor) as i64);
            //let size_f = get_order_size_mm_linear(i, volume_of_notional, num_levels, price);
            let size_f = get_order_size_mm_quadratic(i, volume_of_notional, num_levels, step, (i as f64 + 1.0) * step, sell_side_ref_price);
            let size = (size_f * d.position_factor).ceil() as u64;
            info!("ref: {}, order: sell {:.4} @ {:.3}, at position and price decimals: {} @ {}", 
                        (sell_side_ref_price.clone().to_f64().unwrap()), 
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
    else if ask_side_situation == PositionSituation::OnTheEdge {
        let offset = worst_ask_offset;
        let mut price = sell_side_ref_price + offset;
        if vega_best_bid > 0.0 {
            price = price.max(vega_best_bid + 1.0)
        }
        
        let mut price_sub = (price * d.price_factor) as i64;
        price_sub -= price_sub % ((tick * d.price_factor) as i64);
        let size_f = get_order_size_mm(volume_of_notional, 1, price);
        let size = (size_f * d.position_factor).ceil() as u64;
        info!("mid: {}, order: sell {:.4} @ {:.3}, at position and price decimals: {} @ {}", 
                        (sell_side_ref_price.clone().to_f64().unwrap()), 
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
    
    if dispose_of_short_pos {
        // we're setting up on order that will reduce our position (or so we hope)
        let price = sell_side_ref_price + ask_offset - 1.0/d.price_factor;
        let mut price_sub = (price * d.price_factor) as i64;
        price_sub -= price_sub % ((tick * d.price_factor) as i64);
        let size = 1 as u64;
        let size_f = size as f64 / d.position_factor;
        info!("To reduce short position: buy {:.4} @ {:.3}, at position and price decimals: {} @ {}",  
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
            side: Side::Buy.into(),
            time_in_force: tif.into(),
            r#type: typ.into(),
            reduce_only: false,
            post_only: false,
            iceberg_opts: None,
        });
    }
    else if dispose_of_long_pos {
        // we're setting up on order that will reduce our position (or so we hope)
        let price = buy_side_ref_price - bid_offset + 1.0/d.price_factor;
        let mut price_sub = (price * d.price_factor) as i64;
        price_sub -= price_sub % ((tick * d.price_factor) as i64);
        let size = 1 as u64;
        let size_f = size as f64 / d.position_factor;
        info!("To reduce long position: sell {:.4} @ {:.3}, at position and price decimals: {} @ {}",  
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
            side: Side::Sell.into(),
            time_in_force: tif.into(),
            r#type: typ.into(),
            reduce_only: false,
            post_only: false,
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


// the point is to return the correct size of each i so that with the given
// num_levels, price and step_size we hit the volume of notional. 
fn get_order_size_mm_quadratic(
    i: u64,
    volume_of_notional: u64,
    num_levels: u64,
    step: f64,
    offset: f64,
    ref_price: f64,
) -> f64 {
    let buffer = 0.05;
    // times two because the triangle needs 2x height to match size of rectangle
    let delta = step * (num_levels as f64);
    let delta_cubed = delta * delta * delta; 
    let slope = step * 3.0 * (volume_of_notional as f64) / ref_price / delta_cubed;
    
    return (1.0+buffer) * slope * offset * offset;
}



pub fn get_asset(mkt: &Market) -> String {
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

pub struct Decimals {
    position_decimal_places: i64,
    price_decimal_places: u64,
    asset_decimal_places: u64,
    pub position_factor: f64,
    pub price_factor: f64,
    pub asset_factor: f64,
}

impl Decimals {
    pub fn new(mkt: &Market, asset: &Asset) -> Decimals {
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

