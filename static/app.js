// Real-time dashboard that consumes the Flask /metrics endpoint and draws the chart.
(function () {
  const $ = (sel) => document.querySelector(sel);

  const numberFmt = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 0 });
  const rpsFmt = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 2, minimumFractionDigits: 0 });

  const bodyDataset = document.body.dataset.initial;
  let initial = {};
  try {
    initial = bodyDataset ? JSON.parse(bodyDataset) : {};
  } catch (err) {
    console.error('Initial state malformed', err);
  }

  const elements = {
    total: $('#totalRequests'),
    currentRps: $('#currentRps'),
    avgRps: $('#avgRps'),
    windowRps: $('#windowRps'),
    uniqueIps: $('#uniqueIps'),
    uptime: $('#uptime'),
    subtitle: $('#graphSubtitle'),
    scoreboardBody: $('#scoreboardBody'),
    status: $('#status'),
    chart: $('#chart'),
  };

  // Lightweight canvas line chart
  class LineChart {
    constructor(canvas, capacity = 240) {
      this.canvas = canvas;
      this.ctx = canvas.getContext('2d');
      this.capacity = capacity;
      this.prevSnapshot = [];
      this.currentSnapshot = [];
      this.slideProgress = 1;
      this.animationStart = 0;
      this.animationDuration = 420;
      this.animFrame = null;
      this.padding = { top: 22, right: 24, bottom: 32, left: 56 };
      window.addEventListener('resize', () => this.draw());
    }

    sanitize(values) {
      if (!Array.isArray(values)) return [];
      return values
        .slice(-this.capacity)
        .map((value) => (typeof value === 'number' && isFinite(value) ? Math.max(value, 0) : 0));
    }

    cancelAnimation() {
      if (this.animFrame) {
        cancelAnimationFrame(this.animFrame);
        this.animFrame = null;
      }
    }

    setData(values) {
      const cleaned = this.sanitize(values);
      this.cancelAnimation();
      this.prevSnapshot = cleaned.slice();
      this.currentSnapshot = cleaned.slice();
      this.slideProgress = 1;
      this.draw();
    }

    push(value) {
      const sanitizedValue = typeof value === 'number' && isFinite(value) ? Math.max(value, 0) : 0;
      const base = this.currentSnapshot.length ? this.currentSnapshot.slice() : this.prevSnapshot.slice();
      if (!base.length) {
        const initial = this.sanitize([sanitizedValue]);
        this.prevSnapshot = initial.slice();
        this.currentSnapshot = initial.slice();
        this.slideProgress = 1;
        this.draw();
        return;
      }

      const next = base.slice();
      next.push(sanitizedValue);
      if (next.length > this.capacity) next.shift();

      const cleaned = this.sanitize(next);
      this.prevSnapshot = base.slice(-this.capacity);
      this.currentSnapshot = cleaned;
      this.slideProgress = 0;
      this.startSlideAnimation();
    }

    startSlideAnimation() {
      this.cancelAnimation();
      this.animationStart = performance.now();
      const tick = (timestamp) => {
        const raw = Math.min(1, (timestamp - this.animationStart) / this.animationDuration);
        const eased = 1 - Math.pow(1 - raw, 3);
        this.slideProgress = eased;
        this.draw();
        if (raw < 1) {
          this.animFrame = requestAnimationFrame(tick);
        } else {
          this.slideProgress = 1;
          this.draw();
        }
      };

      this.draw();
      this.animFrame = requestAnimationFrame(tick);
    }

    sample(series, position) {
      if (!series.length) return 0;
      const maxIndex = series.length - 1;
      if (position <= 0) return series[0];
      if (position >= maxIndex) return series[maxIndex];
      const left = Math.floor(position);
      const right = Math.ceil(position);
      const t = position - left;
      const a = series[left];
      const b = series[right];
      return a + (b - a) * t;
    }

    computeDisplayData() {
      if (this.slideProgress >= 1 || !this.prevSnapshot.length) {
        return this.currentSnapshot.slice();
      }

      const progress = this.slideProgress;
      const prev = this.prevSnapshot;
      const curr = this.currentSnapshot;
      if (!curr.length) return [];

      if (prev.length === curr.length && prev.length > 1) {
        const extended = prev.concat([curr[curr.length - 1]]);
        return curr.map((_, index) => this.sample(extended, index + progress));
      }

      const len = curr.length;
      const result = new Array(len);
      for (let i = 0; i < len; i += 1) {
        const a = prev[i] ?? prev[prev.length - 1] ?? curr[i] ?? 0;
        const b = curr[i] ?? curr[curr.length - 1] ?? a;
        result[i] = a + (b - a) * progress;
      }
      return result;
    }

    niceMax(max) {
      if (max <= 10) return 10;
      const magnitude = 10 ** Math.floor(Math.log10(max));
      const norm = max / magnitude;
      let nice;
      if (norm <= 1) nice = 1;
      else if (norm <= 2) nice = 2;
      else if (norm <= 5) nice = 5;
      else nice = 10;
      return nice * magnitude;
    }

    draw(dataset) {
      const values = dataset && dataset.length ? dataset : this.computeDisplayData();
      const ctx = this.ctx;
      const dpr = Math.max(1, window.devicePixelRatio || 1);
      const rect = this.canvas.getBoundingClientRect();
      const width = Math.max(1, Math.floor(rect.width * dpr));
      const height = Math.max(1, Math.floor(rect.height * dpr));

      if (this.canvas.width !== width || this.canvas.height !== height) {
        this.canvas.width = width;
        this.canvas.height = height;
      }

      ctx.clearRect(0, 0, width, height);

      const { top, right, bottom, left } = this.padding;
      const padT = top * dpr;
      const padR = right * dpr;
      const padB = bottom * dpr;
      const padL = left * dpr;
      const plotWidth = width - padL - padR;
      const plotHeight = height - padT - padB;

      ctx.save();
      ctx.fillStyle = 'rgba(255, 255, 255, 0.02)';
      ctx.fillRect(padL, padT, plotWidth, plotHeight);

      const maxVal = values.length ? Math.max(...values) : 10;
      const yMax = this.niceMax(maxVal * 1.1);

      ctx.strokeStyle = 'rgba(255, 255, 255, 0.08)';
      ctx.fillStyle = 'rgba(231, 233, 238, 0.65)';
      ctx.lineWidth = 1 * dpr;
      ctx.font = `${11 * dpr}px Inter, Segoe UI, sans-serif`;
      ctx.textAlign = 'right';
      ctx.textBaseline = 'middle';

      const gridLines = 4;
      for (let i = 0; i <= gridLines; i++) {
        const ratio = i / gridLines;
        const y = padT + plotHeight - ratio * plotHeight;
        ctx.beginPath();
        ctx.moveTo(padL, y);
        ctx.lineTo(padL + plotWidth, y);
        ctx.stroke();
        const label = Math.round(yMax * ratio);
        ctx.fillText(label.toString(), padL - 8 * dpr, y);
      }

      if (values.length > 1) {
        const stepX = plotWidth / Math.max(1, values.length - 1);
        const gradient = ctx.createLinearGradient(padL, padT, padL + plotWidth, padT);
        gradient.addColorStop(0, '#6f7dff');
        gradient.addColorStop(1, '#b367ff');
        ctx.lineWidth = 2.4 * dpr;
        ctx.strokeStyle = gradient;
        ctx.lineJoin = 'round';
        ctx.lineCap = 'round';
        ctx.beginPath();
        values.forEach((value, index) => {
          const x = padL + index * stepX;
          const y = padT + plotHeight - (value / yMax) * plotHeight;
          if (index === 0) ctx.moveTo(x, y);
          else ctx.lineTo(x, y);
        });
        ctx.stroke();
      }

      ctx.restore();
    }
  }

  const chart = new LineChart(elements.chart, 240);
  let lastTimestamp = 0;
  let initialised = false;

  const formatDuration = (seconds) => {
    const s = Math.max(0, Math.floor(seconds));
    const hrs = Math.floor(s / 3600);
    const mins = Math.floor((s % 3600) / 60);
    const secs = s % 60;
    const parts = [hrs, mins, secs].map((value) => value.toString().padStart(2, '0'));
    return `${parts[0]}:${parts[1]}:${parts[2]}`;
  };

  const setStatus = (message) => {
    elements.status.textContent = message || '';
  };

  const renderScoreboard = (entries) => {
    const tbody = elements.scoreboardBody;
    tbody.innerHTML = '';
    if (!entries || !entries.length) {
      const row = document.createElement('tr');
      row.className = 'empty-row';
      row.innerHTML = '<td colspan="4">Aucune requête enregistrée pour le moment…</td>';
      tbody.append(row);
      return;
    }

    entries.forEach((entry, index) => {
      const share = Math.max(0, Math.min(1, Number(entry.share) || 0));
      const row = document.createElement('tr');
      row.innerHTML = `
        <td class="rank">${index + 1}</td>
        <td class="ip">${entry.ip || '-'}</td>
        <td class="requests">${numberFmt.format(entry.requests || 0)}</td>
        <td class="share">
          <div class="share-bar"><span style="width: ${share * 100}%"></span></div>
          <span class="share-text">${rpsFmt.format(share * 100)}%</span>
        </td>
      `;
      tbody.append(row);
    });
  };

  const applyTimeline = (timeline, { replace = false } = {}) => {
    if (!Array.isArray(timeline) || !timeline.length) return;
    const sorted = timeline
      .filter((entry) => typeof entry.timestamp === 'number')
      .sort((a, b) => a.timestamp - b.timestamp);
    if (!sorted.length) return;

    if (replace) {
      chart.setData(sorted.map((entry) => Number(entry.rps) || 0));
      lastTimestamp = sorted[sorted.length - 1].timestamp;
      return;
    }

    sorted.forEach((entry) => {
      if (entry.timestamp > lastTimestamp) {
        chart.push(Number(entry.rps) || 0);
        lastTimestamp = entry.timestamp;
      }
    });
  };

  const updateView = (payload, { replaceTimeline = false } = {}) => {
    if (!payload || typeof payload !== 'object') return;

    applyTimeline(payload.timeline, { replace: replaceTimeline });

    if (typeof payload.total === 'number') {
      elements.total.textContent = numberFmt.format(payload.total);
    }
    if (typeof payload.current_rps === 'number') {
      elements.currentRps.textContent = rpsFmt.format(payload.current_rps);
    }
    if (typeof payload.avg_rps === 'number') {
      elements.avgRps.textContent = rpsFmt.format(payload.avg_rps);
    }
    if (typeof payload.window_rps === 'number') {
      elements.windowRps.textContent = rpsFmt.format(payload.window_rps);
    }
    if (typeof payload.unique_ips === 'number') {
      elements.uniqueIps.textContent = numberFmt.format(payload.unique_ips);
    }
    if (typeof payload.uptime_seconds === 'number') {
      elements.uptime.textContent = formatDuration(payload.uptime_seconds);
    }

    renderScoreboard(payload.scoreboard);

    if (payload.timeline && payload.timeline.length) {
      const ts = payload.timeline[payload.timeline.length - 1].timestamp;
      if (typeof ts === 'number') {
        const date = new Date(ts * 1000);
        elements.subtitle.textContent = `Dernière mise à jour · ${date.toLocaleTimeString('fr-FR')}`;
      }
    }
  };

  const poll = async () => {
    try {
      const response = await fetch('/metrics', { cache: 'no-store' });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      const data = await response.json();
      updateView(data, { replaceTimeline: !initialised });
      initialised = true;
      setStatus(`Flux actif · ${new Date().toLocaleTimeString('fr-FR')}`);
    } catch (err) {
      console.error('Metrics poll failed', err);
      setStatus(`Erreur de récupération: ${err.message}`);
    } finally {
      setTimeout(poll, 1000);
    }
  };

  // Initial bootstrap
  updateView(initial, { replaceTimeline: true });
  setStatus('Initialisation du flux…');
  initialised = Boolean(initial.timeline && initial.timeline.length);
  if (initial.timeline && initial.timeline.length) {
    lastTimestamp = initial.timeline[initial.timeline.length - 1].timestamp;
  }
  poll();
})();
