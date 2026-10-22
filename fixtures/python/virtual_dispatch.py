# cgx-fixture: duck-typed dispatch
# Covers: untyped attribute call -> calls:virtual (possible), over-approximate


class Howler:
    def howl(self):
        raise NotImplementedError


class Dog(Howler):
    def howl(self):
        return "woof"


class Cat(Howler):
    def howl(self):
        return "meow"


def make_howl(animal):
    return animal.howl()
