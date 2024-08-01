use log::{error, info};
use reqwest::Error;
use serde::Deserialize;
use tokio::time::{sleep, Duration};
use chrono::{NaiveDateTime, Utc, TimeZone};
use std::sync::{Arc, Mutex};

use crate::ref_price::RefPrice;

#[derive(Deserialize, Debug)]
struct ApiResponse {
    retCode: i32,
    retMsg: String,
    result: ResultData,
    retExtInfo: serde_json::Value,
    time: u64,
}

#[derive(Deserialize, Debug)]
struct ResultData {
    s: String,
    a: Vec<[String; 2]>,
    b: Vec<[String; 2]>,
    ts: u64,
    u: u64,
    seq: u64,
    cts: u64,
}



async fn fetch_order_book(bybit_url: &String, bybit_symbol: &String) -> Result<ApiResponse, reqwest::Error> {
    let bybit_url_owned = String::from(bybit_url);
    let url = bybit_url_owned + "/v5/market/orderbook?category=spot&symbol=" + bybit_symbol;
    info!("Reading Bybit prices from {}", url);
    //let url = "https://api.bybit.com/v5/market/orderbook?category=spot&symbol=VEGAUSDT";
    let response = reqwest::get(url).await?.json::<ApiResponse>().await?;
    Ok(response)
}

pub async fn start(bybit_url: String, mkt: String, rp: Arc<Mutex<RefPrice>>, sleep_in_millis: u64)  {
    loop {
        match fetch_order_book(&bybit_url, &mkt).await {
            Ok(order_book) => {
                //println!("{:#?}", order_book);
                let best_ask_str = &order_book.result.a[0][0];
                let best_bid_str = &order_book.result.b[0][0];
                
                // Convert the best bid price to f64
                let best_ask: f64 = match best_ask_str.parse() {
                    Ok(val) => val,
                    Err(e) => {
                        info!("Error parsing best ask price: {}", e);
                        return;
                    }
                };

                // Convert the best bid price to f64
                let best_bid: f64 = match best_bid_str.parse() {
                    Ok(val) => val,
                    Err(e) => {
                        info!("Error parsing best bid price: {}", e);
                        return;
                    }
                };

                let mid = 0.5*(best_ask+best_bid);
                let spread = best_ask - best_bid;
                let spread_in_bp = 10_000.0*(best_ask - best_bid)/mid;
                
                // Convert Unix timestamp to NaiveDateTime
                let naive_datetime = NaiveDateTime::from_timestamp((order_book.time/1000) as i64, 0);

                // Convert NaiveDateTime to DateTime<Utc>
                let datetime = Utc.from_utc_datetime(&naive_datetime);

                info!("Binance best ask {:.4}; best bid {:.4}; spread {:.5} which is {:.1} bp at {}", best_ask, best_bid, spread,spread_in_bp, datetime.to_rfc3339());

                rp.lock().unwrap().set(best_bid, best_ask);
            }
            Err(e) => {
                eprintln!("Error fetching binance order book: {}", e);
            }
        }
        sleep(Duration::from_millis(sleep_in_millis)).await;
    }
}
