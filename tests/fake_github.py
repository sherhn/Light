#!/usr/bin/env python3
"""A stand-in for api.github.com used by the UI tests (LIGHT_GITHUB_API points at it).

It serves one account (`octocat`, token FAKE_TOKEN) with a handful of repositories whose clone URLs are local git
repositories, so cloning from the list works without the network. Anything but the right token gets a 401 on the
account endpoints, like the real API.
"""
import json
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

FAKE_TOKEN = "ghp_fakeTOKENforTESTS0123456789"
ACCOUNT = {"login": "octocat", "name": "The Octocat"}
OTHER = {"login": "ghost", "name": None}


def make_remote(base, name, files):
    """A one-commit git repository at base/name that a clone can read."""
    path = base / name
    path.mkdir(parents=True, exist_ok=True)
    for rel, text in files.items():
        (path / rel).parent.mkdir(parents=True, exist_ok=True)
        (path / rel).write_text(text, encoding="utf-8")

    def git(*args):
        subprocess.run(["git", "-c", "commit.gpgsign=false", *args], cwd=path, capture_output=True, check=True)

    git("init", "-q", "-b", "main")
    git("config", "user.email", "e2e@example.com")
    git("config", "user.name", "E2E")
    git("config", "core.autocrlf", "false")
    git("add", "-A")
    git("commit", "-q", "-m", "first")
    return path


def repo(owner, name, url, **kw):
    r = {
        "full_name": f"{owner}/{name}", "name": name, "owner": {"login": owner}, "description": kw.get("description"),
        "private": kw.get("private", False), "fork": kw.get("fork", False), "archived": kw.get("archived", False),
        "language": kw.get("language"), "stargazers_count": kw.get("stars", 0), "pushed_at": kw.get("pushed", "2026-10-20T10:00:00Z"),
        "clone_url": url, "default_branch": "main",
    }
    return r


class Server:
    def __init__(self, remotes):
        """remotes: {"lisp-demo": path, "private-notes": path, ...} of local repositories."""
        self.requests = []
        r = remotes
        self.mine = [
            repo("octocat", "lisp-demo", str(r["lisp-demo"]), description="A small Common Lisp demo", language="Common Lisp", stars=12, pushed="2026-10-20T10:00:00Z"),
            repo("octocat", "private-notes", str(r["private-notes"]), description="Only for me", private=True, language="Markdown", pushed="2026-10-01T10:00:00Z"),
            repo("acme", "shared-lib", str(r["lisp-demo"]), description="Library shared by the team", language="Common Lisp", stars=340, pushed="2026-09-01T10:00:00Z"),
            repo("octocat", "forked-tools", str(r["lisp-demo"]), description="A fork", fork=True, pushed="2025-01-01T10:00:00Z"),
            repo("octocat", "old-archive", str(r["lisp-demo"]), description=None, archived=True, pushed="2023-01-01T10:00:00Z"),
        ]
        self.public = [x for x in self.mine if not x["private"] and x["owner"]["login"] == "octocat"]
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def send(self, status, body):
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                url = urlparse(self.path)
                q = parse_qs(url.query)
                auth = self.headers.get("Authorization", "")
                outer.requests.append((url.path, auth))
                authed = auth == f"Bearer {FAKE_TOKEN}"
                page = int(q.get("page", ["1"])[0])
                if url.path == "/user":
                    return self.send(200, ACCOUNT) if authed else self.send(401, {"message": "Bad credentials"})
                if url.path == "/user/repos":
                    if not authed:
                        return self.send(401, {"message": "Bad credentials"})
                    return self.send(200, outer.mine if page == 1 else [])
                parts = url.path.strip("/").split("/")
                if parts[0] == "users" and len(parts) >= 2:
                    login = parts[1]
                    known = {"octocat": ACCOUNT, "ghost": OTHER}
                    if login not in known:
                        return self.send(404, {"message": "Not Found"})
                    if len(parts) == 2:
                        return self.send(200, known[login])
                    if parts[2] == "repos":
                        return self.send(200, (outer.public if login == "octocat" else []) if page == 1 else [])
                self.send(404, {"message": "Not Found"})

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    @property
    def url(self):
        return f"http://127.0.0.1:{self.httpd.server_address[1]}"

    def start(self):
        self.thread.start()
        return self

    def stop(self):
        self.httpd.shutdown()
        self.httpd.server_close()
