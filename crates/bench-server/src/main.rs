//! `bench-server [--plain ADDR] [--tls ADDR] [--cert-out PATH] [--threads N] [--max-connections N] [--stall]
//! [--print-cells]`
//!
//! Listens on loopback (`127.0.0.1:8080`) unless an address is given; the harness passes `0.0.0.0:…` explicitly.
//! Prints `ready plain=<addr> [tls=<addr>]` on stdout once it accepts connections, after writing the TLS certificate
//! (PEM) to `--cert-out`. `--print-cells` prints the cell manifest (`bench/cells.json`) and exits.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::available_parallelism;

use bench_server::{Bound, ServerConfig, cells_json, serve};
use tokio::sync::oneshot;

const USAGE: &str = "usage: bench-server [--plain ADDR] [--tls ADDR] [--cert-out PATH] [--threads N] \
                     [--max-connections N] [--stall] [--print-cells]";

struct Args {
    config: ServerConfig,
    threads: usize,
    cert_out: Option<PathBuf>,
    print_cells: bool,
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.print_cells {
        print!("{}", cells_json());
        return ExitCode::SUCCESS;
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bench-server: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(mut words: impl Iterator<Item = String>) -> Result<Args, String> {
    let threads = available_parallelism().map_or(1, usize::from);
    let mut args = Args {
        config: ServerConfig::default(),
        threads,
        cert_out: None,
        print_cells: false,
    };
    while let Some(flag) = words.next() {
        match flag.as_str() {
            "--plain" => args.config.plain = value(&flag, words.next())?,
            "--tls" => args.config.tls = Some(value(&flag, words.next())?),
            "--cert-out" => args.cert_out = Some(value(&flag, words.next())?),
            "--threads" => args.threads = value(&flag, words.next())?,
            "--max-connections" => args.config.max_connections = value(&flag, words.next())?,
            "--stall" => args.config.stall = true,
            "--print-cells" => args.print_cells = true,
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    if args.threads == 0 || args.config.max_connections == 0 {
        return Err("--threads and --max-connections must be at least 1".into());
    }
    Ok(args)
}

fn value<T: std::str::FromStr<Err: std::fmt::Display>>(
    flag: &str,
    raw: Option<String>,
) -> Result<T, String> {
    let raw = raw.ok_or_else(|| format!("{flag} needs a value"))?;
    raw.parse().map_err(|e| format!("{flag} {raw}: {e}"))
}

/// Serves on a runtime of its own until the process is stopped.
fn run(args: Args) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.threads)
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(async {
        let (ready, bound) = oneshot::channel();
        let serving = serve(args.config, ready, std::future::pending());
        tokio::try_join!(serving, announce(bound, args.cert_out)).map(|_| ())
    })
}

/// Writes the certificate if asked, then prints the bound addresses (the harness waits for this line).
async fn announce(bound: oneshot::Receiver<Bound>, cert_out: Option<PathBuf>) -> io::Result<()> {
    let Ok(bound) = bound.await else {
        return Ok(());
    };
    if let (Some(path), Some(tls)) = (cert_out, &bound.tls) {
        // Once, before the server reports ready: a short blocking write is fine here.
        std::fs::write(path, &tls.certificate_pem)?;
    }
    let tls = bound
        .tls
        .map(|tls| format!(" tls={}", tls.addr))
        .unwrap_or_default();
    println!("ready plain={}{tls}", bound.plain);
    Ok(())
}
