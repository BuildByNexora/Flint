from __future__ import annotations

from email.utils import parsedate_to_datetime
from typing import Callable, Iterable, Optional, Set, Union

from . import Limiter

try:
    import anyio
    from starlette.middleware.base import BaseHTTPMiddleware
    from starlette.requests import Request
    from starlette.responses import JSONResponse, Response
except ImportError as exc:  # pragma: no cover - exercised only without extra deps
    raise ImportError(
        "Flint FastAPI middleware requires the optional dependency: "
        'pip install "flint-limiter[fastapi]"'
    ) from exc


KeyFunc = Callable[[Request], str]
CostValue = Union[int, Callable[[Request], int]]


class FlintRateLimitMiddleware(BaseHTTPMiddleware):
    def __init__(
        self,
        app,
        *,
        limiter: Limiter,
        key: Optional[str] = None,
        key_func: Optional[KeyFunc] = None,
        rate: Optional[int] = None,
        per: Optional[str] = None,
        algorithm: str = "token_bucket",
        cost: CostValue = 1,
        exempt_paths: Optional[Iterable[str]] = None,
    ) -> None:
        super().__init__(app)
        if (key is None) == (key_func is None):
            raise ValueError("exactly one of key or key_func is required")
        if (rate is None) != (per is None):
            raise ValueError("rate and per must be provided together")
        self.limiter = limiter
        self.key = key
        self.key_func = key_func
        self.rate = rate
        self.per = per
        self.algorithm = algorithm
        self.cost = cost
        self.exempt_paths = set(exempt_paths or ())
        self._configured: Set[str] = set()

    async def dispatch(self, request: Request, call_next) -> Response:
        if request.url.path in self.exempt_paths:
            return await call_next(request)

        key = self._request_key(request)
        cost = self._request_cost(request)

        result, status = await anyio.to_thread.run_sync(self._check_request, key, cost)
        headers = _rate_limit_headers(result, status.get("rate"))
        if not result.allowed:
            return JSONResponse(
                {"detail": "rate limit exceeded"},
                status_code=429,
                headers=headers,
            )

        response = await call_next(request)
        response.headers.update(headers)
        return response

    def _ensure_limit(self, key: str) -> None:
        if key in self._configured:
            return
        if self.rate is None or self.per is None:
            if self.limiter.status(key) is None:
                raise RuntimeError(
                    f"limit {key!r} is not configured and lazy rate/per were not provided"
                )
            self._configured.add(key)
            return
        if self.limiter.status(key) is None:
            self.limiter.limit(key, rate=self.rate, per=self.per, algorithm=self.algorithm)
        self._configured.add(key)

    def _check_request(self, key: str, cost: int):
        self._ensure_limit(key)
        result = self.limiter.check(key, cost=cost)
        status = self.limiter.status(key) or {}
        return result, status

    def _request_key(self, request: Request) -> str:
        key = self.key if self.key is not None else self.key_func(request)  # type: ignore[misc]
        if not isinstance(key, str) or not key.strip():
            raise ValueError("rate limit key must be a non-empty string")
        return key

    def _request_cost(self, request: Request) -> int:
        cost = self.cost(request) if callable(self.cost) else self.cost
        if not isinstance(cost, int):
            raise ValueError("cost must be an integer")
        if cost <= 0:
            raise ValueError("cost must be greater than zero")
        return cost


def _rate_limit_headers(result, limit) -> dict:
    reset_epoch = _reset_epoch(result.reset_at)
    retry_after = max(0, reset_epoch - _now_epoch())
    limit = limit if limit is not None else result.cost + result.remaining
    return {
        "X-RateLimit-Limit": str(limit),
        "X-RateLimit-Remaining": str(result.remaining),
        "X-RateLimit-Reset": str(reset_epoch),
        "Retry-After": str(retry_after),
    }


def _reset_epoch(value: str) -> int:
    normalized = value.replace("Z", "+00:00")
    try:
        from datetime import datetime

        return int(datetime.fromisoformat(normalized).timestamp())
    except ValueError:
        return int(parsedate_to_datetime(value).timestamp())


def _now_epoch() -> int:
    from time import time

    return int(time())


__all__ = ["FlintRateLimitMiddleware"]
