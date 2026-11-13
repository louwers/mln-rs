// The styles the page offers: keyless styles from two providers, which draw
// vector tiles of two schemas (OpenMapTiles and Shortbread) and satellite
// imagery in raster tiles. `dark` turns the page's controls dark.

/** A style the page can show; `key` is null for one from a URL. */
export interface Style {
  key: string | null;
  name: string;
  provider: string;
  url: string;
  dark?: boolean;
}

const LIBERTY: Style = {
  key: "liberty",
  name: "Liberty",
  provider: "OpenFreeMap",
  url: "https://tiles.openfreemap.org/styles/liberty",
};

export const STYLES: Style[] = [
  LIBERTY,
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

// The style for `?style=`: a key of STYLES, or any style's URL, which the
// page names by the style's own name once it loads.
export function styleFor(param: string | null): Style {
  if (param === null) return LIBERTY;
  const known = STYLES.find(
    (style) => style.key === param || style.url === param,
  );
  if (known) return known;
  const url = new URL(param, location.href);
  return { key: null, name: "Custom style", provider: url.host, url: url.href };
}
