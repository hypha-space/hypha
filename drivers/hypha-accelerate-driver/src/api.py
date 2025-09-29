import json
from collections.abc import Iterator
from contextlib import AbstractContextManager, contextmanager
from types import TracebackType
from typing import Any, override

import httpx


class Session(AbstractContextManager["Session", None]):
    def __init__(self, socket_path: str) -> None:
        transport = httpx.HTTPTransport(uds=socket_path)
        self._client: httpx.Client = httpx.Client(transport=transport)

    @override
    def __exit__(
        self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None
    ) -> None:
        self._client.close()

    def send(self, resource: Any, path: str) -> None:
        req = {"resource": resource, "path": path}
        _ = self._client.post("http://hypha/resources/send", json=req).raise_for_status()

    def fetch(self, resource: Any) -> Any:
        resp = self._client.post("http://hypha/resources/fetch", json=resource).raise_for_status()
        return resp.json()

    @contextmanager
    def receive(self, resource: Any, path: str) -> Iterator["EventSource"]:
        req = {"resource": resource, "path": path}
        with self._client.stream(
            "POST",
            "http://hypha/resources/receive",
            json=req,
            headers={"Accept": "text/event-stream"},
            timeout=None,  # block indefinitely for SSE updates
        ) as resp:
            yield EventSource(resp)


class EventSource:
    def __init__(self, response: httpx.Response) -> None:
        self._response: httpx.Response = response

    @property
    def response(self) -> httpx.Response:
        return self._response

    def __iter__(self) -> Iterator[Any]:
        for line in self._response.iter_lines():
            fieldname, _, value = line.rstrip("\n").partition(":")

            if fieldname == "data":
                result = json.loads(value)

                yield result
            # Ignore other SSE fields (e.g., event:, id:, retry:)
