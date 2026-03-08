use crate::transports::PySyncByteStream;
use pyo3::exceptions::{PyKeyError, PyTypeError, PyUnicodeDecodeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyDelta, PyDict, PyList};
type PyObject = Py<PyAny>;
use std::sync::{Arc, Mutex};

#[pyclass(name = "URL", from_py_object)]
#[derive(Clone)]
pub struct PyURL {
    pub inner: url::Url,
    display: Option<String>,
}

fn encode_query_params(_py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<String> {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());

    if let Ok(dict) = obj.cast::<PyDict>() {
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            let val: String = v.extract()?;
            serializer.append_pair(&key, &val);
        }
        return Ok(serializer.finish());
    }

    for item in obj.try_iter()? {
        let item = item?;
        let (k, v): (String, String) = item.extract()?;
        serializer.append_pair(&k, &v);
    }
    Ok(serializer.finish())
}

fn collect_body_bytes(py: Python<'_>, content: Option<Py<PyAny>>) -> PyResult<Vec<u8>> {
    let Some(content) = content else {
        return Ok(Vec::new());
    };

    let bound = content.bind(py);
    if let Ok(bytes) = bound.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }
    if let Ok(bytearray) = bound.cast::<PyByteArray>() {
        return Ok(bytearray.to_vec());
    }
    if let Ok(text) = bound.extract::<String>() {
        return Ok(text.into_bytes());
    }

    let mut out = Vec::new();
    for item in bound.try_iter()? {
        let item = item?;
        append_body_chunk(&item, &mut out)?;
    }
    Ok(out)
}

fn append_body_chunk(item: &Bound<'_, PyAny>, out: &mut Vec<u8>) -> PyResult<()> {
    if let Ok(bytes) = item.cast::<PyBytes>() {
        out.extend_from_slice(bytes.as_bytes());
    } else if let Ok(bytearray) = item.cast::<PyByteArray>() {
        out.extend_from_slice(&bytearray.to_vec());
    } else if let Ok(chunk) = item.extract::<Vec<u8>>() {
        out.extend_from_slice(&chunk);
    } else if let Ok(text) = item.extract::<String>() {
        out.extend_from_slice(text.as_bytes());
    } else {
        return Err(PyTypeError::new_err(
            "content iterator items must be bytes, bytearray, or str",
        ));
    }
    Ok(())
}

fn encode_json_body(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let json = py.import("json")?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("ensure_ascii", false)?;
    kwargs.set_item("separators", (",", ":"))?;
    kwargs.set_item("allow_nan", false)?;
    let dumped: String = json
        .getattr("dumps")?
        .call((value,), Some(&kwargs))?
        .extract()?;
    Ok(dumped.into_bytes())
}

fn immediate_awaitable<'py>(py: Python<'py>, value: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(value) })
}

impl PyURL {
    pub fn from_str(url_str: &str) -> PyResult<Self> {
        if let Ok(u) = url::Url::parse(url_str) {
            return Ok(PyURL {
                inner: u,
                display: None,
            });
        }

        if url_str.starts_with('/') || url_str.starts_with('?') || url_str.starts_with('#') {
            let base = url::Url::parse("http://localhost/").unwrap();
            let joined = base
                .join(url_str)
                .map_err(|e| PyValueError::new_err(format!("Invalid URL '{}': {}", url_str, e)))?;
            return Ok(PyURL {
                inner: joined,
                display: Some(url_str.to_string()),
            });
        }

        Err(PyValueError::new_err(format!(
            "Invalid URL '{}': relative URL without a base",
            url_str
        )))
    }
}

#[pymethods]
impl PyURL {
    #[new]
    pub fn new(py: Python<'_>, url: Py<PyAny>) -> PyResult<Self> {
        let bound = url.bind(py);
        if let Ok(url_str) = bound.extract::<String>() {
            return PyURL::from_str(&url_str);
        }
        if let Ok(existing_url) = bound.extract::<PyRef<PyURL>>() {
            return Ok(existing_url.clone());
        }
        Err(PyTypeError::new_err(format!(
            "Invalid type for url. Expected str or URL, got {}",
            bound.get_type().name()?
        )))
    }

    #[getter]
    pub fn scheme(&self) -> &str {
        if self.display.is_some() {
            ""
        } else {
            self.inner.scheme()
        }
    }

    #[getter]
    pub fn host(&self) -> Option<String> {
        if self.display.is_some() {
            Some(String::new())
        } else {
            self.inner.host_str().map(str::to_string)
        }
    }

    #[getter]
    pub fn port(&self) -> Option<u16> {
        if self.display.is_some() {
            None
        } else {
            self.inner.port_or_known_default()
        }
    }

    #[getter]
    pub fn path(&self) -> &str {
        self.inner.path()
    }

    #[getter]
    pub fn query(&self) -> Option<String> {
        self.inner.query().map(str::to_string)
    }

    #[getter]
    pub fn params(&self, py: Python<'_>) -> PyResult<crate::query_params::PyQueryParams> {
        let query = self.inner.query().unwrap_or("").to_string();
        let query_obj = query.into_pyobject(py)?.into_any().unbind();
        crate::query_params::PyQueryParams::new(py, Some(query_obj))
    }

    #[getter]
    pub fn fragment(&self) -> Option<&str> {
        self.inner.fragment()
    }

    #[getter]
    pub fn is_absolute_url(&self) -> bool {
        if self.display.is_some() {
            return false;
        }
        !self.inner.scheme().is_empty() && self.inner.host_str().is_some()
    }

    #[getter]
    pub fn is_relative_url(&self) -> bool {
        !self.is_absolute_url()
    }

    #[getter]
    pub fn netloc(&self) -> String {
        if self.display.is_some() {
            return String::new();
        }
        match self.inner.port() {
            Some(p) => format!("{}:{}", self.inner.host_str().unwrap_or(""), p),
            None => self.inner.host_str().unwrap_or("").to_string(),
        }
    }

    #[getter]
    pub fn raw_path<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let mut raw = self.inner.path().as_bytes().to_vec();
        if let Some(query) = self.inner.query() {
            raw.push(b'?');
            raw.extend_from_slice(query.as_bytes());
        }
        PyBytes::new(py, &raw)
    }

    #[getter]
    pub fn raw<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
        let scheme = PyBytes::new(py, self.inner.scheme().as_bytes());
        let host = PyBytes::new(py, self.inner.host_str().unwrap_or("").as_bytes());
        let port = self.inner.port();
        let raw_path = self.raw_path(py);
        let tuple = (scheme, host, port, raw_path);
        Ok(tuple.into_pyobject(py)?.into_any().unbind())
    }

    #[getter]
    pub fn _uri_reference<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
        let types_mod = py.import("types")?;
        let ns = types_mod.getattr("SimpleNamespace")?.call0()?;
        ns.setattr("path", self.inner.path())?;
        Ok(ns.into_any().unbind())
    }

    /// Return a copy with specified components replaced.
    #[pyo3(signature = (
        *,
        scheme = None,
        host = None,
        port = None,
        path = None,
        query = None,
        fragment = None,
        raw_path = None,
        params = None,
    ))]
    pub fn copy_with(
        &self,
        py: Python<'_>,
        scheme: Option<&str>,
        host: Option<&str>,
        port: Option<u16>,
        path: Option<&str>,
        query: Option<&str>,
        fragment: Option<&str>,
        raw_path: Option<Vec<u8>>,
        params: Option<Py<PyAny>>,
    ) -> PyResult<PyURL> {
        let mut new_url = self.inner.clone();
        if let Some(s) = scheme {
            new_url
                .set_scheme(s)
                .map_err(|_| PyValueError::new_err("Invalid scheme"))?;
        }
        if let Some(h) = host {
            new_url
                .set_host(Some(h))
                .map_err(|_| PyValueError::new_err("Invalid host"))?;
        }
        if let Some(p) = port {
            new_url
                .set_port(Some(p))
                .map_err(|_| PyValueError::new_err("Invalid port"))?;
        }
        if let Some(p) = path {
            new_url.set_path(p);
        }
        if let Some(rp) = raw_path {
            let raw_str = std::str::from_utf8(&rp)
                .map_err(|_| PyValueError::new_err("raw_path must be valid UTF-8 bytes"))?;
            let mut parts = raw_str.splitn(2, '?');
            let new_path = parts.next().unwrap_or("/");
            let new_query = parts.next();
            new_url.set_path(if new_path.is_empty() { "/" } else { new_path });
            new_url.set_query(new_query);
        }
        if let Some(q) = query {
            new_url.set_query(Some(q));
        }
        if let Some(p) = params {
            let encoded = encode_query_params(py, p.bind(py))?;
            new_url.set_query(Some(&encoded));
        }
        if let Some(f) = fragment {
            new_url.set_fragment(Some(f));
        }
        Ok(PyURL {
            inner: new_url,
            display: None,
        })
    }

    fn __str__(&self) -> String {
        self.display
            .clone()
            .unwrap_or_else(|| self.inner.to_string())
    }

    fn __repr__(&self) -> String {
        format!("URL('{}')", self.__str__())
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        if let Ok(other_url) = other.extract::<PyRef<PyURL>>() {
            return self.__str__() == other_url.__str__();
        }

        if let Ok(other_str) = other.extract::<String>() {
            if let Ok(other_url) = PyURL::from_str(&other_str) {
                return self.__str__() == other_url.__str__();
            }
        }

        false
    }

    fn __hash__(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.__str__().hash(&mut hasher);
        hasher.finish()
    }
}

#[pyclass(name = "Headers", from_py_object)]
#[derive(Clone)]
pub struct PyHeaders {
    // Stored as (lowercase_name, value) for case-insensitive lookup.
    pub inner: Vec<(String, String)>,
}

impl PyHeaders {
    pub fn new_empty() -> Self {
        PyHeaders { inner: Vec::new() }
    }

    pub fn from_reqwest(headers: &reqwest::header::HeaderMap) -> Self {
        let inner = headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        PyHeaders { inner }
    }

    pub fn from_vec(pairs: Vec<(String, String)>) -> Self {
        let inner = pairs
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();
        PyHeaders { inner }
    }

    /// Convert a Python object (dict or list of tuples) to PyHeaders.
    pub fn from_pyobject(py: Python<'_>, obj: Py<PyAny>) -> PyResult<Self> {
        let bound = obj.bind(py);
        if let Ok(dict) = bound.cast::<PyDict>() {
            let mut inner = Vec::new();
            for (k, v) in dict.iter() {
                let key: String = k.extract()?;
                let val: String = v.extract()?;
                inner.push((key.to_lowercase(), val));
            }
            Ok(PyHeaders { inner })
        } else if let Ok(list) = bound.cast::<PyList>() {
            let mut inner = Vec::new();
            for item in list.iter() {
                let pair: (String, String) = item.extract()?;
                inner.push((pair.0.to_lowercase(), pair.1));
            }
            Ok(PyHeaders { inner })
        } else {
            // Try to iterate as generic iterable of 2-tuples
            let mut inner = Vec::new();
            for item in bound.try_iter()? {
                let pair: (String, String) = item?.extract()?;
                inner.push((pair.0.to_lowercase(), pair.1));
            }
            Ok(PyHeaders { inner })
        }
    }
}

#[pymethods]
impl PyHeaders {
    #[new]
    #[pyo3(signature = (items = None))]
    pub fn new(py: Python<'_>, items: Option<Py<PyAny>>) -> PyResult<Self> {
        match items {
            None => Ok(PyHeaders::new_empty()),
            Some(obj) => PyHeaders::from_pyobject(py, obj),
        }
    }

    /// Get the value for a header, case-insensitive. Returns the last value if multiple exist.
    #[pyo3(signature = (key, default = None))]
    pub fn get(&self, key: &str, default: Option<String>) -> Option<String> {
        let lower = key.to_lowercase();
        self.inner
            .iter()
            .rev()
            .find(|(k, _)| k == &lower)
            .map(|(_, v)| v.clone())
            .or(default)
    }

    /// Update headers with new key-value pairs, replacing existing keys.
    pub fn update(&mut self, py: Python<'_>, items: Py<PyAny>) -> PyResult<()> {
        let new_headers = PyHeaders::from_pyobject(py, items)?;
        for (k, v) in new_headers.inner {
            // Remove existing entries for this key
            self.inner.retain(|(existing_k, _)| existing_k != &k);
            self.inner.push((k, v));
        }
        Ok(())
    }

    /// Return all (name, value) pairs.
    pub fn items(&self) -> Vec<(String, String)> {
        self.inner.clone()
    }

    pub fn multi_items(&self) -> Vec<(String, String)> {
        self.inner.clone()
    }

    #[pyo3(signature = (key, split_commas = false))]
    pub fn get_list(&self, key: &str, split_commas: bool) -> Vec<String> {
        let lower = key.to_lowercase();
        let mut values: Vec<String> = self
            .inner
            .iter()
            .filter(|(k, _)| k == &lower)
            .map(|(_, v)| v.clone())
            .collect();

        if split_commas {
            values = values
                .into_iter()
                .flat_map(|v| {
                    v.split(',')
                        .map(|part| part.trim().to_string())
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                })
                .collect();
        }

        values
    }

    #[pyo3(signature = (key, default = None))]
    pub fn pop(&mut self, key: &str, default: Option<String>) -> Option<String> {
        let lower = key.to_lowercase();
        let mut removed: Option<String> = None;
        self.inner.retain(|(k, v)| {
            if k == &lower {
                removed = Some(v.clone());
                false
            } else {
                true
            }
        });
        removed.or(default)
    }

    #[pyo3(signature = (key, default = None))]
    pub fn setdefault(&mut self, key: &str, default: Option<String>) -> String {
        if let Some(existing) = self.get(key, None) {
            return existing;
        }
        let value = default.unwrap_or_default();
        self.__setitem__(key, &value);
        value
    }

    pub fn copy(&self) -> Self {
        self.clone()
    }

    #[getter]
    pub fn encoding(&self) -> &str {
        "utf-8"
    }

    /// Return all header names.
    pub fn keys(&self) -> Vec<String> {
        self.inner.iter().map(|(k, _)| k.clone()).collect()
    }

    /// Return all header values.
    pub fn values(&self) -> Vec<String> {
        self.inner.iter().map(|(_, v)| v.clone()).collect()
    }

    fn __getitem__(&self, key: &str) -> PyResult<String> {
        self.get(key, None)
            .ok_or_else(|| PyKeyError::new_err(key.to_string()))
    }

    fn __setitem__(&mut self, key: &str, value: &str) {
        let lower = key.to_lowercase();
        self.inner.retain(|(k, _)| k != &lower);
        self.inner.push((lower, value.to_string()));
    }

    fn __contains__(&self, key: &str) -> bool {
        let lower = key.to_lowercase();
        self.inner.iter().any(|(k, _)| k == &lower)
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn __bool__(&self) -> bool {
        !self.inner.is_empty()
    }

    fn __iter__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let list = PyList::new(py, self.inner.clone())?;
        let iter = list.call_method0("__iter__")?;
        Ok(iter.unbind())
    }

    fn __repr__(&self) -> String {
        let pairs: Vec<String> = self
            .inner
            .iter()
            .map(|(k, v)| format!("({:?}, {:?})", k, v))
            .collect();
        format!("Headers([{}])", pairs.join(", "))
    }
}

#[pyclass(name = "Request", from_py_object)]
pub struct PyRequest {
    pub method: String,
    pub url: PyURL,
    pub headers: PyHeaders,
    pub content: Vec<u8>,
    pub extensions: Py<PyAny>,
}

impl Clone for PyRequest {
    fn clone(&self) -> Self {
        let extensions = Python::attach(|py| self.extensions.clone_ref(py));
        Self {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            content: self.content.clone(),
            extensions,
        }
    }
}

#[pymethods]
impl PyRequest {
    #[new]
    #[pyo3(signature = (
        method,
        url,
        *,
        headers = None,
        content = None,
        extensions = None,
    ))]
    pub fn new(
        py: Python<'_>,
        method: &str,
        url: Py<PyAny>,
        headers: Option<Py<PyAny>>,
        content: Option<Py<PyAny>>,
        extensions: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let url_bound = url.bind(py);
        let parsed_url = if let Ok(url_str) = url_bound.extract::<String>() {
            PyURL::from_str(&url_str)?
        } else if let Ok(existing_url) = url_bound.extract::<PyRef<PyURL>>() {
            existing_url.clone()
        } else {
            return Err(PyTypeError::new_err(format!(
                "Invalid type for url. Expected str or URL, got {}",
                url_bound.get_type().name()?
            )));
        };
        let parsed_headers = match headers {
            None => PyHeaders::new_empty(),
            Some(h) => PyHeaders::from_pyobject(py, h)?,
        };
        let parsed_extensions = match extensions {
            Some(ext) => ext,
            None => PyDict::new(py).into_any().unbind(),
        };
        Ok(PyRequest {
            method: method.to_uppercase(),
            url: parsed_url,
            headers: parsed_headers,
            content: collect_body_bytes(py, content)?,
            extensions: parsed_extensions,
        })
    }

    #[getter]
    pub fn method(&self) -> &str {
        &self.method
    }

    #[getter]
    pub fn url(&self) -> PyURL {
        self.url.clone()
    }

    #[getter]
    pub fn headers(&self) -> PyHeaders {
        self.headers.clone()
    }

    #[getter]
    pub fn content<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.content)
    }

    #[getter]
    pub fn extensions<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
        Ok(self.extensions.clone_ref(py))
    }

    #[getter]
    pub fn stream<'py>(&self, py: Python<'py>) -> PyResult<Py<PySyncByteStream>> {
        Py::new(
            py,
            PySyncByteStream {
                content: self.content.clone(),
                consumed: false,
            },
        )
    }

    pub fn read(&self) -> Vec<u8> {
        self.content.clone()
    }

    pub fn aread<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let bytes = PyBytes::new(py, &self.content).into_any().unbind();
        immediate_awaitable(py, bytes)
    }

    /// Set a single header value (replaces existing).
    pub fn set_header(&mut self, name: &str, value: &str) {
        let lower = name.to_lowercase();
        self.headers.inner.retain(|(k, _)| k != &lower);
        self.headers.inner.push((lower, value.to_string()));
    }

    fn __repr__(&self) -> String {
        format!("<Request [{} {}]>", self.method, self.url.inner)
    }
}

pub enum ResponseStream {
    Async(reqwest::Response),
    Blocking(reqwest::blocking::Response),
}

// Verify that Arc<Mutex<Option<ResponseStream>>> is Sync without any unsafe impl.
// This holds automatically because reqwest::{Response, blocking::Response} are Send,
// so ResponseStream: Send, so Mutex<Option<ResponseStream>>: Sync.
const _: fn() = || {
    fn assert_sync<T: Sync>() {}
    assert_sync::<std::sync::Arc<std::sync::Mutex<Option<ResponseStream>>>>();
};

#[pyclass(name = "Response", subclass)]
pub struct PyResponse {
    pub status_code: u16,
    pub reason_phrase: String,
    pub headers: PyHeaders,
    pub content: Vec<u8>,
    pub http_version: String,
    pub elapsed_ms: u128,
    pub url: String,
    pub request: Option<Py<PyRequest>>,
    pub encoding: Option<String>,
    pub extensions: Option<Py<PyAny>>,
    // Streaming: Some(response) while stream is unread, None after read.
    pub stream: Option<Arc<Mutex<Option<ResponseStream>>>>,
    // Python-provided streaming object, eg iterator/async iterator content.
    pub py_stream: Option<Py<PyAny>>,
}

/// Parse charset from Content-Type header value.
fn charset_from_content_type(ct: Option<&str>) -> Option<String> {
    ct.and_then(|s| {
        s.split(';').skip(1).find_map(|part| {
            let part = part.trim();
            if part.to_lowercase().starts_with("charset=") {
                Some(
                    part["charset=".len()..]
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string(),
                )
            } else {
                None
            }
        })
    })
}

/// Map HTTP version enum to string.
pub fn version_str(v: reqwest::Version) -> &'static str {
    match v {
        reqwest::Version::HTTP_09 => "HTTP/0.9",
        reqwest::Version::HTTP_10 => "HTTP/1.0",
        reqwest::Version::HTTP_11 => "HTTP/1.1",
        reqwest::Version::HTTP_2 => "HTTP/2",
        reqwest::Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/1.1",
    }
}

/// Convert serde_json::Value to a Python object.
pub fn json_to_python(py: Python<'_>, val: &serde_json::Value) -> PyResult<PyObject> {
    match val {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => {
            let pyobj = pyo3::types::PyBool::new(py, *b);
            Ok(pyobj.as_any().clone().unbind())
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_pyobject(py)?.into_any().unbind())
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_pyobject(py)?.into_any().unbind())
            } else {
                Ok(n.to_string().into_pyobject(py)?.into_any().unbind())
            }
        }
        serde_json::Value::String(s) => Ok(s.as_str().into_pyobject(py)?.into_any().unbind()),
        serde_json::Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                list.append(json_to_python(py, item)?)?;
            }
            Ok(list.into_any().unbind())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(k.as_str(), json_to_python(py, v)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

impl PyResponse {
    /// Build a PyResponse from a blocking reqwest response (eagerly reads body).
    pub fn from_blocking(
        resp: reqwest::blocking::Response,
        elapsed_ms: u128,
        request: Option<Py<PyRequest>>,
    ) -> PyResult<Self> {
        let status = resp.status();
        let status_code = status.as_u16();
        let reason_phrase = status.canonical_reason().unwrap_or("").to_string();
        let http_version = version_str(resp.version()).to_string();
        let headers = PyHeaders::from_reqwest(resp.headers());
        let url = resp.url().to_string();
        let encoding = charset_from_content_type(headers.get("content-type", None).as_deref());
        // Release GIL while reading the body so the local Python server can flush its response.
        let content = crate::without_gil(|| resp.bytes())
            .map_err(crate::map_reqwest_error)?
            .to_vec();
        Ok(PyResponse {
            status_code,
            reason_phrase,
            headers,
            content,
            http_version,
            elapsed_ms,
            url,
            request,
            encoding,
            extensions: None,
            stream: None,
            py_stream: None,
        })
    }

    /// Build a streaming PyResponse backed by a blocking reqwest response.
    pub fn from_blocking_stream(
        resp: reqwest::blocking::Response,
        elapsed_ms: u128,
        request: Option<Py<PyRequest>>,
    ) -> Self {
        let status = resp.status();
        let status_code = status.as_u16();
        let reason_phrase = status.canonical_reason().unwrap_or("").to_string();
        let http_version = version_str(resp.version()).to_string();
        let headers = PyHeaders::from_reqwest(resp.headers());
        let url = resp.url().to_string();
        let encoding = charset_from_content_type(headers.get("content-type", None).as_deref());
        PyResponse {
            status_code,
            reason_phrase,
            headers,
            content: Vec::new(),
            http_version,
            elapsed_ms,
            url,
            request,
            encoding,
            extensions: None,
            stream: Some(Arc::new(Mutex::new(Some(ResponseStream::Blocking(resp))))),
            py_stream: None,
        }
    }

    /// Build a PyResponse from an async reqwest response (eagerly reads body).
    pub async fn from_async(
        resp: reqwest::Response,
        elapsed_ms: u128,
        request: Option<Py<PyRequest>>,
    ) -> PyResult<Self> {
        let status = resp.status();
        let status_code = status.as_u16();
        let reason_phrase = status.canonical_reason().unwrap_or("").to_string();
        let http_version = version_str(resp.version()).to_string();
        let headers = PyHeaders::from_reqwest(resp.headers());
        let url = resp.url().to_string();
        let encoding = charset_from_content_type(headers.get("content-type", None).as_deref());
        let content = resp
            .bytes()
            .await
            .map_err(crate::map_reqwest_error)?
            .to_vec();

        Ok(PyResponse {
            status_code,
            reason_phrase,
            headers,
            content,
            http_version,
            elapsed_ms,
            url,
            request,
            encoding,
            extensions: None,
            stream: None,
            py_stream: None,
        })
    }

    /// Build a streaming PyResponse backed by an async reqwest response.
    pub fn from_async_stream(
        resp: reqwest::Response,
        elapsed_ms: u128,
        request: Option<Py<PyRequest>>,
    ) -> Self {
        let status = resp.status();
        let status_code = status.as_u16();
        let reason_phrase = status.canonical_reason().unwrap_or("").to_string();
        let http_version = version_str(resp.version()).to_string();
        let headers = PyHeaders::from_reqwest(resp.headers());
        let url = resp.url().to_string();
        let encoding = charset_from_content_type(headers.get("content-type", None).as_deref());

        PyResponse {
            status_code,
            reason_phrase,
            headers,
            content: Vec::new(),
            http_version,
            elapsed_ms,
            url,
            request,
            encoding,
            extensions: None,
            stream: Some(Arc::new(Mutex::new(Some(ResponseStream::Async(resp))))),
            py_stream: None,
        }
    }

    pub fn is_stream(&self) -> bool {
        self.stream.is_some()
    }
}

#[pymethods]
impl PyResponse {
    /// Construct a Response directly (for testing / manual use).
    #[new]
    #[pyo3(signature = (
        status_code = 200,
        *,
        content = None,
        text = None,
        html = None,
        json = None,
        stream = None,
        headers = None,
        request = None,
        extensions = None,
        history = None,
        default_encoding = None,
        http_version = None,
    ))]
    pub fn new(
        py: Python<'_>,
        status_code: u16,
        content: Option<Py<PyAny>>,
        text: Option<String>,
        html: Option<String>,
        json: Option<Py<PyAny>>,
        stream: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        request: Option<Py<PyRequest>>,
        extensions: Option<Py<PyAny>>,
        history: Option<Py<PyAny>>,
        default_encoding: Option<Py<PyAny>>,
        http_version: Option<String>,
    ) -> PyResult<Self> {
        let _ = history;
        let _ = default_encoding;
        let reason_phrase = reqwest::StatusCode::from_u16(status_code)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("")
            .to_string();

        let mut headers = match headers {
            None => PyHeaders::new_empty(),
            Some(h) => PyHeaders::from_pyobject(py, h)?,
        };

        let mut body = Vec::new();
        let mut py_stream = stream;

        if py_stream.is_none() {
            if let Some(obj) = content {
                let bound = obj.bind(py);
                if bound.hasattr("__aiter__")? && !bound.hasattr("__iter__")? {
                    py_stream = Some(obj);
                } else {
                    body = collect_body_bytes(py, Some(obj))?;
                }
                if headers.get("content-length", None).is_none() {
                    headers
                        .inner
                        .push(("content-length".to_string(), body.len().to_string()));
                }
            } else if let Some(text) = text {
                body = text.into_bytes();
                if headers.get("content-length", None).is_none() {
                    headers
                        .inner
                        .push(("content-length".to_string(), body.len().to_string()));
                }
                if headers.get("content-type", None).is_none() {
                    headers.inner.push((
                        "content-type".to_string(),
                        "text/plain; charset=utf-8".to_string(),
                    ));
                }
            } else if let Some(html) = html {
                body = html.into_bytes();
                if headers.get("content-length", None).is_none() {
                    headers
                        .inner
                        .push(("content-length".to_string(), body.len().to_string()));
                }
                if headers.get("content-type", None).is_none() {
                    headers.inner.push((
                        "content-type".to_string(),
                        "text/html; charset=utf-8".to_string(),
                    ));
                }
            } else if let Some(json_obj) = json {
                body = encode_json_body(py, json_obj.bind(py))?;
                if headers.get("content-length", None).is_none() {
                    headers
                        .inner
                        .push(("content-length".to_string(), body.len().to_string()));
                }
                if headers.get("content-type", None).is_none() {
                    headers
                        .inner
                        .push(("content-type".to_string(), "application/json".to_string()));
                }
            }
        }

        let encoding = charset_from_content_type(headers.get("content-type", None).as_deref());

        Ok(PyResponse {
            status_code,
            reason_phrase,
            headers,
            content: body,
            http_version: http_version.unwrap_or_else(|| "HTTP/1.1".to_string()),
            elapsed_ms: 0,
            url: String::new(),
            request,
            encoding,
            extensions,
            stream: None,
            py_stream,
        })
    }

    #[getter]
    pub fn status_code(&self) -> u16 {
        self.status_code
    }

    #[getter]
    pub fn reason_phrase(&self) -> &str {
        &self.reason_phrase
    }

    #[getter]
    pub fn headers(&self) -> PyHeaders {
        self.headers.clone()
    }

    #[getter]
    pub fn content<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.content)
    }

    #[getter]
    pub fn http_version(&self) -> &str {
        &self.http_version
    }

    #[getter]
    pub fn elapsed<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDelta>> {
        let total_micros = self.elapsed_ms * 1000;
        let days = (total_micros / 86_400_000_000) as i32;
        let secs = ((total_micros % 86_400_000_000) / 1_000_000) as i32;
        let micros = (total_micros % 1_000_000) as i32;
        PyDelta::new(py, days, secs, micros, true)
    }

    #[getter]
    pub fn url(&self) -> PyResult<PyURL> {
        if self.url.is_empty() {
            // Return a placeholder URL when constructed directly
            PyURL::from_str("http://localhost/")
        } else {
            PyURL::from_str(&self.url)
        }
    }

    #[getter]
    pub fn encoding(&self) -> Option<&str> {
        self.encoding.as_deref()
    }

    #[getter]
    pub fn is_redirect(&self) -> bool {
        self.status_code >= 300 && self.status_code < 400
    }

    #[getter]
    pub fn is_closed(&self) -> bool {
        self.stream.is_none() && self.py_stream.is_none()
    }

    #[getter]
    pub fn is_stream_consumed(&self) -> bool {
        self.stream.is_none() && self.py_stream.is_none()
    }

    #[getter]
    pub fn request(&self, py: Python<'_>) -> Option<Py<PyRequest>> {
        self.request.as_ref().map(|r| r.clone_ref(py))
    }

    #[getter]
    pub fn extensions(&self, py: Python<'_>) -> PyObject {
        self.extensions
            .as_ref()
            .map(|e| e.clone_ref(py))
            .unwrap_or_else(|| PyDict::new(py).into_any().unbind())
    }

    #[setter(extensions)]
    pub fn set_extensions(&mut self, value: Option<Py<PyAny>>) {
        self.extensions = value;
    }

    #[getter]
    pub fn stream(&self, py: Python<'_>) -> PyResult<PyObject> {
        if let Some(stream) = &self.py_stream {
            return Ok(stream.clone_ref(py));
        }
        let stream = Py::new(
            py,
            crate::transports::PyByteStream {
                content: self.content.clone(),
                consumed: false,
            },
        )?;
        Ok(stream.into_bound(py).into_any().unbind())
    }

    /// Decode content to text using the response encoding (defaults to UTF-8).
    #[getter]
    pub fn text(&self) -> PyResult<String> {
        String::from_utf8(self.content.clone())
            .map_err(|e| PyUnicodeDecodeError::new_err(format!("Failed to decode response: {}", e)))
    }

    /// Parse response body as JSON.
    pub fn json(&self, py: Python<'_>) -> PyResult<PyObject> {
        let val: serde_json::Value = serde_json::from_slice(&self.content)
            .map_err(|e| PyValueError::new_err(format!("JSON decode error: {}", e)))?;
        json_to_python(py, &val)
    }

    /// Raise HTTPStatusError for 4xx/5xx responses, otherwise return self.
    pub fn raise_for_status<'py>(slf: Bound<'py, Self>) -> PyResult<Bound<'py, Self>> {
        let status = slf.borrow().status_code;
        if status >= 400 {
            let reason = slf.borrow().reason_phrase.clone();
            let url = slf.borrow().url.clone();
            return Err(crate::HTTPStatusError::new_err(format!(
                "Client error '{}' for url '{}': {} {}",
                status, url, status, reason
            )));
        }
        Ok(slf)
    }

    /// Read the full response body (for streaming responses).
    pub fn read(&mut self) -> PyResult<Vec<u8>> {
        if let Some(stream_arc) = self.stream.take() {
            let mut guard = stream_arc.lock().unwrap();
            if let Some(stream) = guard.take() {
                match stream {
                    ResponseStream::Blocking(resp) => {
                        let bytes = crate::without_gil(|| resp.bytes())
                            .map_err(crate::map_reqwest_error)?;
                        self.content = bytes.to_vec();
                    }
                    ResponseStream::Async(resp) => {
                        let bytes = crate::without_gil(|| {
                            crate::run_blocking(async move {
                                resp.bytes().await.map_err(crate::map_reqwest_error)
                            })
                        })?;
                        self.content = bytes.to_vec();
                    }
                }
            }
        }
        if let Some(py_stream) = self.py_stream.take() {
            let collected = Python::attach(|py| -> PyResult<Vec<u8>> {
                let stream = py_stream.bind(py);
                if stream.hasattr("__iter__")? {
                    let mut out = Vec::new();
                    for item in stream.try_iter()? {
                        let item = item?;
                        append_body_chunk(&item, &mut out)?;
                    }
                    Ok(out)
                } else if stream.hasattr("__aiter__")? {
                    Err(PyTypeError::new_err(
                        "Cannot call read() on an async response stream; use aread()",
                    ))
                } else {
                    Ok(Vec::new())
                }
            })?;
            self.content = collected;
        }
        Ok(self.content.clone())
    }

    pub fn aread<'py>(slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let bytes = PyBytes::new(py, &slf.content).into_any().unbind();
        immediate_awaitable(py, bytes)
    }

    /// Iterate over bytes chunks (reads all as a single chunk for simplicity).
    #[pyo3(signature = (chunk_size = None))]
    pub fn iter_bytes(&mut self, chunk_size: Option<usize>) -> PyResult<Vec<Vec<u8>>> {
        if let Some(py_stream) = self.py_stream.take() {
            let chunks = Python::attach(|py| -> PyResult<Vec<Vec<u8>>> {
                let stream = py_stream.bind(py);
                if stream.hasattr("__iter__")? {
                    let mut out: Vec<Vec<u8>> = Vec::new();
                    for item in stream.try_iter()? {
                        let item = item?;
                        let mut chunk = Vec::new();
                        append_body_chunk(&item, &mut chunk)?;
                        out.push(chunk);
                    }
                    Ok(out)
                } else if stream.hasattr("__aiter__")? {
                    Err(PyTypeError::new_err(
                        "Cannot iterate bytes synchronously for async stream; use aiter_bytes()",
                    ))
                } else {
                    Ok(Vec::new())
                }
            })?;
            self.content = chunks.concat();
            return Ok(chunks);
        }

        self.read()?;
        if self.content.is_empty() {
            Ok(vec![])
        } else {
            let size = chunk_size.unwrap_or(65536).max(1);
            let chunks: Vec<Vec<u8>> = self.content.chunks(size).map(|c| c.to_vec()).collect();
            Ok(chunks)
        }
    }

    /// Iterate over text chunks.
    #[pyo3(signature = (chunk_size = None))]
    pub fn iter_text(&mut self, chunk_size: Option<usize>) -> PyResult<Vec<String>> {
        self.read()?;
        match String::from_utf8(self.content.clone()) {
            Ok(s) => {
                let size = chunk_size.unwrap_or(65536).max(1);
                let chunks: Vec<String> = s
                    .as_bytes()
                    .chunks(size)
                    .filter_map(|c| String::from_utf8(c.to_vec()).ok())
                    .collect();
                Ok(chunks)
            }
            Err(e) => Err(PyUnicodeDecodeError::new_err(e.to_string())),
        }
    }

    #[pyo3(signature = (chunk_size = None))]
    pub fn aiter_bytes<'py>(
        &self,
        py: Python<'py>,
        chunk_size: Option<usize>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Some(py_stream) = &self.py_stream {
            let stream = py_stream.bind(py);
            if stream.hasattr("__aiter__")? {
                return stream.call_method0("__aiter__");
            }
            if stream.hasattr("__iter__")? {
                let mut out = Vec::new();
                for item in stream.try_iter()? {
                    let item = item?;
                    append_body_chunk(&item, &mut out)?;
                }
                let chunk_size = chunk_size.unwrap_or(65536).max(1);
                let first = out
                    .chunks(chunk_size)
                    .next()
                    .map(|c| c.to_vec())
                    .unwrap_or_default();
                let stream_obj = Py::new(
                    py,
                    crate::transports::PyAsyncByteStream {
                        content: first,
                        consumed: false,
                    },
                )?;
                return Ok(stream_obj.into_bound(py).into_any());
            }
        }

        let size = chunk_size.unwrap_or(65536).max(1);
        let first = self
            .content
            .chunks(size)
            .next()
            .map(|c| c.to_vec())
            .unwrap_or_default();
        let stream_obj = Py::new(
            py,
            crate::transports::PyAsyncByteStream {
                content: first,
                consumed: false,
            },
        )?;
        Ok(stream_obj.into_bound(py).into_any())
    }

    pub fn close(&mut self) {
        self.stream = None;
        self.py_stream = None;
    }

    pub fn aclose<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.close();
        immediate_awaitable(py, py.None())
    }

    fn __repr__(&self) -> String {
        format!("<Response [{} {}]>", self.status_code, self.reason_phrase)
    }
}
