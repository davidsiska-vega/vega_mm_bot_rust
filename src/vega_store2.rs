use log::{error, info};
use lru::LruCache;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::fmt;
use std::sync::{Arc, Mutex};
use tokio_stream::StreamExt;
use tonic;
use vega_protobufs::datanode::api::v2::GetLatestMarketDataRequest;
use vega_protobufs::vega::MarketData;

const TRADES_CACHE: usize = 10000;

use vega_protobufs::{
    datanode::api::v2::{
        trading_data_service_client::TradingDataServiceClient, GetMarketRequest, ListAssetsRequest,
        ObserveMarketsDataRequest, ObservePositionsRequest, ObserveTradesRequest,
    },
    vega::{Asset, Market, Position, Trade},
};

#[derive(Clone)]
pub struct TradeStat {
    pub timestamp: i64,
    pub price: String,
    pub size: u64,
    pub block_best_bid: Option<String>,
    pub block_best_ask: Option<String>,
}

pub struct VegaStore {
    market: Market,
    market_data: MarketData,
    previous_market_data: LruCache<i64, MarketData>,
    trades: Vec<TradeStat>,
    positions: HashMap<String, Position>,
    // key = asset ID
    assets: HashMap<String, Asset>,
}

impl VegaStore {
    pub async fn new(
        clt: &mut TradingDataServiceClient<tonic::transport::Channel>,
        mkt_id: &str,
    ) -> Result<VegaStore, Error> {
        // info!("1");
        let mkt_resp = clt
            .get_market(GetMarketRequest {
                market_id: mkt_id.to_string(),
            })
            .await?;

        let market = mkt_resp.get_ref().market.as_ref().unwrap().clone();

        info!("market found: {:?}", market,);
        let mktd_resp = clt
            .get_latest_market_data(GetLatestMarketDataRequest {
                market_id: mkt_id.to_string(),
            })
            .await?;

        let market_data = mktd_resp.get_ref().market_data.as_ref().unwrap().clone();
        info!("market data found: {:?}", market);

        let assets_resp = clt
            .list_assets(ListAssetsRequest {
                asset_id: None,
                pagination: None,
            })
            .await?;

        let mut assets = HashMap::new();
        for a in assets_resp.get_ref().assets.as_ref().unwrap().edges.iter() {
            let asset = a.node.as_ref().unwrap();
            assets.insert(asset.id.clone(), asset.clone());
        }

        let positions: HashMap<String, Position> = HashMap::new();

        let mut previous_market_data: LruCache<i64, MarketData> =
            LruCache::new(core::num::NonZeroUsize::new(1000).unwrap());

        previous_market_data.put(market_data.timestamp, market_data.clone());
        return Ok(VegaStore {
            market,
            market_data,
            previous_market_data,
            assets,
            positions,
            trades: vec![],
        });
    }

    pub fn get_market(&self) -> Market {
        return self.market.clone();
    }

    pub fn get_market_data(&self) -> MarketData {
        return self.market_data.clone();
    }

    pub fn get_asset(&self, id: String) -> Asset {
        return self.assets[&id].clone();
    }

    pub fn get_position(&self, party_id: &str) -> Option<Position> {
        return self.positions.get(party_id).cloned();
    }

    pub fn get_assets(&self) -> Vec<Asset> {
        return self.assets.clone().into_values().collect();
    }

    pub fn save_positions(&mut self, positions: Vec<Position>) {
        for p in positions.into_iter() {
            self.positions.insert(p.party_id.clone(), p.clone());
        }
    }

    pub fn save_market_data(&mut self, md: MarketData) {
        self.update_trades(&md);
        self.previous_market_data.put(md.timestamp, md.clone());
        self.market_data = md
    }

    pub fn update_trades(&mut self, md: &MarketData) {
        for t in self.trades.iter_mut() {
            if t.timestamp == md.timestamp {
                t.block_best_ask = Some(md.best_offer_price.clone());
                t.block_best_bid = Some(md.best_bid_price.clone());
            }
        }
    }

    pub fn save_trade(&mut self, trade: &Trade) {
        if self.trades.len() > TRADES_CACHE {
            self.trades.remove(0);
        }

        let (best_bid, best_ask) = match self.previous_market_data.get(&trade.timestamp) {
            Some(md) => (
                Some(md.best_bid_price.clone()),
                Some(md.best_offer_price.clone()),
            ),
            None => (None, None),
        };
        self.trades.push(TradeStat {
            timestamp: trade.timestamp,
            price: trade.price.clone(),
            size: trade.size,
            block_best_ask: best_ask,
            block_best_bid: best_bid,
        });
    }

    pub fn get_trades(&self) -> Vec<TradeStat> {
        return self.trades.clone();
    }
}

pub fn update_forever(
    store: Arc<Mutex<VegaStore>>,
    clt: TradingDataServiceClient<tonic::transport::Channel>,
    market: &str,
    pubkey1: &str,
) {
    tokio::spawn(update_market_data_forever(
        store.clone(),
        clt.clone(),
        market.to_string(),
    ));
    tokio::spawn(update_position_forever(
        store.clone(),
        clt.clone(),
        market.to_string(),
        pubkey1.to_string(),
    ));
    tokio::spawn(update_trades_forever(
        store.clone(),
        clt.clone(),
        market.to_string(),
        pubkey1.to_string(),
    ));
}

async fn update_market_data_forever(
    store: Arc<Mutex<VegaStore>>,
    mut clt: TradingDataServiceClient<tonic::transport::Channel>,
    market: String,
) {
    // use vega_protobufs::datanode::api::v2::observe_markets_data_response=
    info!("starting market_data stream for party: {}...", &*market);
    let mut stream = match clt
        .observe_markets_data(ObserveMarketsDataRequest {
            market_ids: vec![market],
        })
        .await
    {
        Ok(s) => s.into_inner(),
        Err(e) => panic!("{:?}", e),
    };

    while let Some(item) = stream.next().await {
        match item {
            Ok(resp) => {
                for md in resp.market_data.iter() {
                    store.lock().unwrap().save_market_data(md.clone());
                }
            }
            Err(e) => {
                error!("could not load market data: {} - {}", e, e.message());
            }
        }
    }
}

async fn update_position_forever(
    store: Arc<Mutex<VegaStore>>,
    mut clt: TradingDataServiceClient<tonic::transport::Channel>,
    market: String,
    pubkey: String,
) {
    use vega_protobufs::datanode::api::v2::observe_positions_response::Response;
    info!("starting positions stream for party: {}...", &*pubkey);
    let mut stream = match clt
        .observe_positions(ObservePositionsRequest {
            party_id: Some(pubkey),
            market_id: Some(market),
        })
        .await
    {
        Ok(s) => s.into_inner(),
        Err(e) => panic!("{:?}", e),
    };

    while let Some(item) = stream.next().await {
        match item {
            Ok(resp) => match resp.response {
                Some(r) => match r {
                    Response::Snapshot(o) => {
                        store.lock().unwrap().save_positions(o.positions.clone())
                    }
                    Response::Updates(o) => {
                        store.lock().unwrap().save_positions(o.positions.clone())
                    }
                },
                _ => {}
            },
            Err(e) => {
                error!("could not load position: {} - {}", e, e.message());
            }
        }
    }
}

async fn update_trades_forever(
    store: Arc<Mutex<VegaStore>>,
    mut clt: TradingDataServiceClient<tonic::transport::Channel>,
    market: String,
    pubkey: String,
) {
    info!("starting positions stream for party: {}...", &*pubkey);
    let mut stream = match clt
        .observe_trades(ObserveTradesRequest {
            party_ids: vec![pubkey],
            market_ids: vec![market],
        })
        .await
    {
        Ok(s) => s.into_inner(),
        Err(e) => panic!("{:?}", e),
    };

    while let Some(item) = stream.next().await {
        match item {
            Ok(resp) => {
                for t in resp.trades.iter() {
                    store.lock().unwrap().save_trade(t);
                }
            }
            Err(e) => {
                error!("could not load position: {} - {}", e, e.message());
            }
        }
    }
}

#[derive(Debug)]
pub enum Error {
    GrpcTransportError(tonic::transport::Error),
    GrpcError(tonic::Status),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wallet client error: {}", self.desc())
    }
}

impl From<tonic::transport::Error> for Error {
    fn from(error: tonic::transport::Error) -> Self {
        Error::GrpcTransportError(error)
    }
}

impl From<tonic::Status> for Error {
    fn from(error: tonic::Status) -> Self {
        Error::GrpcError(error)
    }
}
impl StdError for Error {}

impl Error {
    pub fn desc(&self) -> String {
        use Error::*;
        match self {
            GrpcTransportError(e) => format!("GRPC transport error: {}", e),
            GrpcError(e) => format!("GRPC error: {}", e),
        }
    }
}
