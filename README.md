# L7dstat

Layer 7 RQS capture dashboard written in Rust.

## Routes

- `/` counts one request and returns a tiny `200 OK` response.
- `/hit` does the same thing as `/`.
- `/dashboard` serves the web UI.
- `/dashboard/metrics` returns JSON metrics for the dashboard.
- `/metrics` is still available as a compatibility JSON endpoint.

## Ubuntu VPS

Install Rust:

```bash
sudo apt update
sudo apt install -y curl build-essential pkg-config
curl https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
rustc --version
cargo --version
```

Tune Linux for high connection pressure:

```bash
sudo tee /etc/sysctl.d/99-l7dstat.conf >/dev/null <<'EOF'
fs.file-max=2097152
net.core.somaxconn=65535
net.core.netdev_max_backlog=250000
net.ipv4.tcp_max_syn_backlog=262144
net.ipv4.ip_local_port_range=1024 65535
net.ipv4.tcp_fin_timeout=10
net.ipv4.tcp_tw_reuse=1
net.ipv4.tcp_fastopen=3
EOF

sudo sysctl --system
ulimit -n 1048576
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
L7DSTAT_CLOSE_AFTER_HIT=0 \
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
Environment=L7DSTAT_CLOSE_AFTER_HIT=0

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now l7dstat
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
- `L7DSTAT_CLOSE_AFTER_HIT=1` makes hit traffic return `OK` and close immediately without parsing headers.
