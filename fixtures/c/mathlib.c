#include "mathlib.h"

int helper(int x) {
    return x + 1;
}

int square(int x) {
    return helper(x) * helper(x);
}
