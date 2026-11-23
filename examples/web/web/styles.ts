// The styles the page offers, from the shared catalog (examples/styles.json):
// vector tiles across several schemas (OpenMapTiles, Protomaps, Shortbread,
// and Mapeak), raster-dem hillshade, and raster tiles (satellite imagery and
// OpenStreetMap Carto). `dark` turns the page's controls dark.

import catalog from "../../styles.json";
import type { StyleSource } from "./protocol.ts";

/** A style the page can show; `key` is null for one from a URL. */
export interface Style {
  key: string | null;
  name: string;
  provider: string;
  source: StyleSource;
  dark?: boolean;
}

interface CatalogEntry {
  key: string;
  name: string;
  provider: string;
  url?: string;
  json?: object;
  dark?: boolean;
}

export const STYLES: Style[] = (catalog.styles as CatalogEntry[]).map(
  (entry) => ({
    key: entry.key,
    name: entry.name,
    provider: entry.provider,
    source:
      entry.url !== undefined
        ? { url: entry.url }
        : { json: JSON.stringify(entry.json) },
    dark: entry.dark,
  }),
);

const DEFAULT_STYLE: Style =
  STYLES.find((style) => style.key === catalog.default) ?? STYLES[0]!;

/** Whether two styles refer to the same style document. */
export function sameStyle(a: Style, b: Style): boolean {
  if (a.key !== null && b.key !== null) return a.key === b.key;
  if ("url" in a.source && "url" in b.source)
    return a.source.url === b.source.url;
  if ("json" in a.source && "json" in b.source)
    return a.source.json === b.source.json;
  return false;
}

// The style for `?style=`: a key of STYLES, or any style's URL, which the
// page names by the style's own name once it loads.
export function styleFor(param: string | null): Style {
  if (param === null) return DEFAULT_STYLE;
  const known = STYLES.find(
    (style) =>
      style.key === param ||
      ("url" in style.source && style.source.url === param),
  );
  if (known) return known;
  const url = new URL(param, location.href);
  return {
    key: null,
    name: "Custom style",
    provider: url.host,
    source: { url: url.href },
  };
}
