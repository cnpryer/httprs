"""Unit tests for Timeout and Limits configuration classes."""

import httprs


class TestTimeout:
    def test_single_value_sets_all(self):
        t = httprs.Timeout(5.0)
        assert t.connect == 5.0
        assert t.read == 5.0
        assert t.write == 5.0
        assert t.pool == 5.0

    def test_none_default(self):
        t = httprs.Timeout()
        assert t.connect is None
        assert t.read is None
        assert t.write is None
        assert t.pool is None

    def test_keyword_args_individual(self):
        t = httprs.Timeout(connect=2.0, read=10.0, write=5.0, pool=1.0)
        assert t.connect == 2.0
        assert t.read == 10.0
        assert t.write == 5.0
        assert t.pool == 1.0

    def test_positional_with_keyword_override(self):
        # Positional sets the default; keyword overrides individual
        t = httprs.Timeout(5.0, connect=2.0)
        assert t.connect == 2.0
        assert t.read == 5.0
        assert t.write == 5.0
        assert t.pool == 5.0

    def test_equality(self):
        assert httprs.Timeout(5.0) == httprs.Timeout(5.0)
        assert httprs.Timeout(5.0) != httprs.Timeout(3.0)
        assert httprs.Timeout() == httprs.Timeout()

    def test_repr(self):
        r = repr(httprs.Timeout(5.0))
        assert "Timeout" in r
        assert "5" in r

    def test_repr_none(self):
        r = repr(httprs.Timeout())
        assert "None" in r

    def test_float_timeout(self):
        t = httprs.Timeout(0.5)
        assert t.read == 0.5


class TestLimits:
    def test_defaults(self):
        limits = httprs.Limits()
        assert limits.max_connections is None
        assert limits.max_keepalive_connections is None
        assert limits.keepalive_expiry == 5.0

    def test_custom_values(self):
        limits = httprs.Limits(
            max_connections=100,
            max_keepalive_connections=20,
            keepalive_expiry=30.0,
        )
        assert limits.max_connections == 100
        assert limits.max_keepalive_connections == 20
        assert limits.keepalive_expiry == 30.0

    def test_none_keepalive_expiry(self):
        limits = httprs.Limits(keepalive_expiry=None)
        assert limits.keepalive_expiry is None

    def test_equality(self):
        assert httprs.Limits() == httprs.Limits()
        assert httprs.Limits(max_connections=10) != httprs.Limits(max_connections=20)

    def test_repr(self):
        r = repr(httprs.Limits())
        assert "Limits" in r
