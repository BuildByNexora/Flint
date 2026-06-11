from __future__ import annotations

import asyncio
import functools
from typing import Any

from ._native import CheckResult, Limiter, RateLimitExceeded
from .prometheus import CONTENT_TYPE, add_prometheus_route, prometheus_metrics
from .shared import FlintConnectionError, FlintServerError, SharedCheckResult, SharedLimiter


async def _run_sync(func: Any, /, *args: Any, **kwargs: Any) -> Any:
    loop = asyncio.get_running_loop()
    call = functools.partial(func, *args, **kwargs)
    return await loop.run_in_executor(None, call)


async def _alimit(
    self: Limiter,
    key: str,
    *,
    rate: int,
    per: str,
    algorithm: str = "token_bucket",
) -> None:
    return await _run_sync(self.limit, key, rate=rate, per=per, algorithm=algorithm)


async def _aallow(self: Limiter, key: str, *, cost: int = 1) -> bool:
    return await _run_sync(self.allow, key, cost=cost)


async def _acheck(self: Limiter, key: str, *, cost: int = 1) -> CheckResult:
    return await _run_sync(self.check, key, cost=cost)


async def _aallow_all(self: Limiter, items: Any) -> bool:
    return await _run_sync(self.allow_all, items)


async def _acheck_all(self: Limiter, items: Any) -> dict[str, Any]:
    return await _run_sync(self.check_all, items)


async def _astatus(self: Limiter, key: str) -> dict[str, Any] | None:
    return await _run_sync(self.status, key)


async def _alist(self: Limiter) -> list[dict[str, Any]]:
    return await _run_sync(self.list)


async def _areset(self: Limiter, key: str) -> None:
    return await _run_sync(self.reset, key)


async def _ahistory(self: Limiter, key: str) -> list[dict[str, Any]]:
    return await _run_sync(self.history, key)


async def _acompact(self: Limiter) -> None:
    return await _run_sync(self.compact)


async def _aflush(self: Limiter) -> None:
    return await _run_sync(self.flush)


async def _adoctor(self: Limiter) -> dict[str, Any]:
    return await _run_sync(self.doctor)


async def _atop(
    self: Limiter,
    *,
    by: str = "denied",
    limit: int = 20,
) -> list[dict[str, Any]]:
    return await _run_sync(self.top, by=by, limit=limit)


Limiter.alimit = _alimit
Limiter.aallow = _aallow
Limiter.acheck = _acheck
Limiter.aallow_all = _aallow_all
Limiter.acheck_all = _acheck_all
Limiter.astatus = _astatus
Limiter.alist = _alist
Limiter.areset = _areset
Limiter.ahistory = _ahistory
Limiter.acompact = _acompact
Limiter.aflush = _aflush
Limiter.adoctor = _adoctor
Limiter.atop = _atop

__all__ = [
    "CONTENT_TYPE",
    "CheckResult",
    "FlintConnectionError",
    "FlintServerError",
    "Limiter",
    "RateLimitExceeded",
    "SharedCheckResult",
    "SharedLimiter",
    "add_prometheus_route",
    "prometheus_metrics",
]
