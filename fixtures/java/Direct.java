package com.example.direct;

public class Direct {
    public static int chain(int n) {
        int a = stepA(n);
        int b = stepB(a);
        return stepC(b);
    }

    private static int stepA(int n) {
        return n + 1;
    }

    private static int stepB(int n) {
        return n * 2;
    }

    private static int stepC(int n) {
        return n - 3;
    }
}
