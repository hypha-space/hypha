import argparse
import json
import time
import httpx


def main(socket_path: str, work_dir: str, job_json: str) -> None:
    # Parse job spec passed via --job
    job_spec = json.loads(job_json)

    # ensure that type is `parameter-server`
    assert job_spec["executor"]["type"] == "parameter-server"

    print(json.dumps(job_spec))
    
    subscribe_req = {"receive": job_spec["executor"]["updates"], "out_dir": work_dir}
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
                                    file_path = work_dir + "/" + rel_path
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


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--job", required=True)
    args = parser.parse_args()
    main(args.socket, args.work_dir, args.job)
