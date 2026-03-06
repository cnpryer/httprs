"""Unit tests for BasicAuth and DigestAuth."""

from __future__ import annotations

import pytest
import httprs


class TestBasicAuth:
    def test_authorization_header_value(self):
        # user:pass -> base64 "dXNlcjpwYXNz"
        auth = httprs.BasicAuth(username="user", password="pass")
        header = auth.authorization_header()
        assert header == "Basic dXNlcjpwYXNz"

    def test_authorization_header_starts_with_basic(self):
        auth = httprs.BasicAuth(username="alice", password="secret")
        assert auth.authorization_header().startswith("Basic ")

    def test_empty_password(self):
        auth = httprs.BasicAuth(username="user", password="")
        header = auth.authorization_header()
        assert header.startswith("Basic ")
        # user: -> base64 "dXNlcjo="
        assert "dXNlcjo=" in header

    def test_username_getter(self):
        auth = httprs.BasicAuth(username="alice", password="secret")
        assert auth.username == "alice"

    def test_password_getter(self):
        auth = httprs.BasicAuth(username="alice", password="secret")
        assert auth.password == "secret"

    def test_sync_auth_flow_injects_header(self):
        auth = httprs.BasicAuth(username="user", password="pass")
        request = httprs.Request("GET", "https://www.example.com")

        flow = auth.sync_auth_flow(request)
        modified_request = next(flow)
        assert modified_request.headers.get("authorization", "").startswith("Basic")

    def test_sync_auth_flow_stop_iteration(self):
        auth = httprs.BasicAuth(username="user", password="pass")
        request = httprs.Request("GET", "https://www.example.com")

        flow = auth.sync_auth_flow(request)
        next(flow)  # get the modified request

        response = httprs.Response(content=b"Hello, world!", status_code=200)
        with pytest.raises(StopIteration):
            flow.send(response)

    def test_unicode_credentials(self):
        # Non-ASCII characters should be base64 encoded
        auth = httprs.BasicAuth(username="user", password="pässwörд")
        header = auth.authorization_header()
        assert header.startswith("Basic ")
        # Just verify it doesn't crash and produces something
        assert len(header) > len("Basic ")

    def test_repr(self):
        auth = httprs.BasicAuth(username="user", password="pass")
        assert "user" in repr(auth)


class TestDigestAuth:
    def test_compute_header_returns_digest(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        www_auth = (
            'Digest realm="test@example.com", qop="auth", nonce="abc123", opaque="xyz"'
        )
        header = auth.compute_header("GET", "/path", www_auth)
        assert header.startswith("Digest ")

    def test_compute_header_contains_username(self):
        auth = httprs.DigestAuth(username="myuser", password="mypass")
        www_auth = 'Digest realm="test@example.com", qop="auth", nonce="abc123"'
        header = auth.compute_header("GET", "/path", www_auth)
        assert 'username="myuser"' in header

    def test_compute_header_contains_realm(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        www_auth = 'Digest realm="myrealm", qop="auth", nonce="abc123"'
        header = auth.compute_header("GET", "/path", www_auth)
        assert 'realm="myrealm"' in header

    def test_compute_header_contains_nonce(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        nonce = "unique_nonce_12345"
        www_auth = f'Digest realm="test", qop="auth", nonce="{nonce}"'
        header = auth.compute_header("GET", "/path", www_auth)
        assert f'nonce="{nonce}"' in header

    def test_compute_header_contains_response(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        www_auth = 'Digest realm="test", qop="auth", nonce="abc123"'
        header = auth.compute_header("GET", "/path", www_auth)
        assert "response=" in header

    def test_nonce_count_increments(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        www_auth = 'Digest realm="test", qop="auth", nonce="same_nonce"'
        # Second request with same nonce should have nc=00000002
        header1 = auth.compute_header("GET", "/path", www_auth)
        header2 = auth.compute_header("GET", "/path", www_auth)
        # Extract nc values
        nc1 = [p for p in header1.split(",") if "nc=" in p][0].split("nc=")[1].strip()
        nc2 = [p for p in header2.split(",") if "nc=" in p][0].split("nc=")[1].strip()
        assert int(nc1, 16) + 1 == int(nc2, 16)

    def test_sync_auth_flow_first_yield_no_auth(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        request = httprs.Request("GET", "https://www.example.com")

        flow = auth.sync_auth_flow(request)
        first_request = next(flow)
        # First request should not have Authorization header
        assert first_request.headers.get("authorization") is None

    def test_sync_auth_flow_second_yield_with_auth(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        request = httprs.Request("GET", "https://www.example.com")

        flow = auth.sync_auth_flow(request)
        first_request = next(flow)

        # Send 401 with Digest challenge
        headers = {
            "www-authenticate": 'Digest realm="test", qop="auth", nonce="abc123", opaque="xyz"'
        }
        response = httprs.Response(
            content=b"Auth required",
            status_code=401,
            headers=headers,
            request=first_request,
        )
        second_request = flow.send(response)
        assert second_request is not None
        assert second_request.headers.get("authorization", "").startswith("Digest")

    def test_username_getter(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        assert auth.username == "user"

    def test_repr(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        assert "user" in repr(auth)

    def test_compute_header_query_string_affects_digest(self):
        # RFC 7616 §3.4: digest-uri includes the query string; path-only and
        # path+query must produce different HA2 values and thus different headers.
        auth = httprs.DigestAuth(username="user", password="pass")
        www_auth = 'Digest realm="test", qop="auth", nonce="abc123"'
        header_path_only = auth.compute_header("GET", "/search", www_auth)
        header_with_query = auth.compute_header("GET", "/search?q=hello", www_auth)
        # Different URIs must produce different response hashes.
        assert header_path_only != header_with_query

    def test_sync_auth_flow_uri_includes_query_string(self):
        auth = httprs.DigestAuth(username="user", password="pass")
        www_auth = 'Digest realm="test", qop="auth", nonce="abc123", opaque="xyz"'
        request = httprs.Request("GET", "https://example.com/search?q=hello")

        flow = auth.sync_auth_flow(request)
        next(flow)

        response = httprs.Response(
            content=b"Auth required",
            status_code=401,
            headers={"www-authenticate": www_auth},
            request=request,
        )
        second_request = flow.send(response)
        auth_header = second_request.headers.get("authorization", "")
        # The uri field in the Digest header must include the query string.
        assert 'uri="/search?q=hello"' in auth_header

    def test_sync_auth_flow_uri_matches_compute_header(self):
        # The auth flow and a direct compute_header call with the same path+query
        # must produce matching response= values (same HA2, same nonce count).
        www_auth = 'Digest realm="test", nonce="fixed_nonce"'

        auth_flow = httprs.DigestAuth(username="user", password="pass")
        request = httprs.Request("GET", "https://example.com/search?q=hello")
        flow = auth_flow.sync_auth_flow(request)
        next(flow)
        response = httprs.Response(
            content=b"",
            status_code=401,
            headers={"www-authenticate": www_auth},
            request=request,
        )
        second_request = flow.send(response)
        flow_header = second_request.headers.get("authorization", "")

        auth_direct = httprs.DigestAuth(username="user", password="pass")
        direct_header = auth_direct.compute_header("GET", "/search?q=hello", www_auth)

        def extract_response(h):
            for part in h.split(","):
                part = part.strip()
                if part.startswith("response="):
                    return part.split("=", 1)[1].strip('"')
            return None

        assert extract_response(flow_header) == extract_response(direct_header)
