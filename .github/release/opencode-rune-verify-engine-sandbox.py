#!/usr/bin/env python3
"""Live OS-sandbox verification for packaged browser-engine builds (macOS and Linux).

Usage:
  python3 opencode-rune-verify-engine-sandbox.py macos <path/to/opencode-browser-engine>
  python3 opencode-rune-verify-engine-sandbox.py linux <path/to/opencode-browser-engine>

Launches the engine against a local loopback HTTP page, verifies the page loads, and checks that
every spawned renderer subprocess is confined by the OS sandbox:
  - macOS: libSystem sandbox_check(pid, NULL, SANDBOX_FILTER_NONE) == 1
  - Linux: /proc/<pid>/status reports Seccomp: 2 and NoNewPrivs: 1
"""
import ctypes
import glob
import http.server
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time


class _Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"<!doctype html><title>SandboxProbeOK</title><h1>OK</h1>"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[1] not in ("macos", "linux"):
        print("Usage: opencode-rune-verify-engine-sandbox.py <macos|linux> <engine-bin>", file=sys.stderr)
        return 2
    platform, bin_path = sys.argv[1], sys.argv[2]
    work = tempfile.mkdtemp(prefix="opencode-sandbox-probe-")
    sock_path = os.path.join(work, "e.sock")
    prof_dir = os.path.join(work, "profile")
    os.makedirs(prof_dir, exist_ok=True)
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    env = os.environ.copy()
    env["OPENCODE_BROWSER_PROFILE"] = prof_dir
    env["OPENCODE_RUNE_OTEL"] = "0"
    if platform == "macos":
        env["OPENCODE_BROWSER_MOCK_KEYCHAIN"] = "1"
    else:
        env.pop("DISPLAY", None)
        env.pop("WAYLAND_DISPLAY", None)

    proc = subprocess.Popen(
        [bin_path, "--socket", sock_path, "--lang", "en-US"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        line = proc.stdout.readline() if proc.stdout else ""
        if line.strip() != "listening":
            err = proc.stderr.read() if proc.stderr else ""
            raise RuntimeError(f"expected 'listening', got {line!r}; stderr={err}")
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(25)
        sock.connect(sock_path)
        fp = sock.makefile("rwb", buffering=0)

        def send(obj):
            fp.write((json.dumps(obj) + "\n").encode("utf-8"))

        send({
            "type": "hello",
            "mach_service": None,
            "pid": os.getpid(),
            "prompts": True,
            "extensions": False,
            "download_save": True,
        })
        title_ok = False
        browser_pid = proc.pid
        deadline = time.time() + 25
        while time.time() < deadline and not title_ok:
            raw = fp.readline()
            if not raw:
                break
            msg = json.loads(raw)
            kind = msg.get("type")
            if kind == "ready":
                browser_pid = int(msg.get("pid", proc.pid))
                send({
                    "type": "open_tab",
                    "tab_id": 1,
                    "url": f"http://127.0.0.1:{srv.server_address[1]}/",
                    "width_pt": 640.0,
                    "height_pt": 480.0,
                    "scale": 1.0,
                    "focused": True,
                })
            elif kind == "tab_update" and msg.get("tab", {}).get("title") == "SandboxProbeOK":
                title_ok = True
        if not title_ok:
            raise RuntimeError(f"{platform} sandboxed probe page did not load")
        time.sleep(0.5)

        if platform == "macos":
            ps = subprocess.check_output(["ps", "-axo", "pid=,ppid=,command="], text=True)
            renderers = []
            for ln in ps.splitlines():
                parts = ln.strip().split(None, 2)
                if len(parts) == 3 and int(parts[1]) == browser_pid and "--type=renderer" in parts[2]:
                    renderers.append(int(parts[0]))
            if not renderers:
                raise RuntimeError("no macOS renderer subprocess found")
            lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
            lib.sandbox_check.restype = ctypes.c_int
            for rpid in renderers:
                sb = lib.sandbox_check(ctypes.c_int(rpid), ctypes.c_char_p(None), ctypes.c_int(0))
                print(f"macOS sandbox verified: renderer pid={rpid} sandbox_check={sb}")
                if sb != 1:
                    raise RuntimeError(f"macOS renderer pid={rpid} not sandboxed (sandbox_check={sb})")
        else:
            renderers = []
            for cpath in glob.glob("/proc/[0-9]*/cmdline"):
                try:
                    cmd = open(cpath, "rb").read()
                except OSError:
                    continue
                if b"opencode-browser-engine-helper" in cmd and b"--type=renderer" in cmd:
                    renderers.append(int(cpath.split("/")[2]))
            if not renderers:
                raise RuntimeError("no Linux renderer subprocess found")
            for rpid in renderers:
                status = open(f"/proc/{rpid}/status", encoding="utf-8").read()
                seccomp = [l for l in status.splitlines() if l.startswith("Seccomp:")]
                nnp = [l for l in status.splitlines() if l.startswith("NoNewPrivs:")]
                print(f"Linux sandbox verified: renderer pid={rpid} {seccomp} {nnp}")
                if not (any("2" in l for l in seccomp) and any("1" in l for l in nnp)):
                    raise RuntimeError(f"Linux renderer pid={rpid} not sandboxed: {seccomp} {nnp}")

        send({"type": "shutdown"})
        sock.close()
        return 0
    finally:
        srv.shutdown()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
