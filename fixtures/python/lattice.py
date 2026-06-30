"""Fixture exercising the Cycle-3 inheritance / override model."""

import collections.abc
import unittest


class Base:
    def render(self):
        ...

    def describe(self):
        return "base"


class Mixin:
    def serialize(self):
        return {}


# Single inheritance + a name-match override of Base.render.
class Widget(Base):
    def render(self):
        return "widget"

    def extra(self):
        return 1


# Multiple inheritance: two Inherits edges; serialize overrides Mixin.serialize,
# describe overrides Base.describe.
class Panel(Base, Mixin):
    def serialize(self):
        return {"panel": True}

    def describe(self):
        return "panel"


# Dotted external base (`collections.abc.Sequence`) and a stdlib base
# (`unittest.TestCase`): emit Inherits with the best-name; the resolver drops
# the ungroundable external endpoints.
class Seq(collections.abc.Sequence):
    def __len__(self):
        return 0


class CaseTest(unittest.TestCase):
    def test_it(self):
        assert True


# Keyword arg in superclasses (`metaclass=`) must be filtered — only Base is a
# base, not the metaclass.
class WithMeta(Base, metaclass=type):
    def render(self):
        return "meta"


# No supertype at all → no Inherits, no Overrides.
class Lonely:
    def solo(self):
        return 0
