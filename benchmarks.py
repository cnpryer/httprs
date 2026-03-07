import argparse
import statistics
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass  # suppress request logs

    def do_GET(self):
        body = b"Hello"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length) if length else b""
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def _start_server():
    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return f"http://127.0.0.1:{port}"


def _make_client(pkg, base_url):
    if pkg == "httprs":
        import httprs

        client = httprs.Client()
        return {
            "get": lambda: client.get(base_url + "/"),
            "post_bytes": lambda: client.post(base_url + "/", content=b"x" * 256),
            "post_json": lambda: client.post(base_url + "/", json={"key": "value"}),
            "post_form": lambda: client.post(base_url + "/", data={"key": "value"}),
        }
    elif pkg == "requests":
        import requests

        session = requests.Session()
        return {
            "get": lambda: session.get(base_url + "/"),
            "post_bytes": lambda: session.post(base_url + "/", data=b"x" * 256),
            "post_json": lambda: session.post(base_url + "/", json={"key": "value"}),
            "post_form": lambda: session.post(base_url + "/", data={"key": "value"}),
        }
    elif pkg == "httpx":
        import httpx

        client = httpx.Client()
        return {
            "get": lambda: client.get(base_url + "/"),
            "post_bytes": lambda: client.post(base_url + "/", content=b"x" * 256),
            "post_json": lambda: client.post(base_url + "/", json={"key": "value"}),
            "post_form": lambda: client.post(base_url + "/", data={"key": "value"}),
        }
    else:
        raise ValueError(f"Unknown package: {pkg}")


def _run(fn, n, warmup=5):
    for _ in range(warmup):
        fn()
    times = []
    for _ in range(n):
        t0 = time.perf_counter()
        fn()
        times.append((time.perf_counter() - t0) * 1000)
    return times


def _stats(times):
    return {
        "mean": statistics.mean(times),
        "median": statistics.median(times),
        "stdev": statistics.stdev(times) if len(times) > 1 else 0.0,
        "min": min(times),
        "max": max(times),
    }


def _print_table(results, n):
    benchmarks = ["get", "post_bytes", "post_json", "post_form"]
    packages = list(results.keys())
    comparing = len(packages) > 1

    pkg_w = max((len(p) for p in packages), default=8)
    pkg_w = max(pkg_w, len("package"))
    num_w = 7

    col_hdr = (
        f"  {'package':<{pkg_w}}"
        f"  {'mean':>{num_w}}  {'median':>{num_w}}  {'stdev':>{num_w}}"
        f"  {'min':>{num_w}}  {'max':>{num_w}}  (ms)"
    )
    if comparing:
        col_hdr += "    ratio"
    sep = "  " + "-" * (len(col_hdr) - 2)

    for bench in benchmarks:
        print(f"\n{bench}  (n={n})")
        print(col_hdr)
        print(sep)

        httprs_mean = results.get("httprs", {}).get(bench, {}).get("mean")

        for pkg in packages:
            if bench not in results[pkg]:
                continue
            s = results[pkg][bench]
            ratio_str = ""
            if comparing and httprs_mean and pkg != "httprs":
                ratio = s["mean"] / httprs_mean
                if ratio >= 1.0:
                    ratio_str = f"    {ratio:.2f}x slower"
                else:
                    ratio_str = f"    {1 / ratio:.2f}x faster"
            print(
                f"  {pkg:<{pkg_w}}"
                f"  {s['mean']:>{num_w}.3f}"
                f"  {s['median']:>{num_w}.3f}"
                f"  {s['stdev']:>{num_w}.3f}"
                f"  {s['min']:>{num_w}.3f}"
                f"  {s['max']:>{num_w}.3f}"
                f"{ratio_str}"
            )


def main():
    parser = argparse.ArgumentParser(
        description="Benchmark httprs against other HTTP libraries"
    )
    parser.add_argument(
        "--packages",
        nargs="*",
        default=[],
        metavar="PKG",
        help="packages to compare against httprs (e.g. requests httpx)",
    )
    parser.add_argument(
        "-n",
        type=int,
        default=200,
        metavar="N",
        help="iterations per benchmark (default: 200)",
    )
    args = parser.parse_args()

    base_url = _start_server()
    print(f"Server running at {base_url}", file=sys.stderr)

    all_packages = ["httprs"] + args.packages
    results = {}

    for pkg in all_packages:
        try:
            clients = _make_client(pkg, base_url)
        except ImportError:
            print(f"WARNING: '{pkg}' not installed — skipping", file=sys.stderr)
            continue
        except ValueError as e:
            print(f"WARNING: {e} — skipping", file=sys.stderr)
            continue

        print(f"Benchmarking {pkg}...", file=sys.stderr)
        results[pkg] = {}
        for bench, fn in clients.items():
            times = _run(fn, args.n)
            results[pkg][bench] = _stats(times)

    _print_table(results, args.n)
    print()


if __name__ == "__main__":
    main()
