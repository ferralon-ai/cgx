package com.example.lattice;

interface Shape {
    double area();
}

interface Named {
    String label();
}

// Interface extending multiple interfaces -> two `Inherits` relations.
interface Drawable extends Shape, Named {
    void draw();
}

abstract class Base {
    void run() {}

    String describe() {
        return "base";
    }
}

// Class with a superclass (`Inherits`) and an interface (`Implements`).
class Circle extends Base implements Drawable {
    @Override
    public double area() {
        return 3.14;
    }

    @Override
    public String label() {
        return "circle";
    }

    @Override
    public void draw() {}

    // Override of Base.describe() WITHOUT @Override: caught by in-file name+arity
    // matching against the in-file supertype Base.
    String describe() {
        return "circle";
    }

    // Not an override: no supertype declares radius().
    int radius() {
        return 1;
    }
}

// Enum implementing an interface, with an annotated override.
enum Mode implements Named {
    FAST,
    SLOW;

    @Override
    public String label() {
        return "mode";
    }
}

// Record implementing an interface, with an annotated override.
record Pair(int a, int b) implements Named {
    @Override
    public String label() {
        return "pair";
    }
}

// Superclass is an external/JDK type (not declared in this file): the relation is
// still emitted with the best-available simple name; the resolver grounds no edge.
class Box extends RuntimeException {}
