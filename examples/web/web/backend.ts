// Backend selection, fallback and switching for the page (main.ts).
//
// Two separate things are tracked:
// - `?backend=<webgpu|webgl2>` in the URL is what the user asked for.
// - `sessionStorage` records which backends failed to start in this tab, so a
//   browser whose `navigator.gpu` has no usable adapter doesn't retry WebGPU
//   or offer a toggle that bounces back.

import type { Backend } from "./protocol.ts";

export interface BackendCapabilities {
  /** Whether the browser exposes `navigator.gpu`. */
  hasWebGpuApi: boolean;
  /** Whether the browser exposes `WebGL2RenderingContext`. */
  hasWebGl2Api: boolean;
  /** Backends that already failed to start in this tab. */
  unsupported: Record<Backend, boolean>;
}

export const OTHER_BACKEND: Readonly<Record<Backend, Backend>> = {
  webgpu: "webgl2",
  webgl2: "webgpu",
};

export const BACKEND_NAME: Readonly<Record<Backend, string>> = {
  webgpu: "WebGPU",
  webgl2: "WebGL2",
};

const UNSUPPORTED_KEYS: Readonly<Record<Backend, string>> = {
  webgpu: "mln-webgpu-unsupported",
  webgl2: "mln-webgl2-unsupported",
};

const LEGACY_BACKEND_KEY = "mln-backend";
const FAILED_PARAM = "failedBackend";

interface WebGpuDeviceLike {
  lost?: Promise<unknown>;
  pushErrorScope?(filter: "validation" | "out-of-memory" | "internal"): void;
  popErrorScope?(): Promise<unknown>;
  destroy?(): void;
}

interface WebGpuAdapterLike {
  requestDevice(): Promise<WebGpuDeviceLike>;
}

interface WebGpuContextLike {
  configure(config: { device: WebGpuDeviceLike; format: string }): void;
  getCurrentTexture?(): unknown;
  unconfigure?(): void;
}

interface WebGpuCanvas {
  getContext(contextId: "webgpu"): WebGpuContextLike | null;
}

interface WebGpuNavigator {
  gpu?: {
    requestAdapter(options?: {
      powerPreference?: "low-power" | "high-performance";
    }): Promise<WebGpuAdapterLike | null>;
    getPreferredCanvasFormat?(): string;
  };
}

/** Reads which backends this tab already found unsupported. */
export function readUnsupported(
  searchParams: Pick<URLSearchParams, "get"> | null = null,
  storage: Pick<Storage, "getItem"> | null = safeSessionStorage(),
): Record<Backend, boolean> {
  const has = (backend: Backend) => {
    if (searchParams?.get(FAILED_PARAM) === backend) return true;
    try {
      if (storage?.getItem(UNSUPPORTED_KEYS[backend]) === "1") return true;
      return (
        backend === "webgpu" &&
        storage?.getItem(LEGACY_BACKEND_KEY) === "webgl2"
      );
    } catch {
      return false;
    }
  };
  return { webgpu: has("webgpu"), webgl2: has("webgl2") };
}

/** Records that `backend` couldn't start in this tab; returns whether storage took it. */
export function rememberUnsupported(
  backend: Backend,
  storage: Pick<Storage, "setItem"> | null = safeSessionStorage(),
): boolean {
  try {
    if (!storage) return false;
    storage.setItem(UNSUPPORTED_KEYS[backend], "1");
    return true;
  } catch {
    return false;
  }
}

function safeSessionStorage(): Storage | null {
  try {
    return globalThis.sessionStorage ?? null;
  } catch {
    return null;
  }
}

/** Which backend to start with, given `?backend=` and what the browser has. */
export function chooseBackend(
  param: string | null,
  capabilities: BackendCapabilities,
): Backend {
  if (param === "webgl2") return "webgl2";
  if (param === "webgpu") return "webgpu";
  return capabilities.hasWebGpuApi && !capabilities.unsupported.webgpu
    ? "webgpu"
    : "webgl2";
}

/** Whether the other backend is available to switch or fall back to. */
export function canSwitchBackend(
  current: Backend,
  capabilities: BackendCapabilities,
): boolean {
  const other = OTHER_BACKEND[current];
  const hasApi =
    other === "webgpu" ? capabilities.hasWebGpuApi : capabilities.hasWebGl2Api;
  return hasApi && !capabilities.unsupported[other];
}

/**
 * The URL to load after `current` failed to start, or `null` when no other
 * backend is available. Expects `capabilities.unsupported[current]` to be set.
 * When `remembered` is true and no `?backend=` override needs changing, returns
 * `href` unchanged so the caller reloads it. When `sessionStorage` is
 * unavailable, carries the failed backend in `?failedBackend=` so the next
 * load does not loop or offer switching back to it.
 */
export function fallbackUrl(
  current: Backend,
  href: string,
  capabilities: BackendCapabilities,
  remembered: boolean,
): string | null {
  if (!canSwitchBackend(current, capabilities)) return null;

  const next = OTHER_BACKEND[current];
  const url = new URL(href);
  if (remembered) {
    url.searchParams.delete(FAILED_PARAM);
    if (next === chooseBackend(null, capabilities)) {
      url.searchParams.delete("backend");
      return url.href;
    }
  } else {
    url.searchParams.set(FAILED_PARAM, current);
  }
  url.searchParams.set("backend", next);
  return url.href;
}

/** The URL to load when the user switches to `next`, keeping `hash`. */
export function switchBackendUrl(
  next: Backend,
  href: string,
  hash: string,
): string {
  const url = new URL(href);
  url.searchParams.delete(FAILED_PARAM);
  url.searchParams.set("backend", next);
  url.hash = hash;
  return url.href;
}

interface WebGl2ContextLike {
  getExtension(name: "WEBGL_lose_context"): { loseContext(): void } | null;
}

interface WebGl2Canvas {
  getContext(
    contextId: "webgl2",
    options?: WebGLContextAttributes,
  ): WebGl2ContextLike | null;
}

const WEBGL2_ATTRIBUTES: WebGLContextAttributes = {
  alpha: true,
  premultipliedAlpha: true,
  antialias: false,
  depth: true,
  stencil: true,
};

/**
 * Probes whether the browser can create a WebGL2 context on an OffscreenCanvas,
 * matching `mln_gl::CanvasTarget::with_backend`. Used while running WebGPU so
 * a browser that exposes `WebGL2RenderingContext` without a usable context
 * doesn't offer WebGL2.
 */
export function probeWebGl2(
  createCanvas: () => WebGl2Canvas | null = () =>
    typeof OffscreenCanvas === "function" ? new OffscreenCanvas(1, 1) : null,
): boolean {
  try {
    const canvas = createCanvas();
    if (!canvas) return false;
    const gl = canvas.getContext("webgl2", WEBGL2_ATTRIBUTES);
    if (!gl) return false;
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    return true;
  } catch {
    return false;
  }
}

/**
 * Probes whether the browser can hand out a WebGPU adapter, create a device,
 * and configure an OffscreenCanvas surface, matching `mln_webgpu::Device::request`.
 * Used while running WebGL2 so a browser that exposes `navigator.gpu` without
 * a usable device or surface doesn't offer WebGPU.
 */
export async function probeWebGpu(
  nav: WebGpuNavigator = globalThis.navigator as WebGpuNavigator,
  createCanvas: () => WebGpuCanvas | null = () =>
    typeof OffscreenCanvas === "function"
      ? (new OffscreenCanvas(1, 1) as unknown as WebGpuCanvas)
      : null,
): Promise<boolean> {
  if (!nav.gpu) return false;
  let device: WebGpuDeviceLike | null = null;
  let context: WebGpuContextLike | null = null;
  try {
    const adapter = await nav.gpu.requestAdapter({
      powerPreference: "high-performance",
    });
    if (!adapter) return false;
    device = await adapter.requestDevice();
    let lost = false;
    device.lost?.then(
      () => {
        lost = true;
      },
      () => {
        lost = true;
      },
    );
    const canvas = createCanvas();
    context = canvas?.getContext("webgpu") ?? null;
    if (!context) return false;
    const format = nav.gpu.getPreferredCanvasFormat?.() ?? "bgra8unorm";
    device.pushErrorScope?.("validation");
    context.configure({ device, format });
    context.getCurrentTexture?.();
    const validationError = device.popErrorScope
      ? await device.popErrorScope()
      : null;
    // Let an already-resolved `device.lost` promise settle even when
    // `popErrorScope` is absent.
    await Promise.resolve();
    return !lost && validationError === null;
  } catch {
    return false;
  } finally {
    try {
      context?.unconfigure?.();
    } catch {
      // Ignore cleanup errors on a failed or lost surface.
    }
    device?.destroy?.();
  }
}
