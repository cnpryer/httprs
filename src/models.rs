use pyo3::exceptions::{PyKeyError, PyUnicodeDecodeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDelta, PyDict, PyList};
type PyObject = Py<PyAny>;
use std::sync::{Arc, Mutex};

#[pyclass(name = "URL", from_py_object)]
#[derive(Clone)]
pub struct PyURL {
    pub inner: url::Url,
}

#[pymethods]
impl PyURL {
    #[new]
    pub fn new(url_str: &str) -> PyResult<Self> {
        url::Url::parse(url_str)
            .map(|u| PyURL { inner: u })
            .map_err(|e| PyValueError::new_err(format!("Invalid URL '{}': {}", url_str, e)))
    }

    #[getter]
    pub fn scheme(&self) -> &str {
        self.inner.scheme()
    }

    #[getter]
    pub fn host(&self) -> Option<String> {
        self.inner.host_str().map(str::to_string)
    }

    #[getter]
    pub fn port(&self) -> Option<u16> {
        self.inner.port_or_known_default()
    }

    #[getter]
    pub fn path(&self) -> &str {
        self.inner.path()
    }

    #[getter]
    pub fn query(&self) -> Option<&str> {
        self.inner.query()
    }

    #[getter]
    pub fn fragment(&self) -> Option<&str> {
        self.inner.fragment()
    }

    #[getter]
    pub fn netloc(&self) -> String {
        match self.inner.port() {
            Some(p) => format!("{}:{}", self.inner.host_str().unwrap_or(""), p),
            None => self.inner.host_str().unwrap_or("").to_string(),
        }
    }

    /// Return a copy with specified components replaced.
    #[pyo3(signature = (*, scheme = None, host = None, port = None, path = None, query = None, fragment = None))]
    pub fn copy_with(
        &self,
        scheme: Option<&str>,
        host: Option<&str>,
        port: Option<u16>,
        path: Option<&str>,
        query: Option<&str>,
        fragment: Option<&str>,
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
        if let Some(q) = query {
            new_url.set_query(Some(q));
        }
        if let Some(f) = fragment {
            new_url.set_fragment(Some(f));
        }
        Ok(PyURL { inner: new_url })
    }

    fn __str__(&self) -> String {
        self.inner.to_string()
    }

    fn __repr__(&self) -> String {
        format!("URL('{}')", self.inner)
    }

    fn __eq__(&self, other: &PyURL) -> bool {
        self.inner == other.inner
    }

    fn __hash__(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.inner.as_str().hash(&mut hasher);
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
#[derive(Clone)]
pub struct PyRequest {
    pub method: String,
    pub url: PyURL,
    pub headers: PyHeaders,
    pub content: Vec<u8>,
}

#[pymethods]
impl PyRequest {
    #[new]
    #[pyo3(signature = (method, url, *, headers = None, content = None))]
    pub fn new(
        py: Python<'_>,
        method: &str,
        url: &str,
        headers: Option<Py<PyAny>>,
        content: Option<Vec<u8>>,
    ) -> PyResult<Self> {
        let parsed_url = PyURL::new(url)?;
        let parsed_headers = match headers {
            None => PyHeaders::new_empty(),
            Some(h) => PyHeaders::from_pyobject(py, h)?,
        };
        Ok(PyRequest {
            method: method.to_uppercase(),
            url: parsed_url,
            headers: parsed_headers,
            content: content.unwrap_or_default(),
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
// Neither reqwest::Response nor reqwest::blocking::Response is Sync;
// we only access them from one thread at a time via Mutex.
unsafe impl Sync for ResponseStream {}

#[pyclass(name = "Response")]
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
    // Streaming: Some(response) while stream is unread, None after read.
    pub stream: Option<Arc<Mutex<Option<ResponseStream>>>>,
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
            stream: None,
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
            stream: Some(Arc::new(Mutex::new(Some(ResponseStream::Blocking(resp))))),
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
            stream: None,
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
            stream: Some(Arc::new(Mutex::new(Some(ResponseStream::Async(resp))))),
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
    #[pyo3(signature = (status_code = 200, *, content = None, headers = None, request = None, http_version = None))]
    pub fn new(
        py: Python<'_>,
        status_code: u16,
        content: Option<Vec<u8>>,
        headers: Option<Py<PyAny>>,
        request: Option<Py<PyRequest>>,
        http_version: Option<String>,
    ) -> PyResult<Self> {
        let reason_phrase = reqwest::StatusCode::from_u16(status_code)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("")
            .to_string();

        let headers = match headers {
            None => PyHeaders::new_empty(),
            Some(h) => PyHeaders::from_pyobject(py, h)?,
        };

        let encoding = charset_from_content_type(headers.get("content-type", None).as_deref());

        Ok(PyResponse {
            status_code,
            reason_phrase,
            headers,
            content: content.unwrap_or_default(),
            http_version: http_version.unwrap_or_else(|| "HTTP/1.1".to_string()),
            elapsed_ms: 0,
            url: String::new(),
            request,
            encoding,
            stream: None,
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
            PyURL::new("http://localhost/")
        } else {
            PyURL::new(&self.url)
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
    pub fn request(&self, py: Python<'_>) -> Option<Py<PyRequest>> {
        self.request.as_ref().map(|r| r.clone_ref(py))
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
        Ok(self.content.clone())
    }

    /// Iterate over bytes chunks (reads all as a single chunk for simplicity).
    pub fn iter_bytes(&mut self) -> PyResult<Vec<Vec<u8>>> {
        self.read()?;
        if self.content.is_empty() {
            Ok(vec![])
        } else {
            // Return content in 64KB chunks
            let chunks: Vec<Vec<u8>> = self.content.chunks(65536).map(|c| c.to_vec()).collect();
            Ok(chunks)
        }
    }

    /// Iterate over text chunks.
    pub fn iter_text(&mut self) -> PyResult<Vec<String>> {
        self.read()?;
        match String::from_utf8(self.content.clone()) {
            Ok(s) => {
                // Return in 64KB chunks
                let chunks: Vec<String> = s
                    .as_bytes()
                    .chunks(65536)
                    .filter_map(|c| String::from_utf8(c.to_vec()).ok())
                    .collect();
                Ok(chunks)
            }
            Err(e) => Err(PyUnicodeDecodeError::new_err(e.to_string())),
        }
    }

    fn __repr__(&self) -> String {
        format!("<Response [{} {}]>", self.status_code, self.reason_phrase)
    }
}
