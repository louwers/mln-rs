// The local server: the page as Vite builds it, each view's style, and the
// fixtures. Every URL the page loads is on this server, at
// `/fixtures/<scheme>/<rest>` for the cache's `<scheme>://<rest>`.

import { existsSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import path from "node:path";

import { SOURCE } from "./fixtures.mjs";

// In a slow view every fixture comes this late, and fresh for an hour, as
// over a network, so the map fetches each URL once unless it drops what it
// fetched. A tile it drops while it loads is fetched again on its next
// swing into view, which the browser answers from the first fetch, so it
// loads then.
const LATENCY_MS = 100;

const types = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".wasm": "application/wasm",
  ".json": "application/json",
};

/**
 * Serves `root` (the built page), the styles of `views` and `fixtures` on a
 * free port of 127.0.0.1, and `/blank`, an empty page.
 *
 * `beginView({ slow })` starts a view: it returns the list that the server
 * fills with the fixtures the view asks for that the cache doesn't have,
 * and `slow` delays its fixtures (LATENCY_MS) until `endView()`. The
 * runner's cache-only file source answers those as not found, and so does
 * this server (404, which HTTPFileSource reads as a tile the tileset
 * doesn't have, or as a Not Found error for other resources).
 */
export async function startServer({ root, views, fixtures }) {
  let origin;
  let missing = [];
  let slow = false;

  const fixtureUrl = (url) => {
    const match = /^([a-z]+):\/\/(.*)$/.exec(url);
    return match ? `${origin}/fixtures/${match[1]}/${match[2]}` : url;
  };

  /** The style or TileJSON with each URL in it on this server. */
  function served(json) {
    const copy = structuredClone(json);
    if (Array.isArray(copy.tiles)) copy.tiles = copy.tiles.map(fixtureUrl);
    for (const source of Object.values(copy.sources ?? {})) {
      if (typeof source.url === "string") source.url = fixtureUrl(source.url);
      if (Array.isArray(source.urls)) source.urls = source.urls.map(fixtureUrl);
      if (Array.isArray(source.tiles))
        source.tiles = source.tiles.map(fixtureUrl);
      if (typeof source.data === "string")
        source.data = fixtureUrl(source.data);
    }
    if (typeof copy.sprite === "string") copy.sprite = fixtureUrl(copy.sprite);
    if (Array.isArray(copy.sprite))
      copy.sprite = copy.sprite.map((sprite) => ({
        ...sprite,
        url: fixtureUrl(sprite.url),
      }));
    if (typeof copy.glyphs === "string") copy.glyphs = fixtureUrl(copy.glyphs);
    for (const [name, faces] of Object.entries(copy["font-faces"] ?? {})) {
      copy["font-faces"][name] =
        typeof faces === "string"
          ? fixtureUrl(faces)
          : faces.map((face) => ({ ...face, url: fixtureUrl(face.url) }));
    }
    return copy;
  }

  function serveFixture(url, response) {
    const entry = fixtures.cached(url);
    if (!entry) {
      missing.push(url);
      response.writeHead(404).end();
    } else if (entry.kind === SOURCE) {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(
        JSON.stringify(served(JSON.parse(entry.data.toString("utf8")))),
      );
    } else {
      response
        .writeHead(200, { "content-type": "application/octet-stream" })
        .end(entry.data);
    }
  }

  const server = createServer((request, response) => {
    const url = new URL(request.url, "http://x");
    const fixture = /^\/fixtures\/([a-z]+)\/(.*)$/.exec(request.url);
    if (fixture && slow) {
      response.setHeader("cache-control", "max-age=3600");
      setTimeout(
        () => serveFixture(`${fixture[1]}://${fixture[2]}`, response),
        LATENCY_MS,
      );
      return;
    }
    if (fixture) {
      serveFixture(`${fixture[1]}://${fixture[2]}`, response);
      return;
    }
    const style = /^\/style\/(\d+)$/.exec(url.pathname);
    if (style) {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify(served(views[Number(style[1])].style)));
      return;
    }
    if (url.pathname === "/blank") {
      response
        .writeHead(200, { "content-type": "text/html" })
        .end("<!doctype html>");
      return;
    }
    const file = path.join(
      root,
      path.normalize(decodeURIComponent(url.pathname)),
    );
    const target = file.endsWith(path.sep)
      ? path.join(file, "index.html")
      : file;
    if (!target.startsWith(root) || !existsSync(target)) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, {
      "content-type": types[path.extname(target)] ?? "application/octet-stream",
    });
    response.end(readFileSync(target));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  origin = `http://127.0.0.1:${server.address().port}`;

  return {
    origin,
    /** The URL of `view`'s style, with its URLs on this server. */
    styleUrl: (view) => `${origin}/style/${views.indexOf(view)}`,
    beginView({ slow: isSlow }) {
      missing = [];
      slow = isSlow;
      return missing;
    },
    endView() {
      slow = false;
    },
    close: () => server.close(),
  };
}
