package com.example.shapes;

import java.util.List;

public class Shapes {
    private int count;
    public String label, note;

    public Shapes(int count) {
        this.count = count;
    }

    public int total() {
        return count;
    }

    private void reset() {
        this.count = 0;
    }

    static class Helper {
        protected int seed;
    }
}

interface Drawable {
    void draw();
}

enum Color {
    RED,
    GREEN,
    BLUE;

    public boolean isPrimary() {
        return true;
    }
}

record Point(int x, int y) {
    public int sum() {
        return x + y;
    }
}

abstract class Base {
    abstract void run();
}
