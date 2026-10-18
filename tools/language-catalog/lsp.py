"""A minimal Language Server Protocol client over stdio (standard library only)."""

import json
import os
import subprocess
import threading
import queue


class LspClient:
    def __init__(self, cmd, cwd, log=None):
        self.proc = subprocess.Popen(
            cmd, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
        )
        self.next_id = 0
        self.pending = {}
        self.lock = threading.Lock()
        self.notifications = queue.Queue()
        self.log = log
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        out = self.proc.stdout
        while True:
            headers = {}
            while True:
                line = out.readline()
                if not line:
                    return
                line = line.decode("ascii").strip()
                if not line:
                    break
                k, v = line.split(":", 1)
                headers[k.strip().lower()] = v.strip()
            body = out.read(int(headers["content-length"]))
            msg = json.loads(body)
            if "id" in msg and ("result" in msg or "error" in msg):
                with self.lock:
                    q = self.pending.pop(msg["id"], None)
                if q:
                    q.put(msg)
            elif "id" in msg and "method" in msg:
                # A request from the server (workspace/configuration, ...): answer with nothing.
                self._send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
            else:
                self.notifications.put(msg)

    def _send(self, msg):
        data = json.dumps(msg).encode("utf-8")
        self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
        self.proc.stdin.flush()

    def request(self, method, params, timeout=60):
        with self.lock:
            self.next_id += 1
            rid = self.next_id
            q = queue.Queue()
            self.pending[rid] = q
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        msg = q.get(timeout=timeout)
        if "error" in msg:
            raise RuntimeError(f"{method}: {msg['error']}")
        return msg["result"]

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def close(self):
        try:
            self.request("shutdown", None, timeout=10)
            self.notify("exit", None)
        except Exception:
            pass
        try:
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()
