# Vendored ffmpeg.wasm worker

These files are copied from `@ffmpeg/ffmpeg@0.12.15/dist/esm` (`worker.js`,
`const.js`, `errors.js`). The package is MIT-licensed:
<https://github.com/ffmpegwasm/ffmpeg.wasm>

They are served from the app origin because browsers refuse cross-origin Worker
URLs. `web/ffmpeg_bridge.js` passes `worker.js` via `classWorkerURL`. Keep the
three files version-matched when bumping the CDN imports in the bridge.
