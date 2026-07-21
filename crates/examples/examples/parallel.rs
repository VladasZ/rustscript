#!/usr/bin/env rust

use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let start = Instant::now();
    let mut handles = Vec::new();
    for task in 0..10 {
        handles.push(tokio::spawn(async move {
            for i in 0..=20 {
                println!("task {task}: {i}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    println!("done in {}ms", start.elapsed().as_millis());
}
