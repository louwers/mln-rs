# mln-rs-gestures

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::mln_gestures`.

Turns pointer, touch, wheel and keyboard input into camera moves and gesture
momentum, for hosts that have no gesture recognizer of their own (the GLFW app,
the browser build). MapLibre Native has no counterpart: its platform SDKs each
recognize gestures. This one is ported from
[Orbis](https://github.com/sargunv/orbis)'s `orb-camera` input.

## Where each mechanism comes from

The gesture set is MapLibre Compose's. Where maplibre-native-ffi's browser
demo has a gesture, the recognizer takes its rates and releases. That demo is
the reference: ffi's C API is the contract that mln-rs, Orbis and Compose
converge on. Elsewhere the values come from GL JS's handlers. Sources were
read at: ffi `9cbe0b9` (`examples/browser/src/demo.ts`, `src/map/map.cpp`),
Orbis `bef8eda` (`crates/orb-camera/src/input`, `web/src/input.ts`),
maplibre-compose `5b6fb4e` with Compose UI 1.12.1 for the web (`WebVelocityTracker`,
`PointerVelocityTracker1D`), and GL JS v5.13.0 (`src/ui/handler*`). n/a: the
reference has no such mechanism.

| Mechanism | mln-rs | ffi web demo | ffi C API | Orbis | maplibre-compose / Compose UI | GL JS | mln-rs matches |
|---|---|---|---|---|---|---|---|
| Release velocity | Motion since the window start, over the time since then | Motion over the last 100 ms, ÷ min(elapsed, 100 ms) | n/a (the host's) | Least-squares quadratic, slope at the last sample | Pointers: Lsq2, on the web with a 2–3-sample fix and an optional opposite-direction guard (off); pinch and twist streams: Impulse | Sum of the deltas in the last 160 ms, ÷ their time span | ffi demo, which can't reverse or overshoot; divides by the true span, as GL JS does |
| Velocity window | 100 ms, plus the sample before it | 100 ms, plus the sample before it | n/a | 100 ms, 20 samples, cut at 40 ms gaps | 100 ms, 20 samples, cut at 40 ms gaps | 160 ms | ffi demo |
| Release after holding still | None after 40 ms; shorter pauses slow it | Slows it; none after 100 ms | n/a | None after 40 ms | None after 40 ms | None after 160 ms | Compose: a pointer held still doesn't glide |
| Caught glide added to the next release | No | No | n/a | Yes | Yes (`resolveGestureMomentum`) | No | ffi demo and GL JS: repeated flicks compounded up to the 10 000 px/s cap |
| Pan momentum | 140 ms half-life friction; stop 10, min 200, max 10 000 px/s | C API default | 140 ms; 10, 200, 10 000 | Same | Same (`MomentumDefaults`; Android eases along its fling) | Ease; deceleration 2500, max 1400 px/s, linearity 0.3 | ffi C API |
| Zoom, bearing, pitch momentum | 140 ms; zoom stop 0.01, max 2.5 levels; bearing min 60°/s; pitch min 5°/s | C API default | Same | Same | Same | Eases, max 1400 levels/s, 360°/s, 90°/s | ffi C API |
| Snaps after a release | An option per part (`zoom_snap`, `bearing_snap`, `pitch_snap`), applied to the parts the gesture changed; by default only the bearing, north within 7°, 300 ms | Bearing after a turn; zoom to a whole level and pitch flat (C API defaults) | An `mln_momentum_snap` per part (zoom, bearing, pitch); defaults zoom whole within 0.5, bearing north within 7°, pitch flat within 7°, 300 ms | Bearing after a turn | A `snapping` block per part, applied to the parts the input changed; enabled by default only for the bearing (north within 7°); zoom (whole within 0.5) and pitch (flat within 7°) off; 300 ms | n/a | ffi C API for the options, Compose for the defaults: settling the zoom or pitch after every gesture fights the user |
| Scroll zoom | 1/300 level per px, at once | 1/300 per px, at once | n/a | 2σ(px / 450) for wheels, eased 200 ms; 2σ(px / 100) for trackpads | 1/300 per dp | Same as Orbis | ffi demo and Compose; GL JS's trackpad rate read a slow scroll start as a trackpad and zoomed twice as fast |
| Scroll with Ctrl (trackpad pinch) | 1/100 per px | 1/100 per px | n/a | 2σ(px / 100) | n/a | 2σ(px / 100) | ffi demo |
| Wheel or trackpad | Not told apart | Not told apart | n/a | By delta size and timing | Not told apart | By delta size and timing | ffi demo |
| Shift-scroll | As without | As without | n/a | ÷ 4 | n/a | ÷ 4 | ffi demo |
| `deltaMode` | Lines × 40 px, pages × viewport height | Same | n/a | Same | n/a | Lines × 40 | ffi demo |
| Scroll gesture end | 200 ms after the last event | n/a | n/a | 200 ms | 200 ms (`idleDuration`) | 200 ms after the zoom finishes | Compose |
| Drag rotate | 0.5° per px | 0.5° per px | n/a | 0.8 | 0.8° per dp | 0.8 | ffi demo |
| Drag pitch | 0.5° per px, upward tilts | 0.5° per px | n/a | 0.5 | 0.5° per dp | 0.5 | All |
| Pinch and twist | Follow the fingers, after 0.1 levels of spread or 25 px of arc | Follow the fingers at once | n/a | As mln-rs | After 7 dp of span or 3° | As mln-rs | GL JS: a resting second finger doesn't jitter the map |
| Two-finger tilt | Both fingers up or down: 0.5° per px | n/a | n/a | As mln-rs | 0.3° per dp past 16 dp | 0.5° per px | GL JS |
| Double-tap-and-drag zoom | 1/128 level per px | n/a | n/a | 1/128 | 4 levels per viewport height | 1/128 | GL JS |
| Double-click, two-finger tap | ±1 level, 300 ms | ±1, 300 ms (double-click only) | n/a | ±1, 300 ms | ±1 | ±1, 300 ms | All |
| Keys | Arrows 100 px; `+`/`-` 1 level (Shift: 2); Shift-arrows 15°, 10°; 300 ms | Arrows 100 px; `+`/`-` 1 level; 300 ms | n/a | As mln-rs | 100 dp, 1 level, 15°, 10° | As mln-rs | GL JS, a superset of ffi demo |
| Box zoom | Shift-drag, eased 500 ms | n/a | n/a | As mln-rs | Drag mapping | Shift-drag, linear ease | GL JS |
| Click tolerance | 3 px | n/a | n/a | 3 | 3 dp for mice | 3 | All |
| Browser moves | `pointerrawupdate` where the browser has it | `pointermove` | n/a | `pointerrawupdate` | n/a | `mousemove`, `touchmove` | Orbis: the map runs in a worker, and `pointermove` waits for the page's frame |

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
