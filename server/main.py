import os
import subprocess
import threading
import time
from pathlib import Path
from typing import Dict, Optional

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel

app = FastAPI()


def _run_s5cmd_copy(source_uri: str, destination_path: Path) -> None:
    destination_path.parent.mkdir(parents=True, exist_ok=True)
    args = ["s5cmd", "cp", source_uri, str(destination_path)]
    # Use a simple run that surfaces non-zero exit codes
    subprocess.run(args, check=True, text=True)


@app.on_event("startup")
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

    def start(self) -> None:
        if self.thread and self.thread.is_alive():
            return
        self.stop_event.clear()
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

    def _run_once(self) -> int:
        args = [str(self.script_path), self.proof_uuid]
        with self.stdout_path.open("a") as stdout_f, self.stderr_path.open("a") as stderr_f:
            proc = subprocess.Popen(args, stdout=stdout_f, stderr=stderr_f, text=True)
            self.current_proc = proc
            self.pid = proc.pid
            proc.wait()
            return proc.returncode

    def _run_loop(self) -> None:
        while not self.stop_event.is_set():
            try:
                exit_code = self._run_once()
                self.last_exit_code = exit_code
                self.iteration += 1
            except Exception as exc:  # noqa: BLE001
                self.last_error = str(exc)
                # Avoid tight restart loops if spawning fails
                time.sleep(5)
            finally:
                self.current_proc = None
            if self.stop_event.wait(timeout=1):
                break

    def is_active(self) -> bool:
        return self.thread is not None and self.thread.is_alive()

    def is_running(self) -> bool:
        return self.current_proc is not None and self.current_proc.poll() is None


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

    jobs_root = Path(os.environ.get("JOBS_DIR", "/app/jobs"))
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
