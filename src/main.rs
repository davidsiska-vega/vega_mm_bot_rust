use clap::{Arg, Parser};
use log::info;
use serde::{Deserialize, Serialize};
use std::fs;
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;
use vega_crypto::Credentials;
use vega_crypto::Transact;
use vega_protobufs::datanode::api::v2::trading_data_service_client::TradingDataServiceClient;
use vega_store2::update_forever;

// mod api;
mod ref_price;
mod binance_ws;
mod bybit_feed;
mod strategy2;
mod liquidity_vega;
mod vega_store2;
mod opt_offsets;
mod estimate_params;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the configuration
    #[arg(long, default_value_t = String::from("config.json"))]
    config: String,
    
    #[arg(long, default_value_t = false)]
    submit_liquidity: bool,
    
    #[arg(long, default_value_t = false)]
    cancel_liquidity: bool,
    
    #[arg(long, default_value_t = false)]
    amend_liquidity: bool,
}


#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    port: u16,
    vega_grpc_url: String,
    binance_ws_url: String,
    bybit_url: String,
    bybit_market: String,
    wallet_mnemonic_1: String,
    vega_market: String,
    binance_market: String,
    binance_price_scaling: f64,
    ref_price_scaling: f64,
    bond_amount: u64,
    lp_fee_bid: f64,
    volume_of_notional: u64,
    buy_to_sell_ratio: f64,
    levels: u64,
    step: f64,
    tick_size: f64,
    price_range_factor: f64,
    q_lower: i64,
    q_upper: i64,
    pos_lim_scaling: f64,
    kappa: f64,
    kappa_weight: f64,
    lambd: f64,
    phi: f64,
    use_mid: bool,
    use_vega_bidask: bool,
    use_binance_bidask: bool,
    use_bybit_bidask: bool,
    use_vega_trades: bool,
    allow_negative_offset: bool,
    gtt_length: u64,
    dispose_prob: f64,
    dispose_q_lower: i64,
    dispose_q_upper: i64,
    submission_rate: f64,
    dryrun: bool,
}

fn config_validation(c: Config) {
   if c.buy_to_sell_ratio < 1e-8 {
        panic!("buy to sell ratio must be >= 1e-8"); 
   }
   
   if c.binance_price_scaling < 0.0 {
        panic!("binance price scaling must be >= 0.0"); 
   }

   if c.ref_price_scaling <= 0.0 {
        panic!("price scaling must be > 0.0")
   }
   
   if c.lp_fee_bid < 0.0 {
        panic!("config file lp_fee_bid must be >= 0.0");
    }

    if c.step <= 0.0 {
        panic!("config file step must be > 0.0");
    }

    if c.tick_size <= 0.0 {
        panic!("config file tick_size must be > 0.0");
    }

    // We need step size to be a multiple of tick_size
    // let remainder_step_over_tick = c.step % c.tick_size;
    // if remainder_step_over_tick > 1e-12 {
    //     panic!("config file step size must be an integer multiple of tick_size");
    // }

    if c.price_range_factor <= 0.0 {
        panic!("config file price_range_factor must be > 0.0");
    }

    if c.q_lower >= c.q_upper {
        panic!("we need q_lower < q_upper");
    }

    if c.pos_lim_scaling < 1.0 {
        panic!("config file pos_lim_scaling must be >= 1.0");
    }

    if c.kappa <= 0.0 {
        panic!("config file kappa must be > 0.0");
    }

    if c.kappa_weight < 0.0 || c.kappa_weight > 1.0 {
        panic!("config file kappa_weight must be >= 0.0 and <= 1.0");
    }

    if c.lambd < 0.0 {
        panic!("config file lambd must be > 0.0");
    }

    if c.phi < 0.0 {
        panic!("config file phi must be > 0.0");
    }

    if c.submission_rate < 0.01 {
        panic!("config file submission_rate must be >= 0.01, otherwise you risk getting spam-banned as we don't increase PoW difficulty properly.");
    }

    if !c.use_binance_bidask && !c.use_vega_bidask && ! c.use_bybit_bidask {
        panic!("at the moment we need to use at least one of binance, bybit, vega bid/asks to set prices");
    }

    if c.dispose_prob < 0.0 || c.dispose_prob > 1.0 {
        panic!("dispose_prob must be in [0,1]");
    }

    if c.dispose_q_lower >= c.dispose_q_upper {
        panic!("we need dispose_q_lower < dispose_q_upper");
    }

}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pretty_env_logger::init();

    //exit(-1);

    let cli = Cli::parse();

    let data = fs::read_to_string(&*cli.config).expect("unable to read configuration file");
    let config: Config = serde_json::from_str(&data).expect("unable to parse configuration file");

    config_validation(config.clone());

    let w1 = Transact::new(
        Credentials::Mnemonic(&config.wallet_mnemonic_1, 1),
        config.vega_grpc_url.clone(),
    )
    .await?;
    info!("loaded wallet 1 with address {}", w1.public_key());
    
    let addr = config.vega_grpc_url.clone();
    let mut tdclt = TradingDataServiceClient::connect(addr).await?;
    let vstore = Arc::new(Mutex::new(
        vega_store2::VegaStore::new(&mut tdclt, &*config.vega_market).await?,
    ));
    update_forever(
        vstore.clone(),
        tdclt,
        &*config.vega_market,
        &*w1.public_key().clone(),
    );


    if cli.amend_liquidity {
        if cli.cancel_liquidity || cli.submit_liquidity {
            info!("can't do more than one of: amend, submit, cancel liquidity");
            exit(1);    
        }
        liquidity_vega::update_liquidity_commitment(
            w1.clone(),
            config.clone(),
            vstore.clone(),
        ).await;
        exit(0);
    } 
    else if cli.cancel_liquidity {
        if cli.amend_liquidity || cli.submit_liquidity {
            info!("can't do more than one of: amend, submit, cancel liquidity");
            exit(1);    
        }
        liquidity_vega::cancel_liquidity_commitment(
            w1.clone(),
            config.clone(),
            vstore.clone(),
        ).await;
        exit(0);
    }
    else if cli.submit_liquidity { 
        if cli.cancel_liquidity || cli.amend_liquidity {
            info!("can't do more than one of: amend, submit, cancel liquidity");
            exit(1);    
        }
        liquidity_vega::create_liquidity_commitment(
            w1.clone(),
            config.clone(),
            vstore.clone(),
        ).await;
        exit(0);
    }
    else {
        // keep previous reference price around to avoid sending too many transactions
        let old_rp = Arc::new(Mutex::new(ref_price::RefPrice::new()));
        
        let skip_counter_u = (config.gtt_length as f64 / config.submission_rate) as u64;
        let skip_counter = Arc::new(Mutex::new(skip_counter_u));

        // mutex store for binance data
        let binance_rp = Arc::new(Mutex::new(ref_price::RefPrice::new()));
        if config.use_binance_bidask {
            tokio::spawn(binance_ws::start(
                config.binance_ws_url.clone(),
                config.binance_market.clone(),
                binance_rp.clone(),
            ));    
        }

        // mutex store for bybit data
        let bybit_rp = Arc::new(Mutex::new(ref_price::RefPrice::new()));
        if config.use_bybit_bidask {
            tokio::spawn(bybit_feed::start(
                config.bybit_url.clone(),
                config.bybit_market.clone(),
                bybit_rp.clone(),
                1000,
            ));    
        }
        
        let mut rng = rand::thread_rng();
        tokio::spawn(strategy2::start(
            w1.clone(),
            config.clone(),
            vstore.clone(),
            binance_rp.clone(),
            bybit_rp.clone(),
            old_rp.clone(),
            skip_counter.clone(),
        ));


        // just loop forever, waiting for user interupt
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    interval.reset();
                }
            }
        }
    }

}
