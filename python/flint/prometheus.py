from __future__ import annotations

from datetime import datetime
import re
from typing import Callable, Iterable, Mapping, Optional

from . import Limiter


CONTENT_TYPE = "text/plain; version=0.0.4; charset=utf-8"
_METRIC_PREFIX_RE = re.compile(r"^[a-zA-Z_][a-zA-Z0-9_]*$")


def prometheus_metrics(
    limiter: Limiter,
    *,
    prefix: str = "flint",
    include_key_label: bool = True,
    key_label_func: Optional[Callable[[str], str]] = None,
) -> str:
    """Render limiter state in Prometheus text exposition format."""
    _validate_metric_prefix(prefix)
    lines = []
    lines.extend(_metric_header(prefix, "limit_info", "Configured Flint limits.", "gauge"))
    lines.extend(_metric_header(prefix, "limit_rate", "Configured rate capacity.", "gauge"))
    lines.extend(_metric_header(prefix, "limit_per_millis", "Configured limit window in milliseconds.", "gauge"))
    lines.extend(_metric_header(prefix, "limit_remaining", "Remaining quota in the current window.", "gauge"))
    lines.extend(_metric_header(prefix, "limit_reset_at_seconds", "Unix timestamp when the current window resets.", "gauge"))
    lines.extend(_metric_header(prefix, "requests_allowed_total", "Total allowed checks.", "counter"))
    lines.extend(_metric_header(prefix, "requests_denied_total", "Total denied checks.", "counter"))
    lines.extend(_metric_header(prefix, "request_cost_allowed_total", "Total allowed request cost.", "counter"))
    lines.extend(_metric_header(prefix, "request_cost_denied_total", "Total denied request cost.", "counter"))

    for summary in sorted(limiter.list(), key=lambda item: item["key"]):
        labels = _summary_labels(
            summary,
            include_key_label=include_key_label,
            key_label_func=key_label_func,
        )
        lines.append(_sample(prefix, "limit_info", labels, 1))
        lines.append(_sample(prefix, "limit_rate", labels, summary["rate"]))
        lines.append(_sample(prefix, "limit_per_millis", labels, summary["per_millis"]))
        lines.append(_sample(prefix, "limit_remaining", labels, summary["remaining"]))
        lines.append(_sample(prefix, "limit_reset_at_seconds", labels, _to_epoch(summary["reset_at"])))
        lines.append(_sample(prefix, "requests_allowed_total", labels, summary["total_allowed"]))
        lines.append(_sample(prefix, "requests_denied_total", labels, summary["total_denied"]))
        lines.append(_sample(prefix, "request_cost_allowed_total", labels, summary["total_allowed_cost"]))
        lines.append(_sample(prefix, "request_cost_denied_total", labels, summary["total_denied_cost"]))

    lines.append("")
    return "\n".join(lines)


def add_prometheus_route(
    app,
    limiter: Limiter,
    *,
    path: str = "/metrics",
    prefix: str = "flint",
    include_key_label: bool = True,
    key_label_func: Optional[Callable[[str], str]] = None,
) -> None:
    """Register a FastAPI or Starlette route that exposes Flint metrics."""
    try:
        from starlette.responses import Response
    except ImportError as exc:  # pragma: no cover - exercised only without FastAPI extra
        raise ImportError(
            "Flint Prometheus route requires the optional dependency: "
            'pip install "flint-limiter[fastapi]"'
        ) from exc

    async def metrics(request=None):
        return Response(
            prometheus_metrics(
                limiter,
                prefix=prefix,
                include_key_label=include_key_label,
                key_label_func=key_label_func,
            ),
            media_type=CONTENT_TYPE,
        )

    if hasattr(app, "add_api_route"):
        app.add_api_route(path, metrics, methods=["GET"], include_in_schema=False)
    else:
        app.add_route(path, metrics, methods=["GET"])


def _summary_labels(
    summary: Mapping[str, object],
    *,
    include_key_label: bool,
    key_label_func: Optional[Callable[[str], str]],
) -> dict:
    labels = {}
    if include_key_label:
        raw_key = str(summary["key"])
        labels["key"] = key_label_func(raw_key) if key_label_func else raw_key
    labels["algorithm"] = str(summary["algorithm"])
    return labels


def _metric_header(prefix: str, name: str, help_text: str, metric_type: str) -> Iterable[str]:
    metric = f"{prefix}_{name}"
    return [
        f"# HELP {metric} {help_text}",
        f"# TYPE {metric} {metric_type}",
    ]


def _sample(prefix: str, name: str, labels: Mapping[str, str], value) -> str:
    metric = f"{prefix}_{name}"
    return f"{metric}{_labels(labels)} {_format_value(value)}"


def _labels(labels: Mapping[str, str]) -> str:
    encoded = ",".join(f'{key}="{_escape_label(str(value))}"' for key, value in labels.items())
    return f"{{{encoded}}}"


def _escape_label(value: str) -> str:
    return value.replace("\\", "\\\\").replace("\n", "\\n").replace('"', '\\"')


def _format_value(value) -> str:
    if isinstance(value, bool):
        return "1" if value else "0"
    if isinstance(value, float):
        return repr(value)
    return str(int(value))


def _to_epoch(value: str) -> int:
    return int(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp())


def _validate_metric_prefix(prefix: str) -> None:
    if not _METRIC_PREFIX_RE.fullmatch(prefix):
        raise ValueError("Prometheus metric prefix must be a valid metric-name prefix")


__all__ = ["CONTENT_TYPE", "add_prometheus_route", "prometheus_metrics"]
