use serde::Deserialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const URL: &str = "http://127.0.0.1:3999/data";
const TOTAL: usize = 30_000;
const CONCURRENCY: usize = 50;
const WARMUP: usize = 3_000;

#[allow(dead_code)]
#[derive(Deserialize)]
struct Item { i: u32, label: String, active: bool }

#[allow(dead_code)]
#[derive(Deserialize)]
struct Payload { id: u32, name: String, items: Vec<Item> }

async fn run(client: &reqwest::Client, n: usize) -> (u64, f64, f64) {
    let next = Arc::new(AtomicUsize::new(0));
    let lat = Arc::new(Mutex::new(vec![0f64; n]));
    let start = Instant::now();
    let mut handles = Vec::with_capacity(CONCURRENCY);
    for _ in 0..CONCURRENCY {
        let (client, next, lat) = (client.clone(), next.clone(), lat.clone());
        handles.push(tokio::spawn(async move {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n { break; }
                let t = Instant::now();
                let p: Payload = client.get(URL).send().await.unwrap().json().await.unwrap();
                assert_eq!(p.id, 42);
                lat.lock().unwrap()[i] = t.elapsed().as_secs_f64() * 1000.0;
            }
        }));
    }
    for h in handles { h.await.unwrap(); }
    let elapsed = start.elapsed().as_secs_f64();
    let mut l = lat.lock().unwrap().clone();
    l.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ((n as f64 / elapsed) as u64, l[n / 2], l[n * 99 / 100])
}

// current_thread runtime = one CPU core, same as a Node process
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let client = reqwest::Client::builder().pool_max_idle_per_host(CONCURRENCY).build().unwrap();
    run(&client, WARMUP).await;
    let (rps, p50, p99) = run(&client, TOTAL).await;
    println!("{{\"name\":\"Rust reqwest\",\"rps\":{},\"p50\":\"{:.2}\",\"p99\":\"{:.2}\"}}", rps, p50, p99);
}
