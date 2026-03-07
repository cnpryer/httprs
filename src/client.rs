use crate::auth::{PyBasicAuth, PyDigestAuth};
use crate::config::PyTimeout;
use crate::models::{version_str, PyHeaders, PyRequest, PyResponse, ResponseStream};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum RequestBody {
    Empty,
    Bytes(Vec<u8>),
    Json(String),
    Form(Vec<(String, String)>),
}

fn build_body(
    py: Python<'_>,
    content: Option<Vec<u8>>,
    json: Option<Py<PyAny>>,
    data: Option<Py<PyAny>>,
) -> PyResult<RequestBody> {
    if let Some(bytes) = content {
        return Ok(RequestBody::Bytes(bytes));
    }
    if let Some(json_obj) = json {
        let json_mod = py.import("json")?;
        let json_str: String = json_mod
            .call_method1("dumps", (json_obj.bind(py),))?
            .extract()?;
        return Ok(RequestBody::Json(json_str));
    }
    if let Some(data_obj) = data {
        let bound = data_obj.bind(py);
        if let Ok(bytes) = bound.cast::<pyo3::types::PyBytes>() {
            return Ok(RequestBody::Bytes(bytes.as_bytes().to_vec()));
        }
        let dict = bound
            .cast::<pyo3::types::PyDict>()
            .map_err(|_| pyo3::exceptions::PyTypeError::new_err("data must be a dict or bytes"))?;
        let pairs: Vec<(String, String)> = dict
            .iter()
            .map(|(k, v)| {
                let key: String = k.extract().unwrap_or_default();
                let val: String = v.extract().unwrap_or_default();
                (key, val)
            })
            .collect();
        return Ok(RequestBody::Form(pairs));
    }
    Ok(RequestBody::Empty)
}

fn urlencoding_encode(s: &str) -> String {
    let mut encoded = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => encoded.push(c),
            _ => {
                for byte in c.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    encoded
}

fn form_encode_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding_encode(k), urlencoding_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

enum AuthKind {
    Basic(String),
    Digest(Py<PyDigestAuth>),
}

fn extract_auth(py: Python<'_>, auth: &Py<PyAny>) -> PyResult<AuthKind> {
    let bound = auth.bind(py);
    if let Ok(basic) = bound.extract::<PyRef<PyBasicAuth>>() {
        return Ok(AuthKind::Basic(basic.authorization_header().to_string()));
    }
    if let Ok(digest) = bound.cast::<PyDigestAuth>() {
        return Ok(AuthKind::Digest(digest.clone().unbind()));
    }
    if let Ok((user, pass)) = bound.extract::<(String, String)>() {
        let basic = PyBasicAuth::new(&user, &pass);
        return Ok(AuthKind::Basic(basic.authorization_header().to_string()));
    }
    Err(PyValueError::new_err("Unsupported auth type"))
}

fn build_blocking_request(
    client: &reqwest::blocking::Client,
    method: &str,
    url: &str,
    extra_headers: Option<&PyHeaders>,
    default_headers: &PyHeaders,
    body: RequestBody,
    auth: Option<&AuthKind>,
    timeout: Option<Duration>,
) -> PyResult<reqwest::blocking::RequestBuilder> {
    let method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
        .map_err(|_| PyValueError::new_err(format!("Invalid HTTP method: {}", method)))?;

    let mut builder = client.request(method, url);

    for (k, v) in &default_headers.inner {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if let Some(h) = extra_headers {
        for (k, v) in &h.inner {
            builder = builder.header(k.as_str(), v.as_str());
        }
    }

    match body {
        RequestBody::Empty => {}
        RequestBody::Bytes(bytes) => {
            builder = builder.body(bytes);
        }
        RequestBody::Json(json_str) => {
            builder = builder
                .header("content-type", "application/json")
                .body(json_str.into_bytes());
        }
        RequestBody::Form(pairs) => {
            let encoded = form_encode_pairs(&pairs);
            builder = builder
                .header("content-type", "application/x-www-form-urlencoded")
                .body(encoded.into_bytes());
        }
    }

    if let Some(AuthKind::Basic(header_val)) = auth {
        builder = builder.header("authorization", header_val.as_str());
    }

    if let Some(dur) = timeout {
        builder = builder.timeout(dur);
    }

    Ok(builder)
}

fn build_async_request(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    extra_headers: Option<&PyHeaders>,
    default_headers: &PyHeaders,
    body: RequestBody,
    auth: Option<&AuthKind>,
    timeout: Option<Duration>,
) -> PyResult<reqwest::RequestBuilder> {
    let method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
        .map_err(|_| PyValueError::new_err(format!("Invalid HTTP method: {}", method)))?;

    let mut builder = client.request(method, url);

    for (k, v) in &default_headers.inner {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if let Some(h) = extra_headers {
        for (k, v) in &h.inner {
            builder = builder.header(k.as_str(), v.as_str());
        }
    }

    match body {
        RequestBody::Empty => {}
        RequestBody::Bytes(bytes) => {
            builder = builder.body(bytes);
        }
        RequestBody::Json(json_str) => {
            builder = builder
                .header("content-type", "application/json")
                .body(json_str.into_bytes());
        }
        RequestBody::Form(pairs) => {
            let encoded = form_encode_pairs(&pairs);
            builder = builder
                .header("content-type", "application/x-www-form-urlencoded")
                .body(encoded.into_bytes());
        }
    }

    if let Some(AuthKind::Basic(header_val)) = auth {
        builder = builder.header("authorization", header_val.as_str());
    }

    if let Some(dur) = timeout {
        builder = builder.timeout(dur);
    }

    Ok(builder)
}

fn timeout_duration(timeout: &PyTimeout) -> Option<Duration> {
    timeout.read.map(Duration::from_secs_f64)
}

fn parse_timeout_arg(
    py: Python<'_>,
    timeout: Option<Py<PyAny>>,
    default: &PyTimeout,
) -> Option<Duration> {
    match timeout {
        None => timeout_duration(default),
        Some(t) => {
            let bound = t.bind(py);
            if let Ok(f) = bound.extract::<f64>() {
                Some(Duration::from_secs_f64(f))
            } else if let Ok(pt) = bound.extract::<PyRef<PyTimeout>>() {
                timeout_duration(&pt)
            } else {
                timeout_duration(default)
            }
        }
    }
}

/// Returns true if `url` resolves to a private, loopback, or link-local address.
/// Used to block SSRF via open redirects.
fn is_private_url(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(addr)) => {
            addr.is_loopback() || addr.is_private() || addr.is_link_local() || addr.is_unspecified()
        }
        Some(url::Host::Ipv6(addr)) => {
            addr.is_loopback()
                || (addr.segments()[0] & 0xffc0 == 0xfe80) // link-local fe80::/10
                || (addr.segments()[0] & 0xfe00 == 0xfc00) // unique-local fc00::/7
        }
        Some(url::Host::Domain(host)) => host == "localhost",
        None => false,
    }
}

/// Build a redirect policy. When `block_private` is true, any redirect that
/// resolves to a private/loopback address is rejected with an error, preventing
/// SSRF attacks through server-controlled redirects.
fn make_redirect_policy(follow: bool, block_private: bool) -> reqwest::redirect::Policy {
    if !follow {
        return reqwest::redirect::Policy::none();
    }
    if block_private {
        reqwest::redirect::Policy::custom(|attempt| {
            if is_private_url(attempt.url()) {
                attempt.error("redirect to private/loopback address blocked (SSRF protection)")
            } else if attempt.previous().len() >= 20 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        })
    } else {
        reqwest::redirect::Policy::limited(20)
    }
}

#[pyclass(name = "Client")]
pub struct PyClient {
    inner: Option<reqwest::blocking::Client>,
    base_url: Option<String>,
    default_headers: PyHeaders,
    timeout: PyTimeout,
    #[allow(dead_code)]
    follow_redirects: bool,
    #[allow(dead_code)]
    block_private_redirects: bool,
    default_auth: Option<AuthKind>,
}

impl PyClient {
    fn get_client(&self) -> PyResult<&reqwest::blocking::Client> {
        self.inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Client is closed"))
    }

    fn resolve_url(&self, url: &str) -> String {
        if let Some(ref base) = self.base_url {
            if url.starts_with("http://") || url.starts_with("https://") {
                url.to_string()
            } else {
                let base = base.trim_end_matches('/');
                let path = url.trim_start_matches('/');
                format!("{}/{}", base, path)
            }
        } else {
            url.to_string()
        }
    }
}

#[pymethods]
impl PyClient {
    #[new]
    #[pyo3(signature = (
        base_url = None,
        headers = None,
        timeout = None,
        auth = None,
        follow_redirects = true,
        *,
        http2 = false,
        block_private_redirects = false,
    ))]
    pub fn new(
        py: Python<'_>,
        base_url: Option<String>,
        headers: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        follow_redirects: bool,
        http2: bool,
        block_private_redirects: bool,
    ) -> PyResult<Self> {
        let _ = http2;
        let default_headers = match headers {
            None => PyHeaders::new_empty(),
            Some(h) => PyHeaders::from_pyobject(py, h)?,
        };

        let py_timeout = match timeout {
            None => crate::config::PyTimeout::new(Some(5.0), None, None, None, None),
            Some(t) => {
                let bound = t.bind(py);
                if let Ok(pt) = bound.extract::<PyRef<crate::config::PyTimeout>>() {
                    pt.clone()
                } else if let Ok(f) = bound.extract::<f64>() {
                    crate::config::PyTimeout::new(Some(f), None, None, None, None)
                } else {
                    crate::config::PyTimeout::new(Some(5.0), None, None, None, None)
                }
            }
        };

        let default_auth = match auth {
            None => None,
            Some(a) => Some(extract_auth(py, &a)?),
        };

        let redirect_policy = make_redirect_policy(follow_redirects, block_private_redirects);

        let mut client_builder = reqwest::blocking::Client::builder()
            .redirect(redirect_policy)
            .cookie_store(true);

        if let Some(ct) = py_timeout.connect {
            client_builder = client_builder.connect_timeout(Duration::from_secs_f64(ct));
        }

        let inner = client_builder
            .build()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

        Ok(PyClient {
            inner: Some(inner),
            base_url,
            default_headers,
            timeout: py_timeout,
            follow_redirects,
            block_private_redirects,
            default_auth,
        })
    }

    #[pyo3(signature = (
        method,
        url,
        *,
        content = None,
        json = None,
        data = None,
        headers = None,
        auth = None,
        timeout = None,
        follow_redirects = None,
    ))]
    pub fn request(
        &self,
        py: Python<'_>,
        method: &str,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        let _ = follow_redirects;
        let client = self.get_client()?.clone();
        let full_url = self.resolve_url(url);

        let extra_headers = match headers {
            None => None,
            Some(h) => Some(PyHeaders::from_pyobject(py, h)?),
        };

        let req_timeout = parse_timeout_arg(py, timeout, &self.timeout);

        let req_auth = match auth {
            Some(ref a) => Some(extract_auth(py, a)?),
            None => None,
        };
        let effective_auth = req_auth.as_ref().or(self.default_auth.as_ref());

        let body = build_body(py, content, json, data)?;

        // DigestAuth: two-pass — first request without auth, retry with credentials on 401
        if let Some(AuthKind::Digest(digest_py)) = effective_auth {
            let digest_py = digest_py.clone_ref(py);
            let method_str = method.to_string();
            let url_str = {
                // RFC 7616 §3.4: digest-uri is the Request-URI (path + query, no scheme/host)
                if let Ok(parsed) = url::Url::parse(&full_url) {
                    match parsed.query() {
                        Some(q) => format!("{}?{}", parsed.path(), q),
                        None => parsed.path().to_string(),
                    }
                } else {
                    full_url.clone()
                }
            };
            let full_url2 = full_url.clone();
            let default_headers2 = self.default_headers.clone();
            let extra_headers2 = extra_headers.clone();
            let client2 = client.clone();

            let builder = build_blocking_request(
                &client,
                method,
                &full_url,
                extra_headers.as_ref(),
                &self.default_headers,
                body,
                None, // no auth on first pass
                req_timeout,
            )?;
            let start = Instant::now();

            let resp =
                crate::without_gil(move || builder.send().map_err(crate::map_reqwest_error))?;

            if resp.status().as_u16() == 401 {
                let www_auth = resp
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                let auth_header = {
                    let digest_ref = digest_py.bind(py);
                    let digest = digest_ref.borrow();
                    digest.compute_header(&method_str, &url_str, &www_auth)?
                };

                let builder2 = build_blocking_request(
                    &client2,
                    &method_str,
                    &full_url2,
                    extra_headers2.as_ref(),
                    &default_headers2,
                    RequestBody::Empty,
                    None,
                    req_timeout,
                )?;
                let builder2 = builder2.header("authorization", auth_header.as_str());

                crate::without_gil(move || {
                    let resp2 = builder2.send().map_err(crate::map_reqwest_error)?;
                    let elapsed = start.elapsed().as_millis();
                    PyResponse::from_blocking(resp2, elapsed, None)
                })
            } else {
                let elapsed = start.elapsed().as_millis();
                PyResponse::from_blocking(resp, elapsed, None)
            }
        } else {
            let builder = build_blocking_request(
                &client,
                method,
                &full_url,
                extra_headers.as_ref(),
                &self.default_headers,
                body,
                effective_auth,
                req_timeout,
            )?;
            let start = Instant::now();

            // Release GIL while blocking on I/O
            let result = crate::without_gil(|| builder.send());
            let resp = result.map_err(crate::map_reqwest_error)?;
            let elapsed = start.elapsed().as_millis();
            PyResponse::from_blocking(resp, elapsed, None)
        }
    }

    #[pyo3(signature = (url, *, content = None, json = None, data = None, headers = None, auth = None, timeout = None, follow_redirects = None))]
    pub fn get(
        &self,
        py: Python<'_>,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        self.request(
            py,
            "GET",
            url,
            content,
            json,
            data,
            headers,
            auth,
            timeout,
            follow_redirects,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, data = None, headers = None, auth = None, timeout = None, follow_redirects = None))]
    pub fn post(
        &self,
        py: Python<'_>,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        self.request(
            py,
            "POST",
            url,
            content,
            json,
            data,
            headers,
            auth,
            timeout,
            follow_redirects,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, data = None, headers = None, auth = None, timeout = None, follow_redirects = None))]
    pub fn put(
        &self,
        py: Python<'_>,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        self.request(
            py,
            "PUT",
            url,
            content,
            json,
            data,
            headers,
            auth,
            timeout,
            follow_redirects,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, data = None, headers = None, auth = None, timeout = None, follow_redirects = None))]
    pub fn patch(
        &self,
        py: Python<'_>,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        self.request(
            py,
            "PATCH",
            url,
            content,
            json,
            data,
            headers,
            auth,
            timeout,
            follow_redirects,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, data = None, headers = None, auth = None, timeout = None, follow_redirects = None))]
    pub fn delete(
        &self,
        py: Python<'_>,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        self.request(
            py,
            "DELETE",
            url,
            content,
            json,
            data,
            headers,
            auth,
            timeout,
            follow_redirects,
        )
    }

    #[pyo3(signature = (url, *, headers = None, auth = None, timeout = None, follow_redirects = None))]
    pub fn head(
        &self,
        py: Python<'_>,
        url: &str,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        self.request(
            py,
            "HEAD",
            url,
            None,
            None,
            None,
            headers,
            auth,
            timeout,
            follow_redirects,
        )
    }

    #[pyo3(signature = (url, *, headers = None, auth = None, timeout = None, follow_redirects = None))]
    pub fn options(
        &self,
        py: Python<'_>,
        url: &str,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<PyResponse> {
        self.request(
            py,
            "OPTIONS",
            url,
            None,
            None,
            None,
            headers,
            auth,
            timeout,
            follow_redirects,
        )
    }

    /// Build a Request object without sending it.
    #[pyo3(signature = (method, url, *, content = None, json = None, data = None, headers = None))]
    pub fn build_request(
        &self,
        py: Python<'_>,
        method: &str,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
    ) -> PyResult<PyRequest> {
        let full_url = self.resolve_url(url);
        let mut merged_headers = self.default_headers.clone();

        if let Some(h) = headers {
            let extra = PyHeaders::from_pyobject(py, h)?;
            for (k, v) in extra.inner {
                merged_headers.inner.retain(|(ek, _)| ek != &k);
                merged_headers.inner.push((k, v));
            }
        }

        let body_content = match build_body(py, content, json, data)? {
            RequestBody::Empty => vec![],
            RequestBody::Bytes(b) => b,
            RequestBody::Json(s) => s.into_bytes(),
            RequestBody::Form(pairs) => form_encode_pairs(&pairs).into_bytes(),
        };

        let headers_obj: Py<PyHeaders> = Py::new(py, merged_headers)?;
        PyRequest::new(
            py,
            method,
            &full_url,
            Some(headers_obj.into_bound(py).into_any().unbind()),
            Some(body_content),
        )
    }

    /// Send a pre-built Request.
    pub fn send(&self, _py: Python<'_>, request: &PyRequest) -> PyResult<PyResponse> {
        let client = self.get_client()?.clone();
        let method_str = request.method.clone();
        let url = request.url.inner.to_string();
        let headers: Vec<(String, String)> = request.headers.inner.clone();
        let body = if request.content.is_empty() {
            None
        } else {
            Some(request.content.clone())
        };

        let method = reqwest::Method::from_bytes(method_str.as_bytes())
            .map_err(|_| PyValueError::new_err("Invalid method"))?;
        let mut builder = client.request(method, &url);
        for (k, v) in &headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        if let Some(b) = body {
            builder = builder.body(b);
        }
        let start = Instant::now();
        let result = crate::without_gil(|| builder.send());
        let response = result.map_err(crate::map_reqwest_error)?;
        let elapsed = start.elapsed().as_millis();
        PyResponse::from_blocking(response, elapsed, None)
    }

    /// Return a context manager for streaming the response.
    #[pyo3(signature = (method, url, *, content = None, json = None, data = None, headers = None, auth = None, timeout = None))]
    pub fn stream(
        &self,
        py: Python<'_>,
        method: &str,
        url: &str,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
    ) -> PyResult<PyStreamContext> {
        let _ = self.get_client()?;
        Ok(PyStreamContext {
            client_inner: self.inner.as_ref().unwrap().clone(), // blocking::Client is Clone
            method: method.to_string(),
            url: self.resolve_url(url),
            content: content.unwrap_or_default(),
            json: json.map(|j| j.into_bound(py).into_any().unbind()),
            data: data.map(|d| d.into_bound(py).into_any().unbind()),
            extra_headers: headers
                .map(|h| PyHeaders::from_pyobject(py, h))
                .transpose()?,
            auth: auth.map(|a| extract_auth(py, &a)).transpose()?,
            timeout: parse_timeout_arg(py, timeout, &self.timeout),
            default_headers: self.default_headers.clone(),
            response: None,
        })
    }

    pub fn close(&mut self) {
        self.inner = None;
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __exit__(
        &mut self,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        self.close();
        false
    }
}

#[pyclass(name = "StreamContext")]
pub struct PyStreamContext {
    client_inner: reqwest::blocking::Client,
    method: String,
    url: String,
    content: Vec<u8>,
    json: Option<Py<PyAny>>,
    data: Option<Py<PyAny>>,
    extra_headers: Option<PyHeaders>,
    auth: Option<AuthKind>,
    timeout: Option<Duration>,
    default_headers: PyHeaders,
    response: Option<Arc<Mutex<Option<ResponseStream>>>>,
}

#[pymethods]
impl PyStreamContext {
    fn __enter__(mut slf: PyRefMut<'_, Self>, py: Python<'_>) -> PyResult<PyResponse> {
        let body = {
            let content = if slf.content.is_empty() {
                None
            } else {
                Some(slf.content.clone())
            };
            let json = slf.json.as_ref().map(|j| j.clone_ref(py));
            let data = slf.data.as_ref().map(|d| d.clone_ref(py));
            build_body(py, content, json, data)?
        };

        let client = slf.client_inner.clone();
        let builder = build_blocking_request(
            &client,
            &slf.method.clone(),
            &slf.url.clone(),
            slf.extra_headers.as_ref(),
            &slf.default_headers.clone(),
            body,
            slf.auth.as_ref(),
            slf.timeout,
        )?;

        let start = Instant::now();
        let resp = crate::without_gil(move || builder.send().map_err(crate::map_reqwest_error))?;
        let elapsed = start.elapsed().as_millis();

        let py_resp = PyResponse::from_blocking_stream(resp, elapsed, None);
        slf.response = py_resp.stream.clone();
        Ok(py_resp)
    }

    fn __exit__(
        mut slf: PyRefMut<'_, Self>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        // Drop the blocking response to close the connection.
        if let Some(stream_arc) = slf.response.take() {
            let mut guard = stream_arc.lock().unwrap();
            drop(guard.take());
        }
        false
    }
}

#[pyclass(name = "AsyncClient")]
pub struct PyAsyncClient {
    inner: Option<reqwest::Client>,
    base_url: Option<String>,
    default_headers: PyHeaders,
    timeout: PyTimeout,
    #[allow(dead_code)]
    follow_redirects: bool,
    #[allow(dead_code)]
    block_private_redirects: bool,
}

impl PyAsyncClient {
    fn get_client(&self) -> PyResult<reqwest::Client> {
        self.inner
            .clone()
            .ok_or_else(|| PyRuntimeError::new_err("AsyncClient is closed"))
    }

    fn resolve_url(&self, url: &str) -> String {
        if let Some(ref base) = self.base_url {
            if url.starts_with("http://") || url.starts_with("https://") {
                url.to_string()
            } else {
                let base = base.trim_end_matches('/');
                let path = url.trim_start_matches('/');
                format!("{}/{}", base, path)
            }
        } else {
            url.to_string()
        }
    }
}

/// Convert an async reqwest response to PyResponse.
async fn convert_async_response(
    resp: reqwest::Response,
    elapsed_ms: u128,
    request: Option<Py<PyRequest>>,
) -> PyResult<PyResponse> {
    let status = resp.status();
    let status_code = status.as_u16();
    let reason_phrase = status.canonical_reason().unwrap_or("").to_string();
    let http_version = version_str(resp.version()).to_string();
    let headers = PyHeaders::from_reqwest(resp.headers());
    let url = resp.url().to_string();
    let encoding = {
        let ct = headers.get("content-type", None);
        ct.as_deref().and_then(|ct| {
            ct.split(';').skip(1).find_map(|part| {
                let part = part.trim();
                if part.to_lowercase().starts_with("charset=") {
                    Some(part["charset=".len()..].trim_matches('"').to_string())
                } else {
                    None
                }
            })
        })
    };
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

#[pymethods]
impl PyAsyncClient {
    #[new]
    #[pyo3(signature = (
        base_url = None,
        headers = None,
        timeout = None,
        follow_redirects = true,
        *,
        http2 = false,
        block_private_redirects = false,
    ))]
    pub fn new(
        py: Python<'_>,
        base_url: Option<String>,
        headers: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: bool,
        http2: bool,
        block_private_redirects: bool,
    ) -> PyResult<Self> {
        let _ = http2;
        let default_headers = match headers {
            None => PyHeaders::new_empty(),
            Some(h) => PyHeaders::from_pyobject(py, h)?,
        };

        let py_timeout = match timeout {
            None => crate::config::PyTimeout::new(Some(5.0), None, None, None, None),
            Some(t) => {
                let bound = t.bind(py);
                if let Ok(pt) = bound.extract::<PyRef<PyTimeout>>() {
                    pt.clone()
                } else if let Ok(f) = bound.extract::<f64>() {
                    crate::config::PyTimeout::new(Some(f), None, None, None, None)
                } else {
                    crate::config::PyTimeout::new(Some(5.0), None, None, None, None)
                }
            }
        };

        let redirect_policy = make_redirect_policy(follow_redirects, block_private_redirects);

        let mut client_builder = reqwest::Client::builder()
            .redirect(redirect_policy)
            .cookie_store(true);

        if let Some(ct) = py_timeout.connect {
            client_builder = client_builder.connect_timeout(Duration::from_secs_f64(ct));
        }

        let inner = client_builder
            .build()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

        Ok(PyAsyncClient {
            inner: Some(inner),
            base_url,
            default_headers,
            timeout: py_timeout,
            follow_redirects,
            block_private_redirects,
        })
    }

    fn request<'py>(
        &self,
        py: Python<'py>,
        method: String,
        url: String,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.get_client()?;
        let full_url = self.resolve_url(&url);

        let extra_headers = match headers {
            None => None,
            Some(h) => Some(PyHeaders::from_pyobject(py, h)?),
        };

        let mut merged_headers = self.default_headers.clone();
        if let Some(h) = extra_headers {
            for (k, v) in h.inner {
                merged_headers.inner.retain(|(ek, _)| ek != &k);
                merged_headers.inner.push((k, v));
            }
        }

        let auth_header: Option<String> = match auth {
            Some(ref a) => {
                if let Ok(basic) = a.bind(py).extract::<PyRef<PyBasicAuth>>() {
                    Some(basic.authorization_header().to_string())
                } else if let Ok((user, pass)) = a.bind(py).extract::<(String, String)>() {
                    let ba = PyBasicAuth::new(&user, &pass);
                    Some(ba.authorization_header().to_string())
                } else {
                    None
                }
            }
            None => None,
        };

        let body_bytes: Option<Vec<u8>> = if let Some(bytes) = content {
            Some(bytes)
        } else if let Some(ref json_obj) = json {
            let json_mod = py.import("json")?;
            let json_str: String = json_mod
                .call_method1("dumps", (json_obj.bind(py),))?
                .extract()?;
            Some(json_str.into_bytes())
        } else {
            None
        };

        let content_type: Option<String> = if json.is_some() {
            Some("application/json".to_string())
        } else {
            None
        };

        let req_timeout = timeout.or(self.timeout.read).map(Duration::from_secs_f64);

        let headers_vec: Vec<(String, String)> = merged_headers.inner.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = client.request(
                reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
                    .map_err(|_| PyValueError::new_err("Invalid method"))?,
                &full_url,
            );

            for (k, v) in &headers_vec {
                builder = builder.header(k.as_str(), v.as_str());
            }

            if let Some(ref ct) = content_type {
                builder = builder.header("content-type", ct.as_str());
            }

            if let Some(body) = body_bytes {
                builder = builder.body(body);
            }

            if let Some(ref header_val) = auth_header {
                builder = builder.header("authorization", header_val.as_str());
            }

            if let Some(dur) = req_timeout {
                builder = builder.timeout(dur);
            }

            let start = Instant::now();
            let response = builder.send().await.map_err(crate::map_reqwest_error)?;
            let elapsed = start.elapsed().as_millis();
            convert_async_response(response, elapsed, None).await
        })
    }

    #[pyo3(signature = (url, *, content = None, json = None, headers = None, auth = None, timeout = None))]
    pub fn get<'py>(
        &self,
        py: Python<'py>,
        url: String,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "GET".to_string(),
            url,
            content,
            json,
            headers,
            auth,
            timeout,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, headers = None, auth = None, timeout = None))]
    pub fn post<'py>(
        &self,
        py: Python<'py>,
        url: String,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "POST".to_string(),
            url,
            content,
            json,
            headers,
            auth,
            timeout,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, headers = None, auth = None, timeout = None))]
    pub fn put<'py>(
        &self,
        py: Python<'py>,
        url: String,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "PUT".to_string(),
            url,
            content,
            json,
            headers,
            auth,
            timeout,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, headers = None, auth = None, timeout = None))]
    pub fn patch<'py>(
        &self,
        py: Python<'py>,
        url: String,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "PATCH".to_string(),
            url,
            content,
            json,
            headers,
            auth,
            timeout,
        )
    }

    #[pyo3(signature = (url, *, content = None, json = None, headers = None, auth = None, timeout = None))]
    pub fn delete<'py>(
        &self,
        py: Python<'py>,
        url: String,
        content: Option<Vec<u8>>,
        json: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "DELETE".to_string(),
            url,
            content,
            json,
            headers,
            auth,
            timeout,
        )
    }

    #[pyo3(signature = (url, *, headers = None, auth = None, timeout = None))]
    pub fn head<'py>(
        &self,
        py: Python<'py>,
        url: String,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "HEAD".to_string(),
            url,
            None,
            None,
            headers,
            auth,
            timeout,
        )
    }

    #[pyo3(signature = (url, *, headers = None, auth = None, timeout = None))]
    pub fn options<'py>(
        &self,
        py: Python<'py>,
        url: String,
        headers: Option<Py<PyAny>>,
        auth: Option<Py<PyAny>>,
        timeout: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.request(
            py,
            "OPTIONS".to_string(),
            url,
            None,
            None,
            headers,
            auth,
            timeout,
        )
    }

    pub fn close(&mut self) {
        self.inner = None;
    }

    fn __aenter__<'py>(slf: Py<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let slf_clone = slf.clone_ref(py);
        pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(slf_clone) })
    }

    fn __aexit__<'py>(
        &mut self,
        py: Python<'py>,
        _exc_type: Option<Py<PyAny>>,
        _exc_val: Option<Py<PyAny>>,
        _exc_tb: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.inner = None;
        pyo3_async_runtimes::tokio::future_into_py(py, async { Ok(false) })
    }
}
