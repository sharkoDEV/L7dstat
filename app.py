from __future__ import annotations

import heapq
import time
from collections import deque
from dataclasses import dataclass
from threading import Lock
from typing import Deque, Dict, List, Tuple

from flask import Flask, jsonify, render_template, request


@dataclass
class ScoreEntry:
    ip: str
    requests: int
    share: float


class MetricsCollector:
    HISTORY_SECONDS = 3600
    SCOREBOARD_SIZE = 8
    SCOREBOARD_CACHE_SECONDS = 1.0
    IP_SOFT_LIMIT = 10_000
    IP_PRUNE_KEEP = 128
    RATE_LIMIT_PER_SECOND = 800
    RATE_LIMIT_PER_MINUTE = 15_000
    BAN_DURATION_SECONDS = 600
    IP_TRACK_LIMIT = 1_000_000
    ADAPTIVE_WINDOW_SECONDS = 60
    ADAPTIVE_INTENSE_THRESHOLD = 5_000
    ADAPTIVE_SHARE_THRESHOLD = 0.55
    ADAPTIVE_ACTIVATION_TOTAL = 10_000

    def __init__(self) -> None:
        self._lock = Lock()
        now_sec = int(time.time())
        self._start_time = time.time()
        self._total_requests: int = 0
        self._ip_totals: Dict[str, int] = {}
        self._unique_total: int = 0
        self._seen_hashes: set[int] = set()
        self._timeline: Deque[List[int]] = deque(maxlen=self.HISTORY_SECONDS)
        self._timeline.append([now_sec, 0])
        self._current_second = now_sec
        self._last_scoreboard: List[Tuple[str, int]] = []
        self._scoreboard_cache_ts = 0.0
        self._ip_rate: Dict[str, Tuple[int, int]] = {}
        self._ip_minute: Dict[str, Tuple[int, int]] = {}
        self._banned_ips: Dict[str, int] = {}
        self._ip_sliding: Dict[str, Tuple[Deque[List[int]], int]] = {}
        self._window_counts: Deque[int] = deque(maxlen=self.ADAPTIVE_WINDOW_SECONDS)
        self._window_counts.append(0)
        self._rolling_window_total = 0

    def _advance_to(self, target_second: int) -> None:
        if target_second <= self._current_second:
            return

        timeline = self._timeline
        if not timeline:
            timeline.append([target_second, 0])
            self._append_window_second()
            self._current_second = target_second
            return

        last = self._current_second
        while last < target_second:
            last += 1
            timeline.append([last, 0])
            self._append_window_second()
        self._current_second = target_second

    def _prune_ip_totals(self) -> None:
        if len(self._ip_totals) <= self.IP_SOFT_LIMIT:
            return
        keep = heapq.nlargest(self.IP_PRUNE_KEEP, self._ip_totals.items(), key=lambda item: item[1])
        keep_ips = {ip for ip, _ in keep}
        self._ip_totals = {ip: count for ip, count in keep}
        self._ip_rate = {ip: self._ip_rate[ip] for ip in keep_ips if ip in self._ip_rate}
        self._ip_minute = {ip: self._ip_minute[ip] for ip in keep_ips if ip in self._ip_minute}
        self._ip_sliding = {ip: self._ip_sliding[ip] for ip in keep_ips if ip in self._ip_sliding}
        self._last_scoreboard = []
        self._scoreboard_cache_ts = 0.0

    def _append_window_second(self) -> None:
        if len(self._window_counts) == self.ADAPTIVE_WINDOW_SECONDS:
            self._rolling_window_total -= self._window_counts.popleft()
        self._window_counts.append(0)

    def _cleanup_bans(self, now_sec: int) -> None:
        expired = [ip for ip, expiry in self._banned_ips.items() if expiry <= now_sec]
        for ip in expired:
            self._banned_ips.pop(ip, None)

    def _is_banned_locked(self, ip: str, now_sec: int) -> bool:
        expiry = self._banned_ips.get(ip)
        if expiry is None:
            return False
        if expiry <= now_sec:
            self._banned_ips.pop(ip, None)
            return False
        return True

    def _ban_ip(self, ip: str, now_sec: int) -> None:
        self._banned_ips[ip] = now_sec + self.BAN_DURATION_SECONDS
        self._last_scoreboard = []
        self._scoreboard_cache_ts = 0.0
        self._ip_rate.pop(ip, None)
        self._ip_minute.pop(ip, None)
        self._ip_sliding.pop(ip, None)

    def register_request(self, ip: str | None) -> bool:
        now_sec = int(time.time())
        with self._lock:
            # Banning and blocking disabled: only collect metrics
            self._cleanup_bans(now_sec)

            if ip:
                rate_bucket = self._ip_rate.get(ip)
                if rate_bucket and rate_bucket[0] == now_sec:
                    sec_count = rate_bucket[1] + 1
                else:
                    sec_count = 1
                self._ip_rate[ip] = (now_sec, sec_count)
                # Do not enforce per-second limits

                minute_key = now_sec // 60
                minute_bucket = self._ip_minute.get(ip)
                if minute_bucket and minute_bucket[0] == minute_key:
                    minute_count = minute_bucket[1] + 1
                else:
                    minute_count = 1
                self._ip_minute[ip] = (minute_key, minute_count)
                # Do not enforce per-minute limits

                counts = self._ip_totals
                counts[ip] = counts.get(ip, 0) + 1

                ip_hash = hash(ip)
                if ip_hash not in self._seen_hashes and len(self._seen_hashes) < self.IP_TRACK_LIMIT:
                    self._seen_hashes.add(ip_hash)
                    self._unique_total += 1

                sliding = self._ip_sliding.get(ip)
                if sliding is None:
                    dq: Deque[List[int]] = deque()
                    window_total_ip = 0
                else:
                    dq, window_total_ip = sliding

                if dq and dq[-1][0] == now_sec:
                    dq[-1][1] += 1
                else:
                    dq.append([now_sec, 1])
                window_total_ip += 1

                threshold_second = now_sec - self.ADAPTIVE_WINDOW_SECONDS + 1
                while dq and dq[0][0] < threshold_second:
                    window_total_ip -= dq[0][1]
                    dq.popleft()

                if dq:
                    self._ip_sliding[ip] = (dq, window_total_ip)
                else:
                    self._ip_sliding.pop(ip, None)
                    window_total_ip = 0

                # Do not enforce adaptive intense threshold

                if len(counts) > self.IP_SOFT_LIMIT:
                    self._prune_ip_totals()

            self._total_requests += 1

            if now_sec < self._current_second:
                now_sec = self._current_second

            self._advance_to(now_sec)
            self._timeline[-1][1] += 1
            if self._window_counts:
                self._window_counts[-1] += 1
            else:
                self._window_counts.append(1)
            self._rolling_window_total += 1

            if ip:
                sliding = self._ip_sliding.get(ip)
                if sliding:
                    _, window_total_ip = sliding
                    total_recent = max(self._rolling_window_total, 1)
                    # Do not enforce adaptive share threshold
            return True

    def snapshot(self, window: int = 60, chart: int = 240) -> dict:
        now = time.time()
        now_sec = int(now)

        with self._lock:
            self._cleanup_bans(now_sec)
            self._advance_to(now_sec)
            timeline_snapshot = list(self._timeline)
            total = self._total_requests
            unique_count = self._unique_total
            start_time = self._start_time
            scoreboard_stale = (now - self._scoreboard_cache_ts) >= self.SCOREBOARD_CACHE_SECONDS
            cached_scoreboard = list(self._last_scoreboard)
            ip_snapshot = list(self._ip_totals.items()) if scoreboard_stale else None

        if scoreboard_stale:
            top_pairs = heapq.nlargest(self.SCOREBOARD_SIZE, ip_snapshot, key=lambda item: item[1]) if ip_snapshot else []
            with self._lock:
                self._last_scoreboard = top_pairs
                self._scoreboard_cache_ts = now
        else:
            top_pairs = cached_scoreboard

        chart_start = now_sec - chart + 1
        if timeline_snapshot:
            first_ts = timeline_snapshot[0][0]
            chart_start = max(chart_start, first_ts)
        timeline = [
            {"timestamp": ts, "rps": count}
            for ts, count in timeline_snapshot
            if ts >= chart_start
        ]
        if not timeline:
            timeline = [{"timestamp": now_sec, "rps": 0}]

        window_slice = timeline[-min(window, len(timeline)) :]
        window_seconds = len(window_slice) or 1
        window_requests = sum(point["rps"] for point in window_slice)
        window_rps = window_requests / window_seconds

        elapsed_seconds = max(int(now - start_time), 1)
        avg_rps = total / elapsed_seconds
        current_rps = timeline[-1]["rps"] if timeline else 0

        total_for_share = total if total else 1
        scoreboard = [
            ScoreEntry(ip=ip, requests=count, share=count / total_for_share)
            for ip, count in top_pairs[: self.SCOREBOARD_SIZE]
        ]

        return {
            "total": total,
            "avg_rps": round(avg_rps, 2),
            "current_rps": round(current_rps, 2),
            "window_rps": round(window_rps, 2),
            "unique_ips": unique_count,
            "uptime_seconds": elapsed_seconds,
            "timeline": timeline,
            "scoreboard": [entry.__dict__ for entry in scoreboard],
        }


collector = MetricsCollector()

app = Flask(__name__)


@app.before_request
def _track_request() -> None:
    forwarded = request.headers.get("X-Forwarded-For")
    ip = (forwarded.split(",")[0].strip() if forwarded else request.remote_addr) or "inconnu"
    # Always register metrics but never block requests
    collector.register_request(ip)


@app.route("/")
def index():
    snapshot = collector.snapshot()
    return render_template("index.html", initial=snapshot)


@app.route("/metrics")
def metrics_endpoint():
    return jsonify(collector.snapshot())


if __name__ == "__main__":
    app.run(debug=True, host="0.0.0.0", port=5000)
