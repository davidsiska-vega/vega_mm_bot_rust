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
