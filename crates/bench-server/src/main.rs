//! `bench-server [--plain ADDR] [--tls ADDR] [--cert-out PATH] [--threads N] [--max-connections N] [--stall]
//! [--print-cells]`
//!
//! Listens on loopback (`127.0.0.1:8080`) unless an address is given: clients in its network namespace reach it there,
//! and `0.0.0.0:…` is for clients in another namespace.
//! Prints `ready plain=<addr> [tls=<addr>]` on stdout once it accepts connections, after writing the TLS certificate
//! (PEM) to `--cert-out` (which needs `--tls`). `--print-cells` prints the cell manifest (`bench/cells.json`) and
//! exits. The server runs on `--threads` runtime workers; the main thread only waits. SIGTERM (`docker stop`) or
//! SIGINT stops it cleanly: it stops accepting, ends its connections and exits 0.

use std::future::Future;
use std::io::{self, Write};
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
            report(&format!("{message}\n{USAGE}"));
            return ExitCode::from(2);
        }
    };
    let result = if args.print_cells {
        print_cells(io::stdout())
    } else {
        run(args)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            report(&format!("bench-server: {e}"));
            ExitCode::FAILURE
        }
    }
}

/// Writes the cell manifest (`bench/cells.json`).
fn print_cells(mut out: impl Write) -> io::Result<()> {
    out.write_all(cells_json().as_bytes())?;
    out.flush()
}

/// Reports on stderr; a closed stderr is ignored (`eprintln!` would panic on it).
fn report(message: &str) {
    let _ = writeln!(io::stderr(), "{message}");
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
    if args.cert_out.is_some() && args.config.tls.is_none() {
        return Err("--cert-out needs --tls".into());
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

/// Serves on a runtime of its own until the process is stopped. `serve` is spawned onto the workers, so the main
/// thread (which drives `block_on`) does no server work.
fn run(args: Args) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.threads)
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(async {
        let (ready, bound) = oneshot::channel();
        let serving = tokio::spawn(serve(args.config, ready, stop_signal()?));
        announce(bound, args.cert_out, io::stdout()).await?;
        serving.await.map_err(io::Error::other)?
    })
}

/// Completes on SIGTERM or SIGINT. Registered before `serve` starts, so a signal never finds the default action.
#[cfg(unix)]
fn stop_signal() -> io::Result<impl Future<Output = ()> + Send + 'static> {
    use tokio::signal::unix::{SignalKind, signal};
    let (mut term, mut int) = (
        signal(SignalKind::terminate())?,
        signal(SignalKind::interrupt())?,
    );
    Ok(async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    })
}

/// Completes on Ctrl-C (the only stop request outside Unix).
#[cfg(not(unix))]
fn stop_signal() -> io::Result<impl Future<Output = ()> + Send + 'static> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}

/// Writes the certificate if asked, then the ready line (the harness waits for it). A server that failed before it
/// was ready announces nothing; its error comes from `serve`.
async fn announce(
    bound: oneshot::Receiver<Bound>,
    cert_out: Option<PathBuf>,
    mut out: impl Write,
) -> io::Result<()> {
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
    writeln!(out, "ready plain={}{tls}", bound.plain)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::net::SocketAddr;

    use bench_server::Bound;
    use tokio::sync::oneshot;

    use super::{Args, announce, parse, print_cells};

    /// A stdout that has been closed.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn ready_server() -> oneshot::Receiver<Bound> {
        let (ready, bound) = oneshot::channel();
        let plain = SocketAddr::from(([127, 0, 0, 1], 8080));
        ready
            .send(Bound { plain, tls: None })
            .expect("receiver alive");
        bound
    }

    #[tokio::test]
    async fn announce_prints_the_ready_line() {
        let mut out = Vec::new();
        announce(ready_server(), None, &mut out)
            .await
            .expect("announced");
        assert_eq!(out, b"ready plain=127.0.0.1:8080\n");
    }

    #[test]
    fn print_cells_writes_the_manifest_and_reports_a_closed_stdout() {
        let mut out = Vec::new();
        print_cells(&mut out).expect("printed");
        assert_eq!(out, bench_server::cells_json().into_bytes());
        let error = print_cells(Closed).expect_err("closed stdout");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn announce_reports_a_closed_stdout() {
        let error = announce(ready_server(), None, Closed)
            .await
            .expect_err("closed stdout");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    fn args(words: &[&str]) -> Result<Args, String> {
        parse(words.iter().map(|word| (*word).to_owned()))
    }

    #[test]
    fn cert_out_needs_a_tls_port() {
        let refused = args(&["--cert-out", "cert.pem"]).err().expect("refused");
        assert!(refused.contains("--cert-out"), "{refused}");
        assert!(args(&["--tls", "127.0.0.1:0", "--cert-out", "cert.pem"]).is_ok());
    }

    #[test]
    fn bad_arguments_are_refused() {
        for words in [
            &["--threads", "0"][..],
            &["--max-connections", "0"],
            &["--unknown"],
            &["--plain"],
        ] {
            assert!(args(words).is_err(), "{words:?}");
        }
    }
}
