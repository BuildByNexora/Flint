import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

import flint
from flint.fastapi import FlintRateLimitMiddleware


def test_import_still_exports_limiter():
    assert flint.Limiter


def test_static_key_allows_until_limit_then_returns_429(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("route:/api", rate=2, per="1m")
    app = FastAPI()
    app.add_middleware(
        FlintRateLimitMiddleware,
        limiter=limiter,
        key="route:/api",
    )

    @app.get("/api")
    def api():
        return {"ok": True}

    client = TestClient(app)
    first = client.get("/api")
    second = client.get("/api")
    third = client.get("/api")

    assert first.status_code == 200
    assert second.status_code == 200
    assert third.status_code == 429
    assert third.json() == {"detail": "rate limit exceeded"}
    assert third.headers["X-RateLimit-Limit"] == "2"
    assert third.headers["X-RateLimit-Remaining"] == "0"
    assert "X-RateLimit-Reset" in third.headers
    assert "Retry-After" in third.headers


def test_key_func_separates_clients_and_lazy_configures(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    app = FastAPI()
    app.add_middleware(
        FlintRateLimitMiddleware,
        limiter=limiter,
        key_func=lambda request: f"client:{request.headers['x-client']}",
        rate=1,
        per="1m",
    )

    @app.get("/api")
    def api():
        return {"ok": True}

    client = TestClient(app)
    assert client.get("/api", headers={"x-client": "a"}).status_code == 200
    assert client.get("/api", headers={"x-client": "a"}).status_code == 429
    assert client.get("/api", headers={"x-client": "b"}).status_code == 200
    assert limiter.status("client:a")["total_denied"] == 1
    assert limiter.status("client:b")["total_allowed"] == 1


def test_cost_callable_consumes_multiple_units(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    app = FastAPI()
    app.add_middleware(
        FlintRateLimitMiddleware,
        limiter=limiter,
        key="weighted",
        rate=5,
        per="1m",
        cost=lambda request: int(request.headers.get("x-cost", "1")),
    )

    @app.get("/api")
    def api():
        return {"ok": True}

    client = TestClient(app)
    assert client.get("/api", headers={"x-cost": "3"}).status_code == 200
    assert client.get("/api", headers={"x-cost": "3"}).status_code == 429
    assert limiter.status("weighted")["remaining"] == 2
    assert limiter.status("weighted")["total_denied_cost"] == 3


def test_exempt_paths_do_not_consume_quota(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("route", rate=1, per="1m")
    app = FastAPI()
    app.add_middleware(
        FlintRateLimitMiddleware,
        limiter=limiter,
        key="route",
        exempt_paths={"/health"},
    )

    @app.get("/health")
    def health():
        return {"ok": True}

    @app.get("/api")
    def api():
        return {"ok": True}

    client = TestClient(app)
    assert client.get("/health").status_code == 200
    assert client.get("/health").status_code == 200
    assert limiter.status("route")["total_allowed"] == 0
    assert client.get("/api").status_code == 200
    assert client.get("/api").status_code == 429


def test_middleware_configuration_validation(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    app = FastAPI()
    app.get("/")(lambda: {"ok": True})
    with pytest.raises(ValueError, match="exactly one"):
        app.add_middleware(FlintRateLimitMiddleware, limiter=limiter)
        TestClient(app).get("/")

    app = FastAPI()
    app.get("/")(lambda: {"ok": True})
    with pytest.raises(ValueError, match="rate and per"):
        app.add_middleware(
            FlintRateLimitMiddleware,
            limiter=limiter,
            key="x",
            rate=1,
        )
        TestClient(app).get("/")


def test_unconfigured_static_limit_without_lazy_config_fails_clearly(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    app = FastAPI()
    app.add_middleware(
        FlintRateLimitMiddleware,
        limiter=limiter,
        key="missing",
    )
    app.get("/")(lambda: {"ok": True})

    with pytest.raises(RuntimeError, match="not configured"):
        TestClient(app).get("/")


def test_key_func_must_return_non_empty_string(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    app = FastAPI()
    app.add_middleware(
        FlintRateLimitMiddleware,
        limiter=limiter,
        key_func=lambda request: "",
        rate=1,
        per="1m",
    )
    app.get("/")(lambda: {"ok": True})

    with pytest.raises(ValueError, match="non-empty string"):
        TestClient(app).get("/")


def test_cost_callable_must_return_integer(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    app = FastAPI()
    app.add_middleware(
        FlintRateLimitMiddleware,
        limiter=limiter,
        key="x",
        rate=1,
        per="1m",
        cost=lambda request: "1",
    )
    app.get("/")(lambda: {"ok": True})

    with pytest.raises(ValueError, match="cost must be an integer"):
        TestClient(app).get("/")
