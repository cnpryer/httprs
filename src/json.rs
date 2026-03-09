use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};

const JSON_MAX_DEPTH: usize = 100;

fn non_finite_float_label(value: f64) -> &'static str {
    if value.is_nan() {
        "nan"
    } else if value.is_sign_positive() {
        "inf"
    } else {
        "-inf"
    }
}

fn non_finite_float_error(value: f64) -> PyErr {
    PyValueError::new_err(format!(
        "Out of range float values are not JSON compliant: {}",
        non_finite_float_label(value)
    ))
}

fn json_key_from_py(key: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(py_str) = key.cast::<PyString>() {
        return Ok(py_str.to_string_lossy().into_owned());
    }

    if key.is_none() {
        return Ok("null".to_string());
    }

    if let Ok(py_bool) = key.cast::<PyBool>() {
        return Ok(if py_bool.is_true() {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }

    if key.cast::<PyInt>().is_ok() {
        return Ok(key.str()?.to_str()?.to_string());
    }

    if let Ok(py_float) = key.cast::<PyFloat>() {
        let value = py_float.value();
        if !value.is_finite() {
            return Err(non_finite_float_error(value));
        }
        return Ok(key.str()?.to_str()?.to_string());
    }

    Err(PyTypeError::new_err(format!(
        "keys must be str, int, float, bool or None, not {}",
        key.get_type().name()?
    )))
}

fn json_value_from_py(value: &Bound<'_, PyAny>, depth: usize) -> PyResult<serde_json::Value> {
    if depth > JSON_MAX_DEPTH {
        return Err(PyValueError::new_err("JSON payload is too deeply nested"));
    }

    if value.is_none() {
        return Ok(serde_json::Value::Null);
    }

    if let Ok(py_bool) = value.cast::<PyBool>() {
        return Ok(serde_json::Value::Bool(py_bool.is_true()));
    }

    if let Ok(py_str) = value.cast::<PyString>() {
        return Ok(serde_json::Value::String(
            py_str.to_string_lossy().into_owned(),
        ));
    }

    if value.cast::<PyInt>().is_ok() {
        let int_text_obj = value.str()?;
        let int_text = int_text_obj.to_str()?;
        let number = int_text.parse::<serde_json::Number>().map_err(|_| {
            PyValueError::new_err(format!(
                "Integer out of range for JSON encoding: {}",
                int_text
            ))
        })?;
        return Ok(serde_json::Value::Number(number));
    }

    if let Ok(py_float) = value.cast::<PyFloat>() {
        let float_value = py_float.value();
        if !float_value.is_finite() {
            return Err(non_finite_float_error(float_value));
        }
        let number = serde_json::Number::from_f64(float_value)
            .ok_or_else(|| PyValueError::new_err("Failed to encode float value"))?;
        return Ok(serde_json::Value::Number(number));
    }

    if let Ok(py_list) = value.cast::<PyList>() {
        let mut out = Vec::with_capacity(py_list.len());
        for item in py_list.iter() {
            out.push(json_value_from_py(&item, depth + 1)?);
        }
        return Ok(serde_json::Value::Array(out));
    }

    if let Ok(py_tuple) = value.cast::<PyTuple>() {
        let mut out = Vec::with_capacity(py_tuple.len());
        for item in py_tuple.iter() {
            out.push(json_value_from_py(&item, depth + 1)?);
        }
        return Ok(serde_json::Value::Array(out));
    }

    if let Ok(py_dict) = value.cast::<PyDict>() {
        let mut out = serde_json::Map::with_capacity(py_dict.len());
        for (k, v) in py_dict.iter() {
            let key = json_key_from_py(&k)?;
            let value = json_value_from_py(&v, depth + 1)?;
            out.insert(key, value);
        }
        return Ok(serde_json::Value::Object(out));
    }

    Err(PyTypeError::new_err(format!(
        "Object of type {} is not JSON serializable",
        value.get_type().name()?
    )))
}

pub(crate) fn json_dumps(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let json_value = json_value_from_py(value, 0)?;
    serde_json::to_string(&json_value)
        .map_err(|err| PyValueError::new_err(format!("Failed to serialize JSON body: {}", err)))
}
