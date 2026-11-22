// The styles the page offers, as the desktop app does
// (examples/desktop/src/styles.rs): keyless styles from three providers, which
// draw vector tiles of two schemas (OpenMapTiles and Shortbread), satellite
// imagery in raster tiles, and Mapterhorn's terrain under one of them. `dark`
// turns the page's controls dark.

import type {
  HillshadeLayerSpecification,
  RasterDEMSourceSpecification,
  SkySpecification,
} from "maplibre-gl";

/**
 * 3D terrain that the map adds to a style that has none once it loads
 * (WebMap::set_style_url), as does GL JS's map in the comparison
 * (compare.ts): the `raster-dem` source, the style's `terrain` on it at
 * `exaggeration`, a hillshade of it with the paint `hillshade` under the
 * layer `hillshadeBefore` if the style has it, and `sky` where the style has
 * none, so that fog shows at a high pitch.
 */
export interface Terrain {
  source: RasterDEMSourceSpecification;
  exaggeration: number;
  hillshade: HillshadeLayerSpecification["paint"];
  hillshadeBefore: string;
  sky: SkySpecification;
}

/** A style the page can show; `key` is null for one from a URL. */
export interface Style {
  key: string | null;
  name: string;
  provider: string;
  url: string;
  dark?: boolean;
  terrain?: Terrain;
}

const LIBERTY: Style = {
  key: "liberty",
  name: "Liberty",
  provider: "OpenFreeMap",
  url: "https://tiles.openfreemap.org/styles/liberty",
};

// Mapterhorn's global Terrarium tiles (mapterhorn.com/data-access), whose
// TileJSON brings their attribution.
const MAPTERHORN: Terrain = {
  source: {
    type: "raster-dem",
    url: "https://tiles.mapterhorn.com/tilejson.json",
  },
  exaggeration: 1.3,
  hillshade: {
    "hillshade-exaggeration": 0.3,
    "hillshade-shadow-color": "#473b24",
  },
  hillshadeBefore: "building",
  // The style spec's default colors, whose white fog greys the far ground.
  sky: {
    "sky-color": "#88c6fc",
    "horizon-color": "#ffffff",
    "fog-color": "#ffffff",
    "fog-ground-blend": 0.5,
  },
};

export const STYLES: Style[] = [
  LIBERTY,
  {
    key: "liberty-3d",
    name: "Liberty 3D",
    provider: "OpenFreeMap + Mapterhorn",
    url: LIBERTY.url,
    terrain: MAPTERHORN,
  },
  {
    key: "bright",
    name: "Bright",
    provider: "OpenFreeMap",
    url: "https://tiles.openfreemap.org/styles/bright",
  },
  {
    key: "positron",
    name: "Positron",
    provider: "OpenFreeMap",
    url: "https://tiles.openfreemap.org/styles/positron",
  },
  {
    key: "dark",
    name: "Dark Matter",
    provider: "OpenFreeMap",
    url: "https://tiles.openfreemap.org/styles/dark",
    dark: true,
  },
  {
    key: "fiord",
    name: "Fiord",
    provider: "OpenFreeMap",
    url: "https://tiles.openfreemap.org/styles/fiord",
    dark: true,
  },
  {
    key: "colorful",
    name: "Colorful",
    provider: "VersaTiles",
    url: "https://tiles.versatiles.org/assets/styles/colorful/style.json",
  },
  {
    key: "satellite",
    name: "Satellite",
    provider: "VersaTiles",
    url: "https://tiles.versatiles.org/assets/styles/satellite/style.json",
    dark: true,
  },
];

// The style for `?style=`: a key of STYLES, or any style's URL (the first of
// STYLES there), which the page names by the style's own name once it loads.
export function styleFor(param: string | null): Style {
  if (param === null) return LIBERTY;
  const known = STYLES.find(
    (style) => style.key === param || style.url === param,
  );
  if (known) return known;
  const url = new URL(param, location.href);
  return { key: null, name: "Custom style", provider: url.host, url: url.href };
}
