#include "codec.hpp"
#include "shape.hpp"

static int handle(const net::Packet& p) { return net::parse(p); }

static double report(const geo::Shape& s) { return s.area(); }

int main(int argc, char** argv) {
    net::Packet p{argv[0], argc};
    geo::Circle c;
    c.r = 1.0;
    geo::total(&c);
    report(c);
    return handle(p);
}
