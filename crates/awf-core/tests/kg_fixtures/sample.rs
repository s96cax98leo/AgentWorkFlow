mod util;
use crate::core::{Base, helper as h};
use std::collections::HashMap;

pub trait Runner: Marker {
    fn run(&self);
}

pub struct Server {
    name: String,
}

impl Server {
    pub fn new() -> Self {
        Server { name: String::new() }
    }
    fn step(&self) -> usize { h(1) }
}

impl Runner for Server {
    // @awf:billing/export-invoice
    fn run(&self) {
        self.step();
        util::assist(&self.name);
    }
}

pub fn main() {
    let s = Server::new();
    s.run();
    let _m: HashMap<String, Base> = HashMap::new();
}
