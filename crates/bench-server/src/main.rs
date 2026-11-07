//! `bench-server [--plain ADDR] [--tls ADDR] [--threads N] [--stall] [--print-cells]`
//!
//! Prints `ready plain=<addr> [tls=<addr>]` on stdout once it accepts connections. `--print-cells` prints the cell
//! manifest (`bench/cells.json`) and exits.

use std::io;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::thread::available_parallelism;

use bench_server::{Bound, ServerConfig, cells_json, serve};
use tokio::sync::oneshot;

const USAGE: &str =
    "usage: bench-server [--plain ADDR] [--tls ADDR] [--threads N] [--stall] [--print-cells]";
const DEFAULT_PLAIN: &str = "0.0.0.0:8080";

struct Args {
    config: ServerConfig,
    threads: usize,
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
    match run(args.config, args.threads) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bench-server: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(mut words: impl Iterator<Item = String>) -> Result<Args, String> {
    let default_threads = available_parallelism().map_or(1, usize::from);
    let plain = DEFAULT_PLAIN
        .parse()
        .map_err(|e| format!("{DEFAULT_PLAIN}: {e}"))?;
    let mut args = Args {
        config: ServerConfig {
            plain,
            tls: None,
            stall: false,
        },
        threads: default_threads,
        print_cells: false,
    };
    while let Some(flag) = words.next() {
        match flag.as_str() {
            "--plain" => args.config.plain = value(&flag, words.next())?,
            "--tls" => args.config.tls = Some(value(&flag, words.next())?),
            "--threads" => args.threads = value(&flag, words.next())?,
            "--stall" => args.config.stall = true,
            "--print-cells" => args.print_cells = true,
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    if args.threads == 0 {
        return Err("--threads must be at least 1".into());
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

fn run(config: ServerConfig, threads: usize) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_io()
        .build()?;
    runtime.block_on(async {
        let (ready, bound) = oneshot::channel();
        tokio::try_join!(serve(config, ready), announce(bound)).map(|_| ())
    })
}

/// Prints the bound addresses once the server is ready (the harness waits for this line).
async fn announce(bound: oneshot::Receiver<Bound>) -> io::Result<()> {
    if let Ok(bound) = bound.await {
        let tls: Option<SocketAddr> = bound.tls.map(|tls| tls.addr);
        let tls = tls.map(|addr| format!(" tls={addr}")).unwrap_or_default();
        println!("ready plain={}{tls}", bound.plain);
    }
    Ok(())
}
