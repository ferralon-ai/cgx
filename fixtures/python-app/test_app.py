"""Pytest-style test over the app module.

Exercises: a `test_*` entrypoint hint whose body calls a cross-module symbol
(`app.build_record`), which is what an impacted-tests reverse walk follows.
"""

from app import build_record


def test_build_record():
    record = build_record(3)
    assert record["value"] == 6
