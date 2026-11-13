// A tile workers' Web Worker (tile-worker.ts) on the module that draws with
// WebGPU.

import init, { mlnWebServeTileWorker, panic_for_check } from './pkg/mln_web.js';
import { serve } from './tile-worker.ts';

serve({ default: init, mlnWebServeTileWorker, panic_for_check });
