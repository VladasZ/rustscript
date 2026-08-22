#!/usr/bin/env rust

// Unwinding runs drops innermost frame first. The panic is inside a spawned
// task so the program still exits cleanly.

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
