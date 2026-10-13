#include "shape.hpp"

namespace geo {

double Circle::area() const { return 3.14159 * r * r; }

double Square::area() const { return s * s; }

double total(Shape* s) { return s->area(); }

}  // namespace geo
