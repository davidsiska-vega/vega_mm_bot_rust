# A simple market making bot for vega 

Forked from [this price taking bot](https://github.com/jeremyletang/niceish_price_bot) and heavily reliant on [vega rust sdk](https://github.com/jeremyletang/vega-rust-sdk).

## configuration example for the BTC/USD-PERP market:

Can be found in `config-sample.json`

```Json
{
    "port": 1789,
    "vega_grpc_url": "tcp://darling.network:3007",
    "binance_ws_url": "wss://stream.binance.com:443/ws",
    "vega_market": "4e9081e20e9e81f3e747d42cb0c9b8826454df01899e6027a22e771e19cc79fc",
    "binance_market": "BTCUSDT",
    "trade_size": 4,
    "wallet_mnemonic_1": "YOUR MNEMONIC FOR KEY 1",
    "wallet_mnemonic_2": "YOUR MNEMONIC FOR KEY 1",
    "submission_rate": 27
}
```
