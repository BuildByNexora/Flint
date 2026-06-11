import socket
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

import pytest

import flint


def free_port():
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


def wait_for_server(url, token):
    deadline = time.time() + 10
    request = urllib.request.Request(
        f"{url}/v1/health",
        headers={"Authorization": f"Bearer {token}"},
    )
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(request, timeout=0.5) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.05)
    raise RuntimeError("flint server did not become ready")


def test_shared_limiter_server_roundtrip(tmp_path):
    port = free_port()
    token = "test-secret"
    server = f"http://127.0.0.1:{port}"
    root = Path(__file__).resolve().parents[2]
    binary = root / "target" / "debug" / "flint"
    subprocess.run(["cargo", "build", "-p", "flint-cli"], cwd=root, check=True)
    process = subprocess.Popen(
        [
            str(binary),
            "--data-dir",
            str(tmp_path),
            "server",
            "start",
            "--bind",
            f"127.0.0.1:{port}",
            "--token",
            token,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        wait_for_server(server, token)
        client = flint.SharedLimiter(server, token=token)
        client.limit("api:user-42", rate=2, per="1m")

        assert client.allow("api:user-42") is True
        result = client.check("api:user-42")
        assert result.allowed is True
        assert result.remaining == 0
        assert client.allow("api:user-42") is False

        status = client.status("api:user-42")
        assert status["total_allowed"] == 2
        assert status["total_denied"] == 1
        assert client.doctor()["ok"] is True

        bad_client = flint.SharedLimiter(server, token="wrong")
        with pytest.raises(flint.FlintServerError) as exc:
            bad_client.list()
        assert exc.value.status == 401

        unauthenticated_health = urllib.request.Request(f"{server}/v1/health")
        with pytest.raises(urllib.error.HTTPError) as health_exc:
            urllib.request.urlopen(unauthenticated_health, timeout=1)
        assert health_exc.value.code == 401

        assert client.timeout == 10.0
        fast_client = flint.SharedLimiter(server, token=token, timeout=1.5)
        assert fast_client.timeout == 1.5

        client.limit("route:/api", rate=1, per="1m")
        route_status = client.status("route:/api")
        assert route_status["key"] == "route:/api"
        assert client.allow("route:/api") is True
        assert client.allow("route:/api") is False
        client.flush()
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def test_shared_server_refuses_non_loopback_without_token(tmp_path):
    root = Path(__file__).resolve().parents[2]
    subprocess.run(["cargo", "build", "-p", "flint-cli"], cwd=root, check=True)
    binary = root / "target" / "debug" / "flint"
    result = subprocess.run(
        [
            str(binary),
            "--data-dir",
            str(tmp_path),
            "server",
            "start",
            "--bind",
            "0.0.0.0:0",
        ],
        capture_output=True,
        text=True,
        timeout=10,
    )

    assert result.returncode != 0
    assert "non-loopback address without --token" in result.stderr


def test_shared_limiter_connection_error_is_wrapped():
    port = free_port()
    client = flint.SharedLimiter(f"http://127.0.0.1:{port}", timeout=0.1)

    with pytest.raises(flint.FlintConnectionError):
        client.list()
