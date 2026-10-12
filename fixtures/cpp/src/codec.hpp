#pragma once

namespace net {

struct Packet {
    const char* data;
    int len;
};

int checksum(const char* buf, int n);
int decode(char* dst, const char* src, int n);
int decode(char* dst, const char* src);

template <typename T>
int parse(const T& input) {
    char out[64];
    return decode(out, input.data, input.len);
}

}  // namespace net
