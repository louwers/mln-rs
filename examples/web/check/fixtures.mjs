// The fixtures: the render suite's cache database, which the native runner's
// cache-only file source reads too.

import { DatabaseSync } from "node:sqlite";
import { inflateSync } from "node:zlib";

/** Resource::Kind::Source: a TileJSON. */
export const SOURCE = 2;

/** The cache's URL for a URL of the fixtures, as the runner's file source maps it (tools/render-test/src/file_source.rs). */
function cacheUrl(url) {
  for (const prefix of ["local://", "http://localhost:2900/"]) {
    if (url.startsWith(prefix)) return `http://${url.slice(prefix.length)}`;
  }
  return url;
}

function unpack(row) {
  return row.compressed ? inflateSync(row.data) : Buffer.from(row.data);
}

/**
 * Opens the cache database at `file`, read-only. `cached(url)` is the
 * resource or tile at a URL of the fixtures, as `{ kind, data }`, if the
 * cache has it; `kind` is the resource's kind, or `"tile"`.
 */
export function openFixtures(file) {
  const database = new DatabaseSync(file, { readOnly: true });
  const resourceQuery = database.prepare(
    "SELECT kind, data, compressed FROM resources WHERE url = ?",
  );
  const tileQuery = database.prepare(
    "SELECT data, compressed FROM tiles WHERE id = ?",
  );

  // The cache keys tiles by their URL template and coordinates; the page
  // requests them by URL.
  const tiles = new Map();
  for (const {
    id,
    url_template: template,
    pixel_ratio: ratio,
    z,
    x,
    y,
  } of database
    .prepare("SELECT id, url_template, pixel_ratio, z, x, y FROM tiles")
    .all()) {
    const url = template
      .replaceAll("{z}", z)
      .replaceAll("{x}", x)
      .replaceAll("{y}", y)
      .replaceAll("{ratio}", ratio > 1 ? `@${ratio}x` : "");
    tiles.set(url, id);
  }

  function cached(url) {
    const key = cacheUrl(url);
    const resource = resourceQuery.get(key);
    if (resource) return { kind: resource.kind, data: unpack(resource) };
    const tile = tiles.has(key) ? tileQuery.get(tiles.get(key)) : undefined;
    return tile ? { kind: "tile", data: unpack(tile) } : undefined;
  }

  /** A cached JSON document, such as a style. */
  function cachedJson(url) {
    const entry = cached(url);
    if (!entry) throw new Error(`${url} is not in the cache`);
    return JSON.parse(entry.data.toString("utf8"));
  }

  return { cached, cachedJson, close: () => database.close() };
}
