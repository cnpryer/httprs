use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

pub mod auth;
pub mod client;
pub mod config;
pub mod cookies;
pub mod json;
pub mod models;
pub mod proxy;
pub mod query_params;
pub mod transports;

create_exception!(
    httprs,
    HTTPError,
    PyException,
    "Base class for all httprs exceptions."
);
create_exception!(
    httprs,
    RequestError,
    HTTPError,
    "Base class for request-side exceptions."
);
create_exception!(
    httprs,
    TransportError,
    RequestError,
    "Base class for transport-level errors."
);
create_exception!(
    httprs,
    TimeoutException,
    TransportError,
    "Timed out while making a request."
);
create_exception!(
    httprs,
    ConnectTimeout,
    TimeoutException,
    "Timed out while connecting to the host."
);
create_exception!(
    httprs,
    ReadTimeout,
    TimeoutException,
    "Timed out while receiving data from the host."
);
create_exception!(
    httprs,
    WriteTimeout,
    TimeoutException,
    "Timed out while sending data to the host."
);
create_exception!(
    httprs,
    NetworkError,
    TransportError,
    "Failed to make a network connection."
);
create_exception!(
    httprs,
    ConnectError,
    NetworkError,
    "Failed to establish a connection."
);
create_exception!(
    httprs,
    ReadError,
    NetworkError,
    "Failed to receive data from the network."
);
create_exception!(
    httprs,
    UnsupportedProtocol,
    TransportError,
    "Attempted to make a request to an unsupported URL scheme."
);
create_exception!(
    httprs,
    TooManyRedirects,
    RequestError,
    "Too many redirects."
);
create_exception!(
    httprs,
    HTTPStatusError,
    HTTPError,
    "Response closed with an error status code."
);
create_exception!(
    httprs,
    PoolTimeout,
    TimeoutException,
    "Timed out while acquiring a connection from the pool."
);
create_exception!(
    httprs,
    WriteError,
    NetworkError,
    "Failed to send data to the network."
);
create_exception!(
    httprs,
    CloseError,
    NetworkError,
    "Failed while closing a connection."
);
create_exception!(
    httprs,
    ProxyError,
    ConnectError,
    "Failed to connect to a proxy."
);
create_exception!(
    httprs,
    DecodingError,
    ReadError,
    "Failed to decode response content."
);
create_exception!(httprs, InvalidURL, UnsupportedProtocol, "Invalid URL.");
create_exception!(httprs, ProtocolError, HTTPError, "Protocol error.");
create_exception!(httprs, StreamError, HTTPError, "Stream-related error.");
create_exception!(httprs, CookieConflict, HTTPError, "Cookie conflict.");
create_exception!(
    httprs,
    StreamConsumed,
    StreamError,
    "Attempted to read a stream that has already been consumed."
);
create_exception!(
    httprs,
    StreamNotRead,
    StreamError,
    "Attempted to access stream content before reading."
);
create_exception!(
    httprs,
    StreamClosed,
    StreamError,
    "Attempted to read a closed stream."
);
create_exception!(
    httprs,
    ResponseNotRead,
    StreamError,
    "Attempted to access response content before reading."
);
create_exception!(
    httprs,
    RequestNotRead,
    StreamError,
    "Attempted to access request content before reading."
);
create_exception!(
    httprs,
    LocalProtocolError,
    ProtocolError,
    "Client-side protocol error."
);
create_exception!(
    httprs,
    RemoteProtocolError,
    ProtocolError,
    "Server-side protocol error."
);

/// Dedicated multi-thread Tokio runtime used by PyAsyncClient constructors
/// and async streaming responses. Separate from pyo3-async-runtimes' runtime.
static SYNC_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

pub(crate) fn sync_runtime() -> &'static tokio::runtime::Runtime {
    SYNC_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build sync HTTP runtime")
    })
}

/// Run an async future on SYNC_RUNTIME, blocking the calling thread via a channel.
pub(crate) fn run_blocking<F, T>(fut: F) -> PyResult<T>
where
    F: std::future::Future<Output = PyResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    sync_runtime().spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx.recv()
        .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("async task panicked"))?
}

/// Release the Python GIL while executing a blocking closure.
///
/// This is equivalent to Python's `Py_BEGIN_ALLOW_THREADS` / `Py_END_ALLOW_THREADS`.
/// It MUST be called while the GIL is held (i.e., from a `#[pymethods]` function).
/// Required whenever Rust calls blocking I/O that might depend on a Python thread
/// running concurrently (e.g., a local Python HTTP server).
pub(crate) fn without_gil<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    let tstate = unsafe { pyo3::ffi::PyEval_SaveThread() };
    let result = f();
    unsafe { pyo3::ffi::PyEval_RestoreThread(tstate) };
    result
}

/// Map a reqwest::Error to the appropriate Python exception.
pub fn map_reqwest_error(e: reqwest::Error) -> PyErr {
    let msg = e.to_string();
    if e.is_timeout() {
        TimeoutException::new_err(msg)
    } else if e.is_redirect() {
        TooManyRedirects::new_err(msg)
    } else if e.is_connect() {
        ConnectError::new_err(msg)
    } else if e.is_builder() {
        // Unsupported scheme, bad URL, etc.
        UnsupportedProtocol::new_err(msg)
    } else if e.is_request() {
        UnsupportedProtocol::new_err(msg)
    } else if e.is_body() || e.is_decode() {
        ReadError::new_err(msg)
    } else {
        RequestError::new_err(msg)
    }
}

#[pymodule]
fn _httprs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Exceptions
    m.add("HTTPError", m.py().get_type::<HTTPError>())?;
    m.add("RequestError", m.py().get_type::<RequestError>())?;
    m.add("TransportError", m.py().get_type::<TransportError>())?;
    m.add("TimeoutException", m.py().get_type::<TimeoutException>())?;
    m.add("ConnectTimeout", m.py().get_type::<ConnectTimeout>())?;
    m.add("ReadTimeout", m.py().get_type::<ReadTimeout>())?;
    m.add("WriteTimeout", m.py().get_type::<WriteTimeout>())?;
    m.add("NetworkError", m.py().get_type::<NetworkError>())?;
    m.add("ConnectError", m.py().get_type::<ConnectError>())?;
    m.add("ReadError", m.py().get_type::<ReadError>())?;
    m.add(
        "UnsupportedProtocol",
        m.py().get_type::<UnsupportedProtocol>(),
    )?;
    m.add("TooManyRedirects", m.py().get_type::<TooManyRedirects>())?;
    m.add("HTTPStatusError", m.py().get_type::<HTTPStatusError>())?;
    m.add("PoolTimeout", m.py().get_type::<PoolTimeout>())?;
    m.add("WriteError", m.py().get_type::<WriteError>())?;
    m.add("CloseError", m.py().get_type::<CloseError>())?;
    m.add("ProxyError", m.py().get_type::<ProxyError>())?;
    m.add("DecodingError", m.py().get_type::<DecodingError>())?;
    m.add("InvalidURL", m.py().get_type::<InvalidURL>())?;
    m.add("ProtocolError", m.py().get_type::<ProtocolError>())?;
    m.add("StreamError", m.py().get_type::<StreamError>())?;
    m.add("CookieConflict", m.py().get_type::<CookieConflict>())?;
    m.add("StreamConsumed", m.py().get_type::<StreamConsumed>())?;
    m.add("StreamNotRead", m.py().get_type::<StreamNotRead>())?;
    m.add("StreamClosed", m.py().get_type::<StreamClosed>())?;
    m.add("ResponseNotRead", m.py().get_type::<ResponseNotRead>())?;
    m.add("RequestNotRead", m.py().get_type::<RequestNotRead>())?;
    m.add(
        "LocalProtocolError",
        m.py().get_type::<LocalProtocolError>(),
    )?;
    m.add(
        "RemoteProtocolError",
        m.py().get_type::<RemoteProtocolError>(),
    )?;

    // Classes
    m.add_class::<config::PyTimeout>()?;
    m.add_class::<config::PyLimits>()?;
    m.add_class::<models::PyURL>()?;
    m.add_class::<models::PyHeaders>()?;
    m.add_class::<models::PyRequest>()?;
    m.add_class::<models::PyResponse>()?;
    m.add_class::<transports::PyBaseTransport>()?;
    m.add_class::<transports::PyAsyncBaseTransport>()?;
    m.add_class::<transports::PyMockTransport>()?;
    m.add_class::<transports::PyHTTPTransport>()?;
    m.add_class::<transports::PyAsyncHTTPTransport>()?;
    m.add_class::<transports::PyASGITransport>()?;
    m.add_class::<transports::PyWSGITransport>()?;
    m.add_class::<transports::PySyncByteStream>()?;
    m.add_class::<transports::PyAsyncByteStream>()?;
    m.add_class::<transports::PyByteStream>()?;
    m.add_class::<query_params::PyQueryParams>()?;
    m.add_class::<cookies::PyCookies>()?;
    m.add_class::<auth::PyAuth>()?;
    m.add_class::<proxy::PyProxy>()?;
    m.add_class::<auth::PyBasicAuth>()?;
    m.add_class::<auth::PyDigestAuth>()?;
    m.add_class::<client::PyClient>()?;
    m.add_class::<client::PyStreamContext>()?;
    m.add_class::<client::PyAsyncClient>()?;

    let object_ctor = m.py().import("builtins")?.getattr("object")?;
    let sentinel = object_ctor.call0()?;
    m.add("USE_CLIENT_DEFAULT", sentinel)?;

    Ok(())
}
