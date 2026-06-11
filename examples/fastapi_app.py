from fastapi import FastAPI

import flint
from flint.fastapi import FlintRateLimitMiddleware

limiter = flint.Limiter(data_dir=".flint")

app = FastAPI()
app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key_func=lambda request: f"ip:{request.client.host}",
    rate=100,
    per="1m",
    exempt_paths={"/health"},
)
flint.add_prometheus_route(app, limiter)


@app.get("/health")
def health():
    return {"ok": True}


@app.get("/api")
def api():
    return {"ok": True}
