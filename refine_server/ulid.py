"""ULID generation. 26-char Crockford-base32, time-ordered."""
from __future__ import annotations

import os
import time

_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def new_ulid() -> str:
    ts_ms = int(time.time() * 1000)
    rand = int.from_bytes(os.urandom(10), "big")
    n = (ts_ms << 80) | rand
    chars = []
    for _ in range(26):
        chars.append(_ALPHABET[n & 0x1F])
        n >>= 5
    return "".join(reversed(chars))


def is_ulid(s: str) -> bool:
    if len(s) != 26:
        return False
    return all(c in _ALPHABET for c in s.upper())
