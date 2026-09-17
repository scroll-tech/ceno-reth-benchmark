"""Local recovery tests: no GPU outage or Docker daemon required."""

import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest


SERVER = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("check_gpu", SERVER / "check_gpu.py")
CHECK_GPU = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK_GPU)
ERROR = b"called Result::unwrap(): cudaErrorNoDevice: no CUDA-capable device is detected"


class LogWatcherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.jobs = Path(self.temp.name)
        self.log = self.jobs / "job" / "stderr.log"
        self.log.parent.mkdir()

    def watcher(self):
        return CHECK_GPU.LogWatcher(str(self.jobs / "*" / "stderr.log"), CHECK_GPU.DEFAULT_PATTERNS)

    def test_new_log_error_before_discovery_without_newline(self):
        watcher = self.watcher()
        self.log.write_bytes(ERROR)
        self.assertEqual(watcher.scan(), str(self.log))

    def test_historical_error_ignored_but_new_append_detected(self):
        self.log.write_bytes(ERROR + b"\n")
        watcher = self.watcher()
        self.assertIsNone(watcher.scan())
        with self.log.open("ab") as log:
            log.write(ERROR)
        self.assertEqual(watcher.scan(), str(self.log))

    def test_pattern_split_across_scans(self):
        watcher = self.watcher()
        self.log.write_bytes(b"NO CUDA-CAP")
        self.assertIsNone(watcher.scan())
        with self.log.open("ab") as log:
            log.write(b"ABLE DEVICE")
        self.assertEqual(watcher.scan(), str(self.log))

    def test_replaced_log(self):
        self.log.write_bytes(b"old log")
        watcher = self.watcher()
        self.log.rename(self.log.with_suffix(".old"))
        self.log.write_bytes(ERROR)
        self.assertEqual(watcher.scan(), str(self.log))

    def test_truncated_log(self):
        self.log.write_bytes(b"old log\n" * 100)
        watcher = self.watcher()
        self.log.write_bytes(ERROR)
        self.assertEqual(watcher.scan(), str(self.log))

    def test_ordinary_prover_panic_does_not_request_gpu_recreation(self):
        watcher = self.watcher()
        self.log.write_bytes(b"thread panicked: index out of bounds: len 38 index 18446744073709551615\n")
        self.assertIsNone(watcher.scan())


class ProcessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, JOBS_DIR=str(self.root / "jobs"),
                        STARTUP_GPU_CHECK="0", GPU_WATCH_SCAN_INTERVAL_SEC="0.05",
                        STACK_STOP_TIMEOUT_SEC="1", GPU_UNAVAILABLE_RESTART_DELAY_SEC="0",
                        GPU_RECREATE_DELAY_SEC="0", HOST_GPU_POLL_INTERVAL_SEC="0.01")
        for name in ("GPU_ERROR_PATTERNS", "GPU_ERROR_PATTERN", "GPU_ERROR_LOG_GLOB", "GPU_WATCH_READY_FILE"):
            self.env.pop(name, None)

    def start(self, command):
        process = subprocess.Popen(command, env=self.env, start_new_session=True,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)

        def cleanup():
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)
        self.addCleanup(cleanup)
        return process

    def write_script(self, name, text):
        path = self.root / name
        path.write_text(text)
        path.chmod(0o755)
        return path

    def test_entrypoint_exits_75_even_when_server_ignores_term(self):
        server = self.write_script("stuck.py", '''import os, signal, time
from pathlib import Path
signal.signal(signal.SIGTERM, signal.SIG_IGN)
job = Path(os.environ["JOBS_DIR"]) / "new"
job.mkdir(parents=True)
(job / "pid").write_text(str(os.getpid()))
(job / "stderr.log").write_text("cudaErrorNoDevice: no CUDA-capable device is detected")
while True: time.sleep(1)
''')
        process = self.start([str(SERVER / "entrypoint.sh"), sys.executable, str(server)])
        try:
            output, _ = process.communicate(timeout=8)
            self.assertEqual(process.returncode, 75, output)
            self.assertIn("shutdown timed out", output)
        finally:
            # The test runner is not container PID 1; clean up an orphan on failure.
            pid_file = self.root / "jobs/new/pid"
            if pid_file.exists():
                try:
                    os.kill(int(pid_file.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_entrypoint_sigterm_stops_server(self):
        marker = self.root / "started"
        server = self.write_script("normal.py", f'''import signal, time
from pathlib import Path
signal.signal(signal.SIGTERM, lambda *_: exit(0))
Path({str(marker)!r}).touch()
while True: time.sleep(1)
''')
        process = self.start([str(SERVER / "entrypoint.sh"), sys.executable, str(server)])
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertTrue(marker.exists())
        process.send_signal(signal.SIGTERM)
        output, _ = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 0, output)

    def test_startup_gpu_failure_exits_75_without_starting_server(self):
        self.write_script("nvidia-smi", "#!/bin/sh\nexit 1\n")
        self.env.update(PATH=f"{self.root}:{os.environ['PATH']}", STARTUP_GPU_CHECK="1", GPU_READY_MAX_ATTEMPTS="1")
        process = self.start([str(SERVER / "entrypoint.sh"), "false"])
        output, _ = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 75, output)

    def fake_docker(self, first_status):
        self.write_script("nvidia-smi", "#!/bin/sh\necho GPU-test\n")
        self.write_script("docker", f'''#!{sys.executable}
import sys
from pathlib import Path
root = Path({str(self.root)!r})
with (root / "calls").open("a") as calls: calls.write(" ".join(sys.argv[1:]) + "\\n")
if sys.argv[1] == "run":
    count = root / "count"
    n = int(count.read_text()) + 1 if count.exists() else 1
    count.write_text(str(n))
    raise SystemExit({first_status} if n == 1 else 0)
''')
        self.env.update(PATH=f"{self.root}:{os.environ['PATH']}", CONTAINER_NAME="test-reth")

    def test_host_recreates_on_75_with_same_arguments(self):
        self.fake_docker(75)
        process = self.start([str(SERVER / "run_container.sh"), "--gpus", "all", "test-image"])
        output, _ = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 0, output)
        calls = (self.root / "calls").read_text().splitlines()
        self.assertEqual([c for c in calls if c.startswith("run ")],
                         ["run --name test-reth --gpus all test-image"] * 2)
        self.assertEqual(calls.count("rm -f test-reth"), 4)

    def test_host_preserves_other_failure_status(self):
        self.fake_docker(42)
        process = self.start([str(SERVER / "run_container.sh"), "test-image"])
        output, _ = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 42, output)
        self.assertEqual((self.root / "count").read_text(), "1")


if __name__ == "__main__":
    unittest.main()
