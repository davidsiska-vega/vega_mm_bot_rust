use futures_util::lock::MutexGuard as FuturesUtilsMutexGuard;
use log::info;
use num_traits::ToPrimitive;
use vega_protobufs::vega::events::v1::ExpiredOrders;
use std::time::{SystemTime, UNIX_EPOCH};
use num_bigint::BigUint;
use num_traits::cast::FromPrimitive;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;


use vega_crypto::Transact;
use vega_protobufs::vega::{
    commands::v1::{
        input_data::Command, BatchMarketInstructions, OrderCancellation, OrderSubmission,
    },
    instrument::Product,
    order::{TimeInForce, Type},
    Market, Side,
};
use vega_protobufs::vega::{Asset, Position};

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

    if !config.dryrun {
        info!("closing all positions");
        match w1
            .send(Command::BatchMarketInstructions(get_close_batch(
                config.vega_market.clone(),
            )))
            .await
        {
            Ok(o) => info!("w1 close batch result: {:?}", o),
            Err(e) => info!("w1 close batch transaction error: {:?}", e),
        };
    }
    else {
        info!("drurun mode, at this stage would submit a close transaction"); 
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


    let w1_position_size = match store.lock().unwrap().get_position(&*w1.public_key()) {
        Some(p) => p.open_volume,
        None => 0,
    }; 


    info!("Position size: {}", w1_position_size);

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

    let (buy_deltas, sell_deltas) = opt_offsets::calculate_offsets(c.q_lower, c.q_upper, kappa, lambd, c.phi);
    let (ask_offset, submit_asks, bid_offset, submit_bids) =
            opt_offsets::offsets_from_position(buy_deltas, sell_deltas, c.q_lower, c.q_upper, w1_position_size);

    if submit_asks {
        info!("Submitting sells: {}, at offset: {}", submit_asks, ask_offset);
    }
    if submit_bids {
        info!("Submitting buys: {} at offset: {}", submit_bids, bid_offset);
    }
    
    let num_levels = 3;
    let order_size = get_order_size_mm(
                            &d, 
                            w1_position_size, 
                            c.volume_of_notional, 
                            num_levels, 
                            (used_bid as f64 + used_ask as f64)/2.0/d.price_factor
                        );

    info!("Orders size: {}", order_size);
    if order_size == 0 || order_size > 10 {
        info!("order size is: {} not between 1 and 10" , order_size);
        return;
    }

    let batch_w1 = Command::BatchMarketInstructions(
        get_batch(
            c.vega_market.clone(),
            ((used_ask + used_bid)/2) as i64,
            used_bid as i64,
            vega_best_bid as i64,
            bid_offset,
            submit_bids,
            used_ask as i64,
            vega_best_ask as i64,
            ask_offset,
            submit_asks,
            order_size,
            num_levels,
            &d,
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
    mid_price: i64,
    best_bid: i64,
    vega_best_bid: i64, 
    bid_offset: f64,
    submit_bids: bool,
    best_ask: i64,
    vega_best_ask: i64,
    ask_offset: f64,
    submit_asks: bool,
    mut size: i64,
    num_levels: u64,
    d: &Decimals,
) -> BatchMarketInstructions {
    
    if size <= 0 {
        panic!("Size must be positive.");
    }

    let (tif, typ) = (TimeInForce::Gtt, Type::Limit);
    
    // Get the current time
    let current_time = SystemTime::now();

    // Get the duration since the UNIX epoch
    let duration_since_epoch = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");

    // Add some time to current time
    let duration_since_epoch_plus_extra = duration_since_epoch + Duration::from_secs(20);

    // Get the duration in nanoseconds
    let expires_at = duration_since_epoch_plus_extra.as_nanos() as i64;
    
    let step = 1;

    let mut orders: Vec<OrderSubmission> = vec![];
    
    // lets setup the buy side orders
    if submit_bids {
        let side = Side::Buy;
        //let offset = f64::max(0.0 as f64,bid_offset * d.price_factor) as u64;
        let offset = (bid_offset * d.price_factor) as i64;
        for i in 0..=num_levels-1 {
            let price = (mid_price - offset - ((i * step) as i64)).min(vega_best_ask - 1);
            let price_f = price.to_f64().unwrap() / d.price_factor;
            info!("mid: {}, order: buy {}@{}", (mid_price.clone().to_f64().unwrap() / d.price_factor), size, price_f);
            orders.push(OrderSubmission {
                expires_at: expires_at,
                market_id: market_id.clone(),
                pegged_order: None,
                price: price.to_string(),
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
    
    if submit_asks {
        let side = Side::Sell;
        //let offset = f64::max(0.0 as f64,ask_offset * d.price_factor) as u64;
        let offset = (ask_offset * d.price_factor) as i64;
        for i in 0..=num_levels-1 {
            let price = (mid_price + offset + ((i * step) as i64)).max(vega_best_bid + 1);
            let price_f = price.to_f64().unwrap() / d.price_factor;
            info!("mid: {}, order: sell {}@{}", (mid_price.clone().to_f64().unwrap() / d.price_factor), size, price_f);
            orders.push(OrderSubmission {
                expires_at: expires_at,
                market_id: market_id.clone(),
                pegged_order: None,
                price: price.to_string(),
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
    d: &Decimals,
    position_size: i64, 
    volume_of_notional: u64,
    num_levels: u64,
    mid_price: f64,
) -> i64 {
    // volume_of_notional = price x size x num_levels
    // so size = volume_of_notional / price / num_levels 
    
    //info!("asset_factor: {}, position_factor: {}", d.asset_factor, d.position_factor); 
    let size = (volume_of_notional as f64) / mid_price / (num_levels as f64);

    //info!("volume on notional: {}, mid price: {}, num_levels: {}", volume_of_notional, mid_price, num_levels);

    // we include a buffer to be sure we're meeting
    let buffer = 0.1;
    let size = size * (1.0+buffer);
    return (size * d.position_factor) as i64;
}


// fn get_order_submission(
//     d: &Decimals,
//     ref_price: f64,
//     side: vega_wallet_client::commands::Side,
//     market_id: String,
//     target_volume: f64,
// ) -> Vec<vega_wallet_client::commands::OrderSubmission> {
//     use vega_wallet_client::commands::{OrderSubmission, OrderType, Side, TimeInForce};

//     let size = target_volume / 5. * ref_price;

//     fn price_buy(ref_price: f64, f: f64) -> f64 {
//         ref_price * (1f64 - (f * 0.002))
//     }

//     fn price_sell(ref_price: f64, f: f64) -> f64 {
//         ref_price * (1f64 + (f * 0.002))
//     }

//     let price_f: fn(f64, f64) -> f64 = match side {
//         Side::Buy => price_buy,
//         Side::Sell => price_sell,
//         _ => panic!("should never happen"),
//     };

//     let mut orders: Vec<OrderSubmission> = vec![];
//     for i in vec![1, 2, 3, 4, 5].into_iter() {
//         let p =
//             BigUint::from_f64(d.to_market_price_precision(price_f(ref_price, i as f64))).unwrap();

//         orders.push(OrderSubmission {
//             market_id: market_id.clone(),
//             price: p.to_string(),
//             size: d.to_market_position_precision(size) as u64,
//             side,
//             time_in_force: TimeInForce::Gtc,
//             expires_at: 0,
//             r#type: OrderType::Limit,
//             reference: "VEGA_RUST_MM_SIMPLE".to_string(),
//             pegged_order: None,
//         });
//     }

//     return orders;
// }

// fn get_pubkey_balance(
//     store: Arc<Mutex<VegaStore>>,
//     pubkey: String,
//     asset_id: String,
//     d: &Decimals,
// ) -> f64 {
//     d.from_asset_precision(store.lock().unwrap().get_accounts().iter().fold(
//         0f64,
//         |balance, acc| {
//             if acc.asset != asset_id || acc.owner != pubkey {
//                 balance
//             } else {
//                 balance + acc.balance.parse::<f64>().unwrap()
//             }
//         },
//     ))
// }

// // return vol, aep
// fn volume_and_average_entry_price(d: &Decimals, pos: &Option<Position>) -> (f64, f64) {
//     if let Some(p) = pos {
//         let vol = p.open_volume as f64;
//         let aep = p.average_entry_price.parse::<f64>().unwrap();
//         return (
//             d.from_market_position_precision(vol),
//             d.from_market_price_precision(aep),
//         );
//     }

//     return (0., 0.);
// }

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
    position_factor: f64,
    price_factor: f64,
    asset_factor: f64,
}

impl Decimals {
    fn new(mkt: &Market, asset: &Asset) -> Decimals {
        return Decimals {
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

// async fn run_strategy(
//     clt: &WalletClient,
//     pubkey: String,
//     market: String,
//     store: Arc<Mutex<VegaStore>>,
//     rp: Arc<Mutex<RefPrice>>,
// ) {
//     info!("executing trading strategy...");
//     let mkt = store.lock().unwrap().get_market();
//     let asset = store.lock().unwrap().get_asset(get_asset(&mkt));

//     info!(
//         "updating quotes for {}",
//         mkt.tradable_instrument
//             .as_ref()
//             .unwrap()
//             .instrument
//             .as_ref()
//             .unwrap()
//             .name
//     );

//     let d = Decimals::new(&mkt, &asset);

//     let (best_bid, best_ask) = rp.lock().unwrap().get();
//     info!(
//         "new reference prices: bestBid({}), bestAsk({})",
//         best_bid, best_ask
//     );

//     let (open_volume, aep) =
//         volume_and_average_entry_price(&d, &store.lock().unwrap().get_position());

//     let balance = get_pubkey_balance(store.clone(), pubkey.clone(), asset.id.clone(), &d);
//     info!("pubkey balance: {}", balance);

//     let bid_volume = balance * 0.5 - open_volume * aep;
//     let offer_volume = balance * 0.5 + open_volume * aep;
//     let notional_exposure = (open_volume * aep).abs();
//     info!(
//         "openvolume({}), entryPrice({}), notionalExposure({})",
//         open_volume, aep, notional_exposure,
//     );
//     info!("bidVolume({}), offerVolume({})", bid_volume, offer_volume);

//     use vega_wallet_client::commands::{BatchMarketInstructions, OrderCancellation, Side};

//     let mut submissions = get_order_submission(&d, best_bid, Side::Buy, market.clone(), bid_volume);
//     submissions.append(&mut get_order_submission(
//         &d,
//         best_ask,
//         Side::Sell,
//         market.clone(),
//         offer_volume,
//     ));
//     let batch = BatchMarketInstructions {
//         cancellations: vec![OrderCancellation {
//             market_id: market.clone(),
//             order_id: "".to_string(),
//         }],
//         amendments: vec![],
//         submissions,
//     };

//     info!("batch submission: {:?}", batch);
//     clt.send(batch).await.unwrap();
// }
