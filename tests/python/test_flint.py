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
