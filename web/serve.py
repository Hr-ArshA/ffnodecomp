#!/usr/bin/env python3
"""Serve dist/ with the cross-origin isolation headers ffmpeg.wasm asks for.

    python3 web/serve.py          # http://127.0.0.1:8080
"""

import http.server
import os
import socketserver

PORT = int(os.environ.get("PORT", "8080"))
ROOT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "dist")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=ROOT, **kwargs)

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


with socketserver.TCPServer(("127.0.0.1", PORT), Handler) as httpd:
    print(f"Serving {ROOT} on http://127.0.0.1:{PORT}")
    httpd.serve_forever()
