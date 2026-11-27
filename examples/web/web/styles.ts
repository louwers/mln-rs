// The styles the page offers, from the shared catalog (examples/styles.json):
// vector tiles across several schemas (OpenMapTiles, Protomaps, Shortbread,
// and Mapeak), raster-dem hillshade, raster tiles (satellite imagery and
// OpenStreetMap Carto), Mapterhorn's terrain under the satellite imagery, and
// NASA GIBS's imagery of the Arctic in EPSG:3413, a projection that the map
// registers (src/lib.rs). `dark` turns the page's controls dark.

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
 * that sky if the style has none.
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

/**
 * Where the camera goes when the page opens a style or the picker picks it:
 * `center`, a longitude and a latitude as in a style, and `zoom`. The
 * bearing and pitch stay as they are.
 */
export interface StyleCamera {
  center: [number, number];
  zoom: number;
}

/**
 * A style the page can show; `key` is null for one from a URL. Without a
 * `camera`, the camera stays where it is.
 */
export interface Style {
  key: string | null;
  name: string;
  provider: string;
  source: StyleSource;
  dark?: boolean;
  terrain?: Terrain;
  camera?: StyleCamera;
}

interface CatalogEntry {
  key: string;
  name: string;
  provider: string;
  url?: string;
  json?: object;
  dark?: boolean;
  terrain?: Terrain;
  camera?: StyleCamera;
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
    camera: entry.camera,
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
