from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any


class FlintServerError(RuntimeError):
    def __init__(self, status: int, message: str):
        super().__init__(f"Flint server error {status}: {message}")
        self.status = status
        self.message = message


class FlintConnectionError(RuntimeError):
    pass


@dataclass(frozen=True)
class SharedCheckResult:
    key: str
    allowed: bool
    cost: int
    remaining: int
    reset_at: str
    algorithm: str


class SharedLimiter:
    """HTTP client for a Flint shared-mode server."""

    def __init__(
        self,
        server: str = "http://127.0.0.1:7878",
        token: str | None = None,
        timeout: float = 10.0,
    ):
        self.server = server.rstrip("/")
        self.token = token
        self.timeout = timeout

    def limit(
        self,
        key: str,
        *,
        rate: int,
        per: str,
        algorithm: str = "token_bucket",
    ) -> None:
        self._request(
            "POST",
            "/v1/limits",
            {
                "key": key,
                "rate": rate,
                "per": per,
                "algorithm": algorithm,
            },
        )

    def check(self, key: str, *, cost: int = 1) -> SharedCheckResult:
        data = self._request("POST", "/v1/check", {"key": key, "cost": cost})
        return SharedCheckResult(
            key=data["key"],
            allowed=data["allowed"],
            cost=data["cost"],
            remaining=data["remaining"],
            reset_at=data["reset_at"],
            algorithm=data["algorithm"],
        )

    def allow(self, key: str, *, cost: int = 1) -> bool:
        return self.check(key, cost=cost).allowed

    def check_all(self, items: list[Any]) -> dict[str, Any]:
        normalized = []
        for item in items:
            if isinstance(item, str):
                normalized.append({"key": item, "cost": 1})
            elif isinstance(item, dict):
                normalized.append({"key": item["key"], "cost": item.get("cost", 1)})
            elif isinstance(item, (list, tuple)) and len(item) == 2:
                normalized.append({"key": item[0], "cost": item[1]})
            else:
                raise ValueError("items must be keys, {key, cost}, or (key, cost)")
        return self._request("POST", "/v1/check-all", {"items": normalized})

    def allow_all(self, items: list[Any]) -> bool:
        return bool(self.check_all(items)["allowed"])

    def list(self) -> list[dict[str, Any]]:
        return self._request("GET", "/v1/limits")

    def status(self, key: str) -> dict[str, Any] | None:
        return self._request("GET", f"/v1/limits/{urllib.request.quote(key, safe='')}")

    def reset(self, key: str) -> None:
        self._request("POST", "/v1/reset", {"key": key})

    def compact(self) -> None:
        self._request("POST", "/v1/log/compact", {})

    def doctor(self) -> dict[str, Any]:
        return self._request("GET", "/v1/doctor")

    def _request(self, method: str, path: str, body: dict[str, Any] | None = None) -> Any:
        data = None if body is None else json.dumps(body).encode("utf-8")
        headers = {"Accept": "application/json"}
        if body is not None:
            headers["Content-Type"] = "application/json"
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        request = urllib.request.Request(
            f"{self.server}{path}",
            data=data,
            headers=headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                raw = response.read()
        except urllib.error.HTTPError as exc:
            raw = exc.read()
            try:
                payload = json.loads(raw.decode("utf-8"))
                message = payload.get("error", raw.decode("utf-8"))
            except Exception:
                message = raw.decode("utf-8", errors="replace")
            raise FlintServerError(exc.code, message) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise FlintConnectionError(f"could not reach Flint server: {exc}") from exc
        if not raw:
            return None
        return json.loads(raw.decode("utf-8"))
