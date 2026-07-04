#!/usr/bin/env rust

struct Account {
    owner: String,
    balance: i64,
}

impl Account {
    fn new(owner: &str) -> Account {
        Account {
            owner: owner.to_string(),
            balance: 0,
        }
    }

    fn deposit(&mut self, amount: i64) {
        self.balance += amount;
    }

    fn withdraw(&mut self, amount: i64) -> bool {
        if amount > self.balance {
            return false;
        }
        self.balance -= amount;
        true
    }
}

fn main() {
    let mut acc = Account::new("alice");
    acc.deposit(100);
    let ok = acc.withdraw(30);
    println!("{} has {} (withdraw ok: {ok})", acc.owner, acc.balance);
    let overdraw = acc.withdraw(1000);
    println!("overdraw allowed: {overdraw}");
}
