#!/usr/bin/env rust

// `tokio::sync::Mutex` has no `Result` layer on `lock().await` and no
// poison field in debug.

use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let shared = Arc::new(Mutex::new(0i64));
    let mut handles = Vec::new();
    for i in 0..4 {
        let m = Arc::clone(&shared);
        handles.push(tokio::spawn(async move {
            let mut guard = m.lock().await;
            *guard += i;
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    println!("total {}", *shared.lock().await);

    let direct = Mutex::new(String::from("hi"));
    {
        let mut g = direct.lock().await;
        g.push_str(" there");
    }
    println!("{}", direct.lock().await);
    println!("{}", direct.try_lock().is_ok());
    println!("{direct:?}");
}
