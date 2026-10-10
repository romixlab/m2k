"""Offline smoke test: no M2K or emulator needed. Exercises the module boundary (classes exist, exceptions map
cleanly) without talking to real hardware; `tests/device.rs` (opt-in, M2K_TEST_URI) covers the real thing."""

import time

import m2k
import pytest


def test_module_shape():
    assert hasattr(m2k, "M2k")
    assert issubclass(m2k.M2kError, Exception)


def test_connect_refused_raises_m2kerror_quickly():
    start = time.monotonic()
    with pytest.raises(m2k.M2kError):
        # nothing listens on TCP port 1: refused right away, not a timeout
        m2k.M2k(uri="ip:127.0.0.1:1", timeout=2.0)
    assert time.monotonic() - start < 2.0


def test_connect_timeout_is_bounded():
    start = time.monotonic()
    with pytest.raises(m2k.M2kError):
        # 203.0.113.0/24 is TEST-NET-3 (RFC 5737): never routed, so the connect blocks until our timeout
        m2k.M2k(uri="ip:203.0.113.1", timeout=0.3)
    assert time.monotonic() - start < 2.0
