import { WHEEL_NOTCHES, WHEEL_PIXELS } from "./protocol.ts";

// A wheel event as the recognizer takes it (WebMap::wheel's `kind`):
// WHEEL_NOTCHES and the notches for a mouse wheel's detents, which it eases,
// or WHEEL_PIXELS and the pixels for continuous scrolling (a trackpad, a
// precise wheel, a trackpad pinch), which it applies at once. Positive scrolls
// down. Browsers don't say which device scrolled, so this reads it from how
// each reports a notch, as their source has it:
// - Firefox reports a mouse wheel in lines, but only to a page that reads
//   `deltaMode` before `deltaY`; for any other it converts them to pixels. On
//   Windows and Linux a notch is 6 lines (the system's 3, doubled by its
//   `mousewheel.system_scroll_override`) and a `wheelDeltaY` of ∓120 from the
//   wheel's ticks. On macOS it is `NSEvent.deltaY` rounded away from 0, a line
//   for a slow notch, and it has no ticks.
// - Safari, and Chromium on macOS, report a notch as 40 × `NSEvent.deltaY`,
//   4.000244140625 px for a slow one, and a faster turn as a whole multiple
//   of that.
// - Chromium reports a notch on Windows as 100 px (Windows' 3 lines; 33 px at
//   its lowest), and on Linux as 120 px, with a legacy `wheelDeltaY` of ±120.
//   On macOS that `wheelDeltaY` counts the notches when acceleration breaks
//   the multiple above. Its precise scrolling gives a `wheelDeltaY` of −3 ×
//   `deltaY` on macOS and about −`deltaY` elsewhere.
// - Every browser sends a trackpad pinch as pixels with Ctrl. Chromium gives
//   each update a `wheelDeltaY` of ±120 as well, but a `deltaY` of 100 ×
//   ln(scale), a few px for its 120 where a notch has at least 33.
// Chromium divides both deltas by the browser's zoom, so away from 100% its
// notches miss these rules and read as pixels.

/** The fields of a WheelEvent this reads; `wheelDeltaY` is the legacy one. */
export interface WheelDeltas {
  readonly deltaMode: number;
  readonly deltaY: number;
  readonly wheelDeltaY?: number;
  readonly ctrlKey: boolean;
}

// WheelEvent's DOM_DELTA_LINE and DOM_DELTA_PAGE, which Node, where the tests
// run, doesn't have.
const DOM_DELTA_LINE = 1;
const DOM_DELTA_PAGE = 2;
const MAC_NOTCH = 4.000244140625;

/** Reads `deltaMode` before `deltaY`, so Firefox keeps a wheel's lines. */
export function wheelDelta(e: WheelDeltas): { wheel: number; amount: number } {
  const mode = e.deltaMode;
  const legacy = e.wheelDeltaY ?? 0;
  const ticks = legacy !== 0 && legacy % 120 === 0 ? -legacy / 120 : undefined;
  if (mode === DOM_DELTA_LINE)
    return { wheel: WHEEL_NOTCHES, amount: ticks ?? e.deltaY };
  if (mode === DOM_DELTA_PAGE)
    return { wheel: WHEEL_NOTCHES, amount: e.deltaY };
  const delta = e.deltaY;
  if (delta !== 0 && delta % MAC_NOTCH === 0)
    return { wheel: WHEEL_NOTCHES, amount: delta / MAC_NOTCH };
  const precise = Math.abs(legacy + 3 * delta) < 1;
  const pinch = e.ctrlKey && 4 * Math.abs(delta) < Math.abs(legacy);
  if (ticks !== undefined && !precise && !pinch)
    return { wheel: WHEEL_NOTCHES, amount: ticks };
  return { wheel: WHEEL_PIXELS, amount: delta };
}
