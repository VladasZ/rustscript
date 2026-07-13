use std::env;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let tasks: i64 = args[1].parse().unwrap();
    let yields_per_task: i64 = 100;
    let t = Instant::now();
    let mut handles = Vec::new();
    for value in 0..tasks {
        handles.push(tokio::spawn(async move {
            for _ in 0..yields_per_task {
                tokio::task::yield_now().await;
            }
            value
        }));
    }
    let mut checksum: i64 = 0;
    for handle in handles {
        checksum += handle.await.unwrap();
    }
    let ns = t.elapsed().as_nanos();
    println!(
        "tasks={tasks} yields={} checksum={checksum}",
        tasks * yields_per_task
    );
    eprintln!("COMPUTE_NS {ns}");
}
