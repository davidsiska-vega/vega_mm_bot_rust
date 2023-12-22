use clap::Parser;
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
mod binance_ws;
//mod strategy;
mod strategy2;
mod vega_store2;
mod opt_offsets;
mod estimate_params;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the configuration
    #[arg(long, default_value_t = String::from("config.json"))]
    config: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    port: u16,
    vega_grpc_url: String,
    binance_ws_url: String,
    wallet_mnemonic_1: String,
    vega_market: String,
    binance_market: String,
    trade_size: i64,
    volume_of_notional: u64,
    q_lower: i64,
    q_upper: i64,
    kappa: f64,
    lambd: f64,
    phi: f64,
    submission_rate: u64,
    dryrun: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pretty_env_logger::init();

    //exit(-1);

    let cli = Cli::parse();

    let data = fs::read_to_string(&*cli.config).expect("unable to read configuration file");
    let config: Config = serde_json::from_str(&data).expect("unable to parse configuration file");

    let w1 = Transact::new(
        Credentials::Mnemonic(&config.wallet_mnemonic_1, 1),
        config.vega_grpc_url.clone(),
    )
    .await?;
    info!("loaded wallet 1 with address {}", w1.public_key());

    let rp = Arc::new(Mutex::new(binance_ws::RefPrice::new()));

    tokio::spawn(binance_ws::start(
        config.binance_ws_url.clone(),
        config.binance_market.clone(),
        rp.clone(),
    ));

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

    // tokio::spawn(strategy2::start(
    //     w1.clone(),
    //     config.trade_size,
    //     config.vega_market.clone(),
    //     vstore.clone(),
    //     rp.clone(),
    //     config.volume_of_notional,
    //     config.q_lower,
    //     config.q_upper,
    //     config.kappa,
    //     config.lambd,
    //     config.phi,
    //     config.submission_rate,
    // ));

    tokio::spawn(strategy2::start(
        w1.clone(),
        config.clone(),
        vstore.clone(),
        rp.clone(),
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

    // tokio::spawn(api::start(cli.port, vstore.clone(), rp.clone()));
}
