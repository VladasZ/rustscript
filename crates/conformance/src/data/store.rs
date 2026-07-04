use super::models::{Item, Kind};

pub struct Store {
    items: Vec<Item>,
}

impl Store {
    pub fn new() -> Store {
        Store { items: Vec::new() }
    }

    pub fn add(&mut self, item: Item) {
        self.items.push(item);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn count_kind(&self, kind: Kind) -> usize {
        self.items.iter().filter(|i| i.kind == kind).count()
    }

    pub fn items(&self) -> &Vec<Item> {
        &self.items
    }
}
