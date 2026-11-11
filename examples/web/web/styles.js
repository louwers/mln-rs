// The styles the page offers: keyless styles from two providers, which draw
// two tile schemas (OpenMapTiles and Shortbread). `swatch` is the style's
// [land, water, road] colors for its button; `dark` turns the page's
// controls dark.

export const STYLES = [
  {
    key: 'liberty',
    name: 'Liberty',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/liberty',
    swatch: ['#f8f4f0', '#9ebdff', '#e9ac77'],
  },
  {
    key: 'bright',
    name: 'Bright',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/bright',
    swatch: ['#f8f4f0', '#aecfe2', '#c89366'],
  },
  {
    key: 'positron',
    name: 'Positron',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/positron',
    swatch: ['#f2f3f0', '#c2c8ca', '#ffffff'],
  },
  {
    key: 'dark',
    name: 'Dark Matter',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/dark',
    swatch: ['#0c0c0c', '#1b1b1d', '#3b3b3e'],
    dark: true,
  },
  {
    key: 'fiord',
    name: 'Fiord',
    provider: 'OpenFreeMap',
    url: 'https://tiles.openfreemap.org/styles/fiord',
    swatch: ['#45516e', '#38435c', '#7b89a8'],
    dark: true,
  },
  {
    key: 'colorful',
    name: 'Colorful',
    provider: 'VersaTiles',
    url: 'https://tiles.versatiles.org/assets/styles/colorful/style.json',
    swatch: ['#f8f4f0', '#bfd9f2', '#f0ccad'],
  },
];

// The style for `?style=`: a key of STYLES, or any style's URL, which the
// page names by the style's own name once it loads.
export function styleFor(param) {
  if (param === null) return STYLES[0];
  const known = STYLES.find((style) => style.key === param || style.url === param);
  if (known) return known;
  const url = new URL(param, location.href);
  return { key: null, name: 'Custom style', provider: url.host, url: url.href, swatch: null };
}
