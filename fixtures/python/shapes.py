"""Fixture exercising every Cycle-1 Python def kind."""

from abc import abstractmethod

MAX_SIDES = 12
default_name = "shape"
WIDTH, HEIGHT = 4, 3


def area(width, height):
    return width * height


def _internal_helper():
    pass


class Shape:
    sides = 0

    def __init__(self, name):
        self.name = name

    def describe(self):
        return self.name

    @abstractmethod
    def render(self):
        ...

    def _hidden(self):
        pass


@decorator
def decorated_fn():
    pass


class Circle(Shape):
    @staticmethod
    def unit():
        return Circle()
