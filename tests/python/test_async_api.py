import asyncio

import flint


def test_async_check_status_list_and_flush(tmp_path):
    asyncio.run(_async_check_status_list_and_flush(tmp_path))


async def _async_check_status_list_and_flush(tmp_path):
    limiter = flint.Limiter(
        data_dir=str(tmp_path),
        sync="batch",
        flush_every_ms=60_000,
        flush_every_events=10_000,
    )

    await asyncio.wait_for(
        limiter.alimit("async:user-42", rate=2, per="1m"),
        timeout=2,
    )
    first = await asyncio.wait_for(limiter.acheck("async:user-42"), timeout=2)
    second = await asyncio.wait_for(limiter.aallow("async:user-42"), timeout=2)
    status = await asyncio.wait_for(limiter.astatus("async:user-42"), timeout=2)
    limits = await asyncio.wait_for(limiter.alist(), timeout=2)
    history = await asyncio.wait_for(limiter.ahistory("async:user-42"), timeout=2)

    assert first.allowed is True
    assert second is True
    assert status["remaining"] == 0
    assert any(item["key"] == "async:user-42" for item in limits)
    assert len(history) >= 3

    await asyncio.wait_for(limiter.aflush(), timeout=2)


def test_async_multi_limit_compact_doctor_and_top(tmp_path):
    asyncio.run(_async_multi_limit_compact_doctor_and_top(tmp_path))


async def _async_multi_limit_compact_doctor_and_top(tmp_path):
    limiter = flint.Limiter(data_dir=str(tmp_path))
    await limiter.alimit("user:1", rate=1, per="1m")
    await limiter.alimit("org:1", rate=10, per="1m")

    result = await limiter.acheck_all([
        "user:1",
        {"key": "org:1", "cost": 3},
    ])
    assert result["allowed"] is True
    assert await limiter.aallow_all(["user:1", "org:1"]) is False

    await limiter.acompact()
    report = await limiter.adoctor()
    assert report["ok"] is True

    top = await limiter.atop(by="denied", limit=1)
    assert top[0]["key"] == "user:1"

    await limiter.areset("user:1")
    status = await limiter.astatus("user:1")
    assert status["remaining"] == 1
