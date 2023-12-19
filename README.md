# A simple market making bot for vega 

Forked from [this price taking bot](https://github.com/jeremyletang/niceish_price_bot) and heavily reliant on [vega rust sdk](https://github.com/jeremyletang/vega-rust-sdk).

## Building 

Get rust and cargo setup on your platform. 
Use `cd` to get into the same dir as this `README.md`. 
Run
```
cargo build
```
You will see some warnings. One day they'll get cleaned up. 

## Configuration example for the BTC/USD-PERP market:

Can be found in `config-sample.json`. You'll need to put your mnemonic in the json file. Do you trust this code? 

## Running 

Inspect the code and make sure it's not sending your key mnemonic somewhere (are you really, really sure)?!?  

Run
```
RUST_LOG=info ./target/debug/basic_mm_bot --config=../secrets/config-btcusd.json 
```
