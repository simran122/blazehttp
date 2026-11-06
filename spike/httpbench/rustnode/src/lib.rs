// Throwaway prototype: a Rust (reqwest/hyper) HTTP engine exposed to Node via napi-rs.
// All network I/O, HTTP parsing and body reading run on a dedicated Rust runtime
// (RUST_HTTP_THREADS worker threads, default 1), off Node's main thread.
use napi_derive::napi;
use std::sync::{Mutex, OnceLock};
use tokio::runtime::Runtime;
use tokio::sync::Notify;

struct Engine {
    rt: Runtime,
    client: reqwest::Client,
    done: Mutex<Vec<Completed>>,
    notify: Notify,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let threads = std::env::var("RUST_HTTP_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build().unwrap();
        let client = {
            let _guard = rt.enter();
            reqwest::Client::builder().pool_max_idle_per_host(256).build().unwrap()
        };
        Engine { rt, client, done: Mutex::new(Vec::new()), notify: Notify::new() }
    })
}

#[napi(object)]
pub struct Completed {
    pub id: u32,
    pub status: u32,
    pub body: String,
}

/// Variant A: one JS promise per request (one main-thread wake-up per response).
#[napi]
pub async fn get(url: String) -> napi::Result<String> {
    let e = engine();
    let client = e.client.clone();
    e.rt
        .spawn(async move { client.get(url).send().await?.text().await })
        .await
        .map_err(|err| napi::Error::from_reason(err.to_string()))?
        .map_err(|err| napi::Error::from_reason(err.to_string()))
}

/// Variant B, part 1: queue a request; returns immediately.
#[napi]
pub fn submit(id: u32, url: String) {
    let e = engine();
    let client = e.client.clone();
    e.rt.spawn(async move {
        let (status, body) = match client.get(url).send().await {
            Ok(res) => {
                let status = u32::from(res.status().as_u16());
                (status, res.text().await.unwrap_or_default())
            }
            Err(err) => (0, err.to_string()),
        };
        let e = engine();
        e.done.lock().unwrap().push(Completed { id, status, body });
        e.notify.notify_one();
    });
}

/// Variant B, part 2: resolves with ALL responses finished since the last call
/// (one main-thread wake-up per batch).
#[napi]
pub async fn drain() -> Vec<Completed> {
    let e = engine();
    loop {
        {
            let mut done = e.done.lock().unwrap();
            if !done.is_empty() {
                return std::mem::take(&mut *done);
            }
        }
        e.notify.notified().await;
    }
}
