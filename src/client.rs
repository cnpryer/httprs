use crate::auth::{PyBasicAuth, PyDigestAuth};
use crate::config::{PyLimits, PyTimeout};
use crate::cookies::PyCookies;
use crate::json::json_dumps;
use crate::models::{
    parse_default_encoding_arg, version_str, PyHeaders, PyRequest, PyResponse, PyURL,
    ResponseStream,
};
use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyDict, PyList, PyTuple};
use std::collections::HashSet;
use std::error::Error as StdError;
use std::path::{Path, PathBuf};
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
        let json_str = json_dumps(json_obj.bind(py))?;
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

fn extract_multipart_boundary(content_type: &str) -> Option<String> {
    let mut parts = content_type.split(';').map(str::trim);
    let media_type = parts.next()?.to_ascii_lowercase();
    if media_type != "multipart/form-data" {
        return None;
    }
    for part in parts {
        if let Some(value) = part.strip_prefix("boundary=") {
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

fn py_value_to_bytes(value: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let mut out = Vec::new();
    append_bytes_chunk(value, &mut out)?;
    Ok(out)
}

fn collect_multipart_fields(
    py: Python<'_>,
    data: Option<Py<PyAny>>,
) -> PyResult<Vec<(String, Vec<u8>)>> {
    let Some(data) = data else {
        return Ok(Vec::new());
    };
    let bound = data.bind(py);
    let mut fields = Vec::new();

    if let Ok(dict) = bound.cast::<PyDict>() {
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            if let Ok(list) = v.cast::<PyList>() {
                for item in list.iter() {
                    fields.push((key.clone(), py_value_to_bytes(&item)?));
                }
            } else {
                fields.push((key, py_value_to_bytes(&v)?));
            }
        }
        return Ok(fields);
    }

    for item in bound.try_iter()? {
        let item = item?;
        let pair = item.cast::<PyTuple>()?;
        if pair.len() != 2 {
            return Err(PyTypeError::new_err(
                "multipart form fields must be 2-tuples",
            ));
        }
        let key: String = pair.get_item(0)?.extract()?;
        let value = pair.get_item(1)?;
        fields.push((key, py_value_to_bytes(&value)?));
    }
    Ok(fields)
}

fn collect_multipart_files(
    py: Python<'_>,
    files: Option<Py<PyAny>>,
) -> PyResult<Vec<(String, String, String, Vec<u8>)>> {
    let Some(files) = files else {
        return Ok(Vec::new());
    };
    let bound = files.bind(py);
    let mut out = Vec::new();

    let mut append_file = |name: String, file_obj: Bound<'_, PyAny>| -> PyResult<()> {
        let file_obj = file_obj.into_any();

        if let Ok(file_tuple) = file_obj.cast::<PyTuple>() {
            if file_tuple.len() < 2 {
                return Err(PyTypeError::new_err(
                    "file tuples must include at least (filename, content)",
                ));
            }
            let filename: String = file_tuple.get_item(0)?.extract()?;
            let content = py_value_to_bytes(&file_tuple.get_item(1)?)?;
            let content_type = if file_tuple.len() >= 3 {
                file_tuple
                    .get_item(2)?
                    .extract::<String>()
                    .unwrap_or_else(|_| "application/octet-stream".to_string())
            } else {
                "application/octet-stream".to_string()
            };
            out.push((name, filename, content_type, content));
        } else {
            let content = py_value_to_bytes(&file_obj)?;
            out.push((
                name,
                "upload".to_string(),
                "application/octet-stream".to_string(),
                content,
            ));
        }
        Ok(())
    };

    if let Ok(dict) = bound.cast::<PyDict>() {
        for (k, v) in dict.iter() {
            let name: String = k.extract()?;
            append_file(name, v)?;
        }
        return Ok(out);
    }

    for entry in bound.try_iter()? {
        let entry = entry?;
        let tuple = entry.cast::<PyTuple>()?;
        if tuple.len() != 2 {
            return Err(PyTypeError::new_err(
                "files must be a sequence of (name, file) pairs",
            ));
        }
        let name: String = tuple.get_item(0)?.extract()?;
        let file_obj = tuple.get_item(1)?;
        append_file(name, file_obj)?;
    }
    Ok(out)
}

fn build_multipart_body(
    py: Python<'_>,
    data: Option<Py<PyAny>>,
    files: Option<Py<PyAny>>,
    boundary: &str,
) -> PyResult<Vec<u8>> {
    let fields = collect_multipart_fields(py, data)?;
    let file_parts = collect_multipart_files(py, files)?;
    let mut body = Vec::new();

    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(&value);
        body.extend_from_slice(b"\r\n");
    }

    for (name, filename, content_type, content) in file_parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(&content);
        body.extend_from_slice(b"\r\n");
    }

    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(body)
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

fn parse_query_pairs(query: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn merge_query_pairs(
    mut base: Vec<(String, String)>,
    incoming: Vec<(String, String)>,
) -> Vec<(String, String)> {
    if incoming.is_empty() {
        return base;
    }
    let incoming_keys: HashSet<&str> = incoming.iter().map(|(k, _)| k.as_str()).collect();
    base.retain(|(k, _)| !incoming_keys.contains(k.as_str()));
    base.extend(incoming);
    base
}

fn encode_query_pairs(pairs: &[(String, String)]) -> Option<String> {
    if pairs.is_empty() {
        return None;
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    let query = serializer.finish();
    if query.is_empty() {
        None
    } else {
        Some(query)
    }
}

fn merge_url_query(url: &str, default_query: Option<&str>, request_query: Option<&str>) -> String {
    if default_query.is_none() && request_query.is_none() {
        return url.to_string();
    }

    let (url_without_fragment, fragment) = match url.split_once('#') {
        Some((head, tail)) => (head, Some(tail)),
        None => (url, None),
    };
    let (base, existing_query) = match url_without_fragment.split_once('?') {
        Some((head, tail)) => (head, Some(tail)),
        None => (url_without_fragment, None),
    };

    let mut merged = default_query
        .filter(|q| !q.is_empty())
        .map(parse_query_pairs)
        .unwrap_or_default();

    if let Some(query) = existing_query.filter(|q| !q.is_empty()) {
        merged = merge_query_pairs(merged, parse_query_pairs(query));
    }

    if let Some(query) = request_query.filter(|q| !q.is_empty()) {
        merged = merge_query_pairs(merged, parse_query_pairs(query));
    }

    let mut merged_url = base.to_string();
    if let Some(query) = encode_query_pairs(&merged) {
        merged_url.push('?');
        merged_url.push_str(&query);
    }
    if let Some(fragment) = fragment {
        merged_url.push('#');
        merged_url.push_str(fragment);
    }
    merged_url
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

#[derive(Default)]
struct CookieBindingState {
    pending_pairs: Vec<(String, String)>,
    bound_origin: Option<String>,
}

fn cookie_origin_key(url: &url::Url) -> Option<String> {
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    Some(format!("{}://{}:{}", url.scheme(), host, port))
}

fn validate_cookie_pair(name: &str, value: &str) -> PyResult<()> {
    if name.is_empty() {
        return Err(PyValueError::new_err("cookie name cannot be empty"));
    }
    if name
        .chars()
        .any(|c| c.is_ascii_control() || c == ';' || c == '=' || c.is_ascii_whitespace())
    {
        return Err(PyValueError::new_err(
            "cookie name contains invalid characters",
        ));
    }
    if value.chars().any(|c| c.is_ascii_control() || c == ';') {
        return Err(PyValueError::new_err(
            "cookie value contains invalid characters",
        ));
    }
    Ok(())
}

fn parse_cookie_header_pairs(raw: &str) -> PyResult<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for segment in raw.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let (name, value) = segment
            .split_once('=')
            .ok_or_else(|| PyTypeError::new_err("cookies string must be 'name=value' pairs"))?;
        let name = name.trim();
        let value = value.trim();
        validate_cookie_pair(name, value)?;
        pairs.push((name.to_string(), value.to_string()));
    }
    Ok(pairs)
}

fn parse_cookies_arg(
    py: Python<'_>,
    cookies: Option<Py<PyAny>>,
) -> PyResult<Vec<(String, String)>> {
    let Some(cookies) = cookies else {
        return Ok(Vec::new());
    };
    let bound = cookies.bind(py);
    if bound.is_none() {
        return Ok(Vec::new());
    }

    if let Ok(raw) = bound.extract::<String>() {
        return parse_cookie_header_pairs(&raw);
    }

    if let Ok(parsed) = bound.extract::<PyRef<PyCookies>>() {
        let mut out = Vec::new();
        for (name, value) in parsed.items() {
            validate_cookie_pair(&name, &value)?;
            out.push((name, value));
        }
        return Ok(out);
    }

    let mut pairs: Vec<(String, String)> = Vec::new();

    if let Ok(dict) = bound.cast::<PyDict>() {
        for (k, v) in dict.iter() {
            pairs.push((k.extract()?, v.extract()?));
        }
        for (name, value) in &pairs {
            validate_cookie_pair(name, value)?;
        }
        return Ok(pairs);
    }

    if let Ok(list) = bound.cast::<PyList>() {
        for item in list.iter() {
            let (k, v): (String, String) = item.extract()?;
            pairs.push((k, v));
        }
        for (name, value) in &pairs {
            validate_cookie_pair(name, value)?;
        }
        return Ok(pairs);
    }

    for item in bound.try_iter()? {
        let (k, v): (String, String) = item?.extract()?;
        pairs.push((k, v));
    }
    for (name, value) in &pairs {
        validate_cookie_pair(name, value)?;
    }
    Ok(pairs)
}

fn bind_pending_cookies_to_url(
    cookie_jar: &Arc<reqwest::cookie::Jar>,
    state: &mut CookieBindingState,
    url: &url::Url,
) {
    let Some(origin) = cookie_origin_key(url) else {
        return;
    };
    if let Some(bound) = &state.bound_origin {
        if bound != &origin {
            return;
        }
    } else {
        state.bound_origin = Some(origin);
    }
    for (name, value) in state.pending_pairs.drain(..) {
        cookie_jar.add_cookie_str(&format!("{name}={value}"), url);
    }
}

fn bind_default_cookies_for_url(
    cookie_jar: &Arc<reqwest::cookie::Jar>,
    cookie_state: &Arc<Mutex<CookieBindingState>>,
    url: &str,
) {
    let Ok(parsed_url) = url::Url::parse(url) else {
        return;
    };
    let mut state = cookie_state.lock().unwrap();
    bind_pending_cookies_to_url(cookie_jar, &mut state, &parsed_url);
}

enum AuthKind {
    Basic(String),
    Digest(Py<PyDigestAuth>),
}

#[derive(Default)]
struct EventHooks {
    request: Vec<Py<PyAny>>,
    response: Vec<Py<PyAny>>,
}

struct MountTransport {
    prefix: String,
    transport: Py<PyAny>,
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

fn clone_auth_kind(py: Python<'_>, auth: &AuthKind) -> AuthKind {
    match auth {
        AuthKind::Basic(header) => AuthKind::Basic(header.clone()),
        AuthKind::Digest(digest) => AuthKind::Digest(digest.clone_ref(py)),
    }
}

fn parse_event_hook_list(hooks_obj: Bound<'_, PyAny>, hook_type: &str) -> PyResult<Vec<Py<PyAny>>> {
    if hooks_obj.is_none() {
        return Ok(Vec::new());
    }
    let iter = hooks_obj.try_iter().map_err(|_| {
        PyTypeError::new_err(format!(
            "event_hooks['{hook_type}'] must be an iterable of callables"
        ))
    })?;
    let mut hooks = Vec::new();
    for hook in iter {
        let hook = hook?;
        if !hook.is_callable() {
            return Err(PyTypeError::new_err(format!(
                "event_hooks['{hook_type}'] entries must be callable"
            )));
        }
        hooks.push(hook.unbind());
    }
    Ok(hooks)
}

fn parse_event_hooks_arg(py: Python<'_>, event_hooks: Option<Py<PyAny>>) -> PyResult<EventHooks> {
    let Some(event_hooks) = event_hooks else {
        return Ok(EventHooks::default());
    };
    let bound = event_hooks.bind(py);
    if bound.is_none() {
        return Ok(EventHooks::default());
    }

    let request_hooks_obj = bound
        .call_method1("get", ("request", PyList::empty(py)))
        .map_err(|_| {
            PyTypeError::new_err(
                "event_hooks must be a mapping with optional 'request' and 'response' entries",
            )
        })?;
    let response_hooks_obj = bound
        .call_method1("get", ("response", PyList::empty(py)))
        .map_err(|_| {
            PyTypeError::new_err(
                "event_hooks must be a mapping with optional 'request' and 'response' entries",
            )
        })?;

    Ok(EventHooks {
        request: parse_event_hook_list(request_hooks_obj, "request")?,
        response: parse_event_hook_list(response_hooks_obj, "response")?,
    })
}

fn clone_hooks(py: Python<'_>, hooks: &[Py<PyAny>]) -> Vec<Py<PyAny>> {
    hooks.iter().map(|hook| hook.clone_ref(py)).collect()
}

fn run_sync_request_hooks(
    py: Python<'_>,
    hooks: &[Py<PyAny>],
    request: &Py<PyRequest>,
) -> PyResult<()> {
    for hook in hooks {
        hook.bind(py).call1((request.clone_ref(py),))?;
    }
    Ok(())
}

fn run_sync_response_hooks(
    py: Python<'_>,
    hooks: &[Py<PyAny>],
    response: &Py<PyAny>,
) -> PyResult<()> {
    for hook in hooks {
        hook.bind(py).call1((response.clone_ref(py),))?;
    }
    Ok(())
}

async fn run_async_request_hooks(hooks: Vec<Py<PyAny>>, request: Py<PyRequest>) -> PyResult<()> {
    for hook in hooks {
        let hook_call = Python::attach(|py| {
            let hook_result = hook.bind(py).call1((request.clone_ref(py),))?;
            pyo3_async_runtimes::tokio::into_future(hook_result)
        })?;
        let _ = hook_call.await?;
    }
    Ok(())
}

async fn run_async_response_hooks(hooks: Vec<Py<PyAny>>, response: Py<PyAny>) -> PyResult<()> {
    for hook in hooks {
        let hook_call = Python::attach(|py| {
            let hook_result = hook.bind(py).call1((response.clone_ref(py),))?;
            pyo3_async_runtimes::tokio::into_future(hook_result)
        })?;
        let _ = hook_call.await?;
    }
    Ok(())
}

fn body_bytes_for_hook_request(body: &RequestBody) -> Vec<u8> {
    match body {
        RequestBody::Empty => Vec::new(),
        RequestBody::Bytes(bytes) => bytes.clone(),
        RequestBody::Json(json) => json.as_bytes().to_vec(),
        RequestBody::Form(pairs) => form_encode_pairs(pairs).into_bytes(),
    }
}

fn body_content_type_for_hook_request(body: &RequestBody) -> Option<&'static str> {
    match body {
        RequestBody::Json(_) => Some("application/json"),
        RequestBody::Form(_) => Some("application/x-www-form-urlencoded"),
        RequestBody::Empty | RequestBody::Bytes(_) => None,
    }
}

fn build_hook_request(
    py: Python<'_>,
    method: &str,
    url: &str,
    headers: PyHeaders,
    content: Vec<u8>,
) -> PyResult<Py<PyRequest>> {
    let request = PyRequest {
        method: method.to_uppercase(),
        url: PyURL::from_str(url)?,
        headers,
        content,
        py_stream: None,
        extensions: PyDict::new(py).into_any().unbind(),
    };
    Py::new(py, request)
}

fn extract_request_parts(
    py: Python<'_>,
    request_obj: &Py<PyRequest>,
) -> PyResult<(String, String, Vec<(String, String)>, Option<Vec<u8>>)> {
    let request = request_obj.bind(py).borrow();
    let body = if request.content.is_empty() && request.py_stream.is_none() {
        None
    } else {
        Some(request.read(py)?)
    };
    Ok((
        request.method.clone(),
        request.url.inner.to_string(),
        request.headers.inner.clone(),
        body,
    ))
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

fn default_limits() -> PyLimits {
    PyLimits::new(Some(100), Some(20), Some(5.0))
}

fn parse_limits_arg(py: Python<'_>, limits: Option<Py<PyAny>>) -> PyResult<PyLimits> {
    let Some(limits) = limits else {
        return Ok(default_limits());
    };
    let bound = limits.bind(py);
    if bound.is_none() {
        return Ok(default_limits());
    }
    if let Ok(parsed) = bound.extract::<PyRef<PyLimits>>() {
        return Ok(parsed.clone());
    }

    let max_connections = bound
        .getattr("max_connections")
        .ok()
        .and_then(|v| v.extract().ok());
    let max_keepalive_connections = bound
        .getattr("max_keepalive_connections")
        .ok()
        .and_then(|v| v.extract().ok());
    let keepalive_expiry = bound
        .getattr("keepalive_expiry")
        .ok()
        .and_then(|v| v.extract().ok());
    Ok(PyLimits::new(
        max_connections,
        max_keepalive_connections,
        keepalive_expiry,
    ))
}

fn parse_proxy_arg(py: Python<'_>, proxy: Option<Py<PyAny>>) -> PyResult<Option<String>> {
    let Some(proxy) = proxy else {
        return Ok(None);
    };
    let bound = proxy.bind(py);
    if bound.is_none() {
        return Ok(None);
    }
    if let Ok(url) = bound.extract::<String>() {
        return Ok(if url.is_empty() { None } else { Some(url) });
    }
    if let Ok(proxy_ref) = bound.extract::<PyRef<crate::proxy::PyProxy>>() {
        let url = proxy_ref.url().to_string();
        return Ok(if url.is_empty() { None } else { Some(url) });
    }
    if let Ok(url_attr) = bound.getattr("url") {
        let url: String = url_attr.extract()?;
        return Ok(if url.is_empty() { None } else { Some(url) });
    }
    Err(PyTypeError::new_err("proxy must be a str, Proxy, or None"))
}

fn extract_path_like(bound: &Bound<'_, PyAny>) -> PyResult<PathBuf> {
    bound.extract::<PathBuf>()
}

fn read_cert_file(path: &Path) -> PyResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        PyValueError::new_err(format!(
            "failed to read client cert file '{}': {e}",
            path.display()
        ))
    })
}

fn parse_cert_arg(py: Python<'_>, cert: Option<Py<PyAny>>) -> PyResult<Option<reqwest::Identity>> {
    let Some(cert) = cert else {
        return Ok(None);
    };
    let bound = cert.bind(py);
    if bound.is_none() {
        return Ok(None);
    }

    let mut pem = if let Ok(bytes) = bound.cast::<PyBytes>() {
        bytes.as_bytes().to_vec()
    } else if let Ok(bytes) = bound.cast::<PyByteArray>() {
        bytes.to_vec()
    } else if let Ok(tuple) = bound.cast::<PyTuple>() {
        if tuple.len() != 2 {
            return Err(PyTypeError::new_err(
                "cert tuple must be (cert_file, key_file)",
            ));
        }
        let cert_path = extract_path_like(&tuple.get_item(0)?)?;
        let key_path = extract_path_like(&tuple.get_item(1)?)?;
        let mut pem = read_cert_file(&cert_path)?;
        if !pem.ends_with(b"\n") {
            pem.push(b'\n');
        }
        pem.extend(read_cert_file(&key_path)?);
        pem
    } else if let Ok(list) = bound.cast::<PyList>() {
        if list.len() != 2 {
            return Err(PyTypeError::new_err(
                "cert list must be [cert_file, key_file]",
            ));
        }
        let cert_path = extract_path_like(&list.get_item(0)?)?;
        let key_path = extract_path_like(&list.get_item(1)?)?;
        let mut pem = read_cert_file(&cert_path)?;
        if !pem.ends_with(b"\n") {
            pem.push(b'\n');
        }
        pem.extend(read_cert_file(&key_path)?);
        pem
    } else {
        let cert_path = extract_path_like(bound)?;
        read_cert_file(&cert_path)?
    };

    let identity = reqwest::Identity::from_pem(&pem);
    // Clear temporary key material as soon as reqwest has parsed it.
    pem.fill(0);
    let identity =
        identity.map_err(|e| PyValueError::new_err(format!("invalid client certificate: {e}")))?;
    Ok(Some(identity))
}

fn mount_prefix_matches(url: &str, prefix: &str) -> bool {
    if !url.starts_with(prefix) {
        return false;
    }
    if url.len() == prefix.len() {
        return true;
    }
    if prefix.ends_with("://")
        || prefix.ends_with('/')
        || prefix.ends_with('?')
        || prefix.ends_with('#')
    {
        return true;
    }
    matches!(url.as_bytes()[prefix.len()], b'/' | b'?' | b'#' | b':')
}

fn parse_mounts_arg(py: Python<'_>, mounts: Option<Py<PyAny>>) -> PyResult<Vec<MountTransport>> {
    let Some(mounts) = mounts else {
        return Ok(Vec::new());
    };
    let bound = mounts.bind(py);
    if bound.is_none() {
        return Ok(Vec::new());
    }

    let mut parsed = Vec::new();
    if let Ok(dict) = bound.cast::<PyDict>() {
        for (k, v) in dict.iter() {
            let prefix: String = k
                .extract()
                .map_err(|_| PyTypeError::new_err("mount keys must be strings"))?;
            if prefix.is_empty() {
                return Err(PyValueError::new_err("mount prefix cannot be empty"));
            }
            if v.is_none() {
                return Err(PyTypeError::new_err("mount transport cannot be None"));
            }
            parsed.push(MountTransport {
                prefix,
                transport: v.unbind(),
            });
        }
        return Ok(parsed);
    }

    let iter = bound.try_iter().map_err(|_| {
        PyTypeError::new_err("mounts must be a mapping or iterable of (prefix, transport) pairs")
    })?;
    for item in iter {
        let item = item?;
        let tuple = item
            .cast::<PyTuple>()
            .map_err(|_| PyTypeError::new_err("mount entries must be (prefix, transport) pairs"))?;
        if tuple.len() != 2 {
            return Err(PyTypeError::new_err(
                "mount entries must be (prefix, transport) pairs",
            ));
        }
        let prefix: String = tuple
            .get_item(0)?
            .extract()
            .map_err(|_| PyTypeError::new_err("mount keys must be strings"))?;
        if prefix.is_empty() {
            return Err(PyValueError::new_err("mount prefix cannot be empty"));
        }
        let transport = tuple.get_item(1)?;
        if transport.is_none() {
            return Err(PyTypeError::new_err("mount transport cannot be None"));
        }
        parsed.push(MountTransport {
            prefix,
            transport: transport.unbind(),
        });
    }
    Ok(parsed)
}

fn select_mounted_transport(
    py: Python<'_>,
    mounts: &[MountTransport],
    url: &str,
) -> Option<Py<PyAny>> {
    let mut selected: Option<&MountTransport> = None;
    for mount in mounts {
        if !mount_prefix_matches(url, &mount.prefix) {
            continue;
        }
        if selected.is_none_or(|current| mount.prefix.len() > current.prefix.len()) {
            selected = Some(mount);
        }
    }
    selected.map(|mount| mount.transport.clone_ref(py))
}

fn parse_verify_arg(py: Python<'_>, verify: Option<Py<PyAny>>) -> bool {
    let Some(verify) = verify else {
        return true;
    };
    let bound = verify.bind(py);
    if bound.is_none() {
        return true;
    }
    bound.extract::<bool>().unwrap_or(true)
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
fn make_redirect_policy(
    follow: bool,
    block_private: bool,
    max_redirects: usize,
) -> reqwest::redirect::Policy {
    if !follow {
        return reqwest::redirect::Policy::none();
    }
    if block_private {
        reqwest::redirect::Policy::custom(move |attempt| {
            if is_private_url(attempt.url()) {
                attempt.error("redirect to private/loopback address blocked (SSRF protection)")
            } else if attempt.previous().len() >= max_redirects {
                attempt.stop()
            } else {
                attempt.follow()
            }
        })
    } else {
        reqwest::redirect::Policy::limited(max_redirects)
    }
}

fn format_reqwest_builder_error(err: reqwest::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(next) = source {
        parts.push(next.to_string());
        source = next.source();
    }
    parts.join(": ")
}

fn build_blocking_client(
    py_timeout: &PyTimeout,
    follow_redirects: bool,
    block_private_redirects: bool,
    max_redirects: usize,
    trust_env: bool,
    verify: bool,
    cert_identity: Option<&reqwest::Identity>,
    proxy: Option<&str>,
    limits: &PyLimits,
    cookie_jar: Arc<reqwest::cookie::Jar>,
) -> PyResult<reqwest::blocking::Client> {
    if !verify && cert_identity.is_some() {
        return Err(PyValueError::new_err(
            "cert cannot be used when verify=False",
        ));
    }
    let redirect_policy =
        make_redirect_policy(follow_redirects, block_private_redirects, max_redirects);
    let mut client_builder = reqwest::blocking::Client::builder()
        .redirect(redirect_policy)
        .cookie_provider(cookie_jar);

    if !trust_env {
        client_builder = client_builder.no_proxy();
    }
    if !verify {
        client_builder = client_builder.danger_accept_invalid_certs(true);
    }
    if let Some(identity) = cert_identity {
        client_builder = client_builder.identity(identity.clone());
    }
    if let Some(proxy_url) = proxy {
        let reqwest_proxy =
            reqwest::Proxy::all(proxy_url).map_err(|e| PyValueError::new_err(e.to_string()))?;
        client_builder = client_builder.proxy(reqwest_proxy);
    }
    if let Some(ct) = py_timeout.connect {
        client_builder = client_builder.connect_timeout(Duration::from_secs_f64(ct));
    }
    if let Some(idle) = limits.keepalive_expiry {
        client_builder = client_builder.pool_idle_timeout(Duration::from_secs_f64(idle));
    }
    if let Some(max_idle) = limits.max_keepalive_connections {
        client_builder = client_builder.pool_max_idle_per_host(max_idle);
    }

    client_builder
        .build()
        .map_err(|e| PyRuntimeError::new_err(format_reqwest_builder_error(e)))
}

fn build_async_client(
    py_timeout: &PyTimeout,
    follow_redirects: bool,
    block_private_redirects: bool,
    max_redirects: usize,
    trust_env: bool,
    verify: bool,
    cert_identity: Option<&reqwest::Identity>,
    proxy: Option<&str>,
    limits: &PyLimits,
    cookie_jar: Arc<reqwest::cookie::Jar>,
) -> PyResult<reqwest::Client> {
    if !verify && cert_identity.is_some() {
        return Err(PyValueError::new_err(
            "cert cannot be used when verify=False",
        ));
    }
    let redirect_policy =
        make_redirect_policy(follow_redirects, block_private_redirects, max_redirects);
    let mut client_builder = reqwest::Client::builder()
        .redirect(redirect_policy)
        .cookie_provider(cookie_jar);

    if !trust_env {
        client_builder = client_builder.no_proxy();
    }
    if !verify {
        client_builder = client_builder.danger_accept_invalid_certs(true);
    }
    if let Some(identity) = cert_identity {
        client_builder = client_builder.identity(identity.clone());
    }
    if let Some(proxy_url) = proxy {
        let reqwest_proxy =
            reqwest::Proxy::all(proxy_url).map_err(|e| PyValueError::new_err(e.to_string()))?;
        client_builder = client_builder.proxy(reqwest_proxy);
    }
    if let Some(ct) = py_timeout.connect {
        client_builder = client_builder.connect_timeout(Duration::from_secs_f64(ct));
    }
    if let Some(idle) = limits.keepalive_expiry {
        client_builder = client_builder.pool_idle_timeout(Duration::from_secs_f64(idle));
    }
    if let Some(max_idle) = limits.max_keepalive_connections {
        client_builder = client_builder.pool_max_idle_per_host(max_idle);
    }

    client_builder
        .build()
        .map_err(|e| PyRuntimeError::new_err(format_reqwest_builder_error(e)))
}

#[pyclass(name = "Client", subclass)]
pub struct PyClient {
    inner: Option<reqwest::blocking::Client>,
    base_url: Option<String>,
    default_query: Option<String>,
    cookie_jar: Arc<reqwest::cookie::Jar>,
    cookie_state: Arc<Mutex<CookieBindingState>>,
    cert_identity: Option<reqwest::Identity>,
    default_headers: PyHeaders,
    timeout: PyTimeout,
    follow_redirects: bool,
    block_private_redirects: bool,
    max_redirects: usize,
    trust_env: bool,
    verify: bool,
    proxy: Option<String>,
    limits: PyLimits,
    http1: bool,
    http2: bool,
    default_auth: Option<AuthKind>,
    transport: Option<Py<PyAny>>,
    mounts: Vec<MountTransport>,
    event_hooks: EventHooks,
    default_encoding: Option<Py<PyAny>>,
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

    fn bind_default_cookies_for_url(&self, url: &str) {
        bind_default_cookies_for_url(&self.cookie_jar, &self.cookie_state, url);
    }

    fn transport_for_url(&self, py: Python<'_>, url: &str) -> Option<Py<PyAny>> {
        select_mounted_transport(py, &self.mounts, url).or_else(|| {
            self.transport
                .as_ref()
                .map(|transport| transport.clone_ref(py))
        })
    }
}

#[pymethods]
impl PyClient {
    #[new]
    #[pyo3(signature = (
        *,
        auth = None,
        params = None,
        headers = None,
        cookies = None,
        verify = None,
        cert = None,
        trust_env = true,
        http1 = true,
        http2 = false,
        proxy = None,
        mounts = None,
        timeout = None,
        follow_redirects = false,
        limits = None,
        max_redirects = 20,
        event_hooks = None,
        base_url = None,
        transport = None,
        default_encoding = None,
        block_private_redirects = false,
    ))]
    pub fn new(
        py: Python<'_>,
        auth: Option<Py<PyAny>>,
        params: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        cookies: Option<Py<PyAny>>,
        verify: Option<Py<PyAny>>,
        cert: Option<Py<PyAny>>,
        trust_env: bool,
        http1: bool,
        http2: bool,
        proxy: Option<Py<PyAny>>,
        mounts: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: bool,
        limits: Option<Py<PyAny>>,
        max_redirects: usize,
        event_hooks: Option<Py<PyAny>>,
        base_url: Option<String>,
        transport: Option<Py<PyAny>>,
        default_encoding: Option<Py<PyAny>>,
        block_private_redirects: bool,
    ) -> PyResult<Self> {
        let default_encoding = parse_default_encoding_arg(py, default_encoding)?;
        let default_query = params_to_query(py, params)?;
        let cookie_pairs = parse_cookies_arg(py, cookies)?;
        let cert_identity = parse_cert_arg(py, cert)?;
        let mounts = parse_mounts_arg(py, mounts)?;
        let event_hooks = parse_event_hooks_arg(py, event_hooks)?;
        let cookie_jar = Arc::new(reqwest::cookie::Jar::default());
        let cookie_state = Arc::new(Mutex::new(CookieBindingState {
            pending_pairs: cookie_pairs,
            bound_origin: None,
        }));
        if let Some(base) = base_url.as_deref().and_then(|u| url::Url::parse(u).ok()) {
            let mut state = cookie_state.lock().unwrap();
            bind_pending_cookies_to_url(&cookie_jar, &mut state, &base);
        }
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

        let verify = parse_verify_arg(py, verify);
        let proxy = parse_proxy_arg(py, proxy)?;
        let limits = parse_limits_arg(py, limits)?;
        let inner = build_blocking_client(
            &py_timeout,
            follow_redirects,
            block_private_redirects,
            max_redirects,
            trust_env,
            verify,
            cert_identity.as_ref(),
            proxy.as_deref(),
            &limits,
            cookie_jar.clone(),
        )?;

        Ok(PyClient {
            inner: Some(inner),
            base_url,
            default_query,
            cookie_jar,
            cookie_state,
            cert_identity,
            default_headers,
            timeout: py_timeout,
            follow_redirects,
            block_private_redirects,
            max_redirects,
            trust_env,
            verify,
            proxy,
            limits,
            http1,
            http2,
            default_auth,
            transport,
            mounts,
            event_hooks,
            default_encoding,
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
    #[pyo3(signature = (*_args, **kwargs))]
    fn __init__(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        _args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        if let Some(kwargs) = kwargs {
            if let Some(timeout_obj) = kwargs.get_item("timeout")? {
                let timeout = if timeout_obj.is_none() {
                    crate::config::PyTimeout::new(None, None, None, None, None)
                } else if let Ok(pt) = timeout_obj.extract::<PyRef<crate::config::PyTimeout>>() {
                    pt.clone()
                } else if let Ok(f) = timeout_obj.extract::<f64>() {
                    crate::config::PyTimeout::new(Some(f), None, None, None, None)
                } else {
                    crate::config::PyTimeout::new(Some(5.0), None, None, None, None)
                };
                slf.borrow_mut().timeout = timeout;
            }
        }
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
    ) -> PyResult<Py<PyResponse>> {
        let follow = follow_redirects.unwrap_or(self.follow_redirects);
        let client = if follow == self.follow_redirects {
            self.get_client()?.clone()
        } else {
            build_blocking_client(
                &self.timeout,
                follow,
                self.block_private_redirects,
                self.max_redirects,
                self.trust_env,
                self.verify,
                self.cert_identity.as_ref(),
                self.proxy.as_deref(),
                &self.limits,
                self.cookie_jar.clone(),
            )?
        };
        let resolved_url = self.resolve_url(url);
        let full_url = merge_url_query(&resolved_url, self.default_query.as_deref(), None);
        self.bind_default_cookies_for_url(&full_url);

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
        let client_default_encoding = self
            .default_encoding
            .as_ref()
            .map(|encoding| encoding.clone_ref(py));

        let body = build_body(py, content, json, data)?;
        let request_hooks = clone_hooks(py, &self.event_hooks.request);
        let response_hooks = clone_hooks(py, &self.event_hooks.response);

        let mut hook_headers = self.default_headers.clone();
        if let Some(extra) = &extra_headers {
            hook_headers.inner.extend(extra.inner.clone());
        }
        if let Some(content_type) = body_content_type_for_hook_request(&body) {
            hook_headers
                .inner
                .push(("content-type".to_string(), content_type.to_string()));
        }
        if let Some(AuthKind::Basic(header_val)) = effective_auth {
            hook_headers
                .inner
                .push(("authorization".to_string(), header_val.clone()));
        }

        let request_obj = build_hook_request(
            py,
            method,
            &full_url,
            hook_headers,
            body_bytes_for_hook_request(&body),
        )?;
        run_sync_request_hooks(py, &request_hooks, &request_obj)?;

        let send_request = |request_obj: &Py<PyRequest>| -> PyResult<reqwest::blocking::Response> {
            let (method_str, request_url, headers, body_bytes) =
                extract_request_parts(py, request_obj)?;
            self.bind_default_cookies_for_url(&request_url);

            let method = reqwest::Method::from_bytes(method_str.as_bytes())
                .map_err(|_| PyValueError::new_err("Invalid method"))?;
            let mut builder = client.request(method, &request_url);
            for (k, v) in &headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            if let Some(body_bytes) = body_bytes {
                builder = builder.body(body_bytes);
            }
            if let Some(timeout) = req_timeout {
                builder = builder.timeout(timeout);
            }
            crate::without_gil(|| builder.send()).map_err(crate::map_reqwest_error)
        };

        // DigestAuth: two-pass — first request without auth, retry with credentials on 401
        if let Some(AuthKind::Digest(digest_py)) = effective_auth {
            let digest_py = digest_py.clone_ref(py);
            let start = Instant::now();
            let resp = send_request(&request_obj)?;
            let elapsed = start.elapsed().as_millis();
            let first_response = PyResponse::from_blocking(
                resp,
                elapsed,
                Some(request_obj.clone_ref(py)),
                client_default_encoding
                    .as_ref()
                    .map(|encoding| encoding.clone_ref(py)),
            )?;
            let first_response_obj = Py::new(py, first_response)?;
            let first_response_any = first_response_obj.clone_ref(py).into_any();
            run_sync_response_hooks(py, &response_hooks, &first_response_any)?;

            if first_response_obj.bind(py).borrow().status_code == 401 {
                let www_auth = {
                    let response_ref = first_response_obj.bind(py).borrow();
                    response_ref
                        .headers
                        .get("www-authenticate", None)
                        .unwrap_or_default()
                };
                let (method_str, request_url, _, _) = extract_request_parts(py, &request_obj)?;
                let url_str = {
                    // RFC 7616 §3.4: digest-uri is the Request-URI (path + query, no scheme/host)
                    if let Ok(parsed) = url::Url::parse(&request_url) {
                        match parsed.query() {
                            Some(q) => format!("{}?{}", parsed.path(), q),
                            None => parsed.path().to_string(),
                        }
                    } else {
                        request_url
                    }
                };
                let auth_header = {
                    let digest_ref = digest_py.bind(py);
                    let digest = digest_ref.borrow();
                    digest.compute_header(&method_str, &url_str, &www_auth)?
                };

                let second_request_obj = Py::new(py, request_obj.bind(py).borrow().clone())?;
                {
                    // Preserve existing behavior: digest retry does not resend request content.
                    let mut second_request = second_request_obj.bind(py).borrow_mut();
                    second_request.content.clear();
                    second_request.py_stream = None;
                    second_request.set_header("authorization", auth_header.as_str());
                }
                run_sync_request_hooks(py, &request_hooks, &second_request_obj)?;

                let start = Instant::now();
                let resp2 = send_request(&second_request_obj)?;
                let elapsed = start.elapsed().as_millis();
                let second_response = PyResponse::from_blocking(
                    resp2,
                    elapsed,
                    Some(second_request_obj.clone_ref(py)),
                    client_default_encoding
                        .as_ref()
                        .map(|encoding| encoding.clone_ref(py)),
                )?;
                let second_response_obj = Py::new(py, second_response)?;
                let second_response_any = second_response_obj.clone_ref(py).into_any();
                run_sync_response_hooks(py, &response_hooks, &second_response_any)?;
                Ok(second_response_obj)
            } else {
                Ok(first_response_obj)
            }
        } else {
            let start = Instant::now();
            let resp = send_request(&request_obj)?;
            let elapsed = start.elapsed().as_millis();
            let response = PyResponse::from_blocking(
                resp,
                elapsed,
                Some(request_obj.clone_ref(py)),
                client_default_encoding,
            )?;
            let response_obj = Py::new(py, response)?;
            let response_any = response_obj.clone_ref(py).into_any();
            run_sync_response_hooks(py, &response_hooks, &response_any)?;
            Ok(response_obj)
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
    ) -> PyResult<Py<PyResponse>> {
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
    ) -> PyResult<Py<PyResponse>> {
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
    ) -> PyResult<Py<PyResponse>> {
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
    ) -> PyResult<Py<PyResponse>> {
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
    ) -> PyResult<Py<PyResponse>> {
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
    ) -> PyResult<Py<PyResponse>> {
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
    ) -> PyResult<Py<PyResponse>> {
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
        let resolved_url = self.resolve_url(&url_value);
        let request_query = params_to_query(py, params)?;
        let full_url = merge_url_query(
            &resolved_url,
            self.default_query.as_deref(),
            request_query.as_deref(),
        );

        let mut merged_headers = self.default_headers.clone();

        if let Some(h) = headers {
            let extra = PyHeaders::from_pyobject(py, h)?;
            for (k, v) in extra.inner {
                merged_headers.inner.retain(|(ek, _)| ek != &k);
                merged_headers.inner.push((k, v));
            }
        }

        let has_content = content.is_some();
        let mut request_content = content;
        let mut body_content = Vec::new();
        let multipart_boundary = merged_headers
            .get("content-type", None)
            .and_then(|ct| extract_multipart_boundary(&ct));
        if !has_content {
            if files.is_some() || multipart_boundary.is_some() {
                let boundary = if let Some(boundary) = multipart_boundary {
                    boundary
                } else {
                    let boundary = "httprs-boundary".to_string();
                    if merged_headers.get("content-type", None).is_none() {
                        merged_headers.inner.push((
                            "content-type".to_string(),
                            format!("multipart/form-data; boundary={boundary}"),
                        ));
                    }
                    boundary
                };
                let multipart_data = data.or(json);
                body_content = build_multipart_body(py, multipart_data, files, &boundary)?;
            } else if let Some(json_obj) = json {
                let json_str = json_dumps(json_obj.bind(py))?;
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
            request_content = Some(PyBytes::new(py, &body_content).into_any().unbind());
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
            request_content,
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
        let req_auth = match auth {
            Some(ref a) => Some(extract_auth(py, a)?),
            None => None,
        };
        let (default_auth, request_hooks, response_hooks, client_default_encoding) = {
            let this = slf.borrow();
            (
                this.default_auth.as_ref().map(|a| clone_auth_kind(py, a)),
                clone_hooks(py, &this.event_hooks.request),
                clone_hooks(py, &this.event_hooks.response),
                this.default_encoding
                    .as_ref()
                    .map(|encoding| encoding.clone_ref(py)),
            )
        };
        let effective_auth = req_auth.or(default_auth);
        let request_obj = Py::new(py, request.clone())?;
        if let Some(AuthKind::Basic(header_val)) = &effective_auth {
            let mut req_mut = request_obj.bind(py).borrow_mut();
            req_mut.set_header("authorization", header_val);
        }
        run_sync_request_hooks(py, &request_hooks, &request_obj)?;

        let request_url = {
            let req_ref = request_obj.bind(py).borrow();
            req_ref.url.inner.to_string()
        };
        let transport_obj = {
            let this = slf.borrow();
            this.transport_for_url(py, &request_url)
        };
        if let Some(transport) = transport_obj {
            let transport_bound = transport.into_bound(py).into_any();
            if transport_bound.hasattr("handle_request")? {
                let response =
                    transport_bound.call_method1("handle_request", (request_obj.clone_ref(py),))?;
                if let Ok(mut py_response) = response.extract::<PyRefMut<'_, PyResponse>>() {
                    if py_response.request.is_none() {
                        py_response.request = Some(request_obj.clone_ref(py));
                    }
                    py_response.default_encoding = client_default_encoding
                        .as_ref()
                        .map(|encoding| encoding.clone_ref(py));
                }
                let response_obj = response.unbind();
                run_sync_response_hooks(py, &response_hooks, &response_obj)?;
                return Ok(response_obj.into_bound(py).into_any());
            }
        }

        let this = slf.borrow();
        let follow = follow_redirects.unwrap_or(this.follow_redirects);
        let client = if follow == this.follow_redirects {
            this.get_client()?.clone()
        } else {
            build_blocking_client(
                &this.timeout,
                follow,
                this.block_private_redirects,
                this.max_redirects,
                this.trust_env,
                this.verify,
                this.cert_identity.as_ref(),
                this.proxy.as_deref(),
                &this.limits,
                this.cookie_jar.clone(),
            )?
        };
        let (method_str, url, headers, body) = extract_request_parts(py, &request_obj)?;
        this.bind_default_cookies_for_url(&request_url);

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
            PyResponse::from_blocking_stream(
                response,
                elapsed,
                Some(request_obj.clone_ref(py)),
                client_default_encoding
                    .as_ref()
                    .map(|encoding| encoding.clone_ref(py)),
            )
        } else {
            PyResponse::from_blocking(
                response,
                elapsed,
                Some(request_obj.clone_ref(py)),
                client_default_encoding
                    .as_ref()
                    .map(|encoding| encoding.clone_ref(py)),
            )?
        };
        let response_obj = Py::new(py, py_response)?;
        let response_any = response_obj.clone_ref(py).into_any();
        run_sync_response_hooks(py, &response_hooks, &response_any)?;
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
        let resolved_url = self.resolve_url(url);
        let stream_url = merge_url_query(&resolved_url, self.default_query.as_deref(), None);
        self.bind_default_cookies_for_url(&stream_url);
        Ok(PyStreamContext {
            client_inner: self.inner.as_ref().unwrap().clone(), // blocking::Client is Clone
            method: method.to_string(),
            url: stream_url,
            content: content.unwrap_or_default(),
            json: json.map(|j| j.into_bound(py).into_any().unbind()),
            data: data.map(|d| d.into_bound(py).into_any().unbind()),
            extra_headers: headers
                .map(|h| PyHeaders::from_pyobject(py, h))
                .transpose()?,
            auth: auth.map(|a| extract_auth(py, &a)).transpose()?,
            timeout: parse_timeout_arg(py, timeout, &self.timeout),
            default_headers: self.default_headers.clone(),
            default_encoding: self
                .default_encoding
                .as_ref()
                .map(|encoding| encoding.clone_ref(py)),
            response: None,
        })
    }

    pub fn close(&mut self) {
        self.inner = None;
        self.cert_identity = None;
    }

    #[getter]
    pub fn is_closed(&self) -> bool {
        self.inner.is_none()
    }

    #[getter]
    pub fn timeout(&self) -> PyTimeout {
        self.timeout.clone()
    }

    #[getter]
    pub fn proxy(&self) -> Option<String> {
        self.proxy.clone()
    }

    #[getter]
    pub fn limits(&self) -> PyLimits {
        self.limits.clone()
    }

    #[getter]
    pub fn http1(&self) -> bool {
        self.http1
    }

    #[getter]
    pub fn http2(&self) -> bool {
        self.http2
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
    default_encoding: Option<Py<PyAny>>,
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

        let py_resp = PyResponse::from_blocking_stream(
            resp,
            elapsed,
            None,
            slf.default_encoding
                .as_ref()
                .map(|encoding| encoding.clone_ref(py)),
        );
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
    default_query: Option<String>,
    cookie_jar: Arc<reqwest::cookie::Jar>,
    cookie_state: Arc<Mutex<CookieBindingState>>,
    cert_identity: Option<reqwest::Identity>,
    default_headers: PyHeaders,
    timeout: PyTimeout,
    follow_redirects: bool,
    block_private_redirects: bool,
    max_redirects: usize,
    trust_env: bool,
    verify: bool,
    proxy: Option<String>,
    limits: PyLimits,
    http1: bool,
    http2: bool,
    default_auth: Option<AuthKind>,
    transport: Option<Py<PyAny>>,
    mounts: Vec<MountTransport>,
    event_hooks: EventHooks,
    default_encoding: Option<Py<PyAny>>,
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

    fn bind_default_cookies_for_url(&self, url: &str) {
        bind_default_cookies_for_url(&self.cookie_jar, &self.cookie_state, url);
    }

    fn transport_for_url(&self, py: Python<'_>, url: &str) -> Option<Py<PyAny>> {
        select_mounted_transport(py, &self.mounts, url).or_else(|| {
            self.transport
                .as_ref()
                .map(|transport| transport.clone_ref(py))
        })
    }
}

/// Convert an async reqwest response to PyResponse.
async fn convert_async_response(
    resp: reqwest::Response,
    elapsed_ms: u128,
    request: Option<Py<PyRequest>>,
    default_encoding: Option<Py<PyAny>>,
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
        default_encoding,
        extensions: None,
        stream: None,
        py_stream: None,
    })
}

#[pymethods]
impl PyAsyncClient {
    #[new]
    #[pyo3(signature = (
        *,
        auth = None,
        params = None,
        headers = None,
        cookies = None,
        verify = None,
        cert = None,
        trust_env = true,
        http1 = true,
        http2 = false,
        proxy = None,
        mounts = None,
        timeout = None,
        follow_redirects = false,
        limits = None,
        max_redirects = 20,
        event_hooks = None,
        base_url = None,
        transport = None,
        default_encoding = None,
        block_private_redirects = false,
    ))]
    pub fn new(
        py: Python<'_>,
        auth: Option<Py<PyAny>>,
        params: Option<Py<PyAny>>,
        headers: Option<Py<PyAny>>,
        cookies: Option<Py<PyAny>>,
        verify: Option<Py<PyAny>>,
        cert: Option<Py<PyAny>>,
        trust_env: bool,
        http1: bool,
        http2: bool,
        proxy: Option<Py<PyAny>>,
        mounts: Option<Py<PyAny>>,
        timeout: Option<Py<PyAny>>,
        follow_redirects: bool,
        limits: Option<Py<PyAny>>,
        max_redirects: usize,
        event_hooks: Option<Py<PyAny>>,
        base_url: Option<String>,
        transport: Option<Py<PyAny>>,
        default_encoding: Option<Py<PyAny>>,
        block_private_redirects: bool,
    ) -> PyResult<Self> {
        let default_encoding = parse_default_encoding_arg(py, default_encoding)?;
        let default_query = params_to_query(py, params)?;
        let cookie_pairs = parse_cookies_arg(py, cookies)?;
        let cert_identity = parse_cert_arg(py, cert)?;
        let mounts = parse_mounts_arg(py, mounts)?;
        let event_hooks = parse_event_hooks_arg(py, event_hooks)?;
        let cookie_jar = Arc::new(reqwest::cookie::Jar::default());
        let cookie_state = Arc::new(Mutex::new(CookieBindingState {
            pending_pairs: cookie_pairs,
            bound_origin: None,
        }));
        if let Some(base) = base_url.as_deref().and_then(|u| url::Url::parse(u).ok()) {
            let mut state = cookie_state.lock().unwrap();
            bind_pending_cookies_to_url(&cookie_jar, &mut state, &base);
        }
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

        let verify = parse_verify_arg(py, verify);
        let proxy = parse_proxy_arg(py, proxy)?;
        let limits = parse_limits_arg(py, limits)?;
        let default_auth = match auth {
            None => None,
            Some(a) => Some(extract_auth(py, &a)?),
        };
        let inner = build_async_client(
            &py_timeout,
            follow_redirects,
            block_private_redirects,
            max_redirects,
            trust_env,
            verify,
            cert_identity.as_ref(),
            proxy.as_deref(),
            &limits,
            cookie_jar.clone(),
        )?;

        Ok(PyAsyncClient {
            inner: Some(inner),
            base_url,
            default_query,
            cookie_jar,
            cookie_state,
            cert_identity,
            default_headers,
            timeout: py_timeout,
            follow_redirects,
            block_private_redirects,
            max_redirects,
            trust_env,
            verify,
            proxy,
            limits,
            http1,
            http2,
            default_auth,
            transport,
            mounts,
            event_hooks,
            default_encoding,
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
    #[pyo3(signature = (*_args, **kwargs))]
    fn __init__(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        _args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        if let Some(kwargs) = kwargs {
            if let Some(timeout_obj) = kwargs.get_item("timeout")? {
                let timeout = if timeout_obj.is_none() {
                    crate::config::PyTimeout::new(None, None, None, None, None)
                } else if let Ok(pt) = timeout_obj.extract::<PyRef<PyTimeout>>() {
                    pt.clone()
                } else if let Ok(f) = timeout_obj.extract::<f64>() {
                    crate::config::PyTimeout::new(Some(f), None, None, None, None)
                } else {
                    crate::config::PyTimeout::new(Some(5.0), None, None, None, None)
                };
                slf.borrow_mut().timeout = timeout;
            }
        }
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
        let resolved_url = self.resolve_url(&url);
        let full_url = merge_url_query(&resolved_url, self.default_query.as_deref(), None);
        self.bind_default_cookies_for_url(&full_url);
        let request_hooks = clone_hooks(py, &self.event_hooks.request);
        let response_hooks = clone_hooks(py, &self.event_hooks.response);
        let client_default_encoding = self
            .default_encoding
            .as_ref()
            .map(|encoding| encoding.clone_ref(py));

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

        let req_auth = match auth {
            Some(ref a) => Some(extract_auth(py, a)?),
            None => None,
        };
        let effective_auth = req_auth.as_ref().or(self.default_auth.as_ref());
        let auth_header: Option<String> = match effective_auth {
            Some(AuthKind::Basic(header)) => Some(header.clone()),
            _ => None,
        };

        let body_bytes: Option<Vec<u8>> = if let Some(bytes) = content {
            Some(bytes)
        } else if let Some(ref json_obj) = json {
            let json_str = json_dumps(json_obj.bind(py))?;
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
        if let Some(ref ct) = content_type {
            merged_headers
                .inner
                .push(("content-type".to_string(), ct.clone()));
        }
        if let Some(ref header_val) = auth_header {
            merged_headers
                .inner
                .push(("authorization".to_string(), header_val.clone()));
        }
        let request_obj = build_hook_request(
            py,
            &method,
            &full_url,
            merged_headers,
            body_bytes.clone().unwrap_or_default(),
        )?;
        let request_obj_for_hooks = request_obj.clone_ref(py);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            run_async_request_hooks(request_hooks, request_obj_for_hooks).await?;
            let (method_str, url, headers, body) =
                Python::attach(|py| extract_request_parts(py, &request_obj))?;

            let method = reqwest::Method::from_bytes(method_str.as_bytes())
                .map_err(|_| PyValueError::new_err("Invalid method"))?;
            let mut builder = client.request(method, &url);

            for (k, v) in &headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            if let Some(body) = body {
                builder = builder.body(body);
            }
            if let Some(dur) = req_timeout {
                builder = builder.timeout(dur);
            }

            let start = Instant::now();
            let response = builder.send().await.map_err(crate::map_reqwest_error)?;
            let elapsed = start.elapsed().as_millis();
            let request_for_response = Python::attach(|py| request_obj.clone_ref(py));
            let response = convert_async_response(
                response,
                elapsed,
                Some(request_for_response),
                client_default_encoding,
            )
            .await?;
            let response_obj =
                Python::attach(|py| Py::new(py, response).map(|obj| obj.into_any()))?;
            run_async_response_hooks(
                response_hooks,
                Python::attach(|py| response_obj.clone_ref(py)),
            )
            .await?;
            Ok(response_obj)
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
        let resolved_url = self.resolve_url(&url_value);
        let request_query = params_to_query(py, params)?;
        let full_url = merge_url_query(
            &resolved_url,
            self.default_query.as_deref(),
            request_query.as_deref(),
        );

        let mut merged_headers = self.default_headers.clone();
        if let Some(h) = headers {
            let extra = PyHeaders::from_pyobject(py, h)?;
            for (k, v) in extra.inner {
                merged_headers.inner.retain(|(ek, _)| ek != &k);
                merged_headers.inner.push((k, v));
            }
        }

        let has_content = content.is_some();
        let mut request_content = content;
        let mut body_content = Vec::new();
        let multipart_boundary = merged_headers
            .get("content-type", None)
            .and_then(|ct| extract_multipart_boundary(&ct));
        if !has_content {
            if files.is_some() || multipart_boundary.is_some() {
                let boundary = if let Some(boundary) = multipart_boundary {
                    boundary
                } else {
                    let boundary = "httprs-boundary".to_string();
                    if merged_headers.get("content-type", None).is_none() {
                        merged_headers.inner.push((
                            "content-type".to_string(),
                            format!("multipart/form-data; boundary={boundary}"),
                        ));
                    }
                    boundary
                };
                let multipart_data = data.or(json);
                body_content = build_multipart_body(py, multipart_data, files, &boundary)?;
            } else if let Some(json_obj) = json {
                let json_str = json_dumps(json_obj.bind(py))?;
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
            request_content = Some(PyBytes::new(py, &body_content).into_any().unbind());
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
            request_content,
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
        let req_auth = match auth {
            Some(ref a) => Some(extract_auth(py, a)?),
            None => None,
        };
        let (default_auth, request_hooks, response_hooks, client_default_encoding) = {
            let this = slf.borrow();
            (
                this.default_auth.as_ref().map(|a| clone_auth_kind(py, a)),
                clone_hooks(py, &this.event_hooks.request),
                clone_hooks(py, &this.event_hooks.response),
                this.default_encoding
                    .as_ref()
                    .map(|encoding| encoding.clone_ref(py)),
            )
        };
        let effective_auth = req_auth.or(default_auth);
        let request_obj = Py::new(py, request.clone())?;
        if let Some(AuthKind::Basic(header_val)) = &effective_auth {
            let mut req_mut = request_obj.bind(py).borrow_mut();
            req_mut.set_header("authorization", header_val);
        }
        let request_url = {
            let req_ref = request_obj.bind(py).borrow();
            req_ref.url.inner.to_string()
        };
        let transport_obj = {
            let this = slf.borrow();
            this.transport_for_url(py, &request_url)
        };
        if let Some(transport) = transport_obj {
            let transport_bound = transport.into_bound(py).into_any();
            if transport_bound.hasattr("handle_async_request")? {
                let transport_obj = transport_bound.unbind();
                let request_obj_for_hooks = request_obj.clone_ref(py);
                let request_obj_for_transport = request_obj.clone_ref(py);
                let request_obj_for_response = request_obj.clone_ref(py);
                return pyo3_async_runtimes::tokio::future_into_py(py, async move {
                    run_async_request_hooks(request_hooks, request_obj_for_hooks).await?;
                    let awaitable = Python::attach(|py| {
                        transport_obj
                            .bind(py)
                            .call_method1(
                                "handle_async_request",
                                (request_obj_for_transport.clone_ref(py),),
                            )
                            .map(|obj| obj.unbind())
                    })?;
                    let response_obj = Python::attach(|py| {
                        pyo3_async_runtimes::tokio::into_future(awaitable.into_bound(py))
                    })?
                    .await?;
                    Python::attach(|py| {
                        if let Ok(mut py_response) =
                            response_obj.bind(py).extract::<PyRefMut<'_, PyResponse>>()
                        {
                            if py_response.request.is_none() {
                                py_response.request = Some(request_obj_for_response.clone_ref(py));
                            }
                            py_response.default_encoding = client_default_encoding
                                .as_ref()
                                .map(|encoding| encoding.clone_ref(py));
                        }
                        Ok::<(), PyErr>(())
                    })?;
                    run_async_response_hooks(
                        response_hooks,
                        Python::attach(|py| response_obj.clone_ref(py)),
                    )
                    .await?;
                    Ok(response_obj)
                });
            }
            if transport_bound.hasattr("handle_request")? {
                let transport_obj = transport_bound.unbind();
                let request_obj_for_hooks = request_obj.clone_ref(py);
                let request_obj_for_transport = request_obj.clone_ref(py);
                let request_obj_for_response = request_obj.clone_ref(py);
                return pyo3_async_runtimes::tokio::future_into_py(py, async move {
                    run_async_request_hooks(request_hooks, request_obj_for_hooks).await?;
                    let response_obj = Python::attach(|py| {
                        let response = transport_obj.bind(py).call_method1(
                            "handle_request",
                            (request_obj_for_transport.clone_ref(py),),
                        )?;
                        if let Ok(mut py_response) = response.extract::<PyRefMut<'_, PyResponse>>()
                        {
                            if py_response.request.is_none() {
                                py_response.request = Some(request_obj_for_response.clone_ref(py));
                            }
                            py_response.default_encoding = client_default_encoding
                                .as_ref()
                                .map(|encoding| encoding.clone_ref(py));
                        }
                        Ok::<Py<PyAny>, PyErr>(response.unbind())
                    })?;
                    run_async_response_hooks(
                        response_hooks,
                        Python::attach(|py| response_obj.clone_ref(py)),
                    )
                    .await?;
                    Ok(response_obj)
                });
            }
        }

        let this = slf.borrow();
        let follow = follow_redirects.unwrap_or(this.follow_redirects);
        let client = if follow == this.follow_redirects {
            this.get_client()?
        } else {
            build_async_client(
                &this.timeout,
                follow,
                this.block_private_redirects,
                this.max_redirects,
                this.trust_env,
                this.verify,
                this.cert_identity.as_ref(),
                this.proxy.as_deref(),
                &this.limits,
                this.cookie_jar.clone(),
            )?
        };
        this.bind_default_cookies_for_url(&request_url);
        drop(this);
        let request_obj_for_hooks = request_obj.clone_ref(py);
        let request_obj_stream = request_obj.clone_ref(py);
        let request_obj_regular = request_obj.clone_ref(py);
        let stream_default_encoding = client_default_encoding
            .as_ref()
            .map(|encoding| encoding.clone_ref(py));

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            run_async_request_hooks(request_hooks, request_obj_for_hooks).await?;
            let (method_str, url, headers, body) =
                Python::attach(|py| extract_request_parts(py, &request_obj))?;
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
            let response_obj = if stream {
                let response = PyResponse::from_async_stream(
                    response,
                    elapsed,
                    Some(request_obj_stream),
                    stream_default_encoding,
                );
                Python::attach(|py| Py::new(py, response).map(|obj| obj.into_any()))?
            } else {
                let response = convert_async_response(
                    response,
                    elapsed,
                    Some(request_obj_regular),
                    client_default_encoding,
                )
                .await?;
                Python::attach(|py| Py::new(py, response).map(|obj| obj.into_any()))?
            };
            run_async_response_hooks(
                response_hooks,
                Python::attach(|py| response_obj.clone_ref(py)),
            )
            .await?;
            Ok(response_obj)
        })
    }

    pub fn close(&mut self) {
        self.inner = None;
        self.cert_identity = None;
    }

    pub fn aclose<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.inner = None;
        self.cert_identity = None;
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

    #[getter]
    pub fn proxy(&self) -> Option<String> {
        self.proxy.clone()
    }

    #[getter]
    pub fn limits(&self) -> PyLimits {
        self.limits.clone()
    }

    #[getter]
    pub fn http1(&self) -> bool {
        self.http1
    }

    #[getter]
    pub fn http2(&self) -> bool {
        self.http2
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
