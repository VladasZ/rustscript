#!/usr/bin/env rust

// Panic unwinding runs user drops for every live local, innermost frame
// first, like real Rust. The panic happens inside a spawned task so the
// program itself still exits cleanly and compiled and interpreted stdout
// can be compared byte for byte.

struct Guard {
    name: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        println!("dropping {}", self.name);
    }
}

fn doomed() {
    let _outer = Guard {
        name: "outer".to_string(),
    };
    let _inner = Guard {
        name: "inner".to_string(),
    };
    println!("about to panic");
    panic!("boom");
}

#[tokio::main]
async fn main() {
    let handle = tokio::spawn(async {
        doomed();
    });
    let joined = handle.await;
    println!("task panicked: {}", joined.is_err());
    println!("end of main");
}
