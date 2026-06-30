"""Fixture exercising the Cycle-5 intraprocedural SSA dataflow surface.

`transform` carries the same Copy -> Arith -> Copy(return) chain Go's
`Transform` does, so `flows-to a` walks a -> b -> c -> return and proves the
`DerivesFrom` edge direction (derived -> source) end-to-end.
"""


def transform(a):
    b = a
    c = b + 1
    return c
