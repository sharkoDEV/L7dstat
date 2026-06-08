#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${REPO_URL:-https://github.com/sharkoDEV/L7dstat.git}"
INSTALL_DIR="${INSTALL_DIR:-/opt/l7dstat}"
SRC_DIR="${SRC_DIR:-$HOME/L7dstat}"
ADDR="${ADDR:-:5000}"
MAX_CONNS="${L7DSTAT_MAX_CONNS:-200000}"
CLOSE_AFTER_HIT="${L7DSTAT_CLOSE_AFTER_HIT:-0}"

if ! command -v sudo >/dev/null 2>&1; then
  echo "sudo is required"
  exit 1
fi

sudo apt update
sudo apt install -y curl git build-essential pkg-config ca-certificates

if ! command -v cargo >/dev/null 2>&1; then
  curl https://sh.rustup.rs -sSf | sh -s -- -y
fi

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo was not found after rustup install"
  exit 1
fi

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
sudo sysctl -p /etc/sysctl.d/99-l7dstat.conf || true

if [ -d "$SRC_DIR/.git" ]; then
  git -C "$SRC_DIR" pull --ff-only
else
  git clone "$REPO_URL" "$SRC_DIR"
fi

cd "$SRC_DIR"
cargo build --release

sudo mkdir -p "$INSTALL_DIR"
sudo cp ./target/release/l7dstat "$INSTALL_DIR/l7dstat"
sudo chmod 0755 "$INSTALL_DIR/l7dstat"

sudo tee /etc/systemd/system/l7dstat.service >/dev/null <<EOF
[Unit]
Description=L7dstat Rust
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/l7dstat
Restart=always
RestartSec=1
LimitNOFILE=1048576
Environment=ADDR=$ADDR
Environment=L7DSTAT_WORKERS=0
Environment=L7DSTAT_MAX_CONNS=$MAX_CONNS
Environment=L7DSTAT_CLOSE_AFTER_HIT=$CLOSE_AFTER_HIT

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now l7dstat

echo
echo "L7dstat installed and started."
echo "Dashboard: http://SERVER_IP:${ADDR##*:}/dashboard"
echo "Traffic:   http://SERVER_IP:${ADDR##*:}/"
echo
sudo systemctl --no-pager --full status l7dstat || true
