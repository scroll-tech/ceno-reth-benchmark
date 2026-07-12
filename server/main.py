import json
import os
import subprocess
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, Optional

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel

app = FastAPI()

JOBS_ROOT = Path(os.environ.get("JOBS_DIR", "/app/jobs"))
JOB_RETRY_DELAY_SEC = int(os.environ.get("JOB_RETRY_DELAY_SEC", "300"))
JOB_SUCCESS_DELAY_SEC = int(os.environ.get("JOB_SUCCESS_DELAY_SEC", "1"))
RECOVER_JOB_STATUSES = set(
    os.environ.get("RECOVER_JOB_STATUSES", "pending,running,error,waiting")
    .replace(" ", "")
    .split(",")
)


def _now_iso() -> str:
    return datetime.utcnow().replace(microsecond=0).isoformat() + "Z"


def _manifest_path(job_dir: Path) -> Path:
    return job_dir / "job.json"


def _read_manifest(path: Path) -> Dict[str, Any]:
    if not path.exists():
        return {}
    try:
        with path.open("r") as f:
            return json.load(f)
    except json.JSONDecodeError:
        return {}


def _write_manifest(path: Path, data: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        json.dump(data, f, indent=2, sort_keys=True)


def _run_s5cmd_copy(source_uri: str, destination_path: Path) -> None:
    destination_path.parent.mkdir(parents=True, exist_ok=True)
    args = ["s5cmd", "cp", source_uri, str(destination_path)]
    # Use a simple run that surfaces non-zero exit codes
    subprocess.run(args, check=True, text=True)


def download_proving_keys_on_startup() -> None:
    pass
#     app_pk_uri = os.environ.get("APP_PK_URI")
#     agg_pk_uri = os.environ.get("AGG_PK_URI")
#     if not app_pk_uri or not agg_pk_uri:
#         raise ValueError("APP_PK_URI and AGG_PK_URI must be set")
#
#     app_pk_path = Path(os.environ.get("APP_PK_PATH", "/app/app_pk"))
#     agg_pk_path = Path(os.environ.get("AGG_PK_PATH", "/app/agg_pk"))
#
#     # Download only if missing to keep startup idempotent
#     try:
#         if not app_pk_path.exists():
#             _run_s5cmd_copy(app_pk_uri, app_pk_path)
#         if not agg_pk_path.exists():
#             _run_s5cmd_copy(agg_pk_uri, agg_pk_path)
#     except Exception as e:  # Keep server up but surface the error in logs
#         # Printing rather than logging to avoid adding a logger dependency
#         print(f"[startup] failed to download proving keys: {e}", flush=True)


def recover_jobs_from_disk() -> None:
    jobs_root = JOBS_ROOT
    if not jobs_root.exists():
        return
    script_path = Path(__file__).parent / "prove_block.sh"
    if not script_path.exists():
        print("[startup] skipping job recovery: prove_block.sh missing", flush=True)
        return
    for manifest_path in jobs_root.glob("*/job.json"):
        manifest = _read_manifest(manifest_path)
        proof_uuid = manifest.get("proof_uuid")
        status = manifest.get("status")
        if not proof_uuid or status not in RECOVER_JOB_STATUSES:
            continue
        if proof_uuid in JOBS:
            continue
        job_dir = manifest_path.parent
        stdout_path = job_dir / "stdout.log"
        stderr_path = job_dir / "stderr.log"
        job = Job(
            proof_uuid=proof_uuid,
            script_path=script_path,
            job_dir=job_dir,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            mode=manifest.get("mode", "prove-stark"),
        )
        JOBS[proof_uuid] = job
        print(f"[startup] restarting job {proof_uuid}", flush=True)
        job.start()


@app.on_event("startup")
def run_startup_hooks() -> None:
    download_proving_keys_on_startup()
    recover_jobs_from_disk()


class Job:
    def __init__(
        self,
        proof_uuid: str,
        script_path: Path,
        job_dir: Path,
        stdout_path: Path,
        stderr_path: Path,
        mode: str,
    ):
        self.proof_uuid = proof_uuid
        self.script_path = script_path
        self.job_dir = job_dir
        self.stdout_path = stdout_path
        self.stderr_path = stderr_path
        self.mode = mode
        self.thread: Optional[threading.Thread] = None
        self.stop_event = threading.Event()
        self.current_proc: Optional[subprocess.Popen] = None
        self.pid: Optional[int] = None
        self.last_exit_code: Optional[int] = None
        self.iteration: int = 0
        self.last_error: Optional[str] = None
        self.manifest_path = _manifest_path(job_dir)
        manifest = _read_manifest(self.manifest_path)
        self.created_at = manifest.get("created_at", _now_iso())
        self.iteration = manifest.get("iterations", 0)
        self.last_exit_code = manifest.get("last_exit_code")
        self.last_error = manifest.get("last_error")
        self.status = manifest.get("status", "pending")

    def start(self) -> None:
        if self.thread and self.thread.is_alive():
            return
        self.stop_event.clear()
        self._persist_status("pending")
        self.thread = threading.Thread(target=self._run_loop, daemon=True)
        self.thread.start()
        # Wait briefly for the process to spawn so pid is populated
        deadline = time.time() + 1.0
        while self.pid is None and time.time() < deadline:
            time.sleep(0.05)

    def stop(self) -> None:
        self.stop_event.set()
        proc = self.current_proc
        if proc and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        self._persist_status("stopped")

    def _run_once(self) -> int:
        args = [str(self.script_path), self.proof_uuid]
        with self.stdout_path.open("a") as stdout_f, self.stderr_path.open("a") as stderr_f:
            proc = subprocess.Popen(args, stdout=stdout_f, stderr=stderr_f, text=True)
            self.current_proc = proc
            self.pid = proc.pid
            self._persist_status("running")
            proc.wait()
            return proc.returncode

    def _run_loop(self) -> None:
        while not self.stop_event.is_set():
            retry_delay = JOB_SUCCESS_DELAY_SEC
            try:
                exit_code = self._run_once()
                self.last_exit_code = exit_code
                self.iteration += 1
                status = "waiting" if exit_code == 0 else "error"
                self._persist_status(status)
                if exit_code != 0:
                    retry_delay = JOB_RETRY_DELAY_SEC
            except Exception as exc:  # noqa: BLE001
                self.last_error = str(exc)
                self._persist_status("error")
                retry_delay = JOB_RETRY_DELAY_SEC
            finally:
                self.current_proc = None
            if retry_delay > 0:
                print(
                    f"[job:{self.proof_uuid}] next attempt in {retry_delay}s "
                    f"(last_exit_code={self.last_exit_code})",
                    flush=True,
                )
            if self.stop_event.wait(timeout=retry_delay):
                break

    def is_active(self) -> bool:
        return self.thread is not None and self.thread.is_alive()

    def is_running(self) -> bool:
        return self.current_proc is not None and self.current_proc.poll() is None

    def _persist_status(self, status: str) -> None:
        self.status = status
        data = {
            "created_at": self.created_at,
            "updated_at": _now_iso(),
            "proof_uuid": self.proof_uuid,
            "mode": self.mode,
            "job_dir": str(self.job_dir),
            "stdout_path": str(self.stdout_path),
            "stderr_path": str(self.stderr_path),
            "status": status,
            "iterations": self.iteration,
            "last_exit_code": self.last_exit_code,
            "last_error": self.last_error,
        }
        _write_manifest(self.manifest_path, data)


JOBS: Dict[str, Job] = {}


class StartProofRequest(BaseModel):
    proof_uuid: str


@app.get("/healthz")
async def health():
    return JSONResponse(status_code=200, content={"status": "healthy"})


@app.post("/start_proof")
async def start_proof(req: StartProofRequest):
    proof_uuid = req.proof_uuid
    mode = "prove-stark"
    script_path = Path(__file__).parent / "prove_block.sh"
    if not script_path.exists():
        raise HTTPException(status_code=500, detail=f"Wrapper script not found at {script_path}")

    # If a job already exists and is active, return existing metadata
    job = JOBS.get(proof_uuid)
    if job and job.is_active():
        return JSONResponse(
            status_code=200,
            content={
                "message": "job already running",
                "proof_uuid": proof_uuid,
                "pid": job.pid,
                "stdout_path": str(job.stdout_path),
                "stderr_path": str(job.stderr_path),
            },
        )

    jobs_root = JOBS_ROOT
    jobs_root.mkdir(parents=True, exist_ok=True)
    job_dir = jobs_root / proof_uuid
    job_dir.mkdir(parents=True, exist_ok=True)

    stdout_path = job_dir / "stdout.log"
    stderr_path = job_dir / "stderr.log"

    if not job:
        job = Job(
            proof_uuid=proof_uuid,
            script_path=script_path,
            job_dir=job_dir,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            mode=mode,
        )
        JOBS[proof_uuid] = job
    job.start()

    return JSONResponse(
        status_code=202,
        content={
            "message": "job started",
            "proof_uuid": proof_uuid,
            "pid": job.pid,
            "job_dir": str(job_dir),
        },
    )


@app.get("/proof_state/{proof_uuid}")
async def get_proof_state(proof_uuid: str):
    j = JOBS.get(proof_uuid)
    if not j:
        return JSONResponse(status_code=404, content={"error": "job not found"})
    if j.is_running():
        status = "InProgress"
    elif j.is_active():
        status = "Waiting"
    elif j.stop_event.is_set():
        status = "Stopped"
    else:
        status = "Idle"
    e2e_latency_ms = None
    latency_ms_path = j.job_dir / "latency_ms.txt"
    if os.path.exists(latency_ms_path):
        with open(latency_ms_path, "r") as f:
            e2e_latency_ms = int(f.read())

    state_instret_path = j.job_dir / "num_instret"
    if os.path.exists(state_instret_path):
        with open(state_instret_path, "r") as f:
            num_instret = int(f.read())
    else:
        num_instret = 0

    return JSONResponse(
        status_code=200,
        content={
            "status": status,
            "job_status": j.status,
            "num_instructions": num_instret,
            "e2e_latency_ms": e2e_latency_ms,
            "iterations": j.iteration,
            "last_exit_code": j.last_exit_code,
            "last_error": j.last_error,
        },
    )


@app.get("/logs")
async def logs(proof_uuid: str, n: int = 200):
    j = JOBS.get(proof_uuid)
    if not j:
        return JSONResponse(status_code=404, content={"error": "job not found"})

    def tail(path: Path, lines: int) -> list[str]:
        if not path.exists():
            return []
        with path.open("r") as f:
            data = f.readlines()
        return data[-lines:]

    return JSONResponse(
        status_code=200,
        content={
            "stdout": tail(j.job_dir / "stdout.log", n),
            "stderr": tail(j.job_dir / "stderr.log", n),
        },
    )


@app.post("/stop_proof")
async def stop_proof(req: StartProofRequest):
    job = JOBS.get(req.proof_uuid)
    if not job:
        return JSONResponse(status_code=404, content={"error": "job not found"})
    job.stop()
    JOBS.pop(req.proof_uuid, None)
    return JSONResponse(
        status_code=200,
        content={"message": "job stopped", "proof_uuid": req.proof_uuid},
    )
