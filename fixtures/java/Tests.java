package com.example.shapes;

import org.junit.jupiter.api.Test;

public class Tests {
    @Test
    public void totalReportsTheSeededCount() {
        Shapes shapes = new Shapes(3);
        int n = shapes.total();
        if (n != 3) {
            throw new AssertionError("total");
        }
    }
}
