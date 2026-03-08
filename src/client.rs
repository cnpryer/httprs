use crate::auth::{PyBasicAuth, PyDigestAuth};
use crate::config::PyTimeout;
use crate::models::{version_str, PyHeaders, PyRequest, PyResponse, ResponseStream};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyDict, PyList, PyTuple};
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
        if let Ok(dict) = bound.cast::<pyo3::types::PyDict>() {
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
        if let Ok(seq) = bound.extract::<Vec<(String, String)>>() {
            return Ok(RequestBody::Form(seq));
        }
        return Err(pyo3::exceptions::PyTypeError::new_err(
            "data must be a dict, bytes, or list of (str, str) pairs",
        ));
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

fn append_bytes_chunk(item: &Bound<'_, PyAny>, out: &mut Vec<u8>) -> PyResult<()> {
    if let Ok(bytes) = item.cast::<PyBytes>() {
        out.extend_from_slice(bytes.as_bytes());
    } else if let Ok(bytearray) = item.cast::<PyByteArray>() {
        out.extend_from_slice(&bytearray.to_vec());
    } else if let Ok(text) = item.extract::<String>() {
        out.extend_from_slice(text.as_bytes());
    } else if let Ok(chunk) = item.extract::<Vec<u8>>() {
        out.extend_from_slice(&chunk);
    } else {
        return Err(pyo3::exceptions::PyTypeError::new_err(
            "content iterator items must be bytes, bytearray, or str",
        ));
    }
    Ok(())
}

fn collect_content_bytes(py: Python<'_>, content: Option<Py<PyAny>>) -> PyResult<Vec<u8>> {
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
        append_bytes_chunk(&item, &mut out)?;
    }
    Ok(out)
}

fn timeout_config_from_arg(
    py: Python<'_>,
    timeout: Option<Py<PyAny>>,
    default: &PyTimeout,
) -> PyTimeout {
    match timeout {
        None => default.clone(),
        Some(t) => {
            let bound = t.bind(py);
            if let Ok(f) = bound.extract::<f64>() {
                PyTimeout::new(Some(f), None, None, None, None)
            } else if let Ok(pt) = bound.extract::<PyRef<PyTimeout>>() {
                pt.clone()
            } else {
                default.clone()
            }
        }
    }
}

fn immediate_awaitable<'py>(py: Python<'py>, value: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move { Ok(value) })
}

fn append_query_string(url: &str, query: &str) -> String {
    if query.is_empty() {
        return url.to_string();
    }
    let sep = if url.contains('?') { "&" } else { "?" };
    format!("{url}{sep}{query}")
}

fn params_to_query(py: Python<'_>, params: Option<Py<PyAny>>) -> PyResult<Option<String>> {
    let Some(params) = params else {
        return Ok(None);
    };
    let bound = params.bind(py);
    if bound.is_none() {
        return Ok(None);
    }
    if let Ok(s) = bound.extract::<String>() {
        return Ok(if s.is_empty() { None } else { Some(s) });
    }
    if let Ok(d) = bound.cast::<PyDict>() {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in d.iter() {
            let key: String = k.extract()?;
            let value: String = v.extract()?;
            ser.append_pair(&key, &value);
        }
        let q = ser.finish();
        return Ok(if q.is_empty() { None } else { Some(q) });
    }
    if let Ok(l) = bound.cast::<PyList>() {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for item in l.iter() {
            let (k, v): (String, String) = item.extract()?;
            ser.append_pair(&k, &v);
        }
        let q = ser.finish();
        return Ok(if q.is_empty() { None } else { Some(q) });
    }
    Ok(None)
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
                || addr.is_unspecified()
                || (addr.segments()[0] & 0xffc0 == 0xfe80) // link-local fe80::/10
                || (addr.segments()[0] & 0xfe00 == 0xfc00) // unique-local fc00::/7
                || (addr.segments()[0] & 0xff00 == 0xff00) // multicast ff00::/8
                || addr.to_ipv4_mapped().is_some_and(|v4| {
                    v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
                })
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

#[pyclass(name = "Client", subclass)]
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
    transport: Option<Py<PyAny>>,
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
        transport = None,
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
        transport: Option<Py<PyAny>>,
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
            transport,
        })
    }

    // TODO(cnpryer):
    //
    // ```python
    // class Client(httprs.Client):
    //     def __init__(self, **kwargs) -> None:
    //         kwargs.setdefault("timeout", DEFAULT_TIMEOUT)
    //         kwargs.setdefault("limits", DEFAULT_CONNECTION_LIMITS)
    //         kwargs.setdefault("follow_redirects", True)
    //         super().__init__(**kwargs)
    // ```
    #[pyo3(signature = (*_args, **_kwargs))]
    fn __init__(
        _slf: &Bound<'_, Self>,
        _args: &Bound<'_, PyTuple>,
        _kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        Ok(())
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

    #[pyo3(signature = (
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

    #[pyo3(signature = (
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

    #[pyo3(signature = (
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

    #[pyo3(signature = (
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

    #[pyo3(signature = (
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

    #[pyo3(signature = (
        url,
        *,
        headers = None,
        auth = None,
        timeout = None,
        follow_redirects = None,
    ))]
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

    #[pyo3(signature = (
        url,
        *,
        headers = None,
        auth = None,
        timeout = None,
        follow_redirects = None,
    ))]
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
    #[pyo3(signature = (
        method,
        url,
        *,
        content = None,
        json = None,
        data = None,
        files = None,
        headers = None,
        params = None,
        timeout = None,
        extensions = None,
    ))]
    pub fn build_request(
        &self,
        py: Python<'_>,
        method: &str,
        url: Py<PyAny>,
        content: Option<Py<PyAny>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        files: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        params: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        extensions: Option<Py<PyAny>>,
    ) -> PyResult<PyRequest> {
        let _ = files;
        let url_value = {
            let bound = url.bind(py);
            if let Ok(url_str) = bound.extract::<String>() {
                url_str
            } else if bound.hasattr("__str__")? {
                bound.str()?.extract()?
            } else {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "url must be a str or URL instance",
                ));
            }
        };
        let mut full_url = self.resolve_url(&url_value);
        if let Some(query) = params_to_query(py, params)? {
            full_url = append_query_string(&full_url, &query);
        }

        let mut merged_headers = self.default_headers.clone();

        if let Some(h) = headers {
            let extra = PyHeaders::from_pyobject(py, h)?;
            for (k, v) in extra.inner {
                merged_headers.inner.retain(|(ek, _)| ek != &k);
                merged_headers.inner.push((k, v));
            }
        }

        let has_content = content.is_some();
        let mut body_content = collect_content_bytes(py, content)?;
        if !has_content {
            if let Some(json_obj) = json {
                let json_mod = py.import("json")?;
                let json_str: String = json_mod
                    .call_method1("dumps", (json_obj.bind(py),))?
                    .extract()?;
                body_content = json_str.into_bytes();
                if merged_headers.get("content-type", None).is_none() {
                    merged_headers
                        .inner
                        .push(("content-type".to_string(), "application/json".to_string()));
                }
            } else if let Some(data_obj) = data {
                let body = build_body(py, None, None, Some(data_obj))?;
                body_content = match body {
                    RequestBody::Empty => Vec::new(),
                    RequestBody::Bytes(b) => b,
                    RequestBody::Json(s) => s.into_bytes(),
                    RequestBody::Form(pairs) => {
                        if merged_headers.get("content-type", None).is_none() {
                            merged_headers.inner.push((
                                "content-type".to_string(),
                                "application/x-www-form-urlencoded".to_string(),
                            ));
                        }
                        form_encode_pairs(&pairs).into_bytes()
                    }
                };
            }
        }

        let timeout_cfg = timeout_config_from_arg(py, timeout, &self.timeout);
        let ext_dict = PyDict::new(py);
        if let Some(ext) = extensions {
            let bound = ext.bind(py);
            if let Ok(d) = bound.cast::<PyDict>() {
                for (k, v) in d.iter() {
                    ext_dict.set_item(k, v)?;
                }
            }
        }
        let timeout_dict = PyDict::new(py);
        timeout_dict.set_item("connect", timeout_cfg.connect)?;
        timeout_dict.set_item("read", timeout_cfg.read)?;
        timeout_dict.set_item("write", timeout_cfg.write)?;
        timeout_dict.set_item("pool", timeout_cfg.pool)?;
        ext_dict.set_item("timeout", timeout_dict)?;

        let headers_obj: Py<PyHeaders> = Py::new(py, merged_headers)?;
        PyRequest::new(
            py,
            method,
            full_url.into_pyobject(py)?.into_any().unbind(),
            Some(headers_obj.into_bound(py).into_any().unbind()),
            Some(PyBytes::new(py, &body_content).into_any().unbind()),
            Some(ext_dict.into_any().unbind()),
        )
    }

    /// Send a pre-built Request.
    #[pyo3(signature = (
        request,
        *,
        stream = false,
        auth = None,
        follow_redirects = None,
    ))]
    pub fn send<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        request: &PyRequest,
        stream: bool,
        auth: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let _ = auth;
        let _ = follow_redirects;

        let request_obj = Py::new(py, request.clone())?;
        let transport_obj: Option<Py<PyAny>> = {
            let this = slf.borrow();
            this.transport.as_ref().map(|t| t.clone_ref(py))
        };
        if let Some(transport) = transport_obj {
            let transport_bound = transport.into_bound(py).into_any();
            if transport_bound.hasattr("handle_request")? {
                return transport_bound
                    .call_method1("handle_request", (request_obj.clone_ref(py),));
            }
        }

        let this = slf.borrow();
        let client = this.get_client()?.clone();
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
        let py_response = if stream {
            PyResponse::from_blocking_stream(response, elapsed, Some(request_obj.clone_ref(py)))
        } else {
            PyResponse::from_blocking(response, elapsed, Some(request_obj.clone_ref(py)))?
        };
        let response_obj = Py::new(py, py_response)?;
        Ok(response_obj.into_bound(py).into_any())
    }

    /// Return a context manager for streaming the response.
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
    ))]
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

    #[getter]
    pub fn is_closed(&self) -> bool {
        self.inner.is_none()
    }

    #[getter]
    pub fn timeout(&self) -> PyTimeout {
        self.timeout.clone()
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

#[pyclass(name = "AsyncClient", subclass)]
pub struct PyAsyncClient {
    inner: Option<reqwest::Client>,
    base_url: Option<String>,
    default_headers: PyHeaders,
    timeout: PyTimeout,
    #[allow(dead_code)]
    follow_redirects: bool,
    #[allow(dead_code)]
    block_private_redirects: bool,
    #[allow(dead_code)]
    transport: Option<Py<PyAny>>,
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
        extensions: None,
        stream: None,
        py_stream: None,
    })
}

#[pymethods]
impl PyAsyncClient {
    #[new]
    #[pyo3(signature = (
        base_url = None,
        headers = None,
        timeout = None,
        transport = None,
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
        transport: Option<Py<PyAny>>,
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
            transport,
        })
    }

    // TODO(cnpryer):
    //
    // ```python
    // class Client(httprs.AsyncClient):
    //     def __init__(self, **kwargs) -> None:
    //         kwargs.setdefault("timeout", DEFAULT_TIMEOUT)
    //         kwargs.setdefault("limits", DEFAULT_CONNECTION_LIMITS)
    //         kwargs.setdefault("follow_redirects", True)
    //         super().__init__(**kwargs)
    // ```
    #[pyo3(signature = (*_args, **_kwargs))]
    fn __init__(
        _slf: &Bound<'_, Self>,
        _args: &Bound<'_, PyTuple>,
        _kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        Ok(())
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

    #[pyo3(signature = (
        url,
        *,
        content = None,
        json = None,
        headers = None,
        auth = None,
        timeout = None,
    ))]
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

    #[pyo3(signature = (
        url,
        *,
        content = None,
        json = None,
        headers = None,
        auth = None,
        timeout = None,
    ))]
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

    #[pyo3(signature = (
        url,
        *,
        content = None,
        json = None,
        headers = None,
        auth = None,
        timeout = None,
    ))]
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

    #[pyo3(signature = (
        url,
        *,
        content = None,
        json = None,
        headers = None,
        auth = None,
        timeout = None,
    ))]
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

    #[pyo3(signature = (
        url,
        *,
        content = None,
        json = None,
        headers = None,
        auth = None,
        timeout = None,
    ))]
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

    #[pyo3(signature = (
        method,
        url,
        *,
        content = None,
        json = None,
        data = None,
        files = None,
        headers = None,
        params = None,
        timeout = None,
        extensions = None,
    ))]
    pub fn build_request(
        &self,
        py: Python<'_>,
        method: &str,
        url: Py<PyAny>,
        content: Option<Py<PyAny>>,
        json: Option<Py<PyAny>>,
        data: Option<Py<PyAny>>,
        files: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        params: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        extensions: Option<Py<PyAny>>,
    ) -> PyResult<PyRequest> {
        let _ = files;
        let url_value = {
            let bound = url.bind(py);
            if let Ok(url_str) = bound.extract::<String>() {
                url_str
            } else if bound.hasattr("__str__")? {
                bound.str()?.extract()?
            } else {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "url must be a str or URL instance",
                ));
            }
        };
        let mut full_url = self.resolve_url(&url_value);
        if let Some(query) = params_to_query(py, params)? {
            full_url = append_query_string(&full_url, &query);
        }

        let mut merged_headers = self.default_headers.clone();
        if let Some(h) = headers {
            let extra = PyHeaders::from_pyobject(py, h)?;
            for (k, v) in extra.inner {
                merged_headers.inner.retain(|(ek, _)| ek != &k);
                merged_headers.inner.push((k, v));
            }
        }

        let has_content = content.is_some();
        let mut body_content = collect_content_bytes(py, content)?;
        if !has_content {
            if let Some(json_obj) = json {
                let json_mod = py.import("json")?;
                let json_str: String = json_mod
                    .call_method1("dumps", (json_obj.bind(py),))?
                    .extract()?;
                body_content = json_str.into_bytes();
                if merged_headers.get("content-type", None).is_none() {
                    merged_headers
                        .inner
                        .push(("content-type".to_string(), "application/json".to_string()));
                }
            } else if let Some(data_obj) = data {
                let body = build_body(py, None, None, Some(data_obj))?;
                body_content = match body {
                    RequestBody::Empty => Vec::new(),
                    RequestBody::Bytes(b) => b,
                    RequestBody::Json(s) => s.into_bytes(),
                    RequestBody::Form(pairs) => {
                        if merged_headers.get("content-type", None).is_none() {
                            merged_headers.inner.push((
                                "content-type".to_string(),
                                "application/x-www-form-urlencoded".to_string(),
                            ));
                        }
                        form_encode_pairs(&pairs).into_bytes()
                    }
                };
            }
        }

        let timeout_cfg = timeout_config_from_arg(py, timeout, &self.timeout);
        let ext_dict = PyDict::new(py);
        if let Some(ext) = extensions {
            let bound = ext.bind(py);
            if let Ok(d) = bound.cast::<PyDict>() {
                for (k, v) in d.iter() {
                    ext_dict.set_item(k, v)?;
                }
            }
        }
        let timeout_dict = PyDict::new(py);
        timeout_dict.set_item("connect", timeout_cfg.connect)?;
        timeout_dict.set_item("read", timeout_cfg.read)?;
        timeout_dict.set_item("write", timeout_cfg.write)?;
        timeout_dict.set_item("pool", timeout_cfg.pool)?;
        ext_dict.set_item("timeout", timeout_dict)?;

        let headers_obj: Py<PyHeaders> = Py::new(py, merged_headers)?;
        PyRequest::new(
            py,
            method,
            full_url.into_pyobject(py)?.into_any().unbind(),
            Some(headers_obj.into_bound(py).into_any().unbind()),
            Some(PyBytes::new(py, &body_content).into_any().unbind()),
            Some(ext_dict.into_any().unbind()),
        )
    }

    #[pyo3(signature = (
        request,
        *,
        stream = false,
        auth = None,
        follow_redirects = None,
    ))]
    pub fn send<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        request: &PyRequest,
        stream: bool,
        auth: Option<Py<PyAny>>,
        follow_redirects: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let _ = auth;
        let _ = follow_redirects;
        let request_obj = Py::new(py, request.clone())?;

        let transport_obj: Option<Py<PyAny>> = {
            let this = slf.borrow();
            this.transport.as_ref().map(|t| t.clone_ref(py))
        };
        if let Some(transport) = transport_obj {
            let transport_bound = transport.into_bound(py).into_any();
            if transport_bound.hasattr("handle_async_request")? {
                return transport_bound
                    .call_method1("handle_async_request", (request_obj.clone_ref(py),));
            }
            if transport_bound.hasattr("handle_request")? {
                let resp =
                    transport_bound.call_method1("handle_request", (request_obj.clone_ref(py),))?;
                return immediate_awaitable(py, resp.unbind());
            }
        }

        let this = slf.borrow();
        let client = this.get_client()?;
        let method_str = request.method.clone();
        let url = request.url.inner.to_string();
        let headers = request.headers.inner.clone();
        let body = if request.content.is_empty() {
            None
        } else {
            Some(request.content.clone())
        };
        let request_obj_stream = request_obj.clone_ref(py);
        let request_obj_regular = request_obj.clone_ref(py);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
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
            let response = builder.send().await.map_err(crate::map_reqwest_error)?;
            let elapsed = start.elapsed().as_millis();
            if stream {
                Ok(PyResponse::from_async_stream(
                    response,
                    elapsed,
                    Some(request_obj_stream),
                ))
            } else {
                convert_async_response(response, elapsed, Some(request_obj_regular)).await
            }
        })
    }

    pub fn close(&mut self) {
        self.inner = None;
    }

    pub fn aclose<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.inner = None;
        immediate_awaitable(py, py.None())
    }

    #[getter]
    pub fn is_closed(&self) -> bool {
        self.inner.is_none()
    }

    #[getter]
    pub fn timeout(&self) -> PyTimeout {
        self.timeout.clone()
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
