// The styles the page offers: keyless styles from two providers, which draw
// two tile schemas (OpenMapTiles and Shortbread). `dark` turns the page's
// controls dark.

export const STYLES = [
  {
    key: 'liberty',
    name: 'Liberty',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/liberty',
  },
  {
    key: 'bright',
    name: 'Bright',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/bright',
  },
  {
    key: 'positron',
    name: 'Positron',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/positron',
  },
  {
    key: 'dark',
    name: 'Dark Matter',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/dark',
    dark: true,
  },
  {
    key: 'fiord',
    name: 'Fiord',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/fiord',
    dark: true,
  },
  {
    key: 'colorful',
    name: 'Colorful',
    provider: 'VersaTiles',
    url: 'https://tiles.versatiles.org/assets/styles/colorful/style.json',
  },
];

// The style for `?style=`: a key of STYLES, or any style's URL, which the
// page names by the style's own name once it loads.
export function styleFor(param) {
  if (param === null) return STYLES[0];
  const known = STYLES.find((style) => style.key === param || style.url === param);
  if (known) return known;
  const url = new URL(param, location.href);
  return { key: null, name: 'Custom style', provider: url.host, url: url.href };
}
