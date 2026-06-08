(function () {
  const $ = (selector) => document.querySelector(selector);
  const numberFmt = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 0 });
  const rpsFmt = new Intl.NumberFormat('fr-FR', {
    maximumFractionDigits: 2,
    minimumFractionDigits: 0,
  });

  const elements = {
    total: $('#totalRequests'),
    currentRps: $('#currentRps'),
    avgRps: $('#avgRps'),
    windowRps: $('#windowRps'),
    nowRps: $('#nowRps'),
    uptime: $('#uptime'),
    workers: $('#workers'),
    connections: $('#connections'),
    acceptedConns: $('#acceptedConns'),
    requestLines: $('#requestLines'),
    subtitle: $('#graphSubtitle'),
    status: $('#status'),
    chart: $('#chart'),
  };

  class LineChart {
    constructor(canvas, capacity = 240) {
      this.canvas = canvas;
      this.ctx = canvas.getContext('2d');
      this.capacity = capacity;
      this.previousValues = [];
      this.targetValues = [];
      this.displayValues = [];
      this.animationFrame = null;
      this.animationStart = 0;
      this.animationDuration = 720;
      this.padding = { top: 22, right: 24, bottom: 32, left: 56 };
      window.addEventListener('resize', () => this.draw());
    }

    setData(values) {
      const next = Array.isArray(values)
        ? values.slice(-this.capacity).map((value) => Math.max(0, Number(value) || 0))
        : [];

      if (!this.displayValues.length) {
        this.previousValues = next.slice();
        this.targetValues = next.slice();
        this.displayValues = next.slice();
        this.draw();
        return;
      }

      this.previousValues = this.resample(this.displayValues, next.length);
      this.targetValues = next;
      this.startAnimation();
    }

    startAnimation() {
      if (this.animationFrame) {
        cancelAnimationFrame(this.animationFrame);
      }

      this.animationStart = performance.now();
      const tick = (timestamp) => {
        const progress = Math.min(1, (timestamp - this.animationStart) / this.animationDuration);
        const eased = 1 - Math.pow(1 - progress, 3);
        this.displayValues = this.targetValues.map((value, index) => {
          const previous = this.previousValues[index] ?? value;
          return previous + (value - previous) * eased;
        });
        this.draw();

        if (progress < 1) {
          this.animationFrame = requestAnimationFrame(tick);
        } else {
          this.displayValues = this.targetValues.slice();
          this.animationFrame = null;
          this.draw();
        }
      };

      this.animationFrame = requestAnimationFrame(tick);
    }

    resample(values, length) {
      if (!length) return [];
      if (!values.length) return new Array(length).fill(0);
      if (values.length === length) return values.slice();

      const result = new Array(length);
      const scale = (values.length - 1) / Math.max(1, length - 1);
      for (let index = 0; index < length; index += 1) {
        const position = index * scale;
        const left = Math.floor(position);
        const right = Math.min(values.length - 1, left + 1);
        const mix = position - left;
        result[index] = values[left] + (values[right] - values[left]) * mix;
      }
      return result;
    }

    niceMax(max) {
      if (max <= 10) return 10;
      const magnitude = 10 ** Math.floor(Math.log10(max));
      const norm = max / magnitude;
      if (norm <= 1) return magnitude;
      if (norm <= 2) return 2 * magnitude;
      if (norm <= 5) return 5 * magnitude;
      return 10 * magnitude;
    }

    draw() {
      const values = this.displayValues.length ? this.displayValues : [0];
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

      const padT = this.padding.top * dpr;
      const padR = this.padding.right * dpr;
      const padB = this.padding.bottom * dpr;
      const padL = this.padding.left * dpr;
      const plotWidth = Math.max(1, width - padL - padR);
      const plotHeight = Math.max(1, height - padT - padB);
      const yMax = this.niceMax(Math.max(...values) * 1.1);

      ctx.fillStyle = '#0c0e10';
      ctx.fillRect(0, 0, width, height);
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.08)';
      ctx.fillStyle = 'rgba(241, 243, 244, 0.62)';
      ctx.lineWidth = 1 * dpr;
      ctx.font = `${11 * dpr}px Inter, Segoe UI, sans-serif`;
      ctx.textAlign = 'right';
      ctx.textBaseline = 'middle';

      for (let i = 0; i <= 4; i += 1) {
        const ratio = i / 4;
        const y = padT + plotHeight - ratio * plotHeight;
        ctx.beginPath();
        ctx.moveTo(padL, y);
        ctx.lineTo(padL + plotWidth, y);
        ctx.stroke();
        ctx.fillText(Math.round(yMax * ratio).toString(), padL - 8 * dpr, y);
      }

      if (values.length > 1) {
        const stepX = plotWidth / Math.max(1, values.length - 1);
        const points = values.map((value, index) => ({
          x: padL + index * stepX,
          y: padT + plotHeight - (value / yMax) * plotHeight,
        }));

        const fill = ctx.createLinearGradient(0, padT, 0, padT + plotHeight);
        fill.addColorStop(0, 'rgba(35, 196, 131, 0.20)');
        fill.addColorStop(1, 'rgba(35, 196, 131, 0.00)');
        this.traceSmoothLine(ctx, points);
        ctx.lineTo(padL + plotWidth, padT + plotHeight);
        ctx.lineTo(padL, padT + plotHeight);
        ctx.closePath();
        ctx.fillStyle = fill;
        ctx.fill();

        this.traceSmoothLine(ctx, points);
        ctx.lineWidth = 2.5 * dpr;
        ctx.strokeStyle = '#23c483';
        ctx.lineJoin = 'round';
        ctx.lineCap = 'round';
        ctx.stroke();
      }
    }

    traceSmoothLine(ctx, points) {
      ctx.beginPath();
      ctx.moveTo(points[0].x, points[0].y);
      for (let index = 0; index < points.length - 1; index += 1) {
        const current = points[index];
        const next = points[index + 1];
        const midX = (current.x + next.x) / 2;
        ctx.bezierCurveTo(midX, current.y, midX, next.y, next.x, next.y);
      }
    }
  }

  const chart = new LineChart(elements.chart, 240);

  const formatDuration = (seconds) => {
    const safe = Math.max(0, Math.floor(seconds || 0));
    const hrs = Math.floor(safe / 3600);
    const mins = Math.floor((safe % 3600) / 60);
    const secs = safe % 60;
    return [hrs, mins, secs].map((value) => String(value).padStart(2, '0')).join(':');
  };

  const updateView = (payload) => {
    if (!payload || typeof payload !== 'object') return;

    elements.total.textContent = numberFmt.format(payload.total || 0);
    elements.currentRps.textContent = numberFmt.format(payload.current_rps || 0);
    elements.avgRps.textContent = rpsFmt.format(payload.avg_rps || 0);
    elements.windowRps.textContent = rpsFmt.format(payload.window_rps || 0);
    elements.nowRps.textContent = numberFmt.format(payload.current_rps || 0);
    elements.uptime.textContent = formatDuration(payload.uptime_seconds);
    elements.workers.textContent = `${payload.workers || 0} / ${payload.cpu || 0}`;
    elements.connections.textContent = `${payload.active_conns || 0} / ${payload.max_conns || 0}`;
    elements.acceptedConns.textContent = numberFmt.format(payload.accepted_conns || 0);
    elements.requestLines.textContent = numberFmt.format(payload.request_lines || 0);

    if (Array.isArray(payload.timeline)) {
      chart.setData(payload.timeline.map((entry) => entry.rps));
      const last = payload.timeline[payload.timeline.length - 1];
      if (last && typeof last.timestamp === 'number') {
        const date = new Date(last.timestamp * 1000);
        elements.subtitle.textContent = `Derniere maj: ${date.toLocaleTimeString('fr-FR')}`;
      }
    }
  };

  const poll = async () => {
    try {
      const response = await fetch('/dashboard/metrics', { cache: 'no-store' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      updateView(await response.json());
      elements.status.textContent = `Actif - ${new Date().toLocaleTimeString('fr-FR')}`;
    } catch (error) {
      elements.status.textContent = `Erreur: ${error.message}`;
    } finally {
      setTimeout(poll, 1000);
    }
  };

  chart.setData([0]);
  elements.status.textContent = 'Initialisation...';
  poll();
})();
