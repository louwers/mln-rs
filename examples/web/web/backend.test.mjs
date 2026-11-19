// Backend selection, fallback and switching (backend.ts). Runs with `pnpm
// test`, in Node, which strips the types from backend.ts.
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  canSwitchBackend,
  chooseBackend,
  fallbackUrl,
  probeWebGl2,
  probeWebGpu,
  readUnsupported,
  rememberUnsupported,
  switchBackendUrl,
} from "./backend.ts";

const caps = (overrides = {}) => ({
  hasWebGpuApi: true,
  hasWebGl2Api: true,
  unsupported: { webgpu: false, webgl2: false, ...overrides.unsupported },
  ...overrides,
  ...(overrides.unsupported
    ? {
        unsupported: {
          webgpu: false,
          webgl2: false,
          ...overrides.unsupported,
        },
      }
    : {}),
});

test("chooseBackend prefers WebGPU when available, else WebGL2", () => {
  assert.equal(chooseBackend(null, caps()), "webgpu");
  assert.equal(chooseBackend(null, caps({ hasWebGpuApi: false })), "webgl2");
  assert.equal(
    chooseBackend(null, caps({ unsupported: { webgpu: true } })),
    "webgl2",
  );
  assert.equal(chooseBackend("webgl2", caps()), "webgl2");
  assert.equal(
    chooseBackend("webgpu", caps({ unsupported: { webgpu: true } })),
    "webgpu",
  );
  // Unknown values fall back to auto-detection rather than forcing WebGPU.
  assert.equal(chooseBackend("other", caps({ hasWebGpuApi: false })), "webgl2");
});

test("canSwitchBackend requires the other backend's API and no recorded failure", () => {
  assert.equal(canSwitchBackend("webgpu", caps()), true);
  assert.equal(canSwitchBackend("webgl2", caps()), true);
  assert.equal(
    canSwitchBackend("webgl2", caps({ hasWebGpuApi: false })),
    false,
  );
  assert.equal(
    canSwitchBackend("webgl2", caps({ unsupported: { webgpu: true } })),
    false,
  );
  assert.equal(
    canSwitchBackend("webgpu", caps({ hasWebGl2Api: false })),
    false,
  );
  assert.equal(
    canSwitchBackend("webgpu", caps({ unsupported: { webgl2: true } })),
    false,
  );
});

test("fallbackUrl avoids looping when ?backend=webgpu fails", () => {
  assert.equal(
    fallbackUrl(
      "webgpu",
      "https://example.com/?style=liberty&backend=webgpu#13/52.37/4.89",
      caps({ unsupported: { webgpu: true } }),
      true,
    ),
    "https://example.com/?style=liberty#13/52.37/4.89",
  );
  assert.equal(
    fallbackUrl(
      "webgpu",
      "https://example.com/?style=liberty&backend=webgpu#13/52.37/4.89",
      caps({ unsupported: { webgpu: true } }),
      false,
    ),
    "https://example.com/?style=liberty&backend=webgl2&failedBackend=webgpu#13/52.37/4.89",
  );
});

test("fallbackUrl keeps the URL clean when storage remembered WebGPU failed", () => {
  assert.equal(
    fallbackUrl(
      "webgpu",
      "https://example.com/#13/52.37/4.89",
      caps({ unsupported: { webgpu: true } }),
      true,
    ),
    "https://example.com/#13/52.37/4.89",
  );
});

test("fallbackUrl handles WebGL2 fallback and double-failure with or without sessionStorage", () => {
  assert.equal(
    fallbackUrl(
      "webgl2",
      "https://example.com/?backend=webgl2#13/52.37/4.89",
      caps({ unsupported: { webgl2: true } }),
      true,
    ),
    "https://example.com/#13/52.37/4.89",
  );
  // Without sessionStorage, fall back from WebGL2 to WebGPU carrying failedBackend=webgl2.
  const webGpuFallback = fallbackUrl(
    "webgl2",
    "https://example.com/?backend=webgl2#13/52.37/4.89",
    caps({ unsupported: { webgl2: true } }),
    false,
  );
  assert.equal(
    webGpuFallback,
    "https://example.com/?backend=webgpu&failedBackend=webgl2#13/52.37/4.89",
  );
  // On the next load with blocked storage, failedBackend suppresses switching back and stops if WebGPU also fails.
  const afterWebGl2Failed = caps({
    unsupported: readUnsupported(new URL(webGpuFallback).searchParams, null),
  });
  assert.equal(canSwitchBackend("webgpu", afterWebGl2Failed), false);
  afterWebGl2Failed.unsupported.webgpu = true;
  assert.equal(
    fallbackUrl("webgpu", webGpuFallback, afterWebGl2Failed, false),
    null,
  );
  // Symmetric case: WebGPU fails without storage -> loads WebGL2 with failedBackend=webgpu -> stops if WebGL2 fails.
  const webGl2Fallback = fallbackUrl(
    "webgpu",
    "https://example.com/",
    caps({ unsupported: { webgpu: true } }),
    false,
  );
  assert.equal(
    webGl2Fallback,
    "https://example.com/?failedBackend=webgpu&backend=webgl2",
  );
  const afterWebGpuFailed = caps({
    unsupported: readUnsupported(new URL(webGl2Fallback).searchParams, null),
  });
  assert.equal(canSwitchBackend("webgl2", afterWebGpuFailed), false);
  afterWebGpuFailed.unsupported.webgl2 = true;
  assert.equal(
    fallbackUrl("webgl2", webGl2Fallback, afterWebGpuFailed, false),
    null,
  );
  assert.equal(
    fallbackUrl(
      "webgl2",
      "https://example.com/",
      caps({ unsupported: { webgpu: true, webgl2: true } }),
      true,
    ),
    null,
  );
  assert.equal(
    fallbackUrl(
      "webgpu",
      "https://example.com/",
      caps({ hasWebGl2Api: false, unsupported: { webgpu: true } }),
      true,
    ),
    null,
  );
});

test("switchBackendUrl updates backend and preserves other params and camera hash", () => {
  assert.equal(
    switchBackendUrl(
      "webgl2",
      "https://example.com/?style=dark&debug=#10/1/2",
      "#13/52.37/4.89",
    ),
    "https://example.com/?style=dark&debug=&backend=webgl2#13/52.37/4.89",
  );
  assert.equal(
    switchBackendUrl(
      "webgpu",
      "https://example.com/?backend=webgl2#10/1/2",
      "#13/52.37/4.89",
    ),
    "https://example.com/?backend=webgpu#13/52.37/4.89",
  );
});

test("probeWebGl2 creates and releases a disposable WebGL2 context without throwing", () => {
  assert.equal(
    probeWebGl2(() => null),
    false,
  );
  let seenId;
  let seenOptions;
  let lost = false;
  assert.equal(
    probeWebGl2(() => ({
      getContext: (id, options) => {
        seenId = id;
        seenOptions = options;
        return {
          getExtension: (name) =>
            name === "WEBGL_lose_context"
              ? {
                  loseContext: () => {
                    lost = true;
                  },
                }
              : null,
        };
      },
    })),
    true,
  );
  assert.equal(seenId, "webgl2");
  assert.deepEqual(seenOptions, {
    alpha: true,
    premultipliedAlpha: true,
    antialias: false,
    depth: true,
    stencil: true,
  });
  assert.equal(lost, true);
  assert.equal(
    probeWebGl2(() => ({ getContext: () => null })),
    false,
  );
  assert.equal(
    probeWebGl2(() => {
      throw new Error("blocked");
    }),
    false,
  );
});

test("probeWebGpu checks adapter, device loss, validation scope and OffscreenCanvas texture", async () => {
  assert.equal(await probeWebGpu({}), false);
  let seenOptions;
  let configured;
  let acquiredTexture = false;
  let unconfigured = false;
  let destroyed = false;
  let pushedScope;
  const device = {
    lost: new Promise(() => {}),
    pushErrorScope: (filter) => {
      pushedScope = filter;
    },
    popErrorScope: async () => null,
    destroy: () => {
      destroyed = true;
    },
  };
  const canvas = () => ({
    getContext: (id) =>
      id === "webgpu"
        ? {
            configure: (cfg) => {
              configured = cfg;
            },
            getCurrentTexture: () => {
              acquiredTexture = true;
              return {};
            },
            unconfigure: () => {
              unconfigured = true;
            },
          }
        : null,
  });
  assert.equal(
    await probeWebGpu(
      {
        gpu: {
          requestAdapter: async (options) => {
            seenOptions = options;
            return { requestDevice: async () => device };
          },
          getPreferredCanvasFormat: () => "rgba8unorm",
        },
      },
      canvas,
    ),
    true,
  );
  assert.deepEqual(seenOptions, { powerPreference: "high-performance" });
  assert.equal(pushedScope, "validation");
  assert.deepEqual(configured, { device, format: "rgba8unorm" });
  assert.equal(acquiredTexture, true);
  assert.equal(unconfigured, true);
  assert.equal(destroyed, true);

  assert.equal(
    await probeWebGpu({ gpu: { requestAdapter: async () => null } }, canvas),
    false,
  );
  // Device creation failure returns false.
  assert.equal(
    await probeWebGpu(
      {
        gpu: {
          requestAdapter: async () => ({
            requestDevice: async () => {
              throw new Error("device failed");
            },
          }),
        },
      },
      canvas,
    ),
    false,
  );
  // Already-lost device returns false and still cleans up.
  let unconfiguredOnLost = false;
  let destroyedOnLost = false;
  assert.equal(
    await probeWebGpu(
      {
        gpu: {
          requestAdapter: async () => ({
            requestDevice: async () => ({
              lost: Promise.resolve({ reason: "unknown" }),
              pushErrorScope: () => {},
              popErrorScope: async () => null,
              destroy: () => {
                destroyedOnLost = true;
              },
            }),
          }),
        },
      },
      () => ({
        getContext: () => ({
          configure: () => {},
          getCurrentTexture: () => ({}),
          unconfigure: () => {
            unconfiguredOnLost = true;
          },
        }),
      }),
    ),
    false,
  );
  assert.equal(unconfiguredOnLost, true);
  assert.equal(destroyedOnLost, true);
  // Validation error reported through error scope returns false and cleans up.
  let unconfiguredOnValidationError = false;
  let destroyedOnValidationError = false;
  assert.equal(
    await probeWebGpu(
      {
        gpu: {
          requestAdapter: async () => ({
            requestDevice: async () => ({
              lost: new Promise(() => {}),
              pushErrorScope: () => {},
              popErrorScope: async () => ({ message: "invalid format" }),
              destroy: () => {
                destroyedOnValidationError = true;
              },
            }),
          }),
        },
      },
      () => ({
        getContext: () => ({
          configure: () => {},
          getCurrentTexture: () => ({}),
          unconfigure: () => {
            unconfiguredOnValidationError = true;
          },
        }),
      }),
    ),
    false,
  );
  assert.equal(unconfiguredOnValidationError, true);
  assert.equal(destroyedOnValidationError, true);
  // Missing webgpu canvas context destroys the created device and returns false.
  let destroyedOnMissingCtx = false;
  assert.equal(
    await probeWebGpu(
      {
        gpu: {
          requestAdapter: async () => ({
            requestDevice: async () => ({
              destroy: () => {
                destroyedOnMissingCtx = true;
              },
            }),
          }),
        },
      },
      () => ({ getContext: () => null }),
    ),
    false,
  );
  assert.equal(destroyedOnMissingCtx, true);
  // configure() throwing destroys the device and returns false.
  let destroyedOnConfigureThrow = false;
  assert.equal(
    await probeWebGpu(
      {
        gpu: {
          requestAdapter: async () => ({
            requestDevice: async () => ({
              destroy: () => {
                destroyedOnConfigureThrow = true;
              },
            }),
          }),
        },
      },
      () => ({
        getContext: () => ({
          configure: () => {
            throw new Error("unsupported surface");
          },
        }),
      }),
    ),
    false,
  );
  assert.equal(destroyedOnConfigureThrow, true);
});

test("readUnsupported and rememberUnsupported handle storage, legacy key, URL marker and blocked storage", () => {
  const entries = new Map();
  const storage = {
    getItem: (key) => entries.get(key) ?? null,
    setItem: (key, value) => entries.set(key, value),
  };
  assert.deepEqual(readUnsupported(null, storage), {
    webgpu: false,
    webgl2: false,
  });
  assert.equal(rememberUnsupported("webgpu", storage), true);
  assert.deepEqual(readUnsupported(null, storage), {
    webgpu: true,
    webgl2: false,
  });

  // Legacy mln-backend=webgl2 flag from an earlier session counts as WebGPU unsupported.
  const legacyEntries = new Map([["mln-backend", "webgl2"]]);
  const legacyStorage = {
    getItem: (key) => legacyEntries.get(key) ?? null,
  };
  assert.deepEqual(readUnsupported(null, legacyStorage), {
    webgpu: true,
    webgl2: false,
  });

  const blocked = {
    getItem: () => {
      throw new Error("denied");
    },
    setItem: () => {
      throw new Error("denied");
    },
  };
  assert.deepEqual(readUnsupported(null, blocked), {
    webgpu: false,
    webgl2: false,
  });
  assert.deepEqual(
    readUnsupported(new URLSearchParams("failedBackend=webgl2"), blocked),
    { webgpu: false, webgl2: true },
  );
  assert.equal(rememberUnsupported("webgpu", blocked), false);
});
