// JS side of the ffmpeg.wasm bridge. wasm-bindgen imports this module by the
// path in src/preview/web_engine.rs; Trunk copies web/ next to the wasm.
//
// The worker files in web/ffmpeg_worker/ are vendored from
// @ffmpeg/ffmpeg@0.12.15/dist/esm. Browsers refuse cross-origin Worker URLs,
// so the CDN import of @ffmpeg/ffmpeg must be told to load the worker from
// this same-origin copy via `classWorkerURL`.
//
// ffmpeg.wasm is a singleton whose MEMFS is shared by every caller, so all
// operations go through one promise chain. Rust still guards against stale
// previews with generation tokens. Every exported call resolves to an object
// instead of rejecting, because wasm-bindgen async imports cannot return
// `Result` here.

import { FFmpeg } from "https://cdn.jsdelivr.net/npm/@ffmpeg/ffmpeg@0.12.15/dist/esm/index.js";
import { toBlobURL } from "https://cdn.jsdelivr.net/npm/@ffmpeg/util@0.12.2/dist/esm/index.js";

const CORE = "https://cdn.jsdelivr.net/npm/@ffmpeg/core@0.12.10/dist/esm";

const ffmpeg = new FFmpeg();
let loading = null;
let queue = Promise.resolve();
let progress = null;
let log = "";

ffmpeg.on("progress", ({ progress: value }) => {
  if (progress) progress(value);
});

ffmpeg.on("log", ({ message }) => {
  log += message + "\n";
  // Only the tail is ever shown, so cap the buffer.
  if (log.length > 65536) log = log.slice(-32768);
});

function load() {
  if (ffmpeg.loaded) return Promise.resolve();
  if (!loading) {
    loading = (async () => {
      await ffmpeg.load({
        // Absolute URL: classes.js resolves classWorkerURL against its own
        // (CDN) import.meta.url, so a root-relative path would hit the CDN.
        classWorkerURL: new URL("/ffmpeg_worker/worker.js", location.href).href,
        coreURL: await toBlobURL(`${CORE}/ffmpeg-core.js`, "text/javascript"),
        wasmURL: await toBlobURL(`${CORE}/ffmpeg-core.wasm`, "application/wasm"),
      });
    })();
  }
  return loading;
}

// Serialize every MEMFS operation: one exec at a time, writes included.
function serial(task) {
  const next = queue.then(task, task);
  queue = next.catch(() => {});
  return next;
}

function guard(task) {
  return serial(async () => {
    try {
      await load();
      return { ok: true, value: await task() };
    } catch (error) {
      return { ok: false, error: String(error?.message ?? error) };
    }
  });
}

export function ffExec(args, onProgress) {
  return serial(async () => {
    log = "";
    progress = onProgress ?? null;
    try {
      await load();
      const code = await ffmpeg.exec(Array.from(args));
      return { code: typeof code === "number" ? code : 0, log };
    } catch (error) {
      return { code: -1, log: `${log}\n${error}` };
    } finally {
      progress = null;
    }
  });
}

export function ffWrite(name, data) {
  return guard(() => ffmpeg.writeFile(name, data));
}

export function ffRead(name) {
  return guard(() => ffmpeg.readFile(name));
}

export function ffDelete(name) {
  return guard(() =>
    ffmpeg.deleteFile(name).catch(() => {
      // Already gone; deleting is best-effort cleanup.
    }),
  );
}

export function ffDownload(name, filename) {
  return guard(async () => {
    const data = await ffmpeg.readFile(name);
    const url = URL.createObjectURL(
      new Blob([data.buffer], { type: "video/mp4" }),
    );
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = filename || "export.mp4";
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    // Give the browser a tick to start the download before revoking.
    setTimeout(() => URL.revokeObjectURL(url), 10000);
  });
}
