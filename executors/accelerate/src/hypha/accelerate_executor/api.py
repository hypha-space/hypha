from contextlib import AbstractContextManager
from types import TracebackType
from typing import Any, override

import httpx


def fetch(socket_path: str, resource: Any) -> Any:
    transport = httpx.HTTPTransport(uds=socket_path)
    resp = (
        httpx.Client(transport=transport)
        .post("http://hypha/resources/fetch", json=resource, timeout=None)
        .raise_for_status()
    )
    return resp.json()


class Session(AbstractContextManager["Session", None]):
    def __init__(self, socket_path: str) -> None:
        transport = httpx.HTTPTransport(uds=socket_path)
        self._client: httpx.Client = httpx.Client(transport=transport)

    @override
    def __exit__(
        self, exc_type: type[BaseException] | None, exc_value: BaseException | None, traceback: TracebackType | None
    ) -> None:
        self._client.close()

    def send_resource(self, resource: Any, path: str, remove_file: bool = True, timeout: float | None = None) -> None:
        timeout_ms = int(timeout * 1000) if timeout is not None else None
        req = {"resource": resource, "path": path, "timeout_ms": timeout_ms, "remove_file": remove_file}
        # We must allow the client to wait at least as long as the requested timeout.
        # If timeout is None, wait forever.
        _ = self._client.post(
            "http://hypha/resources/send", json=req, timeout=httpx.Timeout(None, connect=0.1)
        ).raise_for_status()

    def send_action(self, payload: Any) -> Any:
        resp = self._client.post(
            "http://hypha/action/update", json=payload, timeout=httpx.Timeout(None, connect=0.1)
        ).raise_for_status()
        return resp.json()

    def fetch(self, resource: Any) -> Any:
        resp = self._client.post(
            "http://hypha/resources/fetch", json=resource, timeout=httpx.Timeout(None, connect=0.1)
        ).raise_for_status()
        return resp.json()

    def receive(self, resource: Any, path: str, timeout: Any | None = None) -> Any | None:
        req = {"resource": resource, "path": path, "timeout": timeout}
        resp = self._client.post(
            "http://hypha/resources/receive",
            json=req,
            timeout=httpx.Timeout(None, connect=0.1),
        )
        if resp.status_code == httpx.codes.NO_CONTENT:
            return None
        resp.raise_for_status()
        if resp.status_code == httpx.codes.NO_CONTENT:
            return None
        return resp.json()
