from fastapi import FastAPI
from fastapi.testclient import TestClient
from starlette.applications import Starlette

import flint


def test_prometheus_metrics_exports_limit_state(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit('api:"quoted"', rate=2, per="1m")
    assert limiter.allow('api:"quoted"') is True
    assert limiter.allow('api:"quoted"') is True
    assert limiter.allow('api:"quoted"') is False

    output = flint.prometheus_metrics(limiter)

    assert "# HELP flint_limit_info Configured Flint limits." in output
    assert "# TYPE flint_requests_allowed_total counter" in output
    assert 'flint_limit_info{key="api:\\"quoted\\"",algorithm="token_bucket"} 1' in output
    assert 'flint_limit_rate{key="api:\\"quoted\\"",algorithm="token_bucket"} 2' in output
    assert 'flint_limit_remaining{key="api:\\"quoted\\"",algorithm="token_bucket"} 0' in output
    assert 'flint_requests_allowed_total{key="api:\\"quoted\\"",algorithm="token_bucket"} 2' in output
    assert 'flint_requests_denied_total{key="api:\\"quoted\\"",algorithm="token_bucket"} 1' in output


def test_prometheus_metric_prefix_is_validated(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    for prefix in ["1bad", "bad:name", "ébad"]:
        try:
            flint.prometheus_metrics(limiter, prefix=prefix)
        except ValueError as exc:
            assert "prefix" in str(exc)
        else:
            raise AssertionError(f"invalid Prometheus prefix {prefix!r} was accepted")


def test_prometheus_can_avoid_high_cardinality_key_labels(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("ip:127.0.0.1", rate=1, per="1m")
    limiter.allow("ip:127.0.0.1")

    output = flint.prometheus_metrics(limiter, include_key_label=False)

    assert 'key="ip:127.0.0.1"' not in output
    assert 'flint_requests_allowed_total{algorithm="token_bucket"} 1' in output


def test_prometheus_can_redact_key_labels(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("user:42", rate=1, per="1m")
    limiter.allow("user:42")

    output = flint.prometheus_metrics(
        limiter,
        key_label_func=lambda key: key.split(":")[0],
    )

    assert 'key="user:42"' not in output
    assert 'key="user"' in output


def test_fastapi_prometheus_route(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("route:/api", rate=1, per="1m")
    limiter.allow("route:/api")

    app = FastAPI()
    flint.add_prometheus_route(app, limiter)

    response = TestClient(app).get("/metrics")

    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/plain")
    assert 'flint_requests_allowed_total{key="route:/api",algorithm="token_bucket"} 1' in response.text


def test_starlette_prometheus_route(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("route:/api", rate=1, per="1m")
    limiter.allow("route:/api")

    app = Starlette()
    flint.add_prometheus_route(app, limiter, include_key_label=False)

    response = TestClient(app).get("/metrics")

    assert response.status_code == 200
    assert 'flint_requests_allowed_total{algorithm="token_bucket"} 1' in response.text
