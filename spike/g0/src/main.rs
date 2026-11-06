use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); System.alloc(l) }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); System.realloc(p, l, n) }
}
#[global_allocator]
static A: Counting = Counting;

#[allow(dead_code)] #[derive(serde::Deserialize)] struct Item { i: u32, label: String, active: bool }
#[allow(dead_code)] #[derive(serde::Deserialize)] struct Payload { id: u32, name: String, items: Vec<Item> }
#[allow(dead_code)] #[derive(serde::Deserialize)] struct ItemB<'a> { i: u32, #[serde(borrow)] label: &'a str, active: bool }
#[allow(dead_code)] #[derive(serde::Deserialize)] struct PayloadB<'a> { id: u32, #[serde(borrow)] name: &'a str, #[serde(borrow)] items: Vec<ItemB<'a>> }

const ADDR: &str = "127.0.0.1:3999";
const URL: &str = "http://127.0.0.1:3999/data";
const REQ: &[u8] = b"GET /data HTTP/1.1\r\nhost: 127.0.0.1:3999\r\n\r\n";

fn check(b: &[u8]) { let p: Payload = serde_json::from_slice(b).unwrap(); assert_eq!(p.id, 42); }

async fn raw_conn(next: Arc<AtomicUsize>, n: usize) {
    let mut s = tokio::net::TcpStream::connect(ADDR).await.unwrap();
    s.set_nodelay(true).unwrap();
    let mut buf = vec![0u8; 65536];
    loop {
        if next.fetch_add(1, Relaxed) >= n { break; }
        s.write_all(REQ).await.unwrap();
        let mut len = 0;
        let (head, cl) = loop {
            let r = s.read(&mut buf[len..]).await.unwrap(); assert!(r > 0); len += r;
            let mut h = [httparse::EMPTY_HEADER; 16];
            let mut resp = httparse::Response::new(&mut h);
            if let httparse::Status::Complete(hl) = resp.parse(&buf[..len]).unwrap() {
                let cl = resp.headers.iter().find(|h| h.name.eq_ignore_ascii_case("content-length"))
                    .map(|h| std::str::from_utf8(h.value).unwrap().parse::<usize>().unwrap()).unwrap();
                break (hl, cl);
            }
        };
        while len < head + cl { let r = s.read(&mut buf[len..]).await.unwrap(); assert!(r > 0); len += r; }
        check(&buf[head..head + cl]);
    }
}

async fn hyper_conn(next: Arc<AtomicUsize>, n: usize) {
    use http_body_util::{BodyExt, Empty};
    let s = tokio::net::TcpStream::connect(ADDR).await.unwrap();
    s.set_nodelay(true).unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(s)).await.unwrap();
    tokio::spawn(conn);
    loop {
        if next.fetch_add(1, Relaxed) >= n { break; }
        let req = hyper::Request::get("/data").header("host", "127.0.0.1:3999").body(Empty::<bytes::Bytes>::new()).unwrap();
        let res = tx.send_request(req).await.unwrap();
        let b = res.into_body().collect().await.unwrap().to_bytes();
        check(&b);
    }
}

async fn reqwest_conn(c: reqwest::Client, next: Arc<AtomicUsize>, n: usize) {
    loop {
        if next.fetch_add(1, Relaxed) >= n { break; }
        let b = c.get(URL).send().await.unwrap().bytes().await.unwrap();
        check(&b);
    }
}

fn run_async(mode: &str, conc: usize, n: usize) -> f64 {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let rc = { let _g = rt.enter(); reqwest::Client::builder().pool_max_idle_per_host(1000).build().unwrap() };
    rt.block_on(async {
        let next = Arc::new(AtomicUsize::new(0));
        let t = Instant::now();
        let mut hs = vec![];
        for _ in 0..conc {
            let nx = next.clone();
            hs.push(match mode {
                "raw" => tokio::spawn(raw_conn(nx, n)),
                "hyper" => tokio::spawn(hyper_conn(nx, n)),
                _ => tokio::spawn(reqwest_conn(rc.clone(), nx, n)),
            });
        }
        for h in hs { h.await.unwrap(); }
        t.elapsed().as_secs_f64()
    })
}

fn run_ureq(conc: usize, n: usize) -> f64 {
    let agent: ureq::Agent = ureq::Agent::config_builder().max_idle_connections(1000).max_idle_connections_per_host(1000).build().into();
    let next = Arc::new(AtomicUsize::new(0));
    let t = Instant::now();
    let hs: Vec<_> = (0..conc).map(|_| { let a = agent.clone(); let nx = next.clone(); std::thread::spawn(move || loop {
        if nx.fetch_add(1, Relaxed) >= n { break; }
        let b = a.get(URL).call().unwrap().body_mut().read_to_vec().unwrap();
        check(&b);
    })}).collect();
    for h in hs { h.join().unwrap(); }
    t.elapsed().as_secs_f64()
}

fn cpu() -> f64 { let st = std::fs::read_to_string("/proc/self/stat").unwrap(); let f: Vec<&str> = st.rsplit(')').next().unwrap().split_whitespace().collect(); (f[11].parse::<f64>().unwrap() + f[12].parse::<f64>().unwrap()) / 100.0 }
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args[1].as_str();
    let conc: usize = args[2].parse().unwrap();
    let n: usize = args[3].parse().unwrap();
    if mode == "serde" {
        let body = std::fs::read("/bench/body.json").unwrap();
        let a0 = ALLOCS.load(Relaxed);
        for _ in 0..1000 { check(&body); }
        let a1 = ALLOCS.load(Relaxed);
        for _ in 0..1000 { let p: PayloadB = serde_json::from_slice(&body).unwrap(); assert_eq!(p.id, 42); }
        println!("serde owned allocs/parse {:.1}, borrowed {:.1}", (a1 - a0) as f64 / 1000.0, (ALLOCS.load(Relaxed) - a1) as f64 / 1000.0);
        return;
    }
    let go = |m: &str, c, k| if m == "ureq" { run_ureq(c, k) } else { run_async(m, c, k) };
    go(mode, conc, n / 10); // warmup
    let a0 = ALLOCS.load(Relaxed);
    let c0 = cpu();
    let secs = go(mode, conc, n);
    let al = ALLOCS.load(Relaxed) - a0;
    let cs = cpu() - c0;
    println!("{mode} c={conc} rps={:.0} cpu_us/req={:.2} allocs/req={:.1}", n as f64 / secs, cs * 1e6 / n as f64, al as f64 / n as f64);
}
