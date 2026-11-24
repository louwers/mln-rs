// The style picker: a dialog over the map with a preview of every style of
// styles.ts, grouped by who makes it. Each preview is a map of its own that
// shows the area the map shows, in the map's shape, zoomed out to fit, and
// follows the map's camera while the picker is open: the dialog takes no
// input from the map around it, which stays in use.
//
// The previews' maps run in one Web Worker (preview-worker.ts) with tile
// workers' Web Workers of its own, on the module the page compiled. They
// start when the picker opens and the page terminates them when it closes,
// which frees their memory, so a closed picker costs the map nothing. Each
// preview draws into a canvas that the page makes then, and transfers. When
// the previews stop, on a panic or where the browser can't draw them, the
// picker goes on without them.

import type {
  Backend,
  Camera,
  FromPreviews,
  ProjectionType,
  ToPreviews,
} from "./protocol.ts";
import { STYLES, type Style } from "./styles.ts";
import { startTileWorker, tileWorker } from "./workers.ts";

/**
 * A preview's least width, and the space between two (page.css's gap), in
 * CSS pixels.
 */
const MIN_WIDTH = 150;
const GAP = 12;
/**
 * The least and the most of a preview's height for its width, which is the
 * map's between them: a wide window's previews stay tall enough to read,
 * and a phone's short enough to list.
 */
const MIN_ASPECT = 0.5;
const MAX_ASPECT = 1.25;

export interface StylePicker {
  /** Whether the dialog is open. */
  readonly open: boolean;
  show(): void;
  hide(): void;
  /** Marks the style that the map shows. */
  setCurrent(style: Style): void;
  /** The map's camera, which the previews follow. */
  setCamera(camera: Camera): void;
  /** The map's size in CSS pixels, whose shape the previews take. */
  setMapSize(size: [number, number]): void;
  /** The projection the map draws with in place of the styles' own. */
  setProjection(projection: ProjectionType | null): void;
  /** Device pixels per CSS pixel. */
  setRatio(ratio: number): void;
}

export interface StylePickerOptions {
  /** The dialog, a manual popover, and the element in it for the styles. */
  dialog: HTMLElement;
  list: HTMLElement;
  /** The button that opens and closes the dialog. */
  button: HTMLButtonElement;
  backend: Backend;
  /** The module the previews' Web Workers instantiate. */
  module: Promise<WebAssembly.Module>;
  /** How many tile workers' Web Workers the previews share. */
  tileWorkers: number;
  camera: Camera;
  /** The map's size, in CSS pixels. */
  mapSize: [number, number];
  ratio: number;
  /** As the map's (protocol.ts's Start). */
  fallbackFontFaces: string | null;
  localIdeographs: boolean;
  /** The user picked `style`. */
  pick(style: Style): void;
}

/** The Web Workers of the previews, while the picker is open. */
interface Previews {
  worker: Worker;
  tileWorkers: Worker[];
  /** Whether the worker got its start, and so takes the other messages. */
  started: boolean;
}

/** The styles under the headings they are listed by. */
function groups(): Map<string, Style[]> {
  // A provider of several styles is a heading; the others share one.
  const counts = new Map<string, number>();
  for (const style of STYLES)
    counts.set(style.provider, (counts.get(style.provider) ?? 0) + 1);
  const groups = new Map<string, Style[]>();
  for (const style of STYLES) {
    const heading =
      (counts.get(style.provider) ?? 0) > 1 ? style.provider : "Other";
    const group = groups.get(heading);
    if (group) group.push(style);
    else groups.set(heading, [style]);
  }
  return groups;
}

export function createStylePicker(options: StylePickerOptions): StylePicker {
  const { dialog, list, button, backend } = options;
  let { camera, mapSize, ratio } = options;
  let projection: ProjectionType | null = null;
  let previews: Previews | null = null;
  // Every preview's size in CSS pixels, as laid out last.
  let size: [number, number] = [0, 0];

  // The styles' buttons, and the boxes their previews' canvases go in, in
  // STYLES's order within each group.
  const entries: {
    style: Style;
    button: HTMLButtonElement;
    box: HTMLElement;
  }[] = [];
  list.replaceChildren(
    ...[...groups()].map(([heading, styles]) => {
      const title = document.createElement("h3");
      title.className = "style-group-title";
      title.textContent = heading;
      const items = document.createElement("ul");
      items.className = "style-group-list";
      for (const style of styles) {
        const box = document.createElement("span");
        box.className = "style-preview";
        const name = document.createElement("span");
        name.className = "style-name";
        name.textContent = style.name;
        const text = document.createElement("span");
        text.className = "style-text";
        text.append(name);
        if (heading === "Other" && style.provider !== style.name) {
          const provider = document.createElement("span");
          provider.className = "style-detail";
          provider.textContent = style.provider;
          text.append(provider);
        }
        const option = document.createElement("button");
        option.type = "button";
        option.className = "style-option";
        option.append(box, text);
        option.addEventListener("click", () => options.pick(style));
        const item = document.createElement("li");
        item.append(option);
        items.append(item);
        entries.push({ style, button: option, box });
      }
      const section = document.createElement("section");
      section.className = "style-group";
      section.append(title, items);
      return section;
    }),
  );

  const post = (message: ToPreviews, transfer: Transferable[] = []) =>
    previews?.worker.postMessage(message, transfer);
  // The camera a preview shows for the map's: zoomed out until the preview
  // holds all that the map shows, which is more than that along one axis
  // where their shapes differ, and at zoom 0.
  function previewCamera(): Camera {
    const [lat, lon, zoom, bearing, pitch] = camera;
    const smaller = Math.max(mapSize[0] / size[0], mapSize[1] / size[1], 1);
    return [lat, lon, Math.max(0, zoom - Math.log2(smaller)), bearing, pitch];
  }
  const sendCamera = () => {
    if (previews?.started) post({ type: "camera", camera: previewCamera() });
  };

  // Lays the previews out in as many columns as fit the list, in the map's
  // shape, at whole CSS pixels. A preview's box covers exactly the device
  // pixels its map draws, the size times the pixel ratio, rounded down, as
  // the map's canvas does (main.ts). Returns whether the previews' size
  // changed.
  function layout(): boolean {
    const available = entries[0]?.box.closest("ul")?.clientWidth ?? 0;
    if (available === 0) return false; // not shown
    const columns = Math.max(
      1,
      Math.floor((available + GAP) / (MIN_WIDTH + GAP)),
    );
    const width = Math.floor((available - (columns - 1) * GAP) / columns);
    const aspect = mapSize[1] / Math.max(1, mapSize[0]);
    const height = Math.round(
      width * Math.min(MAX_ASPECT, Math.max(MIN_ASPECT, aspect)),
    );
    const drawn = (length: number) => Math.floor(length * ratio) / ratio;
    list.style.setProperty("--preview-column", `${width}px`);
    list.style.setProperty("--preview-width", `${drawn(width)}px`);
    list.style.setProperty("--preview-height", `${drawn(height)}px`);
    const changed = width !== size[0] || height !== size[1];
    size = [width, height];
    return changed;
  }
  // Lays the previews out again: their maps take the new size, and the
  // camera that shows the map's area in it.
  function relayout() {
    if (layout() && previews?.started)
      post({ type: "resize", width: size[0], height: size[1] });
    sendCamera();
  }
  new ResizeObserver(relayout).observe(list);

  function start() {
    layout();
    const canvases = entries.map(({ box }) => {
      const canvas = document.createElement("canvas");
      box.replaceChildren(canvas);
      return canvas;
    });
    const worker =
      backend === "webgl2"
        ? new Worker(new URL("./preview-worker.ts", import.meta.url), {
            type: "module",
            name: "webgl2",
          })
        : new Worker(new URL("./preview-worker.ts", import.meta.url), {
            type: "module",
          });
    const mine: Previews = {
      worker,
      tileWorkers: Array.from({ length: options.tileWorkers }, () =>
        tileWorker(backend),
      ),
      started: false,
    };
    previews = mine;
    worker.onmessage = ({ data }: MessageEvent<FromPreviews>) => {
      console.error(`the style previews stopped: ${data.message}`);
      if (previews === mine) stop();
    };
    // A module that doesn't compile stops the map, which the page reports.
    void options.module.then(
      (module) => {
        if (previews !== mine) return; // closed meanwhile
        const controls = mine.tileWorkers.map((tiles) =>
          startTileWorker(tiles, module),
        );
        const offscreen = canvases.map((canvas) =>
          canvas.transferControlToOffscreen(),
        );
        post(
          {
            type: "start",
            module,
            tileWorkers: controls,
            previews: entries.map(({ style }, i) => ({
              // One per entry, made above.
              canvas: offscreen[i]!,
              style: style.source,
              terrain: style.terrain,
            })),
            width: size[0],
            height: size[1],
            ratio,
            camera: previewCamera(),
            projection,
            fallbackFontFaces: options.fallbackFontFaces,
            localIdeographs: options.localIdeographs,
          },
          [...offscreen, ...controls],
        );
        mine.started = true;
      },
      () => {},
    );
  }

  function stop() {
    if (!previews) return;
    previews.worker.terminate();
    for (const tiles of previews.tileWorkers) tiles.terminate();
    previews = null;
    // A transferred canvas draws nothing more: the next start makes new ones.
    for (const { box } of entries) box.replaceChildren();
  }

  let current: Style | null = null;
  function show() {
    if (dialog.matches(":popover-open")) return;
    dialog.showPopover();
    button.setAttribute("aria-expanded", "true");
    start();
    const option =
      entries.find(({ style }) => style === current)?.button ??
      entries[0]?.button;
    option?.focus();
  }
  function hide() {
    if (!dialog.matches(":popover-open")) return;
    const focused = dialog.contains(document.activeElement);
    dialog.hidePopover();
    button.setAttribute("aria-expanded", "false");
    stop();
    if (focused) button.focus();
  }
  // A manual popover: the map around it stays in use, so only the page
  // closes it.
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") hide();
  });

  return {
    get open() {
      return dialog.matches(":popover-open");
    },
    show,
    hide,
    setCurrent(style) {
      current = style;
      for (const entry of entries)
        entry.button.setAttribute(
          "aria-current",
          String(entry.style.key === style.key),
        );
    },
    setCamera(next) {
      camera = next;
      sendCamera();
    },
    setMapSize(next) {
      mapSize = next;
      relayout();
    },
    setProjection(next) {
      projection = next;
      if (previews?.started) post({ type: "projection", projection });
    },
    setRatio(next) {
      ratio = next;
      layout();
      if (previews?.started) post({ type: "pixelRatio", ratio });
    },
  };
}
