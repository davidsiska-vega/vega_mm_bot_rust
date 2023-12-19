# A simple market making bot for vega 

Forked from [this price taking bot](https://github.com/jeremyletang/niceish_price_bot) and heavily reliant on [vega rust sdk](https://github.com/jeremyletang/vega-rust-sdk).

## Good to know

It so simple it will certainly lose money as it is now. 
There areonly two ways it will not lose money. 
1. Set the `dryrun` config parameter set to `true`. In that case it won't send any transactions. 
1. Run it against a vega testnet.

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
