// The comparison with MapLibre GL JS: a GL JS map of the same style over the
// page's canvas, clipped to the right of a divider, so that each side shows
// one map and takes the input on its own side. The page imports this module
// the first time a comparison opens, so GL JS loads only then.
//
// The map that last took input leads, and the other jumps to its camera. A
// follower's camera changes don't move the leader, so the maps don't echo
// each other's moves. When the lead changes hands, the new follower only
// stops its own motion: the new leader keeps the camera it has.
//
// GL JS's map lives in a box of its own inside the clipped layer: GL JS's
// stylesheet makes its container `position: relative`, which would undo the
// layer's fixed inset and leave the map no height.

import { Map as GlMap, getVersion, setWorkerUrl } from "maplibre-gl";

import "maplibre-gl/dist/maplibre-gl.css";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";

import type { Camera } from "./protocol.ts";

// The bundle moves GL JS away from its worker, so the page says where it is.
setWorkerUrl(workerUrl);

export interface Comparison {
  /** Moves GL JS's map to the page's map's camera, if the page's map leads. */
  cameraChanged(camera: Camera): void;
  /** Makes the page's map lead without input on its canvas: its zoom buttons and compass. */
  lead(): void;
  setStyle(url: string): void;
  /** Removes GL JS's map and the divider. */
  remove(): void;
}

// Where the divider sits, as a fraction of the page's width. It stays when the
// comparison closes, for the next time it opens.
let split = 0.5;

/**
 * Opens a comparison over `canvas` (the page's map) with `style`, at
 * `camera`. `follow(camera)` moves the page's map to GL JS's camera while
 * GL JS leads; `stop()` stops the page's map's own motion when GL JS takes
 * the lead.
 */
export function compare(
  canvas: HTMLCanvasElement,
  {
    style,
    camera,
    follow,
    stop,
  }: {
    style: string;
    camera: Camera;
    follow: (camera: Camera) => void;
    stop: () => void;
  },
): Comparison {
  const layer = document.createElement("div");
  layer.className = "gl-layer";
  const container = document.createElement("div");
  container.className = "gl-map";
  layer.append(container);
  const divider = document.createElement("div");
  divider.className = "divider";
  divider.tabIndex = 0;
  divider.role = "slider";
  divider.ariaLabel = "Divider between the maps";
  divider.ariaValueMin = "0";
  divider.ariaValueMax = "100";
  const labels = [
    label("compare-label mln", "mln-rs"),
    label("compare-label gl", `GL JS ${getVersion()}`),
  ];
  canvas.after(layer, divider, ...labels);

  const [lat, lon, zoom, bearing, pitch] = camera;
  const map = new GlMap({
    container,
    style,
    center: [lon, lat],
    zoom,
    bearing,
    pitch,
    // The page shows the style's attribution for both maps.
    attributionControl: false,
  });
  map.on("error", ({ error }) => console.warn("MapLibre GL JS:", error));

  let leader: "mln" | "gl" = "mln";
  const glCamera = (): Camera => {
    const center = map.getCenter();
    return [
      center.lat,
      center.lng,
      map.getZoom(),
      map.getBearing(),
      map.getPitch(),
    ];
  };
  const listening = new AbortController();
  const { signal } = listening;
  const lead = (next: "mln" | "gl") => {
    if (next === leader) return;
    leader = next;
    if (next === "mln") map.stop();
    else stop();
  };
  // Input on a map makes it lead and stops the other's motion, such as the
  // momentum after a drag. The capture phase sees the input before either
  // map's own handlers do.
  for (const type of ["pointerdown", "wheel", "keydown"] as const) {
    canvas.addEventListener(type, () => lead("mln"), {
      capture: true,
      passive: true,
      signal,
    });
    layer.addEventListener(type, () => lead("gl"), {
      capture: true,
      passive: true,
      signal,
    });
  }
  // A press keeps its pointer on GL JS's map, so a drag that crosses the
  // divider or ends over the controls is still GL JS's.
  layer.addEventListener(
    "pointerdown",
    (e) => {
      if (
        e.target instanceof Element &&
        !e.target.hasPointerCapture(e.pointerId)
      ) {
        e.target.setPointerCapture(e.pointerId);
      }
    },
    { capture: true, signal },
  );
  map.on("move", () => {
    if (leader === "gl") follow(glCamera());
  });

  addDividerHandlers(divider, signal);

  return {
    cameraChanged([lat, lon, zoom, bearing, pitch]) {
      if (leader === "mln")
        map.jumpTo({ center: [lon, lat], zoom, bearing, pitch });
    },
    lead() {
      lead("mln");
    },
    setStyle(url) {
      map.setStyle(url);
    },
    remove() {
      listening.abort();
      map.remove();
      for (const element of [layer, divider, ...labels]) element.remove();
    },
  };
}

function setSplit(divider: HTMLElement, fraction: number) {
  split = Math.min(1, Math.max(0, fraction));
  document.documentElement.style.setProperty("--split", `${split * 100}%`);
  divider.ariaValueNow = String(Math.round(split * 100));
}

// A drag or the arrow keys move the divider.
function addDividerHandlers(divider: HTMLElement, signal: AbortSignal) {
  setSplit(divider, split);
  divider.addEventListener(
    "pointerdown",
    (e) => divider.setPointerCapture(e.pointerId),
    { signal },
  );
  divider.addEventListener(
    "pointermove",
    (e) => {
      if (divider.hasPointerCapture(e.pointerId))
        setSplit(divider, e.clientX / innerWidth);
    },
    { signal },
  );
  divider.addEventListener(
    "keydown",
    (e) => {
      const step = e.shiftKey ? 0.1 : 0.02;
      const steps: Record<string, number> = {
        ArrowLeft: split - step,
        ArrowRight: split + step,
        Home: 0,
        End: 1,
      };
      const next = steps[e.key];
      if (next === undefined) return;
      e.preventDefault();
      setSplit(divider, next);
    },
    { signal },
  );
}

function label(className: string, text: string): HTMLElement {
  const element = document.createElement("div");
  element.className = className;
  element.textContent = text;
  return element;
}
