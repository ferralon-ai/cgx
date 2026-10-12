#include <cstring>

#include "codec.hpp"

namespace net {

int checksum(const char* buf, int n) {
    int sum = 0;
    for (int i = 0; i < n; ++i) {
        sum += buf[i];
    }
    return sum;
}

int decode(char* dst, const char* src, int n) {
    std::memcpy(dst, src, n);
    return checksum(dst, n);
}

int decode(char* dst, const char* src) { return decode(dst, src, std::strlen(src)); }

}  // namespace net
