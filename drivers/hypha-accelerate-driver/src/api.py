from contextlib import AbstractContextManager
from types import TracebackType
from typing import Any

import httpx


class Session(AbstractContextManager["Session", None]):
    def __init__(self, socket_path: str) -> None:
        transport = httpx.HTTPTransport(uds=socket_path)
        self._client = httpx.Client(transport=transport)

    def __enter__(self) -> "Session":
        return self

    def __exit__(
        self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None
    ) -> None:
        self._client.close()

    def wait_for_task(self) -> Any:
        while True:
            try:
                resp = self._client.get("http://hypha/tasks", timeout=2)
                task_config = resp.json()
                return task_config
            except httpx.ReadTimeout:
                # Retry until we have a result.
                # We use a request timeout to be able to react
                # to SIGTERM signals.
                pass

    def set_task_status(self, task_id: str, status: str, result: Any | None = None) -> None:
        status_json = {
            "status": status,
        }

        if result is not None:
            status_json["result"] = result

        self._client.post(f"http://hypha/status/{task_id}", json=status_json)

    def get_parameters(self, task_id: str) -> tuple[int, str]:
        resp = self._client.get(f"http://hypha/inputs/{task_id}")

        model = resp.json()
        return model["parameters"]["version"], model["parameters"]["path"]
