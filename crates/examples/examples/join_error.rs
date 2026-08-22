#!/usr/bin/env rust

// A panicked task's `handle.await` answers a real `JoinError`.

fn doomed() {
    panic!("boom");
}

#[tokio::main]
async fn main() {
    let handle = tokio::spawn(async {
        doomed();
    });
    let joined = handle.await;
    match joined {
        Ok(v) => println!("unexpected: {v:?}"),
        Err(e) => {
            println!("{e}");
            println!("{e:?}");
            println!("{} {}", e.is_panic(), e.is_cancelled());
        }
    }
    println!("end of main");
}
