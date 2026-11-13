// The debug overlay (F3, `?debug`, or the switch in the about panel): the
// frame rate and the map's numbers, the camera, a graph of the last frames'
// time stacked by phase, and the features under the pointer. The map's Web
// Worker reports frames, numbers and features only while it shows.
//
// Hovering a bar of the graph shows that frame's phases in the legend, which
// otherwise shows their average over the last second.

import { type Camera, type DebugFeature, type DebugStats, type FrameSample, PHASES } from './protocol.ts';

type Phase = (typeof PHASES)[number];

const PHASE_INFO: Record<Phase, { color: string; title: string }> = {
  input: { color: '#a594ff', title: 'Gestures and animations stepping the camera' },
  update: { color: '#4cc3ff', title: "The map's RunLoop, at the start of the frame" },
  render: { color: '#5ee08a', title: 'Drawing the frame and submitting it to the GPU' },
  tasks: { color: '#ffb547', title: "The cooperative scheduler's tasks: sprites, glyphs, tiles (8 ms budget)" },
  between: { color: '#ff7a9a', title: "The map's RunLoop between this frame and the one before" },
};

/** Frames the graph keeps: about two seconds at 60 Hz. */
const HISTORY = 120;
/** The graph's width per frame and its height, in CSS pixels. */
const BAR = 2;
const GRAPH_HEIGHT = 64;
/** The milliseconds the graph's height spans; taller frames are clipped. */
const GRAPH_MS = 40;
/** How often the numbers change, so that they stay readable. */
const TEXT_MS = 250;
/** The features the panel lists at most, and their properties. */
const MAX_FEATURES = 6;
const MAX_PROPERTIES = 10;

export interface DebugOverlay {
  /** Whether it shows. */
  readonly shown: boolean;
  show(on: boolean): void;
  frame(sample: FrameSample): void;
  stats(stats: DebugStats): void;
  camera(camera: Camera): void;
  /**
   * What the page knows of the map, rather than its Web Worker: the canvas's
   * size in device pixels, and its pixel ratio.
   */
  page(info: { style: string; backend: string; width: number; height: number; ratio: number }): void;
  /** The features at `point`, or null once the pointer left the canvas. */
  features(point: [number, number] | null, features: DebugFeature[]): void;
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
    grid('map', ['style', 'canvas', 'backend', 'replies', 'loaded', 'memory']),
    graph,
    legend,
    legendNote,
  );

  const panel = el('section', 'debug debug-features');
  panel.setAttribute('aria-label', 'Features under the pointer');
  panel.setAttribute('aria-live', 'off');

  hud.hidden = true;
  panel.hidden = true;
  parent.append(hud, panel);

  const samples: FrameSample[] = [];
  let hovered: number | null = null; // index into samples
  let shown = false;
  let textShown = 0;
  let drawQueued = false;

  const set = (label: string, text: string) => {
    const row = rows.get(label);
    if (row && row.textContent !== text) row.textContent = text;
  };

  function showText(force: boolean) {
    const now = performance.now();
    if (!force && now - textShown < TEXT_MS) return;
    textShown = now;
    const recent = lastSecond();
    if (recent.length > 0) {
      const average = recent.reduce((sum, s) => sum + s.interval, 0) / recent.length;
      const worst = Math.max(...recent.map((s) => s.interval));
      fps.textContent = `${Math.round(1000 / average)} fps`;
      frameMs.textContent = `${average.toFixed(1)} ms · max ${worst.toFixed(1)}`;
    }
    showLegend();
  }

  // The samples of the last second, newest last.
  function lastSecond(): FrameSample[] {
    let total = 0;
    let i = samples.length;
    while (i > 0 && total < 1000) total += samples[--i]?.interval ?? 0;
    return samples.slice(i).filter((s) => s.interval > 0);
  }

  function showLegend() {
    const hoveredSample = hovered === null ? undefined : samples[hovered];
    const values = hoveredSample?.phases ?? averagePhases(lastSecond());
    PHASES.forEach((phase, i) => {
      const value = legendValues.get(phase);
      if (value) value.textContent = (values[i] ?? 0).toFixed(1);
    });
    const total = values.reduce((sum, v) => sum + v, 0);
    legendNote.textContent = hoveredSample
      ? `frame −${samples.length - 1 - (hovered ?? 0)}: ${total.toFixed(1)} ms work, ` +
        `${hoveredSample.interval.toFixed(1)} ms frame`
      : `average: ${total.toFixed(1)} ms work per frame`;
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
    const offset = HISTORY - samples.length;
    samples.forEach((sample, i) => {
      const x = (offset + i) * BAR;
      // The frame's whole interval behind its work.
      context.fillStyle = i === hovered ? 'rgb(255 255 255 / 0.32)' : 'rgb(255 255 255 / 0.1)';
      context.fillRect(x, y(sample.interval), BAR, GRAPH_HEIGHT - y(sample.interval));
      let top = 0;
      PHASES.forEach((phase, p) => {
        const ms = sample.phases[p] ?? 0;
        if (ms <= 0) return;
        context.fillStyle = PHASE_INFO[phase].color;
        context.fillRect(x, y(top + ms), BAR, y(top) - y(top + ms));
        top += ms;
      });
    });
    // 60 and 30 fps, labelled at the right, where the newest frames are.
    context.font = '9px ui-monospace, monospace';
    context.textAlign = 'right';
    context.textBaseline = 'middle';
    for (const [ms, label] of [[1000 / 60, '60 fps'], [1000 / 30, '30 fps']] as const) {
      const line = Math.round(y(ms));
      context.fillStyle = 'rgb(255 255 255 / 0.3)';
      context.fillRect(0, line, HISTORY * BAR, 1);
      const width = context.measureText(label).width + 6;
      context.fillStyle = 'rgb(14 18 24 / 0.85)';
      context.fillRect(HISTORY * BAR - width, line - 5, width, 10);
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
    const bar = Math.floor(e.offsetX / BAR) - (HISTORY - samples.length);
    hovered = bar >= 0 && bar < samples.length ? bar : null;
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
      if (!on) panel.hidden = true;
      samples.length = 0;
      hovered = null;
      if (on) showText(true);
    },
    frame(sample) {
      if (!shown) return;
      samples.push(sample);
      // The graph stays put under a hovering pointer.
      if (samples.length > HISTORY) {
        samples.shift();
        if (hovered !== null) hovered = hovered > 0 ? hovered - 1 : null;
      }
      showText(false);
      queueDraw();
    },
    stats({ tileWorkers, replies, memory, loadedMs }) {
      set('replies', `${replies.toLocaleString('en')} from ${tileWorkers} tile workers`);
      set('loaded', loadedMs === null ? 'loading…' : `in ${Math.round(loadedMs).toLocaleString('en')} ms`);
      set('memory', memory > 0 ? `${(memory / 2 ** 20).toFixed(1)} MiB wasm` : '–');
    },
    camera([lat, lon, zoom, bearing, pitch]) {
      set('center', `${lat.toFixed(5)}, ${lon.toFixed(5)}`);
      set('zoom', zoom.toFixed(2));
      const heading = ((bearing % 360) + 360) % 360;
      set('bearing', `${bearing.toFixed(1)}° ${compassPoint(heading)}`);
      set('pitch', `${pitch.toFixed(1)}°`);
      set('tile', tileAt(lat, lon, zoom));
    },
    page({ style, backend, width, height, ratio }) {
      set('style', style);
      set('backend', backend);
      set('canvas', `${width}×${height} px @ ${Number(ratio.toFixed(2))}×`);
    },
    features(point, features) {
      panel.hidden = !shown || point === null;
      if (panel.hidden || point === null) return;
      panel.replaceChildren(featureList(point, features));
    },
  };
}

function averagePhases(samples: FrameSample[]): number[] {
  const sums = PHASES.map(() => 0);
  for (const sample of samples) PHASES.forEach((_, i) => (sums[i] = (sums[i] ?? 0) + (sample.phases[i] ?? 0)));
  return sums.map((sum) => (samples.length ? sum / samples.length : 0));
}

function featureList([x, y]: [number, number], features: DebugFeature[]): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const heading = el('div', 'debug-group');
  heading.textContent = features.length === 0
    ? `nothing at ${Math.round(x)}, ${Math.round(y)}`
    : `${features.length} ${features.length === 1 ? 'feature' : 'features'} at ${Math.round(x)}, ${Math.round(y)}`;
  fragment.append(heading);
  for (const feature of features.slice(0, MAX_FEATURES)) {
    const item = el('article', 'debug-feature');
    const title = el('div', 'debug-feature-title');
    const layer = el('span', 'debug-feature-layer');
    layer.textContent = feature.layer;
    title.append(layer);
    if (feature.type) {
      const type = el('span', 'debug-pill');
      type.textContent = feature.type;
      title.append(type);
    }
    const origin = el('div', 'debug-muted');
    origin.textContent = [
      feature.sourceLayer ? `${feature.source} › ${feature.sourceLayer}` : feature.source,
      feature.id === null ? null : `#${feature.id}`,
      feature.geometry,
    ].filter((part) => part !== null).join(' · ');
    item.append(title, origin);

    const entries = [
      ...Object.entries(feature.properties),
      ...Object.entries(feature.state).map(([key, value]) => [`state.${key}`, value] as const),
    ];
    if (entries.length > 0) {
      const properties = el('dl', 'debug-grid debug-properties');
      for (const [key, value] of entries.slice(0, MAX_PROPERTIES)) {
        const dt = el('dt', '');
        dt.textContent = key;
        dt.title = key;
        const dd = el('dd', '');
        dd.textContent = typeof value === 'string' ? value : JSON.stringify(value);
        dd.title = dd.textContent;
        properties.append(dt, dd);
      }
      item.append(properties);
      if (entries.length > MAX_PROPERTIES) {
        const more = el('div', 'debug-muted debug-more');
        more.textContent = `+${entries.length - MAX_PROPERTIES} more`;
        item.append(more);
      }
    }
    fragment.append(item);
  }
  if (features.length > MAX_FEATURES) {
    const more = el('div', 'debug-muted debug-more');
    more.textContent = `+${features.length - MAX_FEATURES} more`;
    fragment.append(more);
  }
  return fragment;
}

const COMPASS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'];

/** The compass point the map faces at `heading` degrees, [0, 360). */
function compassPoint(heading: number): string {
  return COMPASS[Math.round(heading / 45) % 8] ?? 'N';
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
