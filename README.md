# L7dstat

Layer 7 RQS capture dashboard written in Rust.

## Routes

- `/` counts one request and returns a tiny `200 OK` response.
- `/hit` does the same thing as `/`.
- `/dashboard` serves the web UI.
- `/dashboard/metrics` returns JSON metrics for the dashboard.
- `/metrics` is still available as a compatibility JSON endpoint.

## Ubuntu VPS

One-command install, accurate mode by default:

```bash
curl -fsSL https://raw.githubusercontent.com/sharkoDEV/L7dstat/main/install.sh | bash
```

Force close-after-hit accurate mode:

```bash
curl -fsSL https://raw.githubusercontent.com/sharkoDEV/L7dstat/main/install.sh | L7DSTAT_CLOSE_AFTER_HIT=1 bash
```

Build:

```bash
cd /path/to/L7dstat
cargo build --release
```

Run:

```bash
ulimit -n 1048576
ADDR=:5000 \
L7DSTAT_WORKERS=$(nproc) \
L7DSTAT_MAX_CONNS=200000 \
L7DSTAT_CLOSE_AFTER_HIT=1 \
L7DSTAT_FLUSH_EVERY=1 \
./target/release/l7dstat
```

For traffic that opens tons of new connections and does not reuse keep-alive, use the more aggressive mode:

```bash
ulimit -n 1048576
ADDR=:5000 \
L7DSTAT_WORKERS=$(nproc) \
L7DSTAT_MAX_CONNS=200000 \
L7DSTAT_CLOSE_AFTER_HIT=1 \
./target/release/l7dstat
```

Open:

```text
http://SERVER_IP:5000/dashboard
```

Send traffic to:

```text
http://SERVER_IP:5000/
```

## Systemd

```bash
sudo mkdir -p /opt/l7dstat
sudo cp ./target/release/l7dstat /opt/l7dstat/l7dstat

sudo tee /etc/systemd/system/l7dstat.service >/dev/null <<'EOF'
[Unit]
Description=L7dstat Rust
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/l7dstat
ExecStart=/opt/l7dstat/l7dstat
Restart=always
RestartSec=1
LimitNOFILE=1048576
Environment=ADDR=:5000
Environment=L7DSTAT_WORKERS=0
Environment=L7DSTAT_MAX_CONNS=200000
Environment=L7DSTAT_CLOSE_AFTER_HIT=1
Environment=L7DSTAT_FLUSH_EVERY=1
Environment=L7DSTAT_FLUSH_INTERVAL_MS=100

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable l7dstat
sudo systemctl restart l7dstat
sudo systemctl status l7dstat --no-pager
```

## Benchmark

Benchmark only your own VPS:

```bash
sudo apt install -y wrk
ulimit -n 1048576
wrk -t$(nproc) -c10000 -d30s http://127.0.0.1:5000/
```

If it stays around `1k RPS`, increase client concurrency or test from another machine. A single slow client, provider limits, firewall rules, or network packet limits can cap visible RPS before the Rust server is the bottleneck.

## Runtime Options

- `ADDR=:8080` changes the listen address.
- `L7DSTAT_WORKERS=16` changes the number of event-loop workers. `0` means all CPU cores.
- `L7DSTAT_MAX_CONNS=200000` changes the active connection cap. Use `0` to disable it.
- `L7DSTAT_CLOSE_AFTER_HIT=1` makes hit traffic count as soon as the request line is received, return `OK`, and close. This is the most accurate mode for hostile/new-connection traffic.
- `L7DSTAT_CLOSE_AFTER_HIT=0` keeps hit connections alive and counts after full headers are parsed.
- `L7DSTAT_FLUSH_EVERY=1` counts every hit immediately for accurate live stats. Higher values are faster but less instant.
- `L7DSTAT_FLUSH_INTERVAL_MS=100` flushes batched connection counters when `L7DSTAT_FLUSH_EVERY` is higher than `1`.
