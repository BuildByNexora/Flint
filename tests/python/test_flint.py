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
