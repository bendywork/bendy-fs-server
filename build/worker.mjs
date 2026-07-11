import wasmInit, { fetch as wasmFetch } from './bendy_fs_server.js';
import wasmModule from './bendy_fs_server_bg.wasm';

let ready;

export default {
  async fetch(request, env, ctx) {
    if (!ready) ready = wasmInit({ module_or_path: wasmModule });
    await ready;
    try {
      return await wasmFetch(request, env, ctx);
    } catch (e) {
      console.error('Worker fetch error:', e?.message || e, e?.stack || '');
      return new Response('Internal Server Error', { status: 500 });
    }
  }
};
