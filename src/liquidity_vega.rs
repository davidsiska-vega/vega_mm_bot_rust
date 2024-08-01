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

use crate::{vega_store2::VegaStore};
use crate::{Config, vega_store2};
use crate::opt_offsets;
use crate::estimate_params::{self, estimate_lambda2, estimate_kappa};
use crate::strategy2::{Decimals, get_asset, MarketAsset};


pub async fn create_liquidity_commitment(
    mut w1: Transact,
    config: Config,
    store: Arc<Mutex<VegaStore>>,
) {
    
    let mkt = store.lock().unwrap().get_market();
    let asset = match get_asset(&mkt) {
        MarketAsset::Future(a) => store.lock().unwrap().get_asset(a),
        MarketAsset::Perpetual(a) => store.lock().unwrap().get_asset(a),
        MarketAsset::Spot(base, _quote) => store.lock().unwrap().get_asset(base),
    };
    let d = Decimals::new(&mkt, &asset);

    if !config.dryrun {
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
    else {
        info!("drurun mode, at this stage would create liquidity commitment"); 
    }
}

pub async fn cancel_liquidity_commitment(
    mut w1: Transact,
    config: Config,
    store: Arc<Mutex<VegaStore>>,
) {
    
    let mkt = store.lock().unwrap().get_market();
    let asset = match get_asset(&mkt) {
        MarketAsset::Future(a) => store.lock().unwrap().get_asset(a),
        MarketAsset::Perpetual(a) => store.lock().unwrap().get_asset(a),
        MarketAsset::Spot(base, _quote) => store.lock().unwrap().get_asset(base),
    };
    let d = Decimals::new(&mkt, &asset);

    if !config.dryrun {
        match w1
            .send(Command::LiquidityProvisionCancellation(get_liquidity_cancellation_transaction(
                config.vega_market.clone(),
            )))
            .await
        {
            Ok(o) => info!("cancel liquidity result: {:?}", o),
            Err(e) => info!("cancel liquidity error: {:?}", e),
        };
    }
    else {
        info!("drurun mode, at this stage would cancel liquidity"); 
    }
}

pub async fn update_liquidity_commitment(
    mut w1: Transact,
    config: Config,
    store: Arc<Mutex<VegaStore>>,
) {
    
    let mkt = store.lock().unwrap().get_market();
    let asset = match get_asset(&mkt) {
        MarketAsset::Future(a) => store.lock().unwrap().get_asset(a),
        MarketAsset::Perpetual(a) => store.lock().unwrap().get_asset(a),
        MarketAsset::Spot(base, _quote) => store.lock().unwrap().get_asset(base),
    };
    let d = Decimals::new(&mkt, &asset);

    if !config.dryrun {
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
            Ok(o) => info!("submit amend liquidity result: {:?}", o),
            Err(e) => info!("submit amend liquidity  error: {:?}", e),
        };
    }
    else {
        info!("drurun mode, at this stage would amend liquidity commitment"); 
    }
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
