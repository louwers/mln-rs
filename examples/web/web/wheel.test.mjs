// wheelDelta against each browser's events, as their source has them (see
// wheel.ts). Runs with `pnpm test`, in Node, which strips the types from
// wheel.ts.
import assert from "node:assert/strict";
import { test } from "node:test";

import { WHEEL_NOTCHES, WHEEL_PIXELS } from "./protocol.ts";
import { wheelDelta } from "./wheel.ts";

const PIXELS = 0;
const LINES = 1;
const PAGES = 2;

const read = (deltaMode, deltaY, wheelDeltaY, ctrlKey = false) =>
  wheelDelta({ deltaMode, deltaY, wheelDeltaY, ctrlKey });
const notches = (amount) => ({ wheel: WHEEL_NOTCHES, amount });
const pixels = (amount) => ({ wheel: WHEEL_PIXELS, amount });

test("Firefox on Windows and Linux: a notch is 6 lines and ∓120 ticks", () => {
  assert.deepEqual(read(LINES, 6, -120), notches(1));
  assert.deepEqual(read(LINES, -12, 240), notches(-2));
  assert.deepEqual(read(LINES, 6, -120, true), notches(1));
});

test("Firefox on macOS: a notch is its lines, with no ticks", () => {
  // wheelDeltaY is −round(3 × deltaY in px), with a line of 19 px.
  assert.deepEqual(read(LINES, 1, -57), notches(1));
  assert.deepEqual(read(LINES, -3, 171), notches(-3));
});

test("pages are notches", () => {
  assert.deepEqual(read(PAGES, 1, -120), notches(1));
});

test("Safari and Chromium on macOS: a notch is 4.000244140625 px", () => {
  assert.deepEqual(read(PIXELS, 4.000244140625, -12), notches(1));
  assert.deepEqual(read(PIXELS, -12.000732421875, 36), notches(-3));
  assert.deepEqual(read(PIXELS, 4.000244140625, -120), notches(1));
  assert.deepEqual(read(PIXELS, 4.000244140625, -12, true), notches(1));
});

test("Chromium on macOS: an accelerated notch off that multiple counts its ticks", () => {
  assert.deepEqual(read(PIXELS, 12, -120), notches(1));
});

test("Chromium on Windows and Linux: a notch is ±120 ticks, with or without Ctrl", () => {
  assert.deepEqual(read(PIXELS, 100, -120), notches(1));
  assert.deepEqual(read(PIXELS, -120, 120), notches(-1));
  assert.deepEqual(read(PIXELS, 33.333333, -120), notches(1));
  assert.deepEqual(read(PIXELS, 200, -240), notches(2));
  assert.deepEqual(read(PIXELS, 100, -120, true), notches(1));
  assert.deepEqual(read(PIXELS, 120, -120, true), notches(1));
});

test("a trackpad pinch is pixels in every browser", () => {
  // Chromium: 100 × ln(scale) a step and ±120 ticks, 5% in and out.
  const step = 100 * Math.log(1.05);
  assert.deepEqual(read(PIXELS, -step, 120, true), pixels(-step));
  assert.deepEqual(read(PIXELS, step, -120, true), pixels(step));
  // Firefox and Safari: −100 × the magnification, and −3 × deltaY ticks.
  assert.deepEqual(read(PIXELS, -5, 15, true), pixels(-5));
  assert.deepEqual(read(PIXELS, 40, -120, true), pixels(40));
});

test("precise scrolling is pixels", () => {
  // Chromium on macOS, Firefox and Safari: −3 × deltaY ticks.
  assert.deepEqual(read(PIXELS, 40, -120), pixels(40));
  assert.deepEqual(read(PIXELS, 7.5, -22), pixels(7.5));
  // Chromium on Windows and Linux: about −deltaY.
  assert.deepEqual(read(PIXELS, 37, -37), pixels(37));
});
