package com.example.flow;

/** Intraprocedural dataflow (SSA) fixture: every lowered construct in one file. */
public class Dataflow {

    // Straight-line copy chain a -> b -> c.
    int chain(int a) {
        int b = a;
        int c = b;
        return c;
    }

    // Binary arithmetic: c derives from both operands.
    int arith(int a, int b) {
        int c = a + b;
        return c;
    }

    // Field projection at depth 1.
    int project(Point u) {
        int n = u.x;
        return n;
    }

    // Deep field access truncates to the base local.
    int deep(Wrapper u) {
        int n = u.inner.value;
        return n;
    }

    // Opaque call: r carries no intraprocedural source, only the callee + args.
    int opaque(int a) {
        int r = helper(a);
        return r;
    }

    // Ternary selects from both branches.
    int select(boolean c, int a, int b) {
        int x = c ? a : b;
        return x;
    }

    // Reassignment bumps the SSA version of x.
    int reassign(int a, int b) {
        int x = a;
        x = b;
        return x;
    }

    // For-each binds the loop var from the iterable.
    int forEach(int[] items) {
        int last = 0;
        for (int e : items) {
            last = e;
        }
        return last;
    }

    int helper(int x) {
        return x;
    }

    static class Point {
        int x;
    }

    static class Wrapper {
        Point inner;
    }
}
