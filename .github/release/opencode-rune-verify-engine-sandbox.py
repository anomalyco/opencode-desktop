#!/usr/bin/env python3
"""Live start and OS-sandbox check for packaged browser-engine builds (macOS and Linux).

Usage:
  python3 opencode-rune-verify-engine-sandbox.py macos <path/to/opencode-browser-engine> [--require-sandbox]
  python3 opencode-rune-verify-engine-sandbox.py linux <path/to/opencode-browser-engine> [--require-sandbox]

Always required: the engine prints `listening`, answers `hello` with `ready`, and loads a local
loopback HTTP page. Then it prints the OS sandbox state of every renderer subprocess:
  - macOS: libSystem sandbox_check(pid, NULL, SANDBOX_FILTER_NONE) == 1
  - Linux: /proc/<pid>/status reports Seccomp: 2 and NoNewPrivs: 1
With --require-sandbox, a renderer that is not sandboxed (or no renderer found) fails the check.

The engine's stderr goes to a file (never an unread pipe). On any failure the script prints the
engine's exit code, its stderr tail, its process tree and the tail of Chromium's debug.log.
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

TIMEOUT_S = 25
TAIL_LINES = 80


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


class ProbeError(Exception):
    pass


def main() -> int:
    args = sys.argv[1:]
    require = "--require-sandbox" in args
    args = [a for a in args if a != "--require-sandbox"]
    if len(args) != 2 or args[0] not in ("macos", "linux"):
        print("Usage: opencode-rune-verify-engine-sandbox.py <macos|linux> <engine-bin> [--require-sandbox]", file=sys.stderr)
        return 2
    platform, bin_path = args
    work = tempfile.mkdtemp(prefix="opencode-sandbox-probe-")
    sock_path = os.path.join(work, "e.sock")
    prof_dir = os.path.join(work, "profile")
    os.makedirs(prof_dir, exist_ok=True)
    err_path = os.path.join(work, "engine-stderr.txt")
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

    started = time.time()
    err_file = open(err_path, "w")
    proc = subprocess.Popen(
        [bin_path, "--socket", sock_path, "--lang", "en-US"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=err_file,
        text=True,
    )
    stdout_lines = []
    try:
        line = proc.stdout.readline() if proc.stdout else ""
        stdout_lines.append(line)
        # Keep stdout drained too, so the engine can never block on a full pipe.
        threading.Thread(target=lambda: stdout_lines.extend(proc.stdout), daemon=True).start()
        if line.strip() != "listening":
            raise ProbeError(f"expected 'listening' on stdout, got {line!r}")
        print(f"{platform}: listening after {time.time() - started:.2f}s")
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(TIMEOUT_S)
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
        ready = False
        title_ok = False
        browser_pid = proc.pid
        deadline = time.time() + TIMEOUT_S
        while time.time() < deadline and not title_ok:
            try:
                raw = fp.readline()
            except socket.timeout:
                break
            if not raw:
                raise ProbeError("engine closed the socket before the probe page loaded")
            msg = json.loads(raw)
            kind = msg.get("type")
            if kind == "ready":
                ready = True
                print(f"{platform}: ready after {time.time() - started:.2f}s")
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
        if not ready:
            raise ProbeError(f"no 'ready' within {TIMEOUT_S}s after 'hello'")
        if not title_ok:
            raise ProbeError(f"probe page did not load within {TIMEOUT_S}s")
        print(f"{platform}: probe page loaded after {time.time() - started:.2f}s")
        time.sleep(0.5)

        rows = renderer_sandbox(platform, browser_pid, bin_path)
        if not rows:
            print(f"{platform}: no renderer subprocess found")
            if require:
                raise ProbeError("no renderer subprocess found (--require-sandbox)")
        for pid, sandboxed, detail in rows:
            print(f"{platform}: renderer pid={pid} sandboxed={'yes' if sandboxed else 'no'} {detail}")
        unsandboxed = [pid for pid, sandboxed, _ in rows if not sandboxed]
        if unsandboxed and require:
            raise ProbeError(f"renderer(s) not sandboxed (--require-sandbox): {unsandboxed}")
        if not require:
            print(f"{platform}: sandbox state reported only (no --require-sandbox)")

        send({"type": "shutdown"})
        sock.close()
        return 0
    except Exception as e:
        print(f"::error::{platform} browser engine probe failed: {type(e).__name__}: {e}")
        report_failure(platform, proc, work, err_path, stdout_lines)
        return 1
    finally:
        srv.shutdown()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)
        err_file.close()
        shutil.rmtree(work, ignore_errors=True)


def renderer_sandbox(platform, browser_pid, bin_path):
    """(pid, sandboxed, detail) for each renderer of the engine."""
    if platform == "macos":
        lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        lib.sandbox_check.restype = ctypes.c_int
        rows = []
        # Match by this probe's own app bundle, not by parent pid: run 44980898474's macOS x64
        # job loaded the page yet found no renderer whose parent was the engine.
        for pid, ppid, cmd in macos_processes(bin_path):
            if "--type=renderer" in cmd:
                sb = lib.sandbox_check(ctypes.c_int(pid), ctypes.c_char_p(None), ctypes.c_int(0))
                rows.append((pid, sb == 1, f"sandbox_check={sb} ppid={ppid} engine={browser_pid}"))
        return rows
    # Linux renderers are forked by the zygote and keep the zygote's command line
    # (`--type=zygote`), so find them as the zygote's children instead of by `--type=renderer`.
    procs = {p["pid"]: p for p in linux_tree(browser_pid)}
    rows = []
    for p in procs.values():
        parent = procs.get(p["ppid"])
        has_children = any(c["ppid"] == p["pid"] for c in procs.values())
        forked = p["type"] == "zygote" and parent is not None and parent["type"] == "zygote" and not has_children
        if p["type"] == "renderer" or forked:
            rows.append((p["pid"], p["seccomp"] == "2" and p["nnp"] == "1", f"Seccomp={p['seccomp']} NoNewPrivs={p['nnp']}"))
    return rows


def macos_processes(bin_path):
    """Processes running from the engine's app bundle (`-ww`: never cut the command line)."""
    # Both spellings: a temp folder under /var is /private/var once resolved.
    bundles = {os.path.dirname(os.path.dirname(f(bin_path))) for f in (os.path.abspath, os.path.realpath)}
    out = subprocess.check_output(["ps", "-axww", "-o", "pid=,ppid=,command="], text=True)
    rows = []
    for ln in out.splitlines():
        parts = ln.strip().split(None, 2)
        if len(parts) == 3 and any(b in parts[2] for b in bundles):
            rows.append((int(parts[0]), int(parts[1]), parts[2]))
    return rows


def linux_tree(root):
    """The engine's process tree from /proc, root first."""
    info = {}
    for stat in glob.glob("/proc/[0-9]*/stat"):
        pid = int(stat.split("/")[2])
        try:
            ppid = int(open(stat).read().rsplit(")", 1)[1].split()[1])
            cmd = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
            status = dict(l.split(":", 1) for l in open(f"/proc/{pid}/status").read().splitlines() if ":" in l)
        except (OSError, ValueError, IndexError):
            continue
        ptype = next((c.decode(errors="replace")[len("--type="):] for c in cmd if c.startswith(b"--type=")), "browser")
        info[pid] = {
            "pid": pid,
            "ppid": ppid,
            "type": ptype,
            "exe": os.path.basename(cmd[0].decode(errors="replace")),
            "state": status.get("State", "?").strip(),
            "seccomp": status.get("Seccomp", "?").strip(),
            "nnp": status.get("NoNewPrivs", "?").strip(),
        }
    out, queue = [], [root]
    while queue:
        pid = queue.pop(0)
        if pid in info:
            out.append(info[pid])
            queue.extend(p for p, v in info.items() if v["ppid"] == pid)
    return out


def report_failure(platform, proc, work, err_path, stdout_lines):
    code = proc.poll()
    print(f"--- engine exit code: {'still running' if code is None else code}")
    print("--- engine process tree:")
    if platform == "linux":
        for p in linux_tree(proc.pid):
            print(f"  pid={p['pid']} ppid={p['ppid']} type={p['type']} exe={p['exe']} state={p['state']} Seccomp={p['seccomp']} NoNewPrivs={p['nnp']}")
    else:
        for pid, ppid, cmd in macos_processes(proc.args[0]):
            ptype = next((a for a in cmd.split() if a.startswith("--type=")), "--type=browser")
            print(f"  pid={pid} ppid={ppid} {ptype} {cmd[:160]}")
    if code is None:
        proc.kill()
        try:
            print(f"--- engine exit code after kill: {proc.wait(timeout=10)}")
        except subprocess.TimeoutExpired:
            print("--- engine did not exit 10s after kill")
    print_tail("engine stdout", "".join(stdout_lines))
    print_tail("engine stderr", open(err_path, errors="replace").read())
    logs = glob.glob(os.path.join(work, "**", "debug.log"), recursive=True)
    if not logs:
        print("--- no Chromium debug.log under the profile")
    for log in logs:
        print_tail(f"Chromium {os.path.relpath(log, work)}", open(log, errors="replace").read())


def print_tail(name, text):
    lines = text.splitlines()
    print(f"--- {name}: {len(text)} bytes, last {min(len(lines), TAIL_LINES)} lines:")
    for ln in lines[-TAIL_LINES:]:
        print(f"  {ln}")


if __name__ == "__main__":
    raise SystemExit(main())
