#!/usr/bin/env python3
"""Phase 1 of Rullst issue #832 (13.1 performance work): measurements that reuse the Benchmarks harness.

Servers are the Benchmarks images (bench/<slug>:local, built by perf-832/build.sh), started with the harness's
containers.start_server on the harness's CPU split, loaded with the harness's wrk (loadgen.run_load, 125
connections, 5 s warm-up, 10 s measurement), against the harness's PostgreSQL container. Nothing here changes
what a framework does; the one patched build (rullst-ablate) is described in perf-832/ablate.patch.

Modes:
  throughput  variants x rounds on one path: rps, latency percentiles, CPU per request
  sql         statements and transactions one request runs (pg_stat_statements, log_statement, xact counters)
  profile     perf CPU profile of a server under wrk load
  alloc       heaptrack allocation profile of a native copy of a server under oha load
Usage: BENCH_REPO=<Rullst/Benchmarks checkout> WRK=<wrk> drive.py MODE [options]
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

BENCH_REPO = Path(os.environ["BENCH_REPO"]).resolve()
sys.path.insert(0, str(BENCH_REPO / "bench"))

import common  # noqa: E402
import containers  # noqa: E402
import loadgen  # noqa: E402
import system  # noqa: E402
import workload as W  # noqa: E402
from common import log  # noqa: E402

ALL = ["install", "headers", "cookies", "waf", "csrf", "vary", "connectinfo"]
# Core mounts them innermost to outermost in this order (rullst-core server/stack.rs, security/baseline.rs).
INNER_TO_OUTER = ["vary", "csrf", "waf", "cookies", "headers", "install", "connectinfo"]


def skip(*keep: str) -> str:
    return ",".join(part for part in ALL if part not in keep)


def additive(n: int) -> str:
    """Skip every part except the n innermost ones."""
    return skip(*INNER_TO_OUTER[:n])


# name -> (image slug, manifest to take env from, extra env)
VARIANTS: dict[str, tuple[str, str, dict]] = {
    "axum": ("axum", "axum", {}),
    "minimal": ("rullst-minimal", "rullst-minimal", {}),
    "rullst": ("rullst", "rullst", {}),
    "axum-p": ("axum-p", "axum", {}),
    "minimal-p": ("rullst-minimal-p", "rullst-minimal", {}),
    "rullst-p": ("rullst-p", "rullst", {}),
    "patched-full": ("rullst-ablate", "rullst", {}),
    "bare": ("rullst-ablate", "rullst", {"RULLST_ABLATE": skip()}),
    "add-vary": ("rullst-ablate", "rullst", {"RULLST_ABLATE": additive(1)}),
    "add-csrf": ("rullst-ablate", "rullst", {"RULLST_ABLATE": additive(2)}),
    "add-waf": ("rullst-ablate", "rullst", {"RULLST_ABLATE": additive(3)}),
    "add-cookies": ("rullst-ablate", "rullst", {"RULLST_ABLATE": additive(4)}),
    "add-headers": ("rullst-ablate", "rullst", {"RULLST_ABLATE": additive(5)}),
    "add-install": ("rullst-ablate", "rullst", {"RULLST_ABLATE": additive(6)}),
    "bare-nolog": ("rullst-ablate", "rullst", {"RULLST_ABLATE": skip(), "RUST_LOG": "off"}),
    "full-nolog": ("rullst-ablate", "rullst", {"RUST_LOG": "off"}),
}
for part in ALL:
    VARIANTS[f"no-{part}"] = ("rullst-ablate", "rullst", {"RULLST_ABLATE": part})


class Ctx:
    def __init__(self) -> None:
        self.config = common.load_config()
        self.split = system.cpu_split(self.config["server"]["cpus"], self.config["min_runner_cpus"], 0)
        # The harness itself stays off the server CPUs.
        os.sched_setaffinity(0, self.split["client"])
        self.wrk = Path(os.environ["WRK"])
        self.loadgen = loadgen.describe_binary(self.wrk, self.config["load_generator"])
        self.manifests = {slug: common.load_manifest(slug) for slug in ("axum", "rullst", "rullst-minimal")}
        self.database_info = containers.start_database(self.config, self.split["database"])
        applied: set[str] = set()
        for slug, manifest in self.manifests.items():
            setup = manifest.get("db_setup")
            text = (common.FRAMEWORKS / slug / setup).read_text() if setup else None
            if text is not None and text not in applied:
                containers.apply_db_setup(self.config, manifest)
                applied.add(text)
        # As in the harness, query statistics are off while measuring.
        containers.set_statement_tracking(self.config, False)
        self.db_handle = containers.database_handle(self.config)

    def manifest(self, variant: str) -> dict:
        image, base, env = VARIANTS[variant]
        manifest = dict(self.manifests[base])
        manifest["slug"] = image
        manifest["env"] = {**manifest.get("env", {}), **env}
        return manifest

    def start(self, variant: str) -> containers.Server:
        containers.reset_tables(self.config)
        manifest = self.manifest(variant)
        server = containers.start_server(manifest, self.config, self.split["server"], keep_logs=False)
        server.wait_ready(manifest.get("startup_timeout_s", 120))
        time.sleep(self.config["warmup"].get("settle_s", 2) + 1)
        return server

    def load(self, server: containers.Server, path: str, connections: int, seconds: int, statuses: bool = False) -> dict:
        metrics = loadgen.run_load(
            self.wrk, server.base_url, connections=connections, duration_s=seconds, config=self.config,
            client_cpus=self.split["client"], server=server, database=self.db_handle,
            db_cpus=self.split["database"], workload=W.static(path), count_statuses=statuses)
        metrics.pop("_ram_samples", None)
        return metrics


def environment(ctx: Ctx) -> dict:
    def out(cmd: list[str]) -> str:
        return subprocess.run(cmd, capture_output=True, text=True, check=False).stdout.strip()

    env = system.environment(ctx.split, ctx.loadgen)
    env["benchmarks_commit"] = out(["git", "-C", str(BENCH_REPO), "rev-parse", "HEAD"])
    env["kernel"] = out(["uname", "-a"])
    env["docker"] = out(["docker", "--version"])
    env["runner_image"] = os.environ.get("ImageVersion", "")
    env["database"] = ctx.database_info
    return env


def write(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=1) + "\n")


def keep(metrics: dict) -> dict:
    return {k: metrics.get(k) for k in (
        "rps", "latency_ms", "requests_ok", "non_2xx", "transport_errors", "timeouts", "error_rate",
        "server_cpu_cores", "rps_per_cpu", "db_cpu_cores", "db_us_per_request", "cpu_us_per_request",
        "loadgen_cpu_cores", "client_cpu_busy", "client_bound", "limited_by", "server_ram_peak_mib", "duration_s")}


def throughput(args) -> None:
    ctx = Ctx()
    out = {"mode": "throughput", "path": args.path, "rounds": args.rounds, "env": environment(ctx), "runs": []}
    variants = args.variants.split(",")
    target = Path(args.out)
    try:
        for number in range(args.rounds):
            # Rotate the order each round so that drift over the job is not charged to the same variant.
            order = variants[number % len(variants):] + variants[:number % len(variants)]
            for variant in order:
                log(f"round {number + 1}/{args.rounds}: {variant} {args.path}")
                server = ctx.start(variant)
                try:
                    warm = ctx.load(server, args.path, 125, args.warmup, statuses=True)
                    time.sleep(2)
                    before = containers.rows_updated(ctx.config) if "updates" in args.path else None
                    measured = ctx.load(server, args.path, 125, args.seconds)
                    record = {"variant": variant, "round": number + 1, "warmup_errors": warm["non_2xx"]
                              + warm["transport_errors"], **keep(measured)}
                    if before is not None:
                        time.sleep(1.5)
                        record["rows_updated"] = containers.rows_updated(ctx.config) - before
                    out["runs"].append(record)
                finally:
                    server.stop()
                time.sleep(ctx.config["server"]["cooldown_s"])
                write(target, out)
    finally:
        containers.remove(containers.DB_CONTAINER)
    write(target, out)


def http_get(port: int, path: str) -> int:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=20)
    try:
        conn.request("GET", path)
        response = conn.getresponse()
        response.read()
        return response.status
    finally:
        conn.close()


XACTS = ("SELECT pg_stat_statements_reset(); SELECT xact_commit + xact_rollback FROM pg_stat_database "
         "WHERE datname = current_database()")


def xacts(config: dict) -> int:
    return int(containers.psql(config, XACTS).splitlines()[-1])


def db_log_lines() -> list[str]:
    result = subprocess.run(["docker", "logs", containers.DB_CONTAINER], capture_output=True, text=True, check=False)
    return (result.stdout + result.stderr).splitlines()


def sql(args) -> None:
    ctx = Ctx()
    config = ctx.config
    out = {"mode": "sql", "env": environment(ctx), "runs": []}
    target = Path(args.out)
    try:
        containers.psql(config, "ALTER SYSTEM SET log_statement = 'all'")
        containers.psql(config, "ALTER SYSTEM SET log_line_prefix = '%m [%p] '")
        containers.set_statement_tracking(config, True)
        for variant in args.variants.split(","):
            for path in args.paths.split(","):
                log(f"sql: {variant} {path}")
                server = ctx.start(variant)
                try:
                    for _ in range(60):  # warm the pool and the statement caches
                        http_get(server.port, path)
                    time.sleep(13)
                    r0 = xacts(config)
                    time.sleep(13)
                    r1 = xacts(config)  # also resets pg_stat_statements
                    marker = len(db_log_lines())
                    status = http_get(server.port, path)
                    time.sleep(13)
                    r2 = xacts(config)
                    request_lines = db_log_lines()[marker:]
                    statements = containers.psql(
                        config, "SELECT json_agg(json_build_object('calls', calls, 'rows', rows, 'query', query) "
                                "ORDER BY calls DESC) FROM pg_stat_statements WHERE query NOT ILIKE '%pg_stat_%' "
                                "AND query NOT ILIKE '%pg_database%'")
                    control = r1 - r0
                    log_statements = [l.split("] ", 1)[-1] for l in request_lines
                                      if re.search(r"LOG:\s+(statement|execute)", l) and "pg_stat_" not in l]
                    out["runs"].append({
                        "variant": variant, "path": path, "status": status,
                        "transactions_by_xact_counter": (r2 - r1) - control,
                        "control_window_transactions": control,
                        "pg_stat_statements": json.loads(statements or "null"),
                        "log_statements": log_statements,
                        "log_statement_count": len(log_statements),
                    })
                finally:
                    server.stop()
                write(target, out)
    finally:
        containers.remove(containers.DB_CONTAINER)
    write(target, out)


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=True, text=True, **kwargs)


def profile(args) -> None:
    ctx = Ctx()
    outdir = Path(args.out)
    outdir.mkdir(parents=True, exist_ok=True)
    meta = {"mode": "profile", "env": environment(ctx), "profiles": []}
    try:
        for target in args.targets.split(","):
            variant, path = target.split("=", 1)
            label = f"{variant}{re.sub(r'[^a-z0-9]+', '_', path.lower()).rstrip('_')}"
            log(f"profile: {variant} {path}")
            server = ctx.start(variant)
            try:
                ctx.load(server, path, 125, 5, statuses=True)
                holder: dict = {}
                thread = threading.Thread(target=lambda: holder.update(m=ctx.load(server, path, 125, args.seconds + 8)))
                thread.start()
                time.sleep(3)
                pid = server.state()["Pid"]
                perf_data = outdir / f"{label}.perf.data"
                run(["sudo", "perf", "record", "-F", str(args.freq), "-e", "cpu-clock", "-g", "-p", str(pid),
                     "-o", str(perf_data), "--", "sleep", str(args.seconds)], stderr=subprocess.PIPE)
                thread.join()
                symfs = (Path(args.symfs) / VARIANTS[variant][0]).resolve()
                run(["sudo", "chown", f"{os.getuid()}", str(perf_data)])
                script = outdir / f"{label}.perf.txt"
                with script.open("w") as handle:
                    run(["perf", "script", "-i", str(perf_data), "--symfs", str(symfs), "-F", "comm,tid,ip,sym,dso"],
                        stdout=handle, stderr=subprocess.PIPE)
                folded = outdir / f"{label}.folded"
                with script.open() as source, folded.open("w") as handle:
                    run(["inferno-collapse-perf"], stdin=source, stdout=handle, stderr=subprocess.PIPE)
                with folded.open() as source, (outdir / f"{label}.svg").open("w") as handle:
                    run(["inferno-flamegraph", "--title", f"{variant} {path} (perf, {args.freq} Hz)"], stdin=source,
                        stdout=handle, stderr=subprocess.PIPE)
                perf_data.unlink()
                script.unlink()
                meta["profiles"].append({"label": label, "variant": variant, "path": path,
                                         "load_during_profile": keep(holder.get("m", {}))})
            finally:
                server.stop()
            write(outdir / "profile-meta.json", meta)
    finally:
        containers.remove(containers.DB_CONTAINER)
    write(outdir / "profile-meta.json", meta)


def wait_http(port: int, seconds: float = 60) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if http_get(port, "/text") == 200:
                return
        except OSError:
            time.sleep(0.3)
    raise common.BenchError("native server did not start")


def oha(ctx: Ctx, binary: Path, port: int, path: str, count: int, connections: int = 8) -> dict:
    cmd = ["taskset", "-c", system.format_cpu_list(ctx.split["client"]), str(binary), "-n", str(count),
           "-c", str(connections), "--no-tui", "-j", f"http://127.0.0.1:{port}{path}"]
    result = subprocess.run(cmd, capture_output=True, text=True, check=True)
    data = json.loads(result.stdout)
    return {"summary": data.get("summary"), "status_codes": data.get("statusCodeDistribution"),
            "latency_percentiles": data.get("latencyPercentiles")}


def alloc(args) -> None:
    ctx = Ctx()
    outdir = Path(args.out)
    outdir.mkdir(parents=True, exist_ok=True)
    meta = {"mode": "alloc", "env": environment(ctx), "runs": []}
    server_cpus = system.format_cpu_list(ctx.split["server"])
    port = ctx.config["server"]["port"]
    try:
        for target in args.targets.split(","):
            variant, path = target.split("=", 1)
            image = VARIANTS[variant][0]
            binary = Path(args.bin) / image
            for count in (args.n1, args.n2):
                label = f"{variant}{re.sub(r'[^a-z0-9]+', '_', path.lower()).rstrip('_')}-n{count}"
                log(f"alloc: {variant} {path} n={count}")
                containers.reset_tables(ctx.config)
                manifest = ctx.manifest(variant)
                env = {**os.environ, "PORT": str(port), "DATABASE_URL": containers.database_url(ctx.config),
                       "DB_POOL_SIZE": str(ctx.config["server"]["pool_size"]), "BENCH_WORKERS": "2",
                       **{k: str(v) for k, v in manifest.get("env", {}).items()}}
                prefix = outdir / label
                proc = subprocess.Popen(["taskset", "-c", server_cpus, "heaptrack", "-o", str(prefix), str(binary)],
                                        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                try:
                    wait_http(port)
                    oha(ctx, Path(args.oha), port, path, args.warm)
                    result = oha(ctx, Path(args.oha), port, path, count)
                finally:
                    proc.send_signal(signal.SIGTERM)
                    try:
                        proc.wait(timeout=60)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                    time.sleep(3)
                found = sorted(outdir.glob(f"{label}*.zst")) + sorted(outdir.glob(f"{label}*.gz"))
                entry = {"label": label, "variant": variant, "path": path, "requests": count, "oha": result,
                         "heaptrack_file": found[0].name if found else None}
                if found:
                    printed = subprocess.run(["heaptrack_print", "-f", str(found[0]), "-n", "30"],
                                             capture_output=True, text=True, check=False)
                    (outdir / f"{label}.print.txt").write_text(printed.stdout + printed.stderr)
                meta["runs"].append(entry)
                write(outdir / "alloc-meta.json", meta)
    finally:
        containers.remove(containers.DB_CONTAINER)
    write(outdir / "alloc-meta.json", meta)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="mode", required=True)
    t = sub.add_parser("throughput")
    t.add_argument("--variants", required=True)
    t.add_argument("--path", required=True)
    t.add_argument("--rounds", type=int, default=5)
    t.add_argument("--warmup", type=int, default=5)
    t.add_argument("--seconds", type=int, default=10)
    t.add_argument("--out", required=True)
    s = sub.add_parser("sql")
    s.add_argument("--variants", required=True)
    s.add_argument("--paths", required=True)
    s.add_argument("--out", required=True)
    p = sub.add_parser("profile")
    p.add_argument("--targets", required=True, help="variant=path,...")
    p.add_argument("--seconds", type=int, default=15)
    p.add_argument("--freq", type=int, default=997)
    p.add_argument("--symfs", required=True)
    p.add_argument("--out", required=True)
    a = sub.add_parser("alloc")
    a.add_argument("--targets", required=True, help="variant=path,...")
    a.add_argument("--bin", required=True)
    a.add_argument("--oha", required=True)
    a.add_argument("--n1", type=int, default=3000)
    a.add_argument("--n2", type=int, default=13000)
    a.add_argument("--warm", type=int, default=1000)
    a.add_argument("--out", required=True)
    args = parser.parse_args()
    try:
        {"throughput": throughput, "sql": sql, "profile": profile, "alloc": alloc}[args.mode](args)
    except common.BenchError as exc:
        log(f"error: {exc}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
