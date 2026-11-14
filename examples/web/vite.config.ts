import { defineConfig } from 'vite';

// The page in web/, built into dist/ with every asset's name hashed by its
// content, so that a browser never mixes files of two builds. Its URLs are
// relative, so that it works at any path (GitHub Pages serves it under
// /mln-rs/). web/pkg is wasm-bindgen's output, which `mise run web:build`
// writes first.
export default defineConfig({
  root: 'web',
  base: './',
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    target: 'es2023',
    // GL JS, which only a comparison loads, is a chunk of about 1 MB.
    chunkSizeWarningLimit: 1100,
  },
  worker: {
    format: 'es',
  },
});
