# cgx-fixture: direct calls
# Covers: module-level function calls (certain)


def step_a(n):
    return n + 1


def step_b(n):
    return n * 2


def step_c(n):
    return n - 3


def chain(n):
    a = step_a(n)
    b = step_b(a)
    return step_c(b)
