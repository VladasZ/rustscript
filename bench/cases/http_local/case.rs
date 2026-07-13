use std::env;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();
    let base = &args[1];
    let requests: i64 = args[2].parse().unwrap();
    let t = Instant::now();
    let client = reqwest::blocking::Client::new();
    let mut ids: i64 = 0;
    let mut values: i64 = 0;
    for i in 0..requests {
        let url = format!("{base}/item/{i}");
        let item: serde_json::Value = client.get(url).send().unwrap().json().unwrap();
        ids += item["id"].as_i64().unwrap();
        values += item["value"].as_i64().unwrap();
    }
    let ns = t.elapsed().as_nanos();
    println!("requests={requests} ids={ids} values={values}");
    eprintln!("COMPUTE_NS {ns}");
}
