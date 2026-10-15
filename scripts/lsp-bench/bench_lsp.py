#!/usr/bin/env python3
"""Tier-B end-to-end LSP benchmark driver (stdlib only).

Spawns a real `bsl-analyzer-app lsp` over stdio and measures what the
in-process harness cannot: spawn->initializeResponse, spawn->workspace-ready
(the workspace-load progress `end`; requests before it are declined with
ContentModified), per-request wall latency including the task pool and
serialization, and the server's RSS timeline via a 1 Hz `/proc` sampler.

Diagnostics profiles (separate runs, never mixed):
  push              production config as-is: didOpen -> publishDiagnostics.
  pull              a *shadow workspace* (tempdir with the config copied and
                    `[features] workspace_diagnostics` forced on, everything
                    else symlinked) so the real workspace's config is never
                    touched — verified by hash before/after:
                    didOpen -> textDocument/diagnostic.
  workspace-stream  pull shadow + one workspace/diagnostic request with
                    partial-result streaming, then a cancelled second request.

Outputs under --out: run.json (timings, per-file latencies, errors) and
rss.csv (synchronized RSS/VmHWM inputs), and input-manifest.json (relative
paths and content hashes). Optional --request-samples and --update-cycles
retain every warm request and BSL/XML/config update latency as individual samples.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import sys
import tempfile
import threading
import time
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lsp_client import CONTENT_MODIFIED, LspClient, LspError, RequestTimeout  # noqa: E402

CONFIG_NAME = "bsl-analyzer.toml"


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def workspace_manifest(workspace):
    entries = []
    for dirpath, dirnames, filenames in os.walk(workspace):
        dirnames[:] = sorted(
            name for name in dirnames
            if name not in (".git", ".bsl-analyzer", "target")
            and not os.path.islink(os.path.join(dirpath, name))
        )
        for name in sorted(filenames):
            if name == "input-manifest.json":
                continue
            path = os.path.join(dirpath, name)
            if os.path.islink(path):
                continue
            relative = os.path.relpath(path, workspace).replace(os.sep, "/")
            entries.append({"path": relative, "sha256": sha256_file(path), "bytes": os.path.getsize(path)})
    digest = hashlib.sha256()
    for entry in entries:
        digest.update(entry["path"].encode("utf-8"))
        digest.update(b"\0")
        digest.update(bytes.fromhex(entry["sha256"]))
    return {"sha256": digest.hexdigest(), "files": entries}


def generate_1000_workspace(workspace):
    """Create the fixed protocol corpus; refuse to overwrite any existing data."""
    if os.path.exists(workspace) and os.listdir(workspace):
        raise SystemExit(f"bench_lsp: refusing to generate into non-empty path: {workspace}")
    os.makedirs(workspace, exist_ok=True)
    with open(os.path.join(workspace, CONFIG_NAME), "w", encoding="utf-8") as fh:
        fh.write('[source]\nroot = "."\n\n[diagnostics.parameters]\nUnresolvedBareCall = true\n')
    with open(os.path.join(workspace, "Configuration.xml"), "w", encoding="utf-8") as fh:
        fh.write(
            '<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses" version="2.20">\n'
            '  <Configuration uuid="00000000-0000-0000-0000-000000000000">\n'
            "    <Properties><Name>Benchmark</Name></Properties>\n"
            "  </Configuration>\n</MetaDataObject>\n"
        )
    for index in range(1000):
        name = f"Module{index:04d}"
        directory = os.path.join(workspace, "CommonModules", name)
        source = os.path.join(directory, "Ext", "Module.bsl")
        metadata = os.path.join(workspace, "CommonModules", name + ".xml")
        os.makedirs(os.path.dirname(source), exist_ok=True)
        with open(metadata, "w", encoding="utf-8") as fh:
            fh.write(
                '<?xml version="1.0" encoding="UTF-8"?>\n'
                '<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses">\n'
                f'  <CommonModule uuid="00000000-0000-0000-0000-{index + 1:012d}">\n'
                f"    <Properties><Name>{name}</Name><Server>true</Server>"
                "<ServerCall>true</ServerCall></Properties>\n"
                "  </CommonModule>\n</MetaDataObject>\n"
            )
        # Keep the symbol graph broad but shallow: a deep call chain overflows
        # the server's analysis stack and measures a crash instead of scale.
        previous = "1"
        if index == 1:
            body = (
                "Функция Значение() Экспорт\n"
                "    Возврат 1;\n"
                "КонецФункции\n\n"
                "Процедура Benchmark() Экспорт\n"
                "    Module0999.Значение();\n"
                "    BenchmarkGlobalCall();\n"
                "    НетТакойПроцедуры();\n"
                "КонецПроцедуры\n"
            )
        else:
            body = (
                "Функция Значение() Экспорт\n"
                f"    Возврат {previous};\n"
                "КонецФункции\n"
                + (
                    "\nФункция BenchmarkGlobalCall() Экспорт\n"
                    "    Возврат 1;\n"
                    "КонецФункции\n"
                    if index == 0
                    else ""
                )
            )
        with open(source, "w", encoding="utf-8") as fh:
            fh.write(body)
        if index == 0:
            metadata_text = open(metadata, encoding="utf-8").read()
            metadata_text = metadata_text.replace(
                "<Server>true</Server>", "<Server>true</Server><Global>true</Global>", 1
            )
            with open(metadata, "w", encoding="utf-8") as fh:
                fh.write(metadata_text)
    manifest = workspace_manifest(workspace)
    if len(manifest["files"]) != 2002:
        raise AssertionError(f"expected 1000 BSL/XML pairs and two config files, got {len(manifest['files'])}")
    with open(os.path.join(workspace, "input-manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(
            {"module_count": 1000, "workspace_sha256": manifest["sha256"], "files": manifest["files"]},
            fh,
            ensure_ascii=False,
            indent=2,
        )
    return manifest["sha256"]


def source_root(workspace):
    config = os.path.join(workspace, CONFIG_NAME)
    if os.path.exists(config):
        with open(config, "rb") as fh:
            root = tomllib.load(fh).get("source", {}).get("root")
        if root:
            return os.path.join(workspace, root)
    return workspace


def pick_bsl_files(workspace, count):
    files = []
    for dirpath, dirnames, filenames in os.walk(source_root(workspace)):
        dirnames.sort()
        for name in sorted(filenames):
            if name.lower().endswith(".bsl"):
                files.append(os.path.join(dirpath, name))
                if len(files) >= count:
                    return files
    return files


def pull_diagnostics(client, uri, expected_code, expected_token, should_exist, timeout,
                    include_received_at=False):
    """Pull until a successful response contains the expected semantic signal."""
    deadline = time.monotonic() + timeout
    attempts = 0
    last = None
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        attempts += 1
        response = client.request(
            "textDocument/diagnostic", {"textDocument": {"uri": uri}},
            timeout=min(5.0, remaining),
        )
        if response.received_at is not None and response.received_at > deadline:
            break
        if not response.ok:
            if response.error_code == CONTENT_MODIFIED:
                continue
            raise LspError(f"textDocument/diagnostic failed: {response.error}")
        last = (response.result or {}).get("items", [])
        matched = [
            diagnostic for diagnostic in last
            if diagnostic.get("code") == expected_code
            and expected_token in str(diagnostic.get("message", ""))
        ]
        if bool(matched) == should_exist:
            proof = {"attempts": attempts, "diagnostics": len(last)}
            if include_received_at:
                if response.received_at is None:
                    raise LspError("textDocument/diagnostic response has no receive timestamp")
                proof["received_at"] = response.received_at
            return proof
    raise RequestTimeout(
        f"semantic result did not reach code={expected_code} token={expected_token!r} "
        f"present={should_exist}; attempts={attempts}; last_diagnostic_count="
        f"{None if last is None else len(last)}; last_diagnostics="
        f"{None if last is None else [(item.get('code'), item.get('message')) for item in last]}"
    )


def request_pull_document(client, uri, timeout):
    """Retry transient ContentModified responses until a current document result arrives."""
    deadline = time.monotonic() + timeout
    attempts = 0
    while time.monotonic() < deadline:
        attempts += 1
        response = client.request(
            "textDocument/diagnostic", {"textDocument": {"uri": uri}},
            timeout=min(5.0, max(0.1, deadline - time.monotonic())),
        )
        if response.error_code != CONTENT_MODIFIED:
            return response, attempts
        time.sleep(0.025)
    raise RequestTimeout(
        f"textDocument/diagnostic remained ContentModified for {attempts} attempts"
    )


def benchmark_controls(workspace):
    """Return the known caller and metadata signal for the corpus or perf-shadow."""
    source = source_root(workspace)
    corpus_caller = os.path.join(
        source, "CommonModules", "Module0001", "Ext", "Module.bsl"
    )
    if os.path.isfile(corpus_caller):
        return {
            "caller": corpus_caller,
            "metadata": os.path.join(source, "CommonModules", "Module0000.xml"),
            "xml_token": "BenchmarkGlobalCall",
            "dataset": "corpus-1000-v4",
        }
    return {
        "caller": os.path.join(
            source, "CommonModules", "Потребитель", "Ext", "Module.bsl"
        ),
        "metadata": os.path.join(source, "CommonModules", "ОбщийМодуль.xml"),
        "xml_token": "ЧисловоеЗначение",
        "dataset": "shared-workspace-perf-shadow",
    }


def make_shadow_workspace(workspace, scope):
    """Mirror physical directories and symlink leaf inputs, with pull diagnostics
    forced on in a copied config. Physical dirs let filesystem walkers discover
    module files; leaf links avoid copying or warming the corpus. The original
    config is never modified. The tempdir is removed on builder failure."""
    shadow = tempfile.mkdtemp(prefix="bsl-bench-shadow-")
    try:
        return _fill_shadow_workspace(shadow, workspace, scope)
    except BaseException:
        shutil.rmtree(shadow, ignore_errors=True)
        raise


def _fill_shadow_workspace(shadow, workspace, scope):
    for dirpath, dirnames, filenames in os.walk(workspace):
        relative_dir = os.path.relpath(dirpath, workspace)
        target_dir = shadow if relative_dir == "." else os.path.join(shadow, relative_dir)
        os.makedirs(target_dir, exist_ok=True)
        dirnames.sort()
        for name in sorted(filenames):
            if relative_dir == "." and name == CONFIG_NAME:
                continue
            source = os.path.join(dirpath, name)
            if os.path.islink(source):
                continue
            os.symlink(source, os.path.join(target_dir, name))

    original = os.path.join(workspace, CONFIG_NAME)
    text = ""
    if os.path.exists(original):
        with open(original, encoding="utf-8") as fh:
            text = fh.read()
    # Drop any existing key first: serde treats the camelCase and snake_case
    # spellings as one field, and a duplicate makes the whole config invalid —
    # which bsl-analyzer silently degrades to defaults (pull off). An inline
    # `features = { … }` table is dropped wholesale for the same reason (the
    # shadow only needs the pull scope; other feature toggles keep defaults).
    text = "\n".join(
        line
        for line in text.splitlines()
        if not line.strip().startswith(
            ("workspace_diagnostics", "workspaceDiagnostics", "features =", "features=")
        )
    )
    if "[features]" in text:
        text = text.replace(
            "[features]", f'[features]\nworkspace_diagnostics = "{scope}"', 1
        )
    else:
        text += f'\n[features]\nworkspace_diagnostics = "{scope}"\n'
    shadow_config = os.path.join(shadow, CONFIG_NAME)
    with open(shadow_config, "w", encoding="utf-8") as fh:
        fh.write(text)
    with open(shadow_config, "rb") as fh:
        parsed = tomllib.load(fh)  # the munged config must still parse…
    if parsed.get("features", {}).get("workspace_diagnostics") != scope:
        raise SystemExit("bench_lsp: shadow config did not end up with the pull scope set")
    return shadow


class RssSampler:
    """Synchronized process RSS timeline and Linux high-water mark."""

    def __init__(self, pid, out_path, peer_pid=None):
        self.pid = pid
        self.peer_pid = peer_pid
        self.out_path = out_path
        self.rows = []
        self.summary = None
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._t0 = time.monotonic()
        self._thread.start()

    @staticmethod
    def _read_status(pid):
        values = {}
        try:
            with open(f"/proc/{pid}/status", encoding="ascii") as fh:
                for line in fh:
                    if line.startswith(("VmRSS:", "VmHWM:")):
                        name, value, *_ = line.split()
                        values[name[:-1]] = int(value)
        except OSError:
            return values
        return values

    def _loop(self):
        while not self._stop.is_set():
            local = self._read_status(self.pid)
            peer = self._read_status(self.peer_pid) if self.peer_pid else {}
            if "VmRSS" in local:
                self.rows.append(
                    (
                        time.monotonic() - self._t0,
                        local["VmRSS"],
                        local.get("VmHWM"),
                        peer.get("VmRSS"),
                        peer.get("VmHWM"),
                    )
                )
            self._stop.wait(1.0)

    def stop(self):
        if self.summary is not None:
            return self.summary
        self._stop.set()
        self._thread.join(5)
        with open(self.out_path, "w", encoding="ascii") as fh:
            fh.write("t_s,rss_kb,vmhwm_kb,peer_rss_kb,peer_vmhwm_kb,pair_rss_kb\n")
            for t, rss, hwm, peer_rss, peer_hwm in self.rows:
                pair_rss = rss + peer_rss if peer_rss is not None else ""
                fh.write(
                    f"{t:.3f},{rss},{hwm or ''},{peer_rss or ''},{peer_hwm or ''},{pair_rss}\n"
                )
        local = self._read_status(self.pid)
        peer = self._read_status(self.peer_pid) if self.peer_pid else {}
        sampled_hwm = [hwm for _, _, hwm, _, _ in self.rows if hwm is not None]
        sampled_peer_hwm = [hwm for _, _, _, _, hwm in self.rows if hwm is not None]
        self.summary = {
            "vmhwm_kb": local.get("VmHWM") or (max(sampled_hwm) if sampled_hwm else None),
            "peer_vmhwm_kb": peer.get("VmHWM") or (max(sampled_peer_hwm) if sampled_peer_hwm else None),
            "pair_rss_samples": sum(peer_rss is not None for _, _, _, peer_rss, _ in self.rows),
        }
        return self.summary


def uri_of(path):
    return "file://" + os.path.abspath(path)


def client_capabilities(profile):
    capabilities = {
        "window": {"workDoneProgress": True},
        "textDocument": {"publishDiagnostics": {}},
        "workspace": {"configuration": True},
    }
    if profile in ("pull", "workspace-stream"):
        capabilities["textDocument"]["diagnostic"] = {"dynamicRegistration": False}
        capabilities["workspace"]["diagnostics"] = {"refreshSupport": True}
    return capabilities


def wait_ready_with_probe(client, probe_uri, boot_timeout, run):
    """Primary readiness = progress `end`; fallback = retry a cheap request
    until it stops being declined with ContentModified."""
    if client.wait_ready(boot_timeout):
        run["timings"]["ready_via"] = "progress_end"
        return True
    deadline = time.monotonic() + boot_timeout
    while time.monotonic() < deadline:
        try:
            response = client.request(
                "textDocument/documentSymbol",
                {"textDocument": {"uri": probe_uri}},
                timeout=10.0,
            )
        except RequestTimeout:
            continue
        # Only a successful answer proves readiness: a hard error (invalid
        # params, internal failure) is not "the workspace finished loading".
        if response.ok:
            run["timings"]["ready_via"] = "probe"
            return True
        if response.error_code != CONTENT_MODIFIED:
            raise SystemExit(f"bench_lsp: readiness probe failed hard: {response.error}")
        time.sleep(0.5)
    return False


def run_profile(args, workspace, run):
    files = pick_bsl_files(workspace, args.files)
    if not files:
        raise SystemExit(f"bench_lsp: no .bsl files under {workspace}")

    # The client spawns with cwd=workspace, so a relative binary path would
    # resolve against the workspace, not the invoker's cwd.
    binary = os.path.abspath(args.binary)
    stderr_path = os.path.join(args.out, "server-stderr.log")
    stderr_log = open(stderr_path, "wb")
    env = os.environ.copy()
    if args.owned_xdg_root:
        owned_xdg_root = os.path.abspath(args.owned_xdg_root)
        if args.run_label == "cold" and os.path.isdir(owned_xdg_root) and os.listdir(owned_xdg_root):
            raise SystemExit("bench_lsp: cold run requires an empty owned XDG root")
        env.update(
            {
                "XDG_CACHE_HOME": os.path.join(owned_xdg_root, "cache"),
                "XDG_STATE_HOME": os.path.join(owned_xdg_root, "state"),
                "XDG_DATA_HOME": os.path.join(owned_xdg_root, "data"),
            }
        )
        for name in ("XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME"):
            os.makedirs(env[name], exist_ok=True)
        run["owned_xdg_root"] = owned_xdg_root
        run["owned_xdg_policy"] = (
            "fresh-empty-root-per-run" if args.run_label == "cold"
            else "reused-group-root-no-lsp-semantic-database"
        )
    client = LspClient(
        [binary, "lsp"], cwd=workspace, env=env,
        default_timeout=args.timeout, stderr=stderr_log
    )
    sampler = RssSampler(client.proc.pid, os.path.join(args.out, "rss.csv"), args.peer_pid)
    try:
        response = client.initialize(uri_of(workspace), client_capabilities(args.profile))
        if not response.ok:
            raise SystemExit(f"bench_lsp: initialize failed: {response.error}")
        run["timings"]["spawn_to_initialize_s"] = time.monotonic() - client.spawn_t

        if args.profile in ("pull", "workspace-stream"):
            # End-to-end proof that the shadow config took effect: pull is
            # opt-in, and without it the server advertises no provider and the
            # run would silently measure nothing.
            provider = (response.result or {}).get("capabilities", {}).get("diagnosticProvider")
            if provider is None:
                raise SystemExit(
                    "bench_lsp: server did not advertise diagnosticProvider — "
                    "pull diagnostics are not enabled in the shadow workspace"
                )

        if not wait_ready_with_probe(client, uri_of(files[0]), args.boot_timeout, run):
            raise SystemExit("bench_lsp: workspace never became ready")
        run["timings"]["spawn_to_ready_s"] = time.monotonic() - client.spawn_t

        for path in files:
            uri = uri_of(path)
            with open(path, encoding="utf-8-sig", errors="replace") as fh:
                text = fh.read()
            entry = {"uri": uri}
            t_open = time.monotonic()
            client.notify(
                "textDocument/didOpen",
                {
                    "textDocument": {
                        "uri": uri,
                        "languageId": "bsl",
                        "version": 1,
                        "text": text,
                    }
                },
            )
            if args.profile == "push":
                message = client.wait_notification(
                    "textDocument/publishDiagnostics",
                    lambda p, uri=uri: p.get("uri") == uri,
                    timeout=args.timeout,
                )
                entry["didopen_to_publish_s"] = time.monotonic() - t_open
                entry["diagnostics"] = len(message["params"]["diagnostics"])
            else:
                pull_started = time.monotonic()
                pulled, attempts = request_pull_document(client, uri, args.timeout)
                entry["pull_s"] = time.monotonic() - pull_started
                entry["pull_attempts"] = attempts
                if pulled.ok:
                    entry["diagnostics"] = len(
                        (pulled.result or {}).get("items", [])
                    )
                else:
                    entry["error"] = pulled.error
                    run["errors"].append({"uri": uri, "error": pulled.error})

            for method, params in (
                ("textDocument/documentSymbol", {"textDocument": {"uri": uri}}),
                ("textDocument/semanticTokens/full", {"textDocument": {"uri": uri}}),
            ):
                feature = client.request(method, params)
                entry[method.rsplit("/", 1)[-1] + "_s"] = feature.elapsed_s
                if not feature.ok:
                    run["errors"].append({"uri": uri, "method": method, "error": feature.error})
            run["files"].append(entry)

        controls = benchmark_controls(workspace)
        caller_uri = uri_of(controls["caller"])
        if args.update_cycles:
            run["semantic_baseline_controls"] = {
                "known_diagnostic": pull_diagnostics(
                    client, caller_uri, "UnresolvedBareCall", "НетТакойПроцедуры",
                    True, args.timeout,
                ),
                "bsl_toggle_initially_absent": pull_diagnostics(
                    client, caller_uri, "UnresolvedBareCall", "BenchmarkUpdateMissing",
                    False, args.timeout,
                ),
                "xml_toggle_initially_absent": pull_diagnostics(
                    client, caller_uri, "UnresolvedBareCall", controls["xml_token"],
                    False, args.timeout,
                ),
            }

        if args.request_samples:
            controls = benchmark_controls(workspace)
            uri = uri_of(controls["caller"] if controls["caller"] in files else files[0])
            samples = []
            for sample in range(1, args.request_samples + 1):
                response = client.request(
                    "textDocument/documentSymbol", {"textDocument": {"uri": uri}}
                )
                item = {
                    "sample": sample,
                    "method": "textDocument/documentSymbol",
                    "elapsed_s": response.elapsed_s,
                    "ok": response.ok,
                }
                if not response.ok:
                    item["error"] = response.error
                    run["errors"].append(
                        {"sample": sample, "method": item["method"], "error": response.error}
                    )
                samples.append(item)
            run["request_samples"] = samples

        if args.update_cycles:
            controls = benchmark_controls(workspace)
            path = controls["caller"]
            if path not in files:
                raise SystemExit("bench_lsp: benchmark semantic caller was not opened")
            uri = uri_of(path)
            with open(path, encoding="utf-8-sig", errors="replace") as fh:
                original_text = fh.read()
            cycles = []
            for cycle in range(1, args.update_cycles + 1):
                enabled = True
                text = original_text.replace(
                    "КонецПроцедуры",
                    ("    BenchmarkUpdateMissing();\n" if enabled else "")
                    + "КонецПроцедуры",
                    1,
                )
                started = time.monotonic()
                client.notify(
                    "textDocument/didChange",
                    {
                        "textDocument": {"uri": uri, "version": cycle * 2},
                        "contentChanges": [{"text": text}],
                    },
                )
                try:
                    proof = pull_diagnostics(
                        client, uri, "UnresolvedBareCall", "BenchmarkUpdateMissing",
                        enabled, args.timeout, include_received_at=True,
                    )
                    changed_elapsed = proof.pop("received_at") - started
                    restore_started = time.monotonic()
                    client.notify(
                        "textDocument/didChange",
                        {
                            "textDocument": {"uri": uri, "version": cycle * 2 + 1},
                            "contentChanges": [{"text": original_text}],
                        },
                    )
                    restore = pull_diagnostics(
                        client, uri, "UnresolvedBareCall", "BenchmarkUpdateMissing",
                        False, args.timeout, include_received_at=True,
                    )
                    cycles.append(
                        {"cycle": cycle, "elapsed_s": changed_elapsed,
                         "ok": True, "expected_present": enabled, **proof,
                         "restore_elapsed_s": restore.pop("received_at") - restore_started,
                         "restored_expected_absent": True, "restore_attempts": restore["attempts"]}
                    )
                except LspError as exc:
                    cycles.append(
                        {
                            "cycle": cycle,
                            "elapsed_s": time.monotonic() - started,
                            "ok": False,
                            "expected_present": enabled,
                            "error": str(exc),
                        }
                    )
                    run["errors"].append(
                        {"cycle": cycle, "method": "textDocument/didChange", "error": str(exc)}
                    )
                    break
            run["bsl_update_cycles"] = cycles

            module_xml = controls["metadata"]
            caller_uri = uri_of(path)
            config_path = os.path.join(workspace, CONFIG_NAME)
            if not all(os.path.isfile(path) for path in (module_xml, config_path)):
                raise SystemExit("bench_lsp: XML/config cycles require the generated 1000-module workspace")

            xml_original = open(module_xml, encoding="utf-8").read()
            xml_enabled = "<Global>true</Global>" in xml_original
            if not xml_enabled:
                raise SystemExit("bench_lsp: benchmark target must begin with Global=true")
            xml_cycles = []
            try:
                for cycle in range(1, args.update_cycles + 1):
                    enabled = False
                    changed = xml_original.replace(
                        f"<Global>{str(not enabled).lower()}</Global>",
                        f"<Global>{str(enabled).lower()}</Global>",
                        1,
                    )
                    started = time.monotonic()
                    if os.path.islink(module_xml):
                        os.unlink(module_xml)
                    with open(module_xml, "w", encoding="utf-8") as fh:
                        fh.write(changed)
                    client.notify(
                        "workspace/didChangeWatchedFiles",
                        {"changes": [{"uri": uri_of(module_xml), "type": 2}]},
                    )
                    try:
                        proof = pull_diagnostics(
                            client, caller_uri, "UnresolvedBareCall", controls["xml_token"],
                            not enabled, args.timeout, include_received_at=True,
                        )
                        changed_elapsed = proof.pop("received_at") - started
                        restore_started = time.monotonic()
                        with open(module_xml, "w", encoding="utf-8") as fh:
                            fh.write(xml_original)
                        client.notify(
                            "workspace/didChangeWatchedFiles",
                            {"changes": [{"uri": uri_of(module_xml), "type": 2}]},
                        )
                        restore = pull_diagnostics(
                            client, caller_uri, "UnresolvedBareCall", controls["xml_token"],
                            False, args.timeout, include_received_at=True,
                        )
                        xml_cycles.append({"cycle": cycle, "elapsed_s": changed_elapsed,
                                           "ok": True, "global_enabled": enabled, **proof,
                                           "restore_elapsed_s": restore.pop("received_at") - restore_started,
                                           "restored_expected_absent": True,
                                           "restore_attempts": restore["attempts"]})
                    except LspError as exc:
                        xml_cycles.append({"cycle": cycle, "elapsed_s": time.monotonic() - started,
                                           "ok": False, "error": str(exc), "global_enabled": enabled})
                        run["errors"].append({"cycle": cycle, "kind": "xml", "error": str(exc)})
                        break
            finally:
                with open(module_xml, "w", encoding="utf-8") as fh:
                    fh.write(xml_original)
            run["xml_update_cycles"] = xml_cycles

            with open(config_path, encoding="utf-8") as fh:
                config_original = fh.read()
            if "UnresolvedBareCall = true" not in config_original:
                raise SystemExit("bench_lsp: generated config lacks the UnresolvedBareCall cycle setting")
            config_cycles = []
            try:
                for cycle in range(1, args.update_cycles + 1):
                    enabled = False
                    changed = config_original.replace(
                        f"UnresolvedBareCall = {str(not enabled).lower()}",
                        f"UnresolvedBareCall = {str(enabled).lower()}",
                        1,
                    )
                    started = time.monotonic()
                    with open(config_path, "w", encoding="utf-8") as fh:
                        fh.write(changed)
                    client.notify("textDocument/didSave", {"textDocument": {"uri": uri_of(config_path)}})
                    try:
                        proof = pull_diagnostics(
                            client, caller_uri, "UnresolvedBareCall", "НетТакойПроцедуры",
                            enabled, args.timeout, include_received_at=True,
                        )
                        changed_elapsed = proof.pop("received_at") - started
                        restore_started = time.monotonic()
                        with open(config_path, "w", encoding="utf-8") as fh:
                            fh.write(config_original)
                        client.notify("textDocument/didSave", {"textDocument": {"uri": uri_of(config_path)}})
                        restore = pull_diagnostics(
                            client, caller_uri, "UnresolvedBareCall", "НетТакойПроцедуры",
                            True, args.timeout, include_received_at=True,
                        )
                        config_cycles.append({"cycle": cycle, "elapsed_s": changed_elapsed,
                                              "ok": True, "rule_enabled": enabled, **proof,
                                              "restore_elapsed_s": restore.pop("received_at") - restore_started,
                                              "restored_expected_present": True,
                                              "restore_attempts": restore["attempts"]})
                    except LspError as exc:
                        config_cycles.append({"cycle": cycle, "elapsed_s": time.monotonic() - started,
                                              "ok": False, "error": str(exc), "rule_enabled": enabled})
                        run["errors"].append({"cycle": cycle, "kind": "config", "error": str(exc)})
                        break
            finally:
                with open(config_path, "w", encoding="utf-8") as fh:
                    fh.write(config_original)
            run["config_update_cycles"] = config_cycles

        if args.profile == "workspace-stream":
            run["stream"] = run_stream(client, args)

        run["memory"] = sampler.stop()
        code = client.shutdown()
        run["timings"]["server_exit_code"] = code
    finally:
        run.setdefault("memory", sampler.stop())
        run["server_exit_code_before_cleanup"] = client.proc.poll()
        client.close()
        stderr_log.close()


def run_stream(client, args):
    """workspace/diagnostic acceptance: partial chunks arrive and accumulate,
    the final report is well-formed, and cancellation actually cancels."""
    outcome = {}
    token = "bench-stream"
    request_id, waiter = client.request_async(
        "workspace/diagnostic", {"previousResultIds": [], "partialResultToken": token}
    )
    response = client.wait_response(request_id, waiter, timeout=args.timeout)
    chunks = [
        n
        for n in client.notifications_snapshot()
        if n.get("method") == "$/progress"
        and (n.get("params") or {}).get("token") == token
    ]
    outcome["chunks"] = len(chunks)
    outcome["final_ok"] = response.ok
    outcome["final_items"] = len((response.result or {}).get("items", [])) if response.ok else None

    cancel_id, cancel_waiter = client.request_async(
        "workspace/diagnostic",
        {"previousResultIds": [], "partialResultToken": token + "-cancel"},
    )
    client.cancel(cancel_id)
    try:
        cancelled = client.wait_response(cancel_id, cancel_waiter, timeout=args.timeout)
        outcome["cancel_error_code"] = cancelled.error_code
    except RequestTimeout:
        outcome["cancel_error_code"] = "timeout"
    return outcome


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", help="path to bsl-analyzer-app")
    parser.add_argument("--workspace")
    parser.add_argument("--out")
    parser.add_argument("--run-label", choices=("cold", "warm"))
    parser.add_argument("--owned-xdg-root", help="dedicated XDG cache/state/data tree for this run or warm group")
    parser.add_argument("--generate-1000-workspace", help="generate the deterministic 1000-module corpus and exit")
    parser.add_argument(
        "--profile", choices=("push", "pull", "workspace-stream"), default="push"
    )
    parser.add_argument("--pull-scope", choices=("all", "extensions"), default="all")
    parser.add_argument("--files", type=int, default=5)
    parser.add_argument(
        "--request-samples", type=int, default=0,
        help="record this many extra documentSymbol request latencies",
    )
    parser.add_argument(
        "--update-cycles", type=int, default=0,
        help="record this many BSL, metadata XML, and config update-to-diagnostics cycles",
    )
    parser.add_argument(
        "--peer-pid", type=int,
        help="sample a peer process on the same 1 Hz RSS clock",
    )
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--boot-timeout", type=float, default=600.0)
    parser.add_argument(
        "--keep-shadow", action="store_true", help="keep the shadow workspace for debugging"
    )
    args = parser.parse_args()

    if args.generate_1000_workspace:
        path = os.path.abspath(args.generate_1000_workspace)
        digest = generate_1000_workspace(path)
        print(json.dumps({"module_count": 1000, "workspace_sha256": digest}, indent=2))
        return
    if not args.binary or not args.workspace or not args.out:
        parser.error("--binary, --workspace and --out are required unless generating the corpus")
    if args.request_samples < 0 or args.update_cycles < 0:
        parser.error("sample and update cycle counts must be non-negative")
    if args.update_cycles and args.files < 1:
        parser.error("--update-cycles requires at least one opened BSL document")

    os.makedirs(args.out, exist_ok=True)
    workspace = os.path.abspath(args.workspace)
    original_config = os.path.join(workspace, CONFIG_NAME)
    config_hash_before = (
        sha256_file(original_config) if os.path.exists(original_config) else None
    )

    run = {
        "profile": args.profile,
        "run_label": args.run_label,
        "workspace": workspace,
        "binary": args.binary,
        "binary_sha256": sha256_file(os.path.abspath(args.binary)),
        "controller_sha256": sha256_file(__file__),
        "client_sha256": sha256_file(os.path.join(os.path.dirname(__file__), "lsp_client.py")),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "timings": {},
        "files": [],
        "errors": [],
        "measurement": {
            "dataset": benchmark_controls(workspace)["dataset"],
            "run_label": args.run_label,
            "pull_scope": args.pull_scope,
            "update_latency_source": (
                "successful textDocument/diagnostic response receive timestamp; "
                "includes XML/config file write; no fixed polling sleep"
            ),
            "home_override": False,
            "persistent_lsp_semantic_storage": False,
            "request_samples_requested": args.request_samples,
            "bsl_xml_config_update_cycles_requested_each": args.update_cycles,
            "peer_pid": args.peer_pid,
        },
    }
    manifest = workspace_manifest(workspace)
    run["input_manifest_sha256"] = manifest["sha256"]
    run["input_file_count"] = len(manifest["files"])
    with open(os.path.join(args.out, "input-manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=2)

    shadow = None
    try:
        target = workspace
        if args.profile in ("pull", "workspace-stream"):
            shadow = make_shadow_workspace(workspace, args.pull_scope)
            run["shadow_workspace"] = shadow
            target = shadow
        run_profile(args, target, run)
    except LspError as exc:
        run["errors"].append({"fatal": str(exc)})
    finally:
        if shadow and not args.keep_shadow:
            shutil.rmtree(shadow, ignore_errors=True)
        if config_hash_before is not None:
            config_hash_after = sha256_file(original_config)
            run["config_untouched"] = config_hash_before == config_hash_after
            if not run["config_untouched"]:
                run["errors"].append({"fatal": "workspace config hash changed during the run"})
        manifest_after = workspace_manifest(workspace)
        run["workspace_unchanged"] = manifest_after["sha256"] == run["input_manifest_sha256"]
        if not run["workspace_unchanged"]:
            run["errors"].append({"fatal": "workspace input hash changed during the run"})
        run["request_samples_recorded"] = len(run.get("request_samples", []))
        for kind in ("bsl", "xml", "config"):
            run[f"{kind}_update_cycles_recorded"] = len(run.get(f"{kind}_update_cycles", []))
        memory = run.get("memory", {})
        if memory.get("vmhwm_kb") is None:
            run["errors"].append({"fatal": "Linux VmHWM was unavailable for the LSP process"})
        if args.peer_pid and memory.get("pair_rss_samples", 0) == 0:
            run["errors"].append({"fatal": "peer RSS had no synchronized samples"})

    with open(os.path.join(args.out, "run.json"), "w", encoding="utf-8") as fh:
        json.dump(run, fh, ensure_ascii=False, indent=2)
    print(json.dumps(run["timings"], indent=2))
    sys.exit(1 if run["errors"] else 0)


if __name__ == "__main__":
    main()
