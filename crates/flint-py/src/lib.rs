#![allow(clippy::useless_conversion)]
#![allow(unexpected_cfgs)]

use std::sync::Arc;

use flint_core::{Algorithm, CheckResult as CoreCheckResult, FlintError, MultiCheckItem, TopBy};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

create_exception!(flint, RateLimitExceeded, PyException);

#[pyclass]
struct Limiter {
    inner: Arc<flint_core::Limiter>,
}

#[pyclass]
struct CheckResult {
    #[pyo3(get)]
    key: String,
    #[pyo3(get)]
    allowed: bool,
    #[pyo3(get)]
    cost: u64,
    #[pyo3(get)]
    remaining: u64,
    #[pyo3(get)]
    reset_at: String,
    #[pyo3(get)]
    algorithm: String,
}

#[pymethods]
#[allow(clippy::useless_conversion)]
impl Limiter {
    #[new]
    #[pyo3(signature = (data_dir=".flint"))]
    fn new(data_dir: &str) -> PyResult<Self> {
        Ok(Self {
            inner: Arc::new(flint_core::Limiter::open(data_dir).map_err(py_err)?),
        })
    }

    #[pyo3(signature = (key, *, rate, per, algorithm="token_bucket"))]
    fn limit(
        &self,
        py: Python<'_>,
        key: String,
        rate: u64,
        per: String,
        algorithm: &str,
    ) -> PyResult<()> {
        let algorithm = Algorithm::parse(algorithm).map_err(py_err)?;
        py.allow_threads(|| self.inner.limit(key, rate, per, algorithm))
            .map_err(py_err)
    }

    #[pyo3(signature = (key, *, cost=1))]
    fn allow(&self, py: Python<'_>, key: String, cost: u64) -> PyResult<bool> {
        py.allow_threads(|| self.inner.allow_cost(&key, cost))
            .map_err(py_err)
    }

    #[pyo3(signature = (key, *, cost=1))]
    fn check(&self, py: Python<'_>, key: String, cost: u64) -> PyResult<CheckResult> {
        let result = py
            .allow_threads(|| self.inner.check_cost(&key, cost))
            .map_err(py_err)?;
        Ok(CheckResult {
            key: result.key,
            allowed: result.allowed,
            cost: result.cost,
            remaining: result.remaining,
            reset_at: result.reset_at.to_rfc3339(),
            algorithm: algorithm_name(result.algorithm).to_string(),
        })
    }

    fn allow_all(&self, py: Python<'_>, items: PyObject) -> PyResult<bool> {
        let items = parse_multi_items(py, items)?;
        let result = py
            .allow_threads(|| self.inner.check_all(items))
            .map_err(py_err)?;
        Ok(result.allowed)
    }

    fn check_all(&self, py: Python<'_>, items: PyObject) -> PyResult<PyObject> {
        let items = parse_multi_items(py, items)?;
        let result = py
            .allow_threads(|| self.inner.check_all(items))
            .map_err(py_err)?;
        let value =
            serde_json::to_value(result).map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        json_to_py(py, value)
    }

    fn status(&self, py: Python<'_>, key: String) -> PyResult<Option<PyObject>> {
        let summary = py
            .allow_threads(|| self.inner.status(&key))
            .map_err(py_err)?;
        summary_to_py(py, summary)
    }

    fn list(&self, py: Python<'_>) -> PyResult<PyObject> {
        let summaries = py.allow_threads(|| self.inner.list()).map_err(py_err)?;
        let out = PyList::empty_bound(py);
        for summary in summaries {
            out.append(summary_to_dict(py, summary)?)?;
        }
        Ok(out.into_py(py))
    }

    fn reset(&self, py: Python<'_>, key: String) -> PyResult<()> {
        py.allow_threads(|| self.inner.reset(&key)).map_err(py_err)
    }

    fn history(&self, py: Python<'_>, key: String) -> PyResult<PyObject> {
        let events = py
            .allow_threads(|| self.inner.history(&key))
            .map_err(py_err)?;
        let value =
            serde_json::to_value(events).map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        json_to_py(py, value)
    }

    fn compact(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| self.inner.compact()).map_err(py_err)
    }

    fn doctor(&self, py: Python<'_>) -> PyResult<PyObject> {
        let report = py.allow_threads(|| self.inner.doctor()).map_err(py_err)?;
        json_to_py(
            py,
            serde_json::to_value(report).map_err(|err| PyRuntimeError::new_err(err.to_string()))?,
        )
    }

    #[pyo3(signature = (*, by="denied", limit=20))]
    fn top(&self, py: Python<'_>, by: &str, limit: usize) -> PyResult<PyObject> {
        let by = parse_top_by(by)?;
        let entries = py
            .allow_threads(|| self.inner.top(by, limit))
            .map_err(py_err)?;
        json_to_py(
            py,
            serde_json::to_value(entries)
                .map_err(|err| PyRuntimeError::new_err(err.to_string()))?,
        )
    }

    #[pyo3(signature = (key, *, rate, per, algorithm="token_bucket", cost=1))]
    fn rate_limit(
        &self,
        py: Python<'_>,
        key: String,
        rate: u64,
        per: String,
        algorithm: &str,
        cost: u64,
    ) -> PyResult<PyObject> {
        let decorator = PyRateLimitDecorator {
            limiter: Arc::clone(&self.inner),
            key,
            rate,
            per,
            algorithm: Algorithm::parse(algorithm).map_err(py_err)?,
            cost,
        };
        Py::new(py, decorator).map(|obj| obj.into_py(py))
    }
}

#[pyclass]
struct PyRateLimitDecorator {
    limiter: Arc<flint_core::Limiter>,
    key: String,
    rate: u64,
    per: String,
    algorithm: Algorithm,
    cost: u64,
}

#[pymethods]
impl PyRateLimitDecorator {
    fn __call__(&self, py: Python<'_>, callable: Py<PyAny>) -> PyResult<PyObject> {
        Py::new(
            py,
            PyRateLimitedFunction {
                limiter: Arc::clone(&self.limiter),
                key: self.key.clone(),
                rate: self.rate,
                per: self.per.clone(),
                algorithm: self.algorithm,
                cost: self.cost,
                callable,
            },
        )
        .map(|obj| obj.into_py(py))
    }
}

#[pyclass]
struct PyRateLimitedFunction {
    limiter: Arc<flint_core::Limiter>,
    key: String,
    rate: u64,
    per: String,
    algorithm: Algorithm,
    cost: u64,
    callable: Py<PyAny>,
}

#[pymethods]
impl PyRateLimitedFunction {
    #[pyo3(signature = (*args, **kwargs))]
    fn __call__(
        &self,
        py: Python<'_>,
        args: &Bound<'_, pyo3::types::PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<PyObject> {
        if self.limiter.status(&self.key).map_err(py_err)?.is_none() {
            self.limiter
                .limit(&self.key, self.rate, &self.per, self.algorithm)
                .map_err(py_err)?;
        }
        let result = self
            .limiter
            .check_cost(&self.key, self.cost)
            .map_err(py_err)?;
        if !result.allowed {
            return Err(rate_limit_error(result)?);
        }
        self.callable
            .bind(py)
            .call(args, kwargs)
            .map(|value| value.into_py(py))
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Limiter>()?;
    m.add_class::<CheckResult>()?;
    m.add(
        "RateLimitExceeded",
        m.py().get_type_bound::<RateLimitExceeded>(),
    )?;
    Ok(())
}

fn parse_top_by(value: &str) -> PyResult<TopBy> {
    match value {
        "allowed" => Ok(TopBy::Allowed),
        "denied" => Ok(TopBy::Denied),
        other => Err(PyValueError::new_err(format!(
            "unsupported top selector: {other}"
        ))),
    }
}

fn parse_multi_items(py: Python<'_>, items: PyObject) -> PyResult<Vec<MultiCheckItem>> {
    let json = py.import_bound("json")?;
    let encoded: String = json
        .call_method1("dumps", (items.bind(py),))?
        .extract()
        .map_err(|err| PyValueError::new_err(err.to_string()))?;
    let value: serde_json::Value =
        serde_json::from_str(&encoded).map_err(|err| PyValueError::new_err(err.to_string()))?;
    let serde_json::Value::Array(values) = value else {
        return Err(PyValueError::new_err(
            "allow_all/check_all expects a list of keys or {key, cost} items",
        ));
    };
    values.into_iter().map(parse_multi_item).collect()
}

fn parse_multi_item(value: serde_json::Value) -> PyResult<MultiCheckItem> {
    match value {
        serde_json::Value::String(key) => Ok(MultiCheckItem { key, cost: 1 }),
        serde_json::Value::Array(values) => {
            if values.len() != 2 {
                return Err(PyValueError::new_err(
                    "tuple/list multi-limit items must be [key, cost]",
                ));
            }
            let key = values[0]
                .as_str()
                .ok_or_else(|| PyValueError::new_err("multi-limit key must be a string"))?
                .to_string();
            let cost = values[1]
                .as_u64()
                .ok_or_else(|| PyValueError::new_err("multi-limit cost must be a positive int"))?;
            Ok(MultiCheckItem { key, cost })
        }
        serde_json::Value::Object(mut values) => {
            let key = values
                .remove("key")
                .and_then(|value| value.as_str().map(ToString::to_string))
                .ok_or_else(|| PyValueError::new_err("multi-limit object requires key"))?;
            let cost = values
                .remove("cost")
                .map(|value| {
                    value.as_u64().ok_or_else(|| {
                        PyValueError::new_err("multi-limit cost must be a positive int")
                    })
                })
                .transpose()?
                .unwrap_or(1);
            Ok(MultiCheckItem { key, cost })
        }
        _ => Err(PyValueError::new_err(
            "multi-limit items must be strings, [key, cost], or {key, cost}",
        )),
    }
}

fn rate_limit_error(result: CoreCheckResult) -> PyResult<PyErr> {
    Python::with_gil(|py| {
        let err = RateLimitExceeded::new_err(format!(
            "rate limit exceeded for {} until {}",
            result.key,
            result.reset_at.to_rfc3339()
        ));
        let obj = err.value_bound(py);
        obj.setattr("key", result.key)?;
        obj.setattr("cost", result.cost)?;
        obj.setattr("remaining", result.remaining)?;
        obj.setattr("reset_at", result.reset_at.to_rfc3339())?;
        obj.setattr("algorithm", algorithm_name(result.algorithm))?;
        Ok(err)
    })
}

fn py_err(err: FlintError) -> PyErr {
    match err {
        FlintError::InvalidDuration(_) | FlintError::UnsupportedAlgorithm(_) => {
            PyValueError::new_err(err.to_string())
        }
        _ => PyRuntimeError::new_err(err.to_string()),
    }
}

fn algorithm_name(algorithm: Algorithm) -> &'static str {
    match algorithm {
        Algorithm::TokenBucket => "token_bucket",
        Algorithm::SlidingWindowLog => "sliding_window_log",
        Algorithm::FixedWindowCounter => "fixed_window_counter",
    }
}

fn summary_to_py(
    py: Python<'_>,
    summary: Option<flint_core::LimitSummary>,
) -> PyResult<Option<PyObject>> {
    summary
        .map(|summary| summary_to_dict(py, summary))
        .transpose()
}

fn summary_to_dict(py: Python<'_>, summary: flint_core::LimitSummary) -> PyResult<PyObject> {
    let dict = PyDict::new_bound(py);
    dict.set_item("key", summary.key)?;
    dict.set_item("rate", summary.rate)?;
    dict.set_item("per_millis", summary.per_millis)?;
    dict.set_item("algorithm", algorithm_name(summary.algorithm))?;
    dict.set_item("remaining", summary.remaining)?;
    dict.set_item("reset_at", summary.reset_at.to_rfc3339())?;
    dict.set_item("total_allowed", summary.total_allowed)?;
    dict.set_item("total_denied", summary.total_denied)?;
    dict.set_item("total_allowed_cost", summary.total_allowed_cost)?;
    dict.set_item("total_denied_cost", summary.total_denied_cost)?;
    dict.set_item(
        "last_allowed_at",
        summary.last_allowed_at.map(|v| v.to_rfc3339()),
    )?;
    dict.set_item(
        "last_denied_at",
        summary.last_denied_at.map(|v| v.to_rfc3339()),
    )?;
    dict.set_item(
        "last_reset_at",
        summary.last_reset_at.map(|v| v.to_rfc3339()),
    )?;
    Ok(dict.into_py(py))
}

fn json_to_py(py: Python<'_>, value: serde_json::Value) -> PyResult<PyObject> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(value) => Ok(value.into_py(py)),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(value.into_py(py))
            } else if let Some(value) = value.as_u64() {
                Ok(value.into_py(py))
            } else if let Some(value) = value.as_f64() {
                Ok(value.into_py(py))
            } else {
                Ok(py.None())
            }
        }
        serde_json::Value::String(value) => Ok(value.into_py(py)),
        serde_json::Value::Array(values) => {
            let list = PyList::empty_bound(py);
            for value in values {
                list.append(json_to_py(py, value)?)?;
            }
            Ok(list.into_py(py))
        }
        serde_json::Value::Object(values) => {
            let dict = PyDict::new_bound(py);
            for (key, value) in values {
                dict.set_item(key, json_to_py(py, value)?)?;
            }
            Ok(dict.into_py(py))
        }
    }
}
