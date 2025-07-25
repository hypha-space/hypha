import argparse
import json
import os
import time

import httpx
import threading
from pathlib import Path


def sse_subscribe(socket_path: str, work_dir: Path, subscribe_req: dict) -> None:
    """Listen for update pointers over SSE and print them. Reconnects on errors."""
    timeout = httpx.Timeout(None, connect=10.0)
    while True:
        try:
            transport = httpx.HTTPTransport(uds=socket_path)
            with httpx.Client(transport=transport, timeout=10) as client:
                with client.stream(
                    "POST",
                    "http://hypha/resources/receive",
                    json=subscribe_req,
                    timeout=timeout,
                ) as r:
                    print("subscribe:", r.status_code)
                    r.raise_for_status()
                    for line in r.iter_lines():
                        if not line:
                            continue
                        if line.startswith(":"):
                            continue  # keepalive/comment
                        if line.startswith("data:"):
                            payload = line[len("data:") :].strip()
                            try:
                                pointer = json.loads(payload)
                                print("update pointer from", pointer.get("from_peer"), ":", pointer)
                                # Attempt to read and print the full file contents
                                rel_path = pointer.get("path")
                                if isinstance(rel_path, str):
                                    file_path = work_dir / rel_path
                                    try:
                                        with open(file_path, "rb") as f:
                                            data = f.read()
                                        try:
                                            content = data.decode("utf-8", errors="replace")
                                        except Exception:
                                            content = data.hex()
                                        print(f"update content ({file_path}): {content}")
                                    except Exception as e:
                                        print(f"failed to read update file {file_path}: {e}")
                            except Exception:
                                print("event:", line)
        except Exception as e:
            print(f"SSE error: {e}. Reconnecting in 1s...")
            time.sleep(1)


def send_updates_loop(
    client: httpx.Client,
    results_spec: dict,
    work_dir: Path,
    interval_sec: float = 2.0,
    count: int = 10,
    payload_size: int = 1024,
    payload_bytes: bytes | None = None,
) -> None:
    """Periodically write an update file into work_dir and ask worker to send it to peers."""
    out_dir = work_dir / "outgoing"
    out_dir.mkdir(parents=True, exist_ok=True)

    for i in range(count):
        path_rel = f"outgoing/update-{i}.bin"
        path_abs = work_dir / path_rel
        # Write payload to disk (job_spec if provided; otherwise synthetic data)
        if payload_bytes is not None:
            data = payload_bytes
        else:
            data = (f"update-{i}-".encode("utf-8") * ((payload_size // len(f"update-{i}-")) + 1))[:payload_size]
        with open(path_abs, "wb") as f:
            f.write(data)

        req = {"send": results_spec, "path": path_rel}
        try:
            resp = client.post("http://hypha/resources/send", json=req, timeout=30.0)
            print("send:", i, resp.status_code, resp.text)
        except Exception as e:
            print(f"send error on {path_rel}: {e}")

        time.sleep(interval_sec)


def main(socket_path: str, work_dir: str, job_json: str) -> None:
    transport = httpx.HTTPTransport(uds=socket_path)
    client = httpx.Client(transport=transport, timeout=10)
    # Parse job spec passed via --job
    job_spec = json.loads(job_json)

    # ensure that type is `diloco-transformer`
    assert job_spec["executor"]["type"] == "diloco-transformer"

    print(json.dumps(job_spec))

    # Request the worker to fetch the model with a plain Fetch payload
    # The bridge manages the output directory internally
    fetch_req = job_spec["executor"]["model"]
    try:
        # Allow more time for model downloads; the worker streams the content
        resp = client.post(
            "http://hypha/resources/fetch", json=fetch_req, timeout=httpx.Timeout(60.0)
        )
        print("fetch:", resp.status_code)
        try:
            files = resp.json()
            print("fetched files:", files)
        except Exception:
            print(resp.text)
    except Exception as e:
        print(f"fetch: error {e}")

    # Subscribe to receive updates in the background.
    subscribe_req = {"receive": job_spec["executor"]["updates"], "out_dir": "incoming"}
    recv_thread = threading.Thread(target=sse_subscribe, args=(socket_path, Path(work_dir), subscribe_req), daemon=True)
    recv_thread.start()

    # Periodically send updates to peers using the results target.
    # Use the job_spec JSON as the payload so each worker sends different content.
    job_payload = json.dumps(job_spec).encode("utf-8")
    send_updates_loop(
        client,
        job_spec["executor"]["results"],
        Path(work_dir),
        payload_bytes=job_payload,
    )

    # Give the worker a moment to forward status
    time.sleep(1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
