use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;

#[pyclass(name = "Cookies", from_py_object)]
#[derive(Clone, Default)]
pub struct PyCookies {
    inner: HashMap<String, String>,
}

#[pymethods]
impl PyCookies {
    #[new]
    #[pyo3(signature = (cookies = None))]
    pub fn new(py: Python<'_>, cookies: Option<Py<PyAny>>) -> PyResult<Self> {
        let mut inner = HashMap::new();
        if let Some(c) = cookies {
            let b = c.bind(py);
            if let Ok(d) = b.cast::<PyDict>() {
                for (k, v) in d.iter() {
                    inner.insert(k.extract()?, v.extract()?);
                }
            }
        }
        Ok(Self { inner })
    }

    #[pyo3(signature = (name, value, **_kwargs))]
    pub fn set(&mut self, name: String, value: String, _kwargs: Option<Bound<'_, PyDict>>) {
        self.inner.insert(name, value);
    }

    #[pyo3(signature = (name, default = None))]
    pub fn get(&self, name: String, default: Option<String>) -> Option<String> {
        self.inner.get(&name).cloned().or(default)
    }

    pub fn items(&self) -> Vec<(String, String)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}
