"""Check direct text delivery and its acknowledgement in an isolated Chrome profile."""
import argparse
import http.server
import queue
import secrets
import subprocess
import tempfile
import threading
from pathlib import Path


def check(voai, browser):
    route = "/" + secrets.token_hex(16)
    events = queue.Queue()
    page = """<!DOCTYPE html><meta charset="utf-8"><title>Voai input test</title>
        <textarea autofocus>до СЛОВО после</textarea><script>
        const field = document.querySelector("textarea");
        const report = body => fetch(location.pathname, {method: "POST", body});
        field.focus();
        field.setSelectionRange(3, 8);
        field.oninput = () => report(field.value);
        report("ready");
        </script>""".encode()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            if self.path != route:
                return self.send_error(404)
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(page)

        def do_POST(self):
            if self.path != route:
                return self.send_error(404)
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= 512:
                return self.send_error(400)
            events.put(self.rfile.read(size).decode())
            self.send_response(204)
            self.end_headers()

    with tempfile.TemporaryDirectory(prefix="voai-chrome-") as profile:
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            threading.Thread(target=server.serve_forever, daemon=True).start()
            url = f"http://127.0.0.1:{server.server_port}{route}"
            chrome = subprocess.Popen([
                browser, f"--user-data-dir={profile}", "--no-first-run",
                "--no-default-browser-check", "--disable-sync", "--use-mock-keychain",
                "--disable-background-networking", "--disable-component-update", "--new-window", url,
            ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                assert events.get(timeout=20) == "ready"
                # Target only this newly launched profile, even when the user's Chrome is open.
                driver = """
                    import AppKit
                    let args = CommandLine.arguments
                    let pid = Int32(args[1])!
                    guard let chrome = NSRunningApplication(processIdentifier: pid) else { exit(1) }
                    chrome.activate()
                    RunLoop.current.run(until: Date().addingTimeInterval(0.4))
                    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { exit(2) }
                    let probe = Process()
                    probe.executableURL = URL(fileURLWithPath: args[2])
                    probe.arguments = ["probe-input", "--pid", args[1]]
                    try probe.run()
                    while probe.isRunning {
                        if NSWorkspace.shared.frontmostApplication?.processIdentifier != pid {
                            chrome.activate()
                        }
                        RunLoop.current.run(until: Date().addingTimeInterval(0.03))
                    }
                    exit(probe.terminationStatus)
                """
                subprocess.run(["swift", "-e", driver, str(chrome.pid), voai], check=True, timeout=25)
                assert events.get(timeout=3) == "до Тест Voai 🎙 после"
                print("Chrome confirmed the Unicode replacement in the DOM.")
            finally:
                chrome.terminate()
                try:
                    chrome.wait(timeout=4)
                except subprocess.TimeoutExpired:
                    chrome.kill()
                    chrome.wait()
                server.shutdown()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("voai", type=Path)
    parser.add_argument("--browser", type=Path, default=Path(
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"))
    args = parser.parse_args()
    check(args.voai.resolve(), args.browser)
