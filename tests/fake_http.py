"""A small HTTP server for the HTTP client tests: JSON, an echo of what was sent, a redirect and an error."""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, status, body, content_type="application/json"):
        data = (json.dumps(body) if content_type == "application/json" else body).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def handle_any(self):
        url = urlparse(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length).decode("utf-8", "replace") if length else ""
        if url.path == "/hello":
            self.send(200, {"hello": "world", "query": {k: v[0] for k, v in parse_qs(url.query).items()}})
        elif url.path == "/echo":
            self.send(200, {"method": self.command, "token": self.headers.get("X-Token"), "type": self.headers.get("Content-Type"), "body": body})
        elif url.path == "/moved":
            self.send_response(302)
            self.send_header("Location", "/hello?from=redirect")
            self.send_header("Content-Length", "0")
            self.end_headers()
        else:
            self.send(404, "nothing at " + url.path + "\n", "text/plain")

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = handle_any


class Server:
    def start(self):
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()
        return self

    def stop(self):
        self.httpd.shutdown()
        self.httpd.server_close()
