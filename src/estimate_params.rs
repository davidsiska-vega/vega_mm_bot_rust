use log::info;
use vega_protobufs::vega::Trade;
use vega_protobufs::vega::trade;

use crate::vega_store2::VegaStore;
use crate::vega_store2::TradeStat;


// We are assuming aggresive orders (and thus trades occuring) happens according to a Poisson distribution
// with parameter lambda trades per minute. 
// The unbiased estimator is: if we had n minutes of observations handy and in each minute i=1,..,,n 
// we observed k_i trades arriving the lambda = (1/n) \sum_{i=1}^n k_i. 
pub fn estimate_lambda(initial_lambd: f64, 
                        trades: &Vec<TradeStat>) -> f64 {
    
    if trades.is_empty() || trades.len() == 1 {
        return initial_lambd; // no trades or one trade, can't do much
    }

    const ONE_MINUTE: u64 = 60 * 1_000_000_000;
    let mut t_per_m: Vec<u64> = Vec::new();

    // we will iterate backwards hence our interest in the last timestamp
    let mut init_t = trades.last().unwrap().timestamp;
    let mut t_this_min: u64 = 0;
    for t in trades.iter().rev().skip(1) {
        println!("timestamp {}", t.timestamp);
        t_this_min = t_this_min + 1;
        if (init_t - t.timestamp >= ONE_MINUTE) {
            println!("a minute passed with prev trade time {} and current trade time {}", init_t, t.timestamp);
            t_per_m.push(t_this_min);
            init_t = t.timestamp;
            t_this_min = 0;
        }
    }

    let mut sum = 0 as u64;
    for i in 0..t_per_m.len() {
        sum += t_per_m[i];
        println!("min {}, trades {}", i, t_per_m[i])
    }
    
    return (sum as f64) / (t_per_m.len() as f64);
}    


pub fn estimate_lambda2(initial_lambd: f64, 
        current_t: u64, // current time 
        estimation_interval: u64, // period over which to consider trades
        trades: &Vec<TradeStat>) -> f64 {
    if trades.is_empty()  {
        return initial_lambd; // no trades or one trade, can't do much
    }
    let mut count_trades_in_interval = 0 as u64;
    let mut actual_interval = 0 as u64;
    for t in trades.iter().rev() {
        actual_interval = current_t - t.timestamp;
        if actual_interval <= estimation_interval {
            count_trades_in_interval += 1;
        }
        else {
            break;
        }
    }

    if count_trades_in_interval == 0 {
        return initial_lambd;
    }

    const ONE_MINUTE: u64 = 60 * 1_000_000_000;
    let mins_in_interval = actual_interval as f64 / (ONE_MINUTE as f64);
    let lambd = count_trades_in_interval as f64 / mins_in_interval;
    return lambd;
}

// Biased but maximum likelyhood estimator of kappa
pub fn estimate_kappa(initial_kappa: f64, 
    current_t: u64, // current time 
    estimation_interval: u64, // period over which to consider trades
    trades: &Vec<TradeStat>,
    price_factor: f64) -> f64 {
    
    if trades.is_empty()  {
        return initial_kappa; // no trades or one trade, can't do much
    }
    
    let mut sum_mid_price_diffs = 0 as f64; 
    let mut trade_count = 0 as u64;
    for t in trades.iter().rev() {
        if (current_t - t.timestamp) <= estimation_interval {
            let mid = (t.block_best_ask + t.block_best_bid) / 2.0;
            let mid_to_price_diff = f64::abs(t.price - mid) / price_factor;
            //info!("mid: {}, trade: {}, diff: {}", mid/price_factor, t.price/price_factor, mid_to_price_diff); 
            sum_mid_price_diffs += mid_to_price_diff;
            trade_count += 1;
        }
        else {
            break;
        }
    }
    if trade_count == 0 {
        // we've not seen any trades that would qualify, we have no data
        return initial_kappa;
    }
    let kappa_mle = (trade_count as f64) / sum_mid_price_diffs;
    return kappa_mle;

}


pub enum BuySellUnsure {
    Buy, 
    Sell,
    Unsure,
}

pub fn trade_is_buy(t: TradeStat) -> BuySellUnsure {
    let mut buy_sell_unsure: BuySellUnsure = BuySellUnsure::Unsure;
    if t.price >= t.block_best_ask && t.price >= t.block_best_bid {
        buy_sell_unsure = BuySellUnsure::Buy;
    } else if t.price <= t.block_best_ask && t.price <= t.block_best_bid {
        buy_sell_unsure = BuySellUnsure::Sell; 
    } 
    return buy_sell_unsure;
}


mod tests {
    use super::*;

    pub fn generate_trades_unif(start_t: u64, end_t: u64, num_trades: u64) -> Vec<TradeStat> {
        let mut trades_vec: Vec<TradeStat> = Vec::new();
        assert_ne!(num_trades,0);
        assert_ne!(num_trades,1);
        let timestep = (end_t - start_t) / (num_trades-1);
        for i in 0..=num_trades-1 {
            let new_trade = TradeStat {
                timestamp: i * timestep,
                price: 98.0,
                size: 1,
                block_best_bid: 99.0,
                block_best_ask: 101.0,
            };
            trades_vec.push(new_trade)
        }
        return trades_vec;
    }

    #[test]
    fn test_estimate_lambda_empty_input() {
        //let empty_trades_vec: Vec<TradeStat> = Vec::new();
        let empty_trades_vec = Vec::new();
        let lambd = estimate_lambda(1.0, &empty_trades_vec);
        assert_eq!(1.0 as f64, 1.0 as f64);
    }

    #[test]
    fn test_estimate_lambda_one_per_minute() {
        let empty_trades_vec = generate_trades_unif(0, 600000000000, 10);
        let lambd = estimate_lambda(1.0, &empty_trades_vec);
        assert_eq!(lambd, 1.0 as f64);
    }

    #[test]
    fn test_estimate_lambda2_one_per_minute() {
        let trades = generate_trades_unif(0, 600000000000, 10);
        let lambd = estimate_lambda2(1.0, 600000000000, 600000000000, &trades);
        assert_eq!(lambd, 1.0 as f64);
    }

}