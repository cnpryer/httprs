use base64::{engine::general_purpose, Engine as _};
use hex;
use md5::{Digest as Md5Digest, Md5};
use pyo3::prelude::*;
use pyo3::types::PyList;
use rand;
use sha2::Sha256;
use std::sync::Mutex;

#[pyclass(name = "BasicAuth", subclass, from_py_object)]
#[derive(Clone)]
pub struct PyBasicAuth {
    pub username: String,
    pub password: String,
    header_value: String,
}

#[pymethods]
impl PyBasicAuth {
    #[new]
    #[pyo3(signature = (username, password = ""))]
    pub fn new(username: &str, password: &str) -> Self {
        let credentials = format!("{}:{}", username, password);
        let encoded = general_purpose::STANDARD.encode(credentials.as_bytes());
        PyBasicAuth {
            username: username.to_string(),
            password: password.to_string(),
            header_value: format!("Basic {}", encoded),
        }
    }

    #[getter]
    pub fn username(&self) -> &str {
        &self.username
    }

    #[getter]
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Return the precomputed Authorization header value.
    pub fn authorization_header(&self) -> &str {
        &self.header_value
    }

    fn __repr__(&self) -> String {
        format!("BasicAuth(username={:?})", self.username)
    }
}

#[derive(Debug)]
struct DigestState {
    nonce_count: u32,
    last_nonce: String,
}

impl DigestState {
    fn new() -> Self {
        DigestState {
            nonce_count: 0,
            last_nonce: String::new(),
        }
    }
}

#[pyclass(name = "DigestAuth", subclass)]
pub struct PyDigestAuth {
    pub username: String,
    pub password: String,
    state: Mutex<DigestState>,
}

/// Parse a WWW-Authenticate: Digest header into its components.
fn parse_digest_challenge(header: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    // Strip "Digest " prefix if present
    let header = header.trim();
    let params = if header.to_lowercase().starts_with("digest ") {
        &header[7..]
    } else {
        header
    };

    // Parse key=value or key="value" pairs
    let mut remaining = params;
    while !remaining.is_empty() {
        remaining = remaining.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        if remaining.is_empty() {
            break;
        }
        // Find the key
        if let Some(eq_pos) = remaining.find('=') {
            let key = remaining[..eq_pos].trim().to_lowercase();
            remaining = remaining[eq_pos + 1..].trim_start();
            // Find the value (quoted or unquoted)
            let (value, rest) = if let Some(s) = remaining.strip_prefix('"') {
                // Quoted value — guard against unclosed/single-char strings to prevent panic.
                let end = s.find('"').map(|i| i + 2).unwrap_or(remaining.len());
                let value = remaining
                    .get(1..end.saturating_sub(1))
                    .unwrap_or("")
                    .to_string();
                (value, &remaining[end..])
            } else {
                // Unquoted: value is until next comma or end
                let end = remaining.find(',').unwrap_or(remaining.len());
                (remaining[..end].trim().to_string(), &remaining[end..])
            };
            map.insert(key, value);
            remaining = rest;
        } else {
            break;
        }
    }
    map
}

fn escape_digest_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

#[pymethods]
impl PyDigestAuth {
    #[new]
    pub fn new(username: &str, password: &str) -> Self {
        PyDigestAuth {
            username: username.to_string(),
            password: password.to_string(),
            state: Mutex::new(DigestState::new()),
        }
    }

    #[getter]
    pub fn username(&self) -> &str {
        &self.username
    }

    #[getter]
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Compute the Authorization: Digest header value given a challenge.
    pub fn compute_header(
        &self,
        method: &str,
        uri: &str,
        www_authenticate: &str,
    ) -> PyResult<String> {
        let challenge = parse_digest_challenge(www_authenticate);

        let realm = challenge.get("realm").cloned().unwrap_or_default();
        let nonce = challenge.get("nonce").cloned().unwrap_or_default();
        let opaque = challenge.get("opaque").cloned();
        let qop = challenge.get("qop").cloned();
        let algorithm = challenge
            .get("algorithm")
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| "MD5".to_string());

        let mut state = self.state.lock().unwrap();

        // Increment nonce count or reset if nonce changed
        if state.last_nonce != nonce {
            state.nonce_count = 1;
            state.last_nonce = nonce.clone();
        } else {
            state.nonce_count += 1;
        }
        let nc = format!("{:08x}", state.nonce_count);
        let cnonce = format!("{:016x}", rand::random::<u64>());

        // HA1 = hash(username:realm:password)
        let ha1_input = format!("{}:{}:{}", self.username, realm, self.password);
        let ha1 = match algorithm.as_str() {
            "SHA-256" | "SHA256" => sha256_hex(&ha1_input),
            _ => md5_hex(&ha1_input),
        };

        // HA2 = hash(method:uri)
        let ha2_input = format!("{}:{}", method.to_uppercase(), uri);
        let ha2 = match algorithm.as_str() {
            "SHA-256" | "SHA256" => sha256_hex(&ha2_input),
            _ => md5_hex(&ha2_input),
        };

        // Response hash
        let response_hash = if let Some(ref q) = qop {
            if q.contains("auth") {
                let input = format!("{}:{}:{}:{}:{}:{}", ha1, nonce, nc, cnonce, "auth", ha2);
                match algorithm.as_str() {
                    "SHA-256" | "SHA256" => sha256_hex(&input),
                    _ => md5_hex(&input),
                }
            } else {
                let input = format!("{}:{}:{}", ha1, nonce, ha2);
                match algorithm.as_str() {
                    "SHA-256" | "SHA256" => sha256_hex(&input),
                    _ => md5_hex(&input),
                }
            }
        } else {
            let input = format!("{}:{}:{}", ha1, nonce, ha2);
            match algorithm.as_str() {
                "SHA-256" | "SHA256" => sha256_hex(&input),
                _ => md5_hex(&input),
            }
        };

        // Build Authorization header
        let mut parts = vec![
            format!("username=\"{}\"", escape_digest_value(&self.username)),
            format!("realm=\"{}\"", escape_digest_value(&realm)),
            format!("nonce=\"{}\"", escape_digest_value(&nonce)),
            format!("uri=\"{}\"", escape_digest_value(uri)),
        ];

        if algorithm != "MD5" {
            parts.push(format!("algorithm={}", algorithm));
        }

        if let Some(ref q) = qop {
            if q.contains("auth") {
                parts.push("qop=auth".to_string());
                parts.push(format!("nc={}", nc));
                parts.push(format!("cnonce=\"{}\"", escape_digest_value(&cnonce)));
            }
        }

        parts.push(format!("response=\"{}\"", response_hash));

        if let Some(op) = opaque {
            parts.push(format!("opaque=\"{}\"", escape_digest_value(&op)));
        }

        Ok(format!("Digest {}", parts.join(", ")))
    }

    fn __repr__(&self) -> String {
        format!("DigestAuth(username={:?})", self.username)
    }
}

#[pyclass(name = "Auth")]
pub struct PyAuth;

#[pymethods]
impl PyAuth {
    #[new]
    pub fn new() -> Self {
        Self
    }

    pub fn auth_flow<'py>(&self, py: Python<'py>, request: Py<PyAny>) -> PyResult<Py<PyAny>> {
        let list = PyList::new(py, [request])?;
        let iter = list.call_method0("__iter__")?;
        Ok(iter.into_any().unbind())
    }
}

impl Default for PyAuth {
    fn default() -> Self {
        Self::new()
    }
}
