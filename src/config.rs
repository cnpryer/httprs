use pyo3::prelude::*;

fn fmt_opt_f64(v: Option<f64>) -> String {
    match v {
        Some(f) => format!("{}", f),
        None => "None".to_string(),
    }
}

fn fmt_opt_usize(v: Option<usize>) -> String {
    match v {
        Some(n) => format!("{}", n),
        None => "None".to_string(),
    }
}

#[pyclass(name = "Timeout", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyTimeout {
    pub connect: Option<f64>,
    pub read: Option<f64>,
    pub write: Option<f64>,
    pub pool: Option<f64>,
}

#[pymethods]
impl PyTimeout {
    #[new]
    #[pyo3(signature = (
        timeout = None,
        *,
        connect = None,
        read = None,
        write = None,
        pool = None,
    ))]
    pub fn new(
        timeout: Option<f64>,
        connect: Option<f64>,
        read: Option<f64>,
        write: Option<f64>,
        pool: Option<f64>,
    ) -> Self {
        PyTimeout {
            connect: connect.or(timeout),
            read: read.or(timeout),
            write: write.or(timeout),
            pool: pool.or(timeout),
        }
    }

    #[getter]
    pub fn connect(&self) -> Option<f64> {
        self.connect
    }

    #[getter]
    pub fn read(&self) -> Option<f64> {
        self.read
    }

    #[getter]
    pub fn write(&self) -> Option<f64> {
        self.write
    }

    #[getter]
    pub fn pool(&self) -> Option<f64> {
        self.pool
    }

    fn __repr__(&self) -> String {
        format!(
            "Timeout(connect={}, read={}, write={}, pool={})",
            fmt_opt_f64(self.connect),
            fmt_opt_f64(self.read),
            fmt_opt_f64(self.write),
            fmt_opt_f64(self.pool),
        )
    }

    fn __eq__(&self, other: &PyTimeout) -> bool {
        self.connect == other.connect
            && self.read == other.read
            && self.write == other.write
            && self.pool == other.pool
    }
}

#[pyclass(name = "Limits", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyLimits {
    pub max_connections: Option<usize>,
    pub max_keepalive_connections: Option<usize>,
    pub keepalive_expiry: Option<f64>,
}

#[pymethods]
impl PyLimits {
    #[new]
    #[pyo3(signature = (
        *,
        max_connections = None,
        max_keepalive_connections = None,
        keepalive_expiry = Some(5.0),
    ))]
    pub fn new(
        max_connections: Option<usize>,
        max_keepalive_connections: Option<usize>,
        keepalive_expiry: Option<f64>,
    ) -> Self {
        PyLimits {
            max_connections,
            max_keepalive_connections,
            keepalive_expiry,
        }
    }

    #[getter]
    pub fn max_connections(&self) -> Option<usize> {
        self.max_connections
    }

    #[getter]
    pub fn max_keepalive_connections(&self) -> Option<usize> {
        self.max_keepalive_connections
    }

    #[getter]
    pub fn keepalive_expiry(&self) -> Option<f64> {
        self.keepalive_expiry
    }

    fn __repr__(&self) -> String {
        format!(
            "Limits(max_connections={}, max_keepalive_connections={}, keepalive_expiry={})",
            fmt_opt_usize(self.max_connections),
            fmt_opt_usize(self.max_keepalive_connections),
            fmt_opt_f64(self.keepalive_expiry),
        )
    }

    fn __eq__(&self, other: &PyLimits) -> bool {
        self.max_connections == other.max_connections
            && self.max_keepalive_connections == other.max_keepalive_connections
            && self.keepalive_expiry == other.keepalive_expiry
    }
}
