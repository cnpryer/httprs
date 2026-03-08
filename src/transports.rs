use crate::models::{PyRequest, PyResponse};
use pyo3::exceptions::{PyNotImplementedError, PyRuntimeError, PyStopAsyncIteration, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyTuple};
use std::time::Instant;

fn immediate_bytes_awaitable<'py>(
    py: Python<'py>,
    content: Vec<u8>,
) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        Python::attach(|py| Ok(PyBytes::new(py, &content).into_any().unbind()))
    })
}

fn immediate_awaitable<'py>(py: Python<'py>, value: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(value) })
}

#[pyclass(name = "AsyncBaseTransport", subclass)]
pub struct PyAsyncBaseTransport;

#[pymethods]
impl PyAsyncBaseTransport {
    #[new]
    #[pyo3(signature = (*_args, **_kwargs))]
    pub fn new(_args: &Bound<'_, PyTuple>, _kwargs: Option<&Bound<'_, PyDict>>) -> Self {
        Self
    }

    pub fn handle_async_request(&self, _request: Py<PyAny>) -> PyResult<Py<PyAny>> {
        Err(PyNotImplementedError::new_err(
            "AsyncBaseTransport.handle_async_request must be implemented",
        ))
    }

    pub fn aclose(&self) {}
}

#[pyclass(name = "BaseTransport", extends = PyAsyncBaseTransport, subclass)]
pub struct PyBaseTransport;

#[pymethods]
impl PyBaseTransport {
    #[new]
    #[pyo3(signature = (*_args, **_kwargs))]
    pub fn new(
        _args: &Bound<'_, PyTuple>,
        _kwargs: Option<&Bound<'_, PyDict>>,
    ) -> (Self, PyAsyncBaseTransport) {
        (Self, PyAsyncBaseTransport)
    }

    pub fn handle_request(&self, _request: Py<PyAny>) -> PyResult<Py<PyAny>> {
        Err(PyNotImplementedError::new_err(
            "BaseTransport.handle_request must be implemented",
        ))
    }

    pub fn close(&self) {}
}

#[pyclass(name = "MockTransport", subclass)]
pub struct PyMockTransport {
    handler: Py<PyAny>,
}

#[pymethods]
impl PyMockTransport {
    #[new]
    pub fn new(handler: Py<PyAny>) -> Self {
        Self { handler }
    }

    pub fn handle_request(&self, py: Python<'_>, request: Py<PyAny>) -> PyResult<Py<PyAny>> {
        self.handler.call1(py, (request,))
    }

    pub fn handle_async_request<'py>(
        &self,
        py: Python<'py>,
        request: Py<PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let result = self.handler.call1(py, (request,))?;
        if result.bind(py).hasattr("__await__")? {
            Ok(result.into_bound(py))
        } else {
            immediate_awaitable(py, result)
        }
    }

    pub fn close(&self) {}
}

#[pyclass(name = "HTTPTransport", subclass)]
pub struct PyHTTPTransport {
    inner: Option<reqwest::blocking::Client>,
}

#[pymethods]
impl PyHTTPTransport {
    #[new]
    #[pyo3(signature = (*_args, **_kwargs))]
    pub fn new(_args: &Bound<'_, PyTuple>, _kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let inner = reqwest::blocking::Client::builder()
            .cookie_store(true)
            .build()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self { inner: Some(inner) })
    }

    pub fn handle_request(&self, request: &PyRequest) -> PyResult<PyResponse> {
        let client = self
            .inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("HTTPTransport is closed"))?;

        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| PyValueError::new_err("Invalid method"))?;
        let mut builder = client.request(method, request.url.inner.as_str());
        for (k, v) in &request.headers.inner {
            builder = builder.header(k.as_str(), v.as_str());
        }
        if !request.content.is_empty() {
            builder = builder.body(request.content.clone());
        }

        let start = Instant::now();
        let response = crate::without_gil(|| builder.send()).map_err(crate::map_reqwest_error)?;
        let elapsed = start.elapsed().as_millis();
        PyResponse::from_blocking(response, elapsed, None)
    }

    pub fn close(&mut self) {
        self.inner = None;
    }

    #[getter]
    pub fn is_closed(&self) -> bool {
        self.inner.is_none()
    }
}

#[pyclass(name = "AsyncHTTPTransport", subclass)]
pub struct PyAsyncHTTPTransport {
    inner: Option<reqwest::Client>,
}

#[pymethods]
impl PyAsyncHTTPTransport {
    #[new]
    #[pyo3(signature = (*_args, **_kwargs))]
    pub fn new(_args: &Bound<'_, PyTuple>, _kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let inner = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self { inner: Some(inner) })
    }

    pub fn handle_async_request<'py>(
        &self,
        py: Python<'py>,
        request: &PyRequest,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self
            .inner
            .clone()
            .ok_or_else(|| PyRuntimeError::new_err("AsyncHTTPTransport is closed"))?;

        let req = request.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let method = reqwest::Method::from_bytes(req.method.as_bytes())
                .map_err(|_| PyValueError::new_err("Invalid method"))?;
            let mut builder = client.request(method, req.url.inner.as_str());
            for (k, v) in &req.headers.inner {
                builder = builder.header(k.as_str(), v.as_str());
            }
            if !req.content.is_empty() {
                builder = builder.body(req.content.clone());
            }

            let start = Instant::now();
            let response = builder.send().await.map_err(crate::map_reqwest_error)?;
            let elapsed = start.elapsed().as_millis();
            PyResponse::from_async(response, elapsed, None).await
        })
    }

    pub fn aclose(&mut self) {
        self.inner = None;
    }

    #[getter]
    pub fn is_closed(&self) -> bool {
        self.inner.is_none()
    }
}

#[pyclass(name = "ASGITransport", subclass)]
pub struct PyASGITransport;

#[pymethods]
impl PyASGITransport {
    #[new]
    #[pyo3(signature = (_app, **_kwargs))]
    pub fn new(_app: Py<PyAny>, _kwargs: Option<&Bound<'_, PyDict>>) -> Self {
        Self
    }

    pub fn handle_async_request<'py>(
        &self,
        _py: Python<'py>,
        _request: Py<PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        Err(PyNotImplementedError::new_err(
            "ASGITransport is not implemented in httprs",
        ))
    }

    pub fn aclose(&self) {}
}

#[pyclass(name = "WSGITransport", subclass)]
pub struct PyWSGITransport;

#[pymethods]
impl PyWSGITransport {
    #[new]
    #[pyo3(signature = (_app, **_kwargs))]
    pub fn new(_app: Py<PyAny>, _kwargs: Option<&Bound<'_, PyDict>>) -> Self {
        Self
    }

    pub fn handle_request(&self, _py: Python<'_>, _request: Py<PyAny>) -> PyResult<Py<PyAny>> {
        Err(PyNotImplementedError::new_err(
            "WSGITransport is not implemented in httprs",
        ))
    }

    pub fn close(&self) {}
}

#[pyclass(name = "SyncByteStream", subclass, from_py_object)]
#[derive(Clone)]
pub struct PySyncByteStream {
    pub content: Vec<u8>,
    pub consumed: bool,
}

#[pymethods]
impl PySyncByteStream {
    #[new]
    #[pyo3(signature = (content = None))]
    pub fn new(content: Option<Vec<u8>>) -> Self {
        Self {
            content: content.unwrap_or_default(),
            consumed: false,
        }
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(mut slf: PyRefMut<'_, Self>, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        if slf.consumed {
            None
        } else {
            slf.consumed = true;
            Some(PyBytes::new(py, &slf.content))
        }
    }

    pub fn close(&self) {}
}

#[pyclass(name = "AsyncByteStream", subclass, from_py_object)]
#[derive(Clone)]
pub struct PyAsyncByteStream {
    pub content: Vec<u8>,
    pub consumed: bool,
}

#[pymethods]
impl PyAsyncByteStream {
    #[new]
    #[pyo3(signature = (content = None))]
    pub fn new(content: Option<Vec<u8>>) -> Self {
        Self {
            content: content.unwrap_or_default(),
            consumed: false,
        }
    }

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(mut slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if slf.consumed {
            Err(PyStopAsyncIteration::new_err(()))
        } else {
            slf.consumed = true;
            let content = std::mem::take(&mut slf.content);
            immediate_bytes_awaitable(py, content)
        }
    }

    pub fn aclose(&self) {}
}

#[pyclass(name = "ByteStream", subclass, from_py_object)]
#[derive(Clone)]
pub struct PyByteStream {
    pub content: Vec<u8>,
    pub consumed: bool,
}

#[pymethods]
impl PyByteStream {
    #[new]
    #[pyo3(signature = (content = None))]
    pub fn new(content: Option<Vec<u8>>) -> Self {
        Self {
            content: content.unwrap_or_default(),
            consumed: false,
        }
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(mut slf: PyRefMut<'_, Self>, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        if slf.consumed {
            None
        } else {
            slf.consumed = true;
            Some(PyBytes::new(py, &slf.content))
        }
    }

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(mut slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if slf.consumed {
            Err(PyStopAsyncIteration::new_err(()))
        } else {
            slf.consumed = true;
            let content = std::mem::take(&mut slf.content);
            immediate_bytes_awaitable(py, content)
        }
    }

    pub fn close(&self) {}

    pub fn aclose(&self) {}
}
