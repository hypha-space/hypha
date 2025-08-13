import json
from collections.abc import Iterator
from contextlib import AbstractContextManager, contextmanager
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

    @property
    def client(self) -> httpx.Client:
        return self._client

    @contextmanager
    def tasks(self) -> Iterator["EventSource"]:
        with self._client.stream(
            "GET", "http://hypha/tasks", headers={"Accept": "text/event-stream"}, timeout=None
        ) as resp:
            yield EventSource(resp)

    def set_task_status(self, task_id: str, status: str, result: Any | None = None) -> None:
        status_json = {
            "status": status,
        }

        if result is not None:
            status_json["result"] = result

        self._client.post(f"http://hypha/status/{task_id}", json=status_json).raise_for_status()

    def send(self, peer_id: str, path: str) -> None:
        self._client.post(
            f"http://hypha/data/{peer_id}", json={"parameters": {"version": 0, "path": path}}
        ).raise_for_status()

    @contextmanager
    def receive(self, peer_id: str) -> Iterator["EventSource"]:
        with self._client.stream(
            "GET", f"http://hypha/data/{peer_id}", headers={"Accept": "text/event-stream"}, timeout=None
        ) as resp:
            yield EventSource(resp)


class EventSource:
    def __init__(self, response: httpx.Response) -> None:
        self._response = response

    @property
    def response(self) -> httpx.Response:
        return self._response

    def __iter__(self) -> Iterator[Any]:
        for line in self._response.iter_lines():
            fieldname, _, value = line.rstrip("\n").partition(":")

            if fieldname == "data":
                result = json.loads(value)

                yield result
