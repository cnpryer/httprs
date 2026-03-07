"""Security regression tests for identified vulnerabilities."""

from __future__ import annotations

import pytest
import httprs


def test_ssrf_redirect_to_private_ip_is_blocked(server):
    """When block_private_redirects=True, a 301 → private IP must not be followed."""
    with httprs.Client(follow_redirects=True, block_private_redirects=True) as client:
        with pytest.raises(httprs.RequestError):
            client.get(server.url + "/redirect_to_private")


def test_ssrf_redirect_allowed_by_default(server):
    """block_private_redirects defaults to False so existing behaviour is unchanged."""
    # /redirect_301 redirects to "/" on the same (127.0.0.1) server; must still work.
    with httprs.Client(follow_redirects=True) as client:
        response = client.get(server.url + "/redirect_301")
    assert response.status_code == 200


def test_digest_unclosed_quote_does_not_panic():
    """A WWW-Authenticate header with an unclosed quote must not cause a panic."""
    auth = httprs.DigestAuth(username="user", password="pass")
    # 'nonce="' has no closing quote — the old code panicked here.
    result = auth.compute_header("GET", "/path", 'Digest realm="x", nonce="')
    assert result.startswith("Digest ")


def test_digest_single_char_quoted_value_does_not_panic():
    """A quoted field containing only the opening quote must not panic."""
    auth = httprs.DigestAuth(username="u", password="p")
    result = auth.compute_header("GET", "/", 'Digest realm="')
    assert result.startswith("Digest ")
