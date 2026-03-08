use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass(name = "Proxy")]
pub struct PyProxy {
    url: String,
}

#[pymethods]
impl PyProxy {
    #[new]
    #[pyo3(signature = (url, **_kwargs))]
    pub fn new(url: String, _kwargs: Option<Bound<'_, PyDict>>) -> Self {
        Self { url }
    }

    #[getter]
    pub fn url(&self) -> &str {
        &self.url
    }
}
