use log::info;


/*  The offsets come from the Avellanda / Stoikov optimal offsets with T \to \infty; 
    This should correspond to the ergodic formulation of the problem. 

    Args:
        q_upper: int, upper bound of the inventory of MM specified in the model
        q_lower: int, lower bound of the inventory of MM specified in the model
        kappa: float, market parameter to represnet the probability e^{-\kappa delta} of pegged LOs to be hit
        lambd: float, market parameter to represnet the coming rate of MOs
        phi: float, risk aversion parameter to represnet running penalty coefficient
    """
*/ 
pub fn calculate_offsets(
    q_lower: i64,
    q_upper: i64,
    kappa: f64,
    lambd: f64,
    phi: f64,
    position_factor: f64,
) -> (Vec<f64>, Vec<f64>) {
    if q_lower >= q_upper {
        panic!("we need q_lower < q_upper");
    }

    let mut buy_deltas: Vec<f64> = vec![];
    for i in q_lower..=q_upper-1 {
        const ONE: f64 = 1.0;
        let buy_delta = 1.0/kappa + (2.0 * (i as f64) + 1.0) * (phi * ONE.exp() / lambd / kappa).sqrt() / 2.0;
        buy_deltas.push(buy_delta);
    }

    let mut sell_deltas: Vec<f64> = vec![];
    for i in q_lower+1..=q_upper {
        const ONE: f64 = 1.0;
        let sell_delta = 1.0/kappa - (2.0 * (i as f64) - 1.0) * (phi * ONE.exp() / lambd / kappa).sqrt() / 2.0;
        sell_deltas.push(sell_delta);
    }
    let result = (buy_deltas, sell_deltas);
    return result;
}

pub fn offsets_from_position(
    buy_deltas: Vec<f64>,
    sell_deltas: Vec<f64>,
    q_lower: i64,
    q_upper: i64, 
    mut pos: i64,
) -> (f64, bool, f64, bool) {
    if q_lower >= q_upper {
        panic!("we need q_lower < q_upper");
    }

    if pos > q_upper {
        pos = q_upper;
    }
    else if pos < q_lower {
        pos = q_lower
    }

    let mut bid_offset = std::f64::MAX;
    let mut ask_offset = std::f64::MAX;

    let deltas_asks_idx = pos - q_lower;
    let mut submit_asks = true; // say position is -3, q_lower is -3 we get deltas_vec_idx = -3 + 3 = 0
    if deltas_asks_idx <= 0 || deltas_asks_idx > q_upper - q_lower {
        submit_asks = false; // say position is -4, q_lower is -3, we get deltas_vex_idx = -4 + 3 = -1, we don't want to go any lower 
    }
    else {
        ask_offset = sell_deltas[(deltas_asks_idx - 1) as usize];
    }
    let deltas_bids_idx = pos - q_lower;
    let mut submit_bids = true; // say position is 3, q_upper = q_lower is 3, we get deltas_vec_idx 3 + 3 = 6
    if deltas_bids_idx > q_upper - q_lower - 1 || deltas_bids_idx < 0 {
        submit_bids = false;
    }
    else {
        bid_offset = buy_deltas[deltas_bids_idx as usize];
    }
    return (ask_offset, submit_asks, bid_offset, submit_bids);
}


mod tests {
    use super::*;

    #[test]
    fn test_calculate_offsets() {
        // Test your calculate_offsets function here
        let q_lower = -3;
        let q_upper = 3;
        let kappa = 0.5;
        let lambd = 0.2;
        let phi = 0.1;

        let (buy_deltas, sell_deltas) = calculate_offsets(q_lower, q_upper, kappa, lambd, phi, 1.0);

        // Check we can run and get the right lengths
        assert_eq!(buy_deltas.len(), (q_upper - q_lower) as usize);
        assert_eq!(sell_deltas.len(), (q_upper - q_lower) as usize);
        
    }

    #[test]
    fn test_offsets_from_position() {
        // Test your offsets_from_position function here
        let q_lower = -3;
        let q_upper = 3;
        
        // first test 0 position 
        let pos = 0;
        let (buy_deltas, sell_deltas) = calculate_offsets(q_lower, q_upper, 0.5, 0.2, 0.1, 1.0);
        let (ask_offset, submit_asks, bid_offset, submit_bids) =
            offsets_from_position(buy_deltas, sell_deltas, q_lower, q_upper, pos);

        // With zero position we want to submit both asks and bids and we want the offsets equal
        assert_eq!(submit_asks, true);
        assert_eq!(submit_bids, true);
        assert_eq!(ask_offset, bid_offset);
        
        // test position at lower boundary 
        let pos: i64 = -3;
        let (buy_deltas, sell_deltas) = calculate_offsets(q_lower, q_upper, 0.5, 0.2, 0.1 , 1.0);
        let (ask_offset, submit_asks, bid_offset, submit_bids) =
            offsets_from_position(buy_deltas, sell_deltas, q_lower, q_upper, pos);

        // With pos at lower boundary we don't want to sell, we want to buy
        assert_eq!(submit_asks, false);  
        assert_eq!(submit_bids, true);
        let inv_low_bdry_bid = bid_offset;
        
        // test position below lower boundary 
        let pos: i64 = -4;
        let (buy_deltas, sell_deltas) = calculate_offsets(q_lower, q_upper, 0.5, 0.2, 0.1, 1.0);
        let (ask_offset, submit_asks, bid_offset, submit_bids) =
            offsets_from_position(buy_deltas, sell_deltas, q_lower, q_upper, pos);

        // With pos below lower boundary we don't want to sell, we want to buy
        assert_eq!(submit_asks, false);  
        assert_eq!(submit_bids, true);
        assert_eq!(bid_offset, inv_low_bdry_bid);

        // test position at upper boundary 
        let pos: i64 = 3;
        let (buy_deltas, sell_deltas) = calculate_offsets(q_lower, q_upper, 0.5, 0.2, 0.1, 1.0);
        let (ask_offset, submit_asks, bid_offset, submit_bids) =
            offsets_from_position(buy_deltas, sell_deltas, q_lower, q_upper, pos);

        // With pos at upper boundary we don't want to buy, we want to sell
        assert_eq!(submit_asks, true);  
        assert_eq!(submit_bids, false);
        assert_eq!(ask_offset, inv_low_bdry_bid); // and we want the extreme offsets matching

        // test position above upper boundary 
        let pos: i64 = 4;
        let (buy_deltas, sell_deltas) = calculate_offsets(q_lower, q_upper, 0.5, 0.2, 0.1, 1.0);
        let (ask_offset, submit_asks, bid_offset, submit_bids) =
            offsets_from_position(buy_deltas, sell_deltas, q_lower, q_upper, pos);

        // With pos above upper boundary we don't want to buy, we want to sell
        assert_eq!(submit_asks, true);  
        assert_eq!(submit_bids, false);
        assert_eq!(ask_offset, inv_low_bdry_bid); // and we want the extreme offsets matching


    }
}