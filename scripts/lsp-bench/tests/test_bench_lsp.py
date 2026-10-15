"""Small controller tests for transient pull-diagnostic retries."""

import os
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import bench_lsp  # noqa: E402
from bench_lsp import pull_diagnostics, request_pull_document  # noqa: E402
from lsp_client import CONTENT_MODIFIED, LspClient, RequestTimeout, Response  # noqa: E402

FAKE_SERVER = os.path.join(os.path.dirname(__file__), "fake_server.py")


class PullRetryCase(unittest.TestCase):
    def test_content_modified_is_retried_until_current_result(self):
        class Client:
            def __init__(self):
                self.calls = 0

            def request(self, method, params, timeout=None):
                self.calls += 1
                if self.calls == 1:
                    return Response(
                        result=None,
                        error={"code": CONTENT_MODIFIED, "message": "busy"},
                        elapsed_s=0.001,
                    )
                return Response(
                    result={"kind": "full", "items": []}, error=None, elapsed_s=0.001
                )

        client = Client()
        response, attempts = request_pull_document(client, "file:///caller.bsl", 1.0)
        self.assertTrue(response.ok)
        self.assertEqual(attempts, 2)
        self.assertEqual(client.calls, 2)

    def test_semantic_pull_retries_busy_and_wrong_state_without_fixed_sleep(self):
        class Client:
            def __init__(self):
                self.responses = [
                    Response(None, {"code": CONTENT_MODIFIED, "message": "busy"}, 0.001,
                             time.monotonic()),
                    Response({"items": [{"code": "Rule", "message": "Token"}]}, None,
                             0.001, time.monotonic()),
                    Response({"items": []}, None, 0.001, time.monotonic()),
                ]
                self.timeouts = []

            def request(self, method, params, timeout=None):
                self.timeouts.append(timeout)
                return self.responses.pop(0)

        client = Client()
        with mock.patch.object(bench_lsp.time, "sleep", side_effect=AssertionError("poll sleep")):
            result = pull_diagnostics(
                client, "file:///caller.bsl", "Rule", "Token", False, 1.0,
                include_received_at=True,
            )
        self.assertEqual(result["attempts"], 3)
        self.assertEqual(result["diagnostics"], 0)
        self.assertIsNotNone(result["received_at"])
        self.assertTrue(all(0 < timeout <= 5.0 for timeout in client.timeouts))


class ResponseReceiveTimingCase(unittest.TestCase):
    def setUp(self):
        self.client = object.__new__(LspClient)

    def test_response_elapsed_uses_receive_time_before_delayed_waiter_wakeup(self):
        class DelayedEvent:
            def wait(self, _timeout):
                time.sleep(0.05)
                return True

        t0 = time.monotonic()
        waiter = {
            "event": DelayedEvent(),
            "error": None,
            "message": {"result": {"items": []}},
            "received_at": t0 + 0.001,
            "t0": t0,
        }
        wall_started = time.monotonic()
        response = self.client.wait_response(1, waiter, timeout=1.0)
        wall_elapsed = time.monotonic() - wall_started

        self.assertAlmostEqual(response.elapsed_s, 0.001, delta=0.005)
        self.assertGreater(wall_elapsed - response.elapsed_s, 0.03)

    def test_happy_fake_server_transcript_has_receive_timestamps(self):
        with tempfile.TemporaryDirectory() as tmp:
            client = LspClient(
                [sys.executable, FAKE_SERVER, "happy", os.path.join(tmp, "received.jsonl")],
                default_timeout=5.0,
            )
            try:
                initialized = client.initialize("file:///ws")
                hover = client.request("textDocument/hover", {})
                self.assertTrue(initialized.ok)
                self.assertTrue(hover.ok)
                self.assertIsNotNone(initialized.received_at)
                self.assertIsNotNone(hover.received_at)
                self.assertGreater(hover.elapsed_s, 0)
                self.assertEqual(client.shutdown(), 0)
            finally:
                client.close()

    def test_response_received_after_absolute_deadline_is_rejected(self):
        class ReadyEvent:
            def wait(self, _timeout):
                return True

        waiter = {
            "event": ReadyEvent(),
            "error": None,
            "message": {"result": {"items": []}},
            "received_at": time.monotonic() + 1.0,
            "t0": time.monotonic(),
        }
        with self.assertRaises(RequestTimeout):
            self.client.wait_response(1, waiter, timeout=0.01)
