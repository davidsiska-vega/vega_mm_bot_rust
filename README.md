## configuration example for the BTC/USD-PERP market:

For both wallet_mnemonic_1 and 2 the first derived key will be used

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
