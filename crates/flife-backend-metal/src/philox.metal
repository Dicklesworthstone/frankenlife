// FL-032: fnp-philox-semantic-v1's blocks on the device, after
// softfloat64.metal (flife-core's rng). An address's block is the first
// four raw outputs of NumPy's Philox with key [world seed, stream code] and
// counter [entity, ordinal, lane, draw]; NumPy increments the counter before
// each block, so the block is Philox4x64-10 of the counter plus one, mod
// 2^256.

constant ulong PHILOX_M0 = 0xD2E7470EE14C6C93ul;
constant ulong PHILOX_M1 = 0xCA5A826395121157ul;
constant ulong PHILOX_W0 = 0x9E3779B97F4A7C15ul;
constant ulong PHILOX_W1 = 0xBB67AE8584CAA73Bul;

struct PhiloxBlock {
    ulong words[4];
};

// Philox4x64-10 of `counter`, plus one with its carries, under `key`.
static PhiloxBlock philox_block(ulong k0, ulong k1, ulong c0, ulong c1, ulong c2, ulong c3) {
    c0 += 1ul;
    bool carry = c0 == 0ul;
    c1 += carry ? 1ul : 0ul;
    carry = carry && c1 == 0ul;
    c2 += carry ? 1ul : 0ul;
    carry = carry && c2 == 0ul;
    c3 += carry ? 1ul : 0ul;
    for (uint round = 0u; round < 10u; ++round) {
        ulong hi0 = mulhi(PHILOX_M0, c0);
        ulong lo0 = PHILOX_M0 * c0;
        ulong hi1 = mulhi(PHILOX_M1, c2);
        ulong lo1 = PHILOX_M1 * c2;
        ulong n0 = hi1 ^ c1 ^ k0;
        ulong n2 = hi0 ^ c3 ^ k1;
        c0 = n0;
        c1 = lo1;
        c2 = n2;
        c3 = lo0;
        k0 += PHILOX_W0;
        k1 += PHILOX_W1;
    }
    PhiloxBlock block;
    block.words[0] = c0;
    block.words[1] = c1;
    block.words[2] = c2;
    block.words[3] = c3;
    return block;
}

// rng::half_open_unit: the leading 52 bits `k` of a word as `k 2^-52`,
// exactly (a 52-bit integer is exact in binary64, and so is the scaling).
static d64 d_half_open_unit(ulong word) {
    ulong k = word >> 12;
    if (k == 0ul) {
        return 0ul;
    }
    uint top = 63u - uint(clz(k));
    return (ulong(top + 971u) << 52) | ((k << (52u - top)) & D_FRAC);
}
