// The styles the page offers, from the shared catalog (examples/styles.json):
// vector tiles across several schemas (OpenMapTiles, Protomaps, Shortbread,
// and Mapeak), raster-dem hillshade, raster tiles (satellite imagery and
// OpenStreetMap Carto), and Mapterhorn's terrain under the satellite imagery.
// `dark` turns the page's controls dark.

import type {
  HillshadeLayerSpecification,
  RasterDEMSourceSpecification,
  SkySpecification,
} from "maplibre-gl";

import catalog from "../../styles.json";
import type { StyleSource } from "./protocol.ts";

/**
 * 3D terrain that the map adds to a style that has none once it loads
 * (WebMap::set_style_url), as does GL JS's map in the comparison
 * (compare.ts): the `raster-dem` source, the style's `terrain` on it at
 * `exaggeration`, with `hillshade`, a hillshade of it with that paint
 * under the layer `before` if the style has it, else on top, and with `sky`,
 * that sky where the style has none, so that fog shows at a high pitch.
 *
 * The catalog's Liberty 3D takes the style spec's default sky colors, whose
 * white fog greys the far ground. Its globe's atmosphere fades out by zoom
 * 7: it scatters along each line of sight to the sea-level sphere, so up
 * close it would whiten the mountains.
 */
export interface Terrain {
  source: RasterDEMSourceSpecification;
  exaggeration: number;
  hillshade?: {
    paint?: HillshadeLayerSpecification["paint"];
    before?: string;
  };
  sky?: SkySpecification;
}

/** A style the page can show; `key` is null for one from a URL. */
export interface Style {
  key: string | null;
  name: string;
  provider: string;
  source: StyleSource;
  dark?: boolean;
  terrain?: Terrain;
}

interface CatalogEntry {
  key: string;
  name: string;
  provider: string;
  url?: string;
  json?: object;
  dark?: boolean;
  terrain?: Terrain;
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
    terrain: entry.terrain,
  }),
);

const DEFAULT_STYLE: Style =
  STYLES.find((style) => style.key === catalog.default) ?? STYLES[0]!;

/**
 * Whether two styles show the same: the same style document, and the same
 * terrain. A catalog entry can share another's URL and add terrain to it,
 * so styles with keys compare by them.
 */
export function sameStyle(a: Style, b: Style): boolean {
  if (a.key !== null && b.key !== null) return a.key === b.key;
  if ("url" in a.source && "url" in b.source)
    return a.source.url === b.source.url;
  if ("json" in a.source && "json" in b.source)
    return a.source.json === b.source.json;
  return false;
}

// The style for `?style=`: a key of STYLES, or any style's URL (the first of
// STYLES there), which the page names by the style's own name once it loads.
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
