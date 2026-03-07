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


def test_digest_header_injection_username_with_quote():
    """Username containing a double-quote must be escaped, not injected."""
    auth = httprs.DigestAuth(username='admin", injected="evil', password="pass")
    www_auth = 'Digest realm="test", qop="auth", nonce="abc123"'
    header = auth.compute_header("GET", "/path", www_auth)
    assert header.startswith("Digest ")
    # Without escaping the header would contain the unescaped `, injected="evil"` as
    # a real field. With proper escaping the quotes around "evil" are backslash-escaped.
    assert ', injected="evil"' not in header


def test_digest_header_username_quote_is_escaped():
    """Double-quote in username must appear as \" (backslash-escaped) in the header."""
    auth = httprs.DigestAuth(username='foo"bar', password="pass")
    www_auth = 'Digest realm="test", nonce="abc123"'
    header = auth.compute_header("GET", "/path", www_auth)
    assert 'username="foo\\"bar"' in header


def test_ssrf_redirect_to_ipv6_link_local_is_blocked(server):
    """block_private_redirects must block redirects to IPv6 link-local addresses."""
    with httprs.Client(follow_redirects=True, block_private_redirects=True) as client:
        with pytest.raises(httprs.RequestError):
            client.get(server.url + "/redirect_to_ipv6_private")
