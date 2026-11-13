// The debug overlay (D, `?debug`, or the switch in the about panel): the
// frame rate, the camera, what the renderer draws and holds, what the map is
// loading, and a graph of the last frames that drew, each frame's work since
// the one before stacked by phase in front of the time between them. The map's
// Web Worker reports frames and numbers only while it shows.
//
// Frames that draw nothing add nothing to the graph, so it pauses while the
// map is still; a marker shows where it resumed. Hovering a bar shows that
// frame's phases in the legend, which otherwise shows their average over the
// last frames.

import { type Camera, type DebugStats, type FrameSample, PHASES } from './protocol.ts';

type Phase = (typeof PHASES)[number];

const PHASE_INFO: Record<Phase, { color: string; title: string }> = {
  map: { color: '#8b9bb0', title: "Gestures, animations and the map's RunLoop, but for tile replies" },
  tiles: { color: '#ff7a9a', title: "Decoding the tile workers' replies: each tile's buckets, feature index and symbols" },
  sources: { color: '#a594ff', title: "Updating the render layers and sources: the sources' tiles and their matrices" },
  symbols: { color: '#f4e04d', title: 'Placing symbols across tiles and updating their opacities' },
  layers: { color: '#4cc3ff', title: 'Updating the layer groups and their drawables from the tiles' },
  upload: { color: '#3ddbd9', title: 'Running the layer tweakers and uploading buffers and textures (and making shaders)' },
  draw: { color: '#5ee08a', title: 'Encoding the render passes and submitting them' },
  tasks: { color: '#ff9b5e', title: "The cooperative scheduler's tasks, such as glyphs and sprites (8 ms budget a frame)" },
};

/** Frames the graph keeps. */
const HISTORY = 120;
/** The graph's width per frame and its height, in CSS pixels. */
const BAR = 2;
const GRAPH_HEIGHT = 72;
/** The milliseconds the graph's height spans; taller frames are clipped. */
const GRAPH_MS = 40;
/** The frame rates the graph marks. */
const RATES = [120, 60, 30];
/** A frame this long after the one before resumes the graph after a pause. */
const PAUSE_MS = 250;
/** The frames the legend averages. */
const AVERAGE = 60;
/** How often the numbers change, so that they stay readable. */
const TEXT_MS = 250;

export interface DebugOverlay {
  /** Whether it shows. */
  readonly shown: boolean;
  show(on: boolean): void;
  frame(sample: FrameSample): void;
  stats(stats: DebugStats): void;
  camera(camera: Camera): void;
  /**
   * What the page knows of the map, rather than its Web Worker: its size in
   * CSS pixels, and the pixel ratio its texture has.
   */
  page(info: { backend: string; width: number; height: number; ratio: number }): void;
}

/** A frame of the graph, and when the page heard of it. */
interface Bar {
  sample: FrameSample;
  arrived: number;
  /** The first frame after a pause: its interval is the pause's. */
  resumed: boolean;
}

/** Creates the overlay, hidden, in `parent`. */
export function createDebugOverlay(parent: HTMLElement): DebugOverlay {
  const hud = el('section', 'debug debug-hud');
  hud.setAttribute('aria-label', 'Debug overlay');
  const headline = el('div', 'debug-headline');
  const fps = el('span', 'debug-fps');
  const frameMs = el('span', 'debug-muted');
  headline.append(fps, frameMs);

  const rows = new Map<string, HTMLElement>();
  const grid = (title: string, labels: string[]) => {
    const group = el('div', '');
    const heading = el('div', 'debug-group');
    heading.textContent = title;
    const list = el('dl', 'debug-grid');
    for (const label of labels) {
      const dt = el('dt', '');
      dt.textContent = label;
      const dd = el('dd', '');
      rows.set(label, dd);
      list.append(dt, dd);
    }
    group.append(heading, list);
    return group;
  };

  const graph = document.createElement('canvas');
  graph.className = 'debug-graph';
  graph.style.width = `${HISTORY * BAR}px`;
  graph.style.height = `${GRAPH_HEIGHT}px`;
  const legend = el('ul', 'debug-legend');
  const legendValues = new Map<Phase, HTMLElement>();
  for (const phase of PHASES) {
    const item = el('li', '');
    item.title = PHASE_INFO[phase].title;
    const swatch = el('i', '');
    swatch.style.background = PHASE_INFO[phase].color;
    const value = el('b', '');
    legendValues.set(phase, value);
    item.append(swatch, phase, value);
    legend.append(item);
  }
  const legendNote = el('div', 'debug-legend-note');

  hud.append(
    headline,
    grid('camera', ['center', 'zoom', 'bearing', 'pitch', 'tile']),
    grid('render', ['backend', 'size', 'texture', 'draws']),
    grid('memory', ['wasm', 'GPU']),
    grid('load', ['requests', 'loaded']),
    graph,
    legend,
    legendNote,
  );
  hud.hidden = true;
  parent.append(hud);

  const bars: Bar[] = [];
  // The frames of the last second, for the frame rate, which the graph's
  // history would cap.
  const lastSecond: Bar[] = [];
  let hovered: number | null = null; // index into bars
  let shown = false;
  let ticker: ReturnType<typeof setInterval> | null = null;
  let drawQueued = false;

  const set = (label: string, text: string) => {
    const row = rows.get(label);
    if (row && row.textContent !== text) row.textContent = text;
  };

  // The frame rate counts the frames that drew in the last second, so it
  // falls to 0 while the map is still.
  function showRate() {
    const now = performance.now();
    while (lastSecond.length > 0 && now - (lastSecond[0]?.arrived ?? now) > 1000) lastSecond.shift();
    fps.textContent = `${lastSecond.length} fps`;
    const intervals = lastSecond.filter((bar) => !bar.resumed).map((bar) => bar.sample.interval);
    frameMs.textContent = intervals.length === 0
      ? 'still'
      : `${(intervals.reduce((sum, ms) => sum + ms, 0) / intervals.length).toFixed(1)} ms · ` +
        `max ${Math.max(...intervals).toFixed(1)}`;
    showLegend();
  }

  function showLegend() {
    const hoveredBar = hovered === null ? undefined : bars[hovered];
    const values = hoveredBar?.sample.phases ?? averagePhases(bars.slice(-AVERAGE));
    PHASES.forEach((phase, i) => {
      const value = legendValues.get(phase);
      if (value) value.textContent = (values[i] ?? 0).toFixed(1);
    });
    const total = values.reduce((sum, v) => sum + v, 0);
    if (hoveredBar) {
      const age = bars.length - 1 - (hovered ?? 0);
      legendNote.textContent = `frame −${age}: ${total.toFixed(1)} ms work` +
        (hoveredBar.resumed ? ', after a pause' : `, ${hoveredBar.sample.interval.toFixed(1)} ms after the last`);
    } else {
      legendNote.textContent = bars.length === 0
        ? 'no frames drawn yet'
        : `last ${Math.min(AVERAGE, bars.length)} frames: ${total.toFixed(1)} ms avg work`;
    }
  }

  function draw() {
    drawQueued = false;
    const ratio = window.devicePixelRatio || 1;
    const width = Math.round(HISTORY * BAR * ratio);
    const height = Math.round(GRAPH_HEIGHT * ratio);
    if (graph.width !== width || graph.height !== height) {
      graph.width = width;
      graph.height = height;
    }
    const context = graph.getContext('2d');
    if (!context) return;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, HISTORY * BAR, GRAPH_HEIGHT);
    const y = (ms: number) => GRAPH_HEIGHT - Math.min(ms, GRAPH_MS) / GRAPH_MS * GRAPH_HEIGHT;
    const offset = HISTORY - bars.length;
    bars.forEach((bar, i) => {
      const x = (offset + i) * BAR;
      if (bar.resumed) {
        // Where the graph paused, rather than the pause's length.
        context.fillStyle = 'rgb(255 255 255 / 0.45)';
        for (let dash = 0; dash < GRAPH_HEIGHT; dash += 4) context.fillRect(x, dash, 1, 2);
      } else {
        // The time since the frame before, behind the work.
        context.fillStyle = i === hovered ? 'rgb(255 255 255 / 0.32)' : 'rgb(255 255 255 / 0.1)';
        context.fillRect(x, y(bar.sample.interval), BAR, GRAPH_HEIGHT - y(bar.sample.interval));
      }
      let top = 0;
      PHASES.forEach((phase, p) => {
        const ms = bar.sample.phases[p] ?? 0;
        if (ms <= 0) return;
        context.fillStyle = PHASE_INFO[phase].color;
        context.fillRect(x, y(top + ms), BAR, y(top) - y(top + ms));
        top += ms;
      });
    });
    // The frame rates, labelled at the right, where the newest frames are.
    context.font = '9px ui-monospace, monospace';
    context.textAlign = 'right';
    context.textBaseline = 'middle';
    for (const rate of RATES) {
      const line = Math.round(y(1000 / rate));
      const label = `${rate}`;
      context.fillStyle = 'rgb(255 255 255 / 0.3)';
      context.fillRect(0, line, HISTORY * BAR, 1);
      const labelWidth = context.measureText(label).width + 6;
      context.fillStyle = 'rgb(14 18 24 / 0.85)';
      context.fillRect(HISTORY * BAR - labelWidth, line - 5, labelWidth, 10);
      context.fillStyle = 'rgb(255 255 255 / 0.65)';
      context.fillText(label, HISTORY * BAR - 3, line);
    }
  }

  function queueDraw() {
    if (drawQueued || !shown) return;
    drawQueued = true;
    requestAnimationFrame(draw);
  }

  graph.addEventListener('pointermove', (e) => {
    const bar = Math.floor(e.offsetX / BAR) - (HISTORY - bars.length);
    hovered = bar >= 0 && bar < bars.length ? bar : null;
    showLegend();
    queueDraw();
  });
  graph.addEventListener('pointerleave', () => {
    hovered = null;
    showLegend();
    queueDraw();
  });

  return {
    get shown() {
      return shown;
    },
    show(on) {
      shown = on;
      hud.hidden = !on;
      bars.length = 0;
      lastSecond.length = 0;
      hovered = null;
      if (ticker !== null) clearInterval(ticker);
      ticker = on ? setInterval(showRate, TEXT_MS) : null;
      if (on) {
        showRate();
        queueDraw();
      }
    },
    frame(sample) {
      if (!shown) return;
      const bar = { sample, arrived: performance.now(), resumed: bars.length === 0 || sample.interval > PAUSE_MS };
      bars.push(bar);
      lastSecond.push(bar);
      // The graph stays put under a hovering pointer.
      if (bars.length > HISTORY) {
        bars.shift();
        if (hovered !== null) hovered = hovered > 0 ? hovered - 1 : null;
      }
      queueDraw();
    },
    stats({ drawCalls, memory, gpuMemory, pending, requests, loadedMs }) {
      set('draws', `${drawCalls.toLocaleString('en')} calls`);
      set('wasm', mib(memory));
      set('GPU', mib(gpuMemory));
      set('requests', `${pending.toLocaleString('en')} pending · ${requests.toLocaleString('en')} made`);
      set('loaded', loadedMs === null ? 'loading…' : `in ${Math.round(loadedMs).toLocaleString('en')} ms`);
    },
    camera([lat, lon, zoom, bearing, pitch]) {
      set('center', `${lat.toFixed(5)}, ${lon.toFixed(5)}`);
      set('zoom', zoom.toFixed(2));
      set('bearing', `${bearing.toFixed(1)}°`);
      set('pitch', `${pitch.toFixed(1)}°`);
      set('tile', tileAt(lat, lon, zoom));
    },
    page({ backend, width, height, ratio }) {
      set('backend', backend);
      set('size', `${width}×${height} CSS px`);
      const scale = Number(ratio.toFixed(2));
      set('texture', `${Math.round(width * ratio)}×${Math.round(height * ratio)} px (${scale}×)`);
    },
  };
}

function averagePhases(bars: Bar[]): number[] {
  const sums = PHASES.map(() => 0);
  for (const { sample } of bars) PHASES.forEach((_, i) => (sums[i] = (sums[i] ?? 0) + (sample.phases[i] ?? 0)));
  return sums.map((sum) => (bars.length ? sum / bars.length : 0));
}

function mib(bytes: number): string {
  return `${(bytes / 2 ** 20).toFixed(1)} MiB`;
}

/** The z/x/y of the Web Mercator tile at the camera's center and whole zoom. */
function tileAt(lat: number, lon: number, zoom: number): string {
  const z = Math.max(0, Math.floor(zoom));
  const n = 2 ** z;
  const clamped = Math.max(-85.0511287798, Math.min(85.0511287798, lat)) * Math.PI / 180;
  const wrap = (v: number) => ((v % n) + n) % n;
  const x = wrap(Math.floor((lon + 180) / 360 * n));
  const y = Math.min(n - 1, Math.max(0, Math.floor((1 - Math.asinh(Math.tan(clamped)) / Math.PI) / 2 * n)));
  return `${z}/${x}/${y}`;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  if (className) element.className = className;
  return element;
}
