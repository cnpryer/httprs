use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::collections::HashSet;

#[pyclass(name = "QueryParams", from_py_object)]
#[derive(Clone, Default)]
pub struct PyQueryParams {
    inner: Vec<(String, String)>,
}

impl PyQueryParams {
    fn parse_params(py: Python<'_>, params: Option<Py<PyAny>>) -> PyResult<Vec<(String, String)>> {
        let mut inner = Vec::new();
        if let Some(p) = params {
            let b = p.bind(py);
            if let Ok(s) = b.extract::<String>() {
                for (k, v) in url::form_urlencoded::parse(s.as_bytes()) {
                    inner.push((k.into_owned(), v.into_owned()));
                }
            } else if let Ok(d) = b.cast::<PyDict>() {
                for (k, v) in d.iter() {
                    inner.push((k.extract()?, v.extract()?));
                }
            } else if let Ok(l) = b.cast::<PyList>() {
                for item in l.iter() {
                    let (k, v): (String, String) = item.extract()?;
                    inner.push((k, v));
                }
            } else {
                for item in b.try_iter()? {
                    let item = item?;
                    let (k, v): (String, String) = item.extract()?;
                    inner.push((k, v));
                }
            }
        }
        Ok(inner)
    }

    fn first_items(&self) -> Vec<(String, String)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (k, v) in &self.inner {
            if seen.insert(k.clone()) {
                out.push((k.clone(), v.clone()));
            }
        }
        out
    }

    fn has_key(&self, key: &str) -> bool {
        self.inner.iter().any(|(k, _)| k == key)
    }

    fn first_value(&self, key: &str) -> Option<String> {
        self.inner
            .iter()
            .find_map(|(k, v)| if k == key { Some(v.clone()) } else { None })
    }
}

#[pymethods]
impl PyQueryParams {
    #[new]
    #[pyo3(signature = (params = None))]
    pub fn new(py: Python<'_>, params: Option<Py<PyAny>>) -> PyResult<Self> {
        Ok(Self {
            inner: Self::parse_params(py, params)?,
        })
    }

    pub fn keys(&self) -> Vec<String> {
        self.first_items().into_iter().map(|(k, _)| k).collect()
    }

    pub fn items(&self) -> Vec<(String, String)> {
        self.first_items()
    }

    pub fn multi_items(&self) -> Vec<(String, String)> {
        self.inner.clone()
    }

    #[pyo3(signature = (key, default = None))]
    pub fn get(&self, py: Python<'_>, key: String, default: Option<Py<PyAny>>) -> Py<PyAny> {
        if let Some(value) = self.first_value(&key) {
            return value.into_pyobject(py).unwrap().into_any().unbind();
        }
        default.unwrap_or_else(|| py.None())
    }

    pub fn get_list(&self, key: String) -> Vec<String> {
        self.inner
            .iter()
            .filter_map(|(k, v)| if k == &key { Some(v.clone()) } else { None })
            .collect()
    }

    #[pyo3(signature = (params = None))]
    pub fn merge(&self, py: Python<'_>, params: Option<Py<PyAny>>) -> PyResult<Self> {
        let incoming = Self::parse_params(py, params)?;
        let incoming_keys: HashSet<String> = incoming.iter().map(|(k, _)| k.clone()).collect();

        let mut merged: Vec<(String, String)> = self
            .inner
            .iter()
            .filter_map(|(k, v)| {
                if incoming_keys.contains(k) {
                    None
                } else {
                    Some((k.clone(), v.clone()))
                }
            })
            .collect();
        merged.extend(incoming);

        Ok(Self { inner: merged })
    }

    fn __getitem__(&self, key: String) -> PyResult<String> {
        self.first_value(&key)
            .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err(key))
    }

    fn __contains__(&self, key: String) -> bool {
        self.has_key(&key)
    }

    fn __iter__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let keys = self.keys();
        let list = PyList::new(py, &keys)?;
        Ok(list.into_any().call_method0("__iter__")?.unbind())
    }

    fn __len__(&self) -> usize {
        self.keys().len()
    }

    fn __bool__(&self) -> bool {
        !self.inner.is_empty()
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        if let Ok(other_params) = other.extract::<PyRef<PyQueryParams>>() {
            let mut left = self.inner.clone();
            let mut right = other_params.inner.clone();
            left.sort();
            right.sort();
            left == right
        } else {
            false
        }
    }

    fn __str__(&self) -> String {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &self.inner {
            ser.append_pair(k, v);
        }
        ser.finish()
    }

    fn __repr__(&self) -> String {
        format!("QueryParams({:?})", self.__str__())
    }
}
