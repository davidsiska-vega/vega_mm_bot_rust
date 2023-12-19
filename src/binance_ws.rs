use log::info;
use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::fmt;
use std::sync::{Arc, Mutex};
use tungstenite::{connect, Message};
use url::Url;

pub struct RefPrice {
    bid_price: f64,
    ask_price: f64,
}

impl RefPrice {
    pub fn new() -> RefPrice {
        return RefPrice {
            bid_price: 0.,
            ask_price: 0.,
        };
    }

    pub fn set(&mut self, bid_price: f64, ask_price: f64) {
        self.bid_price = bid_price;
        self.ask_price = ask_price;
    }

    pub fn get(&self) -> (f64, f64) {
        return (self.bid_price, self.ask_price);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    id: u64,
    method: String,
    params: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Response {
    pub e: String,
    pub a: String,
    pub b: String,
}

pub async fn start(ws_url: String, mkt: String, rp: Arc<Mutex<RefPrice>>) -> Result<(), Error> {
    let url = ws_url.parse::<Url>()?;
    info!("opening websocket with binance API at: {}", url);
    let (mut socket, _) = connect(url)?;
    info!("connected to binance successfully");

    let request = serde_json::to_string(&Request {
        id: 1,
        method: "SUBSCRIBE".to_string(),
        params: vec![format!("{}@ticker", mkt.to_lowercase())],
    })?;

    socket.write_message(Message::Text(request))?;

    // discard first message, it's confirmation from binance
    socket.read_message()?;
    loop {
        let msg = socket.read_message()?;
        match serde_json::from_str::<Response>(&msg.to_string()) {
            Ok(r) => {
                if r.e == "24hrTicker" {
                    //info!("new binance prices: {:?}", r);
                    rp.lock()
                        .unwrap()
                        .set(r.b.parse::<f64>().unwrap(), r.a.parse::<f64>().unwrap());
                }
            }
            _ => continue,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    WSError,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wallet client error: {}", self.desc())
    }
}

impl From<tungstenite::Error> for Error {
    fn from(_: tungstenite::Error) -> Self {
        Error::WSError
    }
}

impl From<url::ParseError> for Error {
    fn from(_: url::ParseError) -> Self {
        Error::WSError
    }
}

impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Error::WSError
    }
}

impl StdError for Error {}

impl Error {
    pub fn desc(&self) -> String {
        use Error::*;
        match self {
            WSError => format!("websocket error"),
        }
    }
}
