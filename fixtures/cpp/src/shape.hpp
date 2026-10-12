#pragma once

namespace geo {

struct Shape {
    virtual ~Shape() {}
    virtual double area() const = 0;
};

struct Circle : Shape {
    double r;
    double area() const override;
};

struct Square : Shape {
    double s;
    double area() const override;
};

double total(Shape* s);

}  // namespace geo
