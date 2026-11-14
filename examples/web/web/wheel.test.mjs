// wheelDelta against each browser's events, as their source has them (see
// wheel.ts). Runs with `pnpm test`, in Node, which strips the types from
// wheel.ts.
import assert from "node:assert/strict";
import { test } from "node:test";

import { WHEEL_NOTCHES, WHEEL_PIXELS } from "./protocol.ts";
import { wheelDelta, wheelReader } from "./wheel.ts";

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

// A pixel event: deltaY, and its wheelDeltaY.
const px = (deltaY, wheelDeltaY, ctrlKey = false) => ({
  deltaMode: PIXELS,
  deltaY,
  wheelDeltaY,
  ctrlKey,
});

test("Safari: a scroll that starts on a notch keeps reading as notches", () => {
  const read = wheelReader();
  assert.deepEqual(read(px(4.000244140625, -12), 0), notches(1));
  // Accelerated: 40 × NSEvent.deltaY px, and −3 × that in ticks.
  assert.deepEqual(read(px(12.5, -37), 50), notches(12.5 / 4.000244140625));
  assert.deepEqual(read(px(-20, 60), 120), notches(-20 / 4.000244140625));
  // Under 4 px is a trackpad's, and ends it.
  assert.deepEqual(read(px(2, -6), 150), pixels(2));
  assert.deepEqual(read(px(12, -36), 200), pixels(12));
});

test("a scroll's notches end after 400 ms, or with Ctrl", () => {
  const read = wheelReader();
  read(px(4.000244140625, -12), 0);
  assert.deepEqual(read(px(12, -36), 401), pixels(12));
  read(px(4.000244140625, -12), 1000);
  assert.deepEqual(read(px(-12, 36, true), 1050), pixels(-12));
});

test("a trackpad scroll doesn't start reading as notches", () => {
  const read = wheelReader();
  assert.deepEqual(read(px(12, -36), 0), pixels(12));
  assert.deepEqual(read(px(15, -45), 20), pixels(15));
});
