import pytest

import flint


def test_token_bucket_persists(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("api:user-42", rate=2, per="1m", algorithm="token_bucket")

    assert limiter.allow("api:user-42") is True
    assert limiter.allow("api:user-42") is True
    assert limiter.allow("api:user-42") is False

    del limiter

    limiter = flint.Limiter(data_dir=str(tmp_path))
    assert limiter.allow("api:user-42") is False
    status = limiter.status("api:user-42")
    assert status["remaining"] == 0
    assert status["total_allowed"] == 2
    assert status["total_denied"] == 2
    assert status["per_millis"] == 60000


def test_check_result_has_context(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("x", rate=1, per="1m", algorithm="token_bucket")
    result = limiter.check("x")
    assert result.allowed is True
    assert result.cost == 1
    assert result.remaining == 0
    assert result.reset_at


def test_unknown_algorithm_raises(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    with pytest.raises(ValueError):
        limiter.limit("x", rate=1, per="1m", algorithm="nope")


def test_millisecond_precision(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("x", rate=1, per="100ms", algorithm="fixed_window_counter")
    assert limiter.allow("x") is True
    assert limiter.allow("x") is False


def test_decorator_raises_rate_limit_exceeded(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    calls = []

    @limiter.rate_limit("decorated", rate=1, per="1m")
    def work(value):
        calls.append(value)
        return value * 2

    assert work(3) == 6
    with pytest.raises(flint.RateLimitExceeded) as exc:
        work(4)
    assert exc.value.key == "decorated"
    assert exc.value.remaining == 0
    assert calls == [3]


def test_cost_based_check_and_decorator(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("ai:user-42", rate=10, per="1m")

    result = limiter.check("ai:user-42", cost=7)
    assert result.allowed is True
    assert result.cost == 7
    assert result.remaining == 3
    assert limiter.allow("ai:user-42", cost=4) is False
    with pytest.raises(ValueError):
        limiter.check("ai:user-42", cost=11)
    status = limiter.status("ai:user-42")
    assert status["total_allowed_cost"] == 7
    assert status["total_denied_cost"] == 4

    calls = []

    @limiter.rate_limit("expensive", rate=5, per="1m", cost=3)
    def expensive():
        calls.append("ok")

    expensive()
    with pytest.raises(flint.RateLimitExceeded) as exc:
        expensive()
    assert exc.value.cost == 3
    assert calls == ["ok"]


def test_multi_limit_check_all_is_atomic(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("user:42", rate=1, per="1m")
    limiter.limit("org:acme", rate=10, per="1m")
    limiter.limit("endpoint:/v1/chat", rate=5, per="1m")

    result = limiter.check_all([
        "user:42",
        {"key": "org:acme", "cost": 4},
        ("endpoint:/v1/chat", 2),
    ])
    assert result["allowed"] is True
    assert result["denied_key"] is None
    assert limiter.status("org:acme")["remaining"] == 6
    assert limiter.status("endpoint:/v1/chat")["remaining"] == 3

    denied = limiter.check_all([
        "user:42",
        {"key": "org:acme", "cost": 4},
        ("endpoint:/v1/chat", 2),
    ])
    assert denied["allowed"] is False
    assert denied["denied_key"] == "user:42"
    assert len(denied["results"]) == 1
    assert limiter.status("org:acme")["remaining"] == 6
    assert limiter.status("endpoint:/v1/chat")["remaining"] == 3
    assert limiter.allow_all(["user:42", "org:acme"]) is False


def test_compact_doctor_top(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    limiter.limit("x", rate=1, per="1m")
    assert limiter.allow("x") is True
    assert limiter.allow("x") is False
    limiter.compact()
    report = limiter.doctor()
    assert report["ok"] is True
    assert report["snapshot_exists"] is True
    top = limiter.top(by="denied", limit=1)
    assert top[0]["key"] == "x"


def test_batch_sync_mode_can_flush_for_recovery(tmp_path):
    limiter = flint.Limiter(
        data_dir=str(tmp_path),
        sync="batch",
        flush_every_ms=60_000,
        flush_every_events=10_000,
    )
    limiter.limit("batched", rate=2, per="1m")
    assert limiter.allow("batched") is True
    limiter.flush()
    del limiter

    limiter = flint.Limiter(data_dir=str(tmp_path))
    status = limiter.status("batched")
    assert status["total_allowed"] == 1
    assert status["remaining"] == 1


def test_batch_sync_validates_thresholds(tmp_path):
    with pytest.raises(ValueError):
        flint.Limiter(data_dir=str(tmp_path), sync="nope")
    with pytest.raises(ValueError):
        flint.Limiter(
            data_dir=str(tmp_path),
            sync="batch",
            flush_every_ms=0,
            flush_every_events=100,
        )
