#![allow(clippy::useless_conversion)]

use std::sync::Arc;

use flint_core::{Algorithm, FlintError};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

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

    fn allow(&self, py: Python<'_>, key: String) -> PyResult<bool> {
        py.allow_threads(|| self.inner.allow(&key)).map_err(py_err)
    }

    fn check(&self, py: Python<'_>, key: String) -> PyResult<CheckResult> {
        let result = py
            .allow_threads(|| self.inner.check(&key))
            .map_err(py_err)?;
        Ok(CheckResult {
            key: result.key,
            allowed: result.allowed,
            remaining: result.remaining,
            reset_at: result.reset_at.to_rfc3339(),
            algorithm: algorithm_name(result.algorithm).to_string(),
        })
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
}

#[pymodule]
fn flint(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Limiter>()?;
    m.add_class::<CheckResult>()?;
    Ok(())
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
    dict.set_item("per_seconds", summary.per_seconds)?;
    dict.set_item("algorithm", algorithm_name(summary.algorithm))?;
    dict.set_item("remaining", summary.remaining)?;
    dict.set_item("reset_at", summary.reset_at.to_rfc3339())?;
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
