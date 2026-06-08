use mio::net::{TcpListener, TcpStream};
use mio::Registry;
use mio::{Events, Interest, Poll, Token};
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::env;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;

const SERVER: Token = Token(0);
const DEFAULT_ADDR: &str = ":5000";
const HISTORY_SECONDS: usize = 300;
const CHART_SECONDS: i64 = 240;
const WINDOW_SECONDS: i64 = 60;
const READ_BUF_SIZE: usize = 8192;
const DEFAULT_FLUSH_EVERY: u64 = 1;
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 100;

const HIT_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\nConnection: keep-alive\r\n\r\nOK\n";
const HIT_CLOSE_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\nConnection: close\r\n\r\nOK\n";
const BAD_REQUEST_RESPONSE: &[u8] = b"HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 12\r\nConnection: close\r\n\r\nBad Request\n";

#[repr(align(64))]
struct PaddedU64(AtomicU64);

#[repr(align(64))]
struct SecondBucket {
    sec: AtomicI64,
    count: AtomicU64,
}

#[derive(Serialize)]
struct DataPoint {
    timestamp: i64,
    rps: u64,
}

#[derive(Serialize)]
struct MetricsSnapshot {
    total: u64,
    avg_rps: f64,
    current_rps: u64,
    window_rps: f64,
    uptime_seconds: u64,
    workers: usize,
    cpu: usize,
    active_conns: usize,
    max_conns: usize,
    accepted_conns: u64,
    request_lines: u64,
    parse_errors: u64,
    timeline: Vec<DataPoint>,
}

struct Metrics {
    started: Instant,
    current_sec: AtomicI64,
    active_conns: AtomicUsize,
    accepted_conns: AtomicU64,
    request_lines: AtomicU64,
    parse_errors: AtomicU64,
    max_conns: usize,
    worker_count: usize,
    total_shards: Vec<PaddedU64>,
    rps_shards: Vec<Vec<SecondBucket>>,
}

impl Metrics {
    fn new(workers: usize, max_conns: usize) -> Self {
        let workers = workers.max(1);
        let shard_count = workers * 512;
        let now = unix_sec();
        let mut rps_shards = Vec::with_capacity(shard_count);

        for _ in 0..shard_count {
            let mut buckets = Vec::with_capacity(HISTORY_SECONDS);
            for idx in 0..HISTORY_SECONDS {
                buckets.push(SecondBucket {
                    sec: AtomicI64::new(now - (HISTORY_SECONDS - idx) as i64),
                    count: AtomicU64::new(0),
                });
            }
            rps_shards.push(buckets);
        }

        Self {
            started: Instant::now(),
            current_sec: AtomicI64::new(now),
            active_conns: AtomicUsize::new(0),
            accepted_conns: AtomicU64::new(0),
            request_lines: AtomicU64::new(0),
            parse_errors: AtomicU64::new(0),
            max_conns,
            worker_count: workers,
            total_shards: (0..shard_count).map(|_| PaddedU64(AtomicU64::new(0))).collect(),
            rps_shards,
        }
    }

    fn add(&self, shard: usize, sec: i64, amount: u64) {
        if amount == 0 {
            return;
        }

        let shard = shard % self.total_shards.len();
        self.total_shards[shard].0.fetch_add(amount, Ordering::Relaxed);

        let bucket = &self.rps_shards[shard][sec.rem_euclid(HISTORY_SECONDS as i64) as usize];
        loop {
            let bucket_sec = bucket.sec.load(Ordering::Acquire);
            if bucket_sec == sec {
                bucket.count.fetch_add(amount, Ordering::Relaxed);
                return;
            }

            if bucket_sec < 0 {
                std::hint::spin_loop();
                continue;
            }

            if bucket
                .sec
                .compare_exchange(bucket_sec, -sec, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                bucket.count.store(0, Ordering::Relaxed);
                bucket.sec.store(sec, Ordering::Release);
                bucket.count.fetch_add(amount, Ordering::Relaxed);
                return;
            }
        }
    }

    fn total(&self) -> u64 {
        self.total_shards
            .iter()
            .map(|shard| shard.0.load(Ordering::Relaxed))
            .sum()
    }

    fn snapshot(&self) -> MetricsSnapshot {
        let total = self.total();
        let uptime = self.started.elapsed().as_secs().max(1);
        let end = self.current_sec.load(Ordering::Relaxed) - 1;
        let start = end - CHART_SECONDS + 1;
        let mut timeline = Vec::with_capacity(CHART_SECONDS as usize);
        let mut window_total = 0;
        let mut current = 0;

        for sec in start..=end {
            let slot = sec.rem_euclid(HISTORY_SECONDS as i64) as usize;
            let mut rps = 0;

            for shard in &self.rps_shards {
                let bucket = &shard[slot];
                if bucket.sec.load(Ordering::Acquire) == sec {
                    rps += bucket.count.load(Ordering::Relaxed);
                }
            }

            if sec == end {
                current = rps;
            }
            if sec > end - WINDOW_SECONDS {
                window_total += rps;
            }
            timeline.push(DataPoint { timestamp: sec, rps });
        }

        MetricsSnapshot {
            total,
            avg_rps: round2(total as f64 / uptime as f64),
            current_rps: current,
            window_rps: round2(window_total as f64 / WINDOW_SECONDS as f64),
            uptime_seconds: uptime,
            workers: self.worker_count,
            cpu: num_cpus::get(),
            active_conns: self.active_conns.load(Ordering::Relaxed),
            max_conns: self.max_conns,
            accepted_conns: self.accepted_conns.load(Ordering::Relaxed),
            request_lines: self.request_lines.load(Ordering::Relaxed),
            parse_errors: self.parse_errors.load(Ordering::Relaxed),
            timeline,
        }
    }
}

struct Assets {
    index: Vec<u8>,
    js: Vec<u8>,
    css: Vec<u8>,
}

struct Conn {
    stream: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
    local_sec: i64,
    local_hits: u64,
    shard: usize,
    wants_write: bool,
    close_after_write: bool,
}

impl Conn {
    fn new(stream: TcpStream, shard: usize) -> Self {
        Self {
            stream,
            input: Vec::with_capacity(READ_BUF_SIZE),
            output: Vec::with_capacity(512),
            local_sec: 0,
            local_hits: 0,
            shard,
            wants_write: false,
            close_after_write: false,
        }
    }

    fn count_hit(&mut self, metrics: &Metrics, flush_every: u64) {
        if flush_every <= 1 {
            metrics.add(self.shard, metrics.current_sec.load(Ordering::Relaxed), 1);
            return;
        }

        let sec = metrics.current_sec.load(Ordering::Relaxed);
        if self.local_sec == 0 {
            self.local_sec = sec;
        }
        if sec != self.local_sec {
            self.flush(metrics);
            self.local_sec = sec;
        }
        self.local_hits += 1;
        if self.local_hits >= flush_every {
            self.flush(metrics);
        }
    }

    fn flush(&mut self, metrics: &Metrics) {
        if self.local_hits > 0 {
            metrics.add(self.shard, self.local_sec, self.local_hits);
            self.local_hits = 0;
        }
    }
}

fn main() -> io::Result<()> {
    let mut workers = env_usize("L7DSTAT_WORKERS", num_cpus::get());
    if workers == 0 {
        workers = num_cpus::get();
    }

    let max_conns = env_usize("L7DSTAT_MAX_CONNS", workers * 65_536);
    let close_after_hit = env_bool_default("L7DSTAT_CLOSE_AFTER_HIT", false);
    let flush_every = env_u64("L7DSTAT_FLUSH_EVERY", DEFAULT_FLUSH_EVERY).max(1);
    let flush_interval = Duration::from_millis(env_u64(
        "L7DSTAT_FLUSH_INTERVAL_MS",
        DEFAULT_FLUSH_INTERVAL_MS,
    ));
    let addr = normalize_addr(&env::var("ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string()));
    let socket_addr = resolve_addr(&addr)?;
    let metrics = Arc::new(Metrics::new(workers, max_conns));
    let assets = Arc::new(load_assets());

    {
        let metrics = metrics.clone();
        thread::spawn(move || loop {
            metrics.current_sec.store(unix_sec(), Ordering::Relaxed);
            thread::sleep(Duration::from_millis(100));
        });
    }

    println!(
        "l7dstat rust fast server on {} with {} workers / {} counter shards / max {} conns / close_after_hit={} / flush_every={}",
        addr,
        workers,
        metrics.total_shards.len(),
        max_conns,
        close_after_hit,
        flush_every
    );

    for worker_id in 0..listener_count(workers) {
        let listener = make_listener(socket_addr)?;
        let metrics = metrics.clone();
        let assets = assets.clone();
        thread::spawn(move || {
            if let Err(err) = run_worker(
                worker_id,
                workers,
                listener,
                metrics,
                assets,
                close_after_hit,
                flush_every,
                flush_interval,
            ) {
                eprintln!("worker {} stopped: {}", worker_id, err);
            }
        });
    }

    loop {
        thread::park();
    }
}

fn run_worker(
    worker_id: usize,
    worker_count: usize,
    mut listener: TcpListener,
    metrics: Arc<Metrics>,
    assets: Arc<Assets>,
    close_after_hit: bool,
    flush_every: u64,
    flush_interval: Duration,
) -> io::Result<()> {
    let mut poll = Poll::new()?;
    poll.registry()
        .register(&mut listener, SERVER, Interest::READABLE)?;

    let mut events = Events::with_capacity(4096);
    let mut conns: HashMap<Token, Conn> = HashMap::with_capacity(8192);
    let mut next_token = 1usize;
    let mut next_shard = worker_id;
    let mut last_pending_flush = Instant::now();

    loop {
        poll.poll(&mut events, Some(flush_interval))?;

        for event in events.iter() {
            match event.token() {
                SERVER => loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            metrics.accepted_conns.fetch_add(1, Ordering::Relaxed);
                            if metrics.max_conns > 0
                                && metrics.active_conns.load(Ordering::Relaxed) >= metrics.max_conns
                            {
                                metrics.add(next_shard, metrics.current_sec.load(Ordering::Relaxed), 1);
                                let _ = stream.write(HIT_CLOSE_RESPONSE);
                                continue;
                            }

                            let token = Token(next_token);
                            next_token = next_token.wrapping_add(1).max(1);

                            let _ = stream.set_nodelay(false);
                            poll.registry()
                                .register(&mut stream, token, Interest::READABLE)?;

                            metrics.active_conns.fetch_add(1, Ordering::Relaxed);
                            conns.insert(token, Conn::new(stream, next_shard));
                            next_shard += worker_count;
                            if next_shard >= metrics.total_shards.len() {
                                next_shard = worker_id;
                            }
                        }
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                        Err(err) => return Err(err),
                    }
                },
                token => {
                    let mut remove = false;
                    if let Some(conn) = conns.get_mut(&token) {
                        if event.is_readable()
                            && handle_read(
                                conn,
                                &metrics,
                                &assets,
                                close_after_hit,
                                flush_every,
                            )
                            .is_err()
                        {
                            remove = true;
                        }

                        if !remove && event.is_writable() && flush_output(conn).is_err() {
                            remove = true;
                        }

                        if !remove && sync_interest(poll.registry(), token, conn).is_err() {
                            remove = true;
                        }

                        if !remove && conn.close_after_write && conn.output.is_empty() {
                            remove = true;
                        }
                    }

                    if remove {
                        if let Some(mut conn) = conns.remove(&token) {
                            conn.flush(&metrics);
                            let _ = poll.registry().deregister(&mut conn.stream);
                            metrics.active_conns.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }

        if flush_every > 1 && last_pending_flush.elapsed() >= flush_interval {
            for conn in conns.values_mut() {
                conn.flush(&metrics);
            }
            last_pending_flush = Instant::now();
        }
    }
}

fn handle_read(
    conn: &mut Conn,
    metrics: &Metrics,
    assets: &Assets,
    close_after_hit: bool,
    flush_every: u64,
) -> io::Result<()> {
    let mut buf = [0u8; READ_BUF_SIZE];

    loop {
        match conn.stream.read(&mut buf) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "closed")),
            Ok(n) => conn.input.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => return Err(err),
        }
    }

    process_requests(conn, metrics, assets, close_after_hit, flush_every)
}

fn process_requests(
    conn: &mut Conn,
    metrics: &Metrics,
    assets: &Assets,
    close_after_hit: bool,
    flush_every: u64,
) -> io::Result<()> {
    loop {
        let Some(line_end) = find_lf(&conn.input) else {
            return Ok(());
        };

        let Some(path) = parse_path(&conn.input[..line_end]) else {
            metrics.parse_errors.fetch_add(1, Ordering::Relaxed);
            queue(conn, BAD_REQUEST_RESPONSE);
            conn.close_after_write = true;
            conn.input.clear();
            return Ok(());
        };
        metrics.request_lines.fetch_add(1, Ordering::Relaxed);

        if close_after_hit && !is_control_path(path) {
            metrics.add(conn.shard, metrics.current_sec.load(Ordering::Relaxed), 1);
            queue(conn, HIT_CLOSE_RESPONSE);
            conn.close_after_write = true;
            conn.input.clear();
            return Ok(());
        }

        let Some(header_end) = find_header_end(&conn.input) else {
            return Ok(());
        };

        let close_requested = header_has_connection_close(&conn.input[..header_end]);
        let path = path.to_vec();
        conn.input.drain(..header_end);

        match path.as_slice() {
            b"/" | b"/hit" => {
                conn.count_hit(metrics, flush_every);
                queue(conn, HIT_RESPONSE);
            }
            b"/dashboard" => {
                conn.flush(metrics);
                queue(conn, &assets.index);
            }
            b"/dashboard/metrics" | b"/metrics" => {
                conn.flush(metrics);
                queue_metrics(conn, metrics)?;
            }
            b"/static/app.js" => queue(conn, &assets.js),
            b"/static/styles.css" => queue(conn, &assets.css),
            _ => {
                conn.count_hit(metrics, flush_every);
                queue(conn, HIT_RESPONSE);
            }
        }

        if close_requested {
            conn.close_after_write = true;
            conn.input.clear();
            return Ok(());
        }
    }
}

fn flush_output(conn: &mut Conn) -> io::Result<()> {
    while !conn.output.is_empty() {
        match conn.stream.write(&conn.output) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "write zero")),
            Ok(n) => {
                conn.output.drain(..n);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn sync_interest(registry: &Registry, token: Token, conn: &mut Conn) -> io::Result<()> {
    let wants_write = !conn.output.is_empty();
    if wants_write == conn.wants_write {
        return Ok(());
    }

    let interest = if wants_write {
        Interest::READABLE | Interest::WRITABLE
    } else {
        Interest::READABLE
    };
    registry.reregister(&mut conn.stream, token, interest)?;
    conn.wants_write = wants_write;
    Ok(())
}

fn queue(conn: &mut Conn, bytes: &[u8]) {
    if conn.output.is_empty() {
        match conn.stream.write(bytes) {
            Ok(n) if n == bytes.len() => return,
            Ok(n) => conn.output.extend_from_slice(&bytes[n..]),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => conn.output.extend_from_slice(bytes),
            Err(_) => conn.close_after_write = true,
        }
    } else {
        conn.output.extend_from_slice(bytes);
    }
}

fn queue_metrics(conn: &mut Conn, metrics: &Metrics) -> io::Result<()> {
    let body = serde_json::to_vec(&metrics.snapshot())
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    queue(conn, header.as_bytes());
    queue(conn, &body);
    Ok(())
}

fn load_assets() -> Assets {
    Assets {
        index: make_response("text/html; charset=utf-8", include_bytes!("../index.html")),
        js: make_response(
            "application/javascript; charset=utf-8",
            include_bytes!("../static/app.js"),
        ),
        css: make_response("text/css; charset=utf-8", include_bytes!("../static/styles.css")),
    }
}

fn make_response(content_type: &str, body: &[u8]) -> Vec<u8> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n",
        content_type,
        body.len()
    );
    let mut response = Vec::with_capacity(header.len() + body.len());
    response.extend_from_slice(header.as_bytes());
    response.extend_from_slice(body);
    response
}

fn make_listener(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_nonblocking(true)?;
    socket.set_reuse_address(true)?;
    set_linux_socket_options(&socket);
    socket.bind(&addr.into())?;
    socket.listen(65_535)?;

    let listener: std::net::TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    Ok(TcpListener::from_std(listener))
}

#[cfg(target_os = "linux")]
fn set_linux_socket_options(socket: &Socket) {
    let fd = socket.as_raw_fd();
    let yes: libc::c_int = 1;
    let fastopen: libc::c_int = 4096;

    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            &yes as *const _ as *const libc::c_void,
            std::mem::size_of_val(&yes) as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_DEFER_ACCEPT,
            &yes as *const _ as *const libc::c_void,
            std::mem::size_of_val(&yes) as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_FASTOPEN,
            &fastopen as *const _ as *const libc::c_void,
            std::mem::size_of_val(&fastopen) as libc::socklen_t,
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn set_linux_socket_options(_socket: &Socket) {}

fn listener_count(workers: usize) -> usize {
    if cfg!(target_os = "linux") {
        workers.max(1)
    } else {
        1
    }
}

fn normalize_addr(addr: &str) -> String {
    if addr.starts_with(':') {
        format!("0.0.0.0{}", addr)
    } else {
        addr.to_string()
    }
}

fn resolve_addr(addr: &str) -> io::Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid ADDR"))
}

fn parse_path(line: &[u8]) -> Option<&[u8]> {
    let first = memchr(line, b' ')?;
    let rest = &line[first + 1..];
    let second = memchr(rest, b' ')?;
    Some(&rest[..second])
}

fn find_lf(buf: &[u8]) -> Option<usize> {
    memchr(buf, b'\n').map(|idx| idx + 1)
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|idx| idx + 4)
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n").map(|idx| idx + 2))
}

fn header_has_connection_close(buf: &[u8]) -> bool {
    let mut start = 0;
    while start < buf.len() {
        let end = match buf[start..].iter().position(|&b| b == b'\n') {
            Some(pos) => start + pos + 1,
            None => buf.len(),
        };
        let line = &buf[start..end];
        if ascii_starts_with(line, b"connection: close") {
            return true;
        }
        start = end;
    }
    false
}

fn ascii_starts_with(line: &[u8], prefix: &[u8]) -> bool {
    if line.len() < prefix.len() {
        return false;
    }
    line.iter()
        .take(prefix.len())
        .zip(prefix.iter())
        .all(|(&a, &b)| a.to_ascii_lowercase() == b)
}

fn is_control_path(path: &[u8]) -> bool {
    matches!(
        path,
        b"/dashboard" | b"/dashboard/metrics" | b"/metrics" | b"/static/app.js" | b"/static/styles.css"
    )
}

fn memchr(buf: &[u8], needle: u8) -> Option<usize> {
    buf.iter().position(|&b| b == needle)
}

fn unix_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(fallback)
}

fn env_bool_default(key: &str, fallback: bool) -> bool {
    match env::var(key) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => fallback,
        },
        Err(_) => fallback,
    }
}
