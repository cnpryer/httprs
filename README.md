# httprs

> **Disclaimer:** This project is under active development and is not ready for use. Do not use this project at this time.

An experimental HTTP library for Python built with Rust.

## Experimenting with httprs

### Install

```bash
pip install --pre httprs
```

### Example

```python
import httprs

httprs.get("https://postman-echo.com/get").json()
```

## Goals

- Simple
- Fast
- Open

## Development

```bash
gh repo clone cnpryer/httprs
cd httprs

just sync
just fmt
just fix
just test
```
