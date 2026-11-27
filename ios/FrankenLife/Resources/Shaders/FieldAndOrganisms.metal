#include <metal_stdlib>
using namespace metal;

/* ============================================================================
 * FrankenLife iOS Native Metal 120Hz HDR Renderer
 *
 * Implements:
 * - Seamless toroidal field rendering (Energy, Composition, Channel C,
 *   V2 TrailCarrier, V2 VascularConductance, and honest Raw Linear Grey)
 * - 1-draw-call GPU instanced SDF organism rendering with GPU-side toroidal
 *   minimum-image epoch interpolation (`prevPos -> currPos`)
 * - All 9 canonical FrankenLife color mappings (Lineage, Energy, Generation,
 *   Motion, Topology, Affinity OKLab, Lineage-Energy OKLab, V2 CellState,
 *   V2 Polarity with 3D apical ticks)
 * - 3 Looks: Glow (HDR bioluminescent), Sphere (3D microscope), Raw (honest G5)
 * - Dual-Kawase HDR Bloom + EDR-aware ACES compositor + interactive overlays
 * ============================================================================ */

struct FrameUniforms {
    float2 viewportSize;       // drawable pixels (width, height)
    float2 worldExtent;        // (L_x, L_y)
    float2 cameraCenter;       // world coords at screen center
    float  worldUnitsPerPixel; // zoom inverse: world units per drawable pixel
    float  interpAlpha;        // [0, 1] toroidal interpolation between prev and curr epoch

    float  maxSpeed;
    float  attemptEnergy;
    float  fieldScale;         // decaying display scale for active field plane
    float  pointRadiusPx;      // base radius in drawable pixels

    uint   colorMapping;       // 0..8 matching ColorMapping enum
    uint   lookMode;           // 0 = Glow, 1 = Sphere, 2 = Raw
    uint   fieldMode;          // 0 = Energy, 1 = Composition, 2 = Component, 3 = Trail, 4 = Vascular
    uint   showField;          // 0 or 1

    float  bloomStrength;
    float  edrHeadroom;        // 1.0 on SDR, >1.0 on ProMotion HDR OLED
    float  timeSeconds;        // animation clock for selection ring pulse
    float  pixelScale;         // display scale factor (e.g. 3.0 on iPhone Pro Max)

    // Interactive overlays (selected organism, followed heir, measurement disk, touch brush)
    // Layout verified by XCTest (160 bytes total, zero hidden padding).
    float2 selectedPos;
    float2 selectedPrevPos;
    float2 followedPos;
    float2 followedPrevPos;
    float2 measureCenter;
    float2 brushCenter;

    float  measureRadius;      // > 0 when measurement disk active
    float  brushRadius;        // > 0 when brush preview active
    uint   hasSelected;
    uint   hasFollowed;

    uint   organismCount;      // base live organism count per torus tile
    uint   tilesX;             // odd periodic tile repeat along X (1, 3, 5)
    uint   tilesY;             // odd periodic tile repeat along Y (1, 3, 5)
    uint   _pad0;
};

struct OrganismGPU {
    float2 currPos;
    float2 prevPos;
    float2 velocity;
    float  energy;
    float  birthAlpha;         // 0..1 smooth fade-in for newborns
    uint   lineageLo;
    uint   lineageHi;
    uint   generation;
    uint   packedMeta;         // [7:0] topo, [15:8] cellState (i8), [31:16] polarity (i16)
    float  aff0;
    float  aff1;
    float  aff2;
    uint   flags;              // bit 0 = selected, bit 1 = followed
};

/* ============================================================================
 * Canonical FrankenLife Palettes & OKLab Color Math
 * ============================================================================ */

constant float3 LINEAGE_SRGB[12] = {
    float3(0x2E, 0xC4, 0xB6) / 255.0f, // teal
    float3(0x9B, 0x5D, 0xE5) / 255.0f, // violet
    float3(0xF1, 0x5B, 0xB5) / 255.0f, // magenta
    float3(0x9B, 0xF6, 0xFF) / 255.0f, // pale cyan
    float3(0xFF, 0xB6, 0x27) / 255.0f, // amber
    float3(0xB5, 0xE6, 0x1D) / 255.0f, // lime
    float3(0xFF, 0x6F, 0x59) / 255.0f, // coral
    float3(0x4E, 0xA8, 0xDE) / 255.0f, // sky
    float3(0xFE, 0xE4, 0x40) / 255.0f, // gold
    float3(0xFF, 0x99, 0xC8) / 255.0f, // rose
    float3(0x7A, 0xE5, 0x82) / 255.0f, // mint
    float3(0xC8, 0xB6, 0xFF) / 255.0f  // lavender
};

constant float3 ENERGY_RAMP_SRGB[9] = {
    float3(0x3B, 0x0F, 0x70) / 255.0f,
    float3(0x64, 0x1A, 0x80) / 255.0f,
    float3(0x8C, 0x29, 0x81) / 255.0f,
    float3(0xB7, 0x37, 0x79) / 255.0f,
    float3(0xDE, 0x49, 0x68) / 255.0f,
    float3(0xF7, 0x70, 0x5C) / 255.0f,
    float3(0xFE, 0x9F, 0x6D) / 255.0f,
    float3(0xFE, 0xCF, 0x92) / 255.0f,
    float3(0xFC, 0xFD, 0xBF) / 255.0f
};

constant float3 GENERATION_RAMP_SRGB[7] = {
    float3(0x9C, 0x17, 0x9E) / 255.0f,
    float3(0xBD, 0x37, 0x86) / 255.0f,
    float3(0xD8, 0x57, 0x6B) / 255.0f,
    float3(0xED, 0x79, 0x53) / 255.0f,
    float3(0xFB, 0x9F, 0x3A) / 255.0f,
    float3(0xFD, 0xCA, 0x26) / 255.0f,
    float3(0xF0, 0xF9, 0x21) / 255.0f
};

constant float3 MOTION_WHEEL_SRGB[6] = {
    float3(0xFF, 0x6F, 0x59) / 255.0f,
    float3(0xFF, 0xB6, 0x27) / 255.0f,
    float3(0xB5, 0xE6, 0x1D) / 255.0f,
    float3(0x2E, 0xC4, 0xB6) / 255.0f,
    float3(0x4E, 0xA8, 0xDE) / 255.0f,
    float3(0x9B, 0x5D, 0xE5) / 255.0f
};

constant float3 TOPOLOGY_SRGB[8] = {
    float3(0x9B, 0xF6, 0xFF) / 255.0f,
    float3(0x2E, 0xC4, 0xB6) / 255.0f,
    float3(0xB5, 0xE6, 0x1D) / 255.0f,
    float3(0xFF, 0xB6, 0x27) / 255.0f,
    float3(0xF1, 0x5B, 0xB5) / 255.0f,
    float3(0x9B, 0x5D, 0xE5) / 255.0f,
    float3(0xFF, 0x6F, 0x59) / 255.0f,
    float3(0xFE, 0xE4, 0x40) / 255.0f
};

constant float3 FIELD_PALETTE_SRGB[6] = {
    float3(0x02, 0x03, 0x08) / 255.0f,
    float3(0x07, 0x16, 0x33) / 255.0f,
    float3(0x0B, 0x33, 0x5C) / 255.0f,
    float3(0x13, 0x5E, 0x76) / 255.0f,
    float3(0x3A, 0x93, 0x86) / 255.0f,
    float3(0xB8, 0xD8, 0x9C) / 255.0f
};

constant float3 COMPONENT_PALETTE_SRGB[5] = {
    float3(0x2C, 0x7B, 0xB6) / 255.0f,
    float3(0xAB, 0xD9, 0xE9) / 255.0f,
    float3(0xFF, 0xFF, 0xBF) / 255.0f,
    float3(0xFD, 0xAE, 0x61) / 255.0f,
    float3(0xD7, 0x19, 0x1C) / 255.0f
};

constant float3 TRAIL_PALETTE_SRGB[6] = {
    float3(0x03, 0x02, 0x06) / 255.0f,
    float3(0x24, 0x0E, 0x2E) / 255.0f,
    float3(0x6E, 0x1F, 0x3D) / 255.0f,
    float3(0xB8, 0x4A, 0x24) / 255.0f,
    float3(0xEE, 0x9B, 0x28) / 255.0f,
    float3(0xFF, 0xF0, 0xB8) / 255.0f
};

constant float3 VASCULAR_PALETTE_SRGB[6] = {
    float3(0x04, 0x02, 0x04) / 255.0f,
    float3(0x2E, 0x07, 0x16) / 255.0f,
    float3(0x6B, 0x0F, 0x2B) / 255.0f,
    float3(0xB8, 0x25, 0x43) / 255.0f,
    float3(0xF2, 0x6A, 0x6B) / 255.0f,
    float3(0xFF, 0xD8, 0xD4) / 255.0f
};

static inline float srgb_to_linear_ch(float c) {
    return (c <= 0.04045f) ? (c / 12.92f) : pow((c + 0.055f) / 1.055f, 2.4f);
}

static inline float3 srgb_to_linear(float3 c) {
    return float3(srgb_to_linear_ch(c.r), srgb_to_linear_ch(c.g), srgb_to_linear_ch(c.b));
}

static inline float3 linear_to_oklab(float3 c) {
    float l = 0.4122214708f * c.r + 0.5363325363f * c.g + 0.0514459929f * c.b;
    float m = 0.2119034982f * c.r + 0.6806995451f * c.g + 0.1073969566f * c.b;
    float s = 0.0883024619f * c.r + 0.2817188376f * c.g + 0.6299787005f * c.b;
    float l_ = pow(max(l, 0.0f), 1.0f / 3.0f);
    float m_ = pow(max(m, 0.0f), 1.0f / 3.0f);
    float s_ = pow(max(s, 0.0f), 1.0f / 3.0f);
    return float3(
        0.2104542553f * l_ + 0.7936177850f * m_ - 0.0040720468f * s_,
        1.9779984951f * l_ - 2.4285922050f * m_ + 0.4505937099f * s_,
        0.0259040371f * l_ + 0.7827717662f * m_ - 0.8086757660f * s_
    );
}

static inline float3 oklab_to_linear(float3 lab) {
    float l_ = lab.x + 0.3963377774f * lab.y + 0.2158037573f * lab.z;
    float m_ = lab.x - 0.1055613458f * lab.y - 0.0638541728f * lab.z;
    float s_ = lab.x - 0.0894841775f * lab.y - 1.2914855480f * lab.z;
    float l = l_ * l_ * l_;
    float m = m_ * m_ * m_;
    float s = s_ * s_ * s_;
    return float3(
        +4.0767416621f * l - 3.3077115913f * m + 0.2309699292f * s,
        -1.2684380046f * l + 2.6097574011f * m - 0.3413193965f * s,
        -0.0041960863f * l - 0.7034186147f * m + 1.7076147010f * s
    );
}

static inline float3 oklab_gamut_clamped_linear(float L, float a, float b) {
    float3 full = oklab_to_linear(float3(L, a, b));
    if (full.r >= 0.0f && full.r <= 1.0f && full.g >= 0.0f && full.g <= 1.0f && full.b >= 0.0f && full.b <= 1.0f) {
        return full;
    }
    float lo = 0.0f;
    float hi = 1.0f;
    for (int i = 0; i < 10; ++i) {
        float mid = 0.5f * (lo + hi);
        float3 c = oklab_to_linear(float3(L, mid * a, mid * b));
        bool ok = (c.r >= 0.0f && c.r <= 1.0f && c.g >= 0.0f && c.g <= 1.0f && c.b >= 0.0f && c.b <= 1.0f);
        lo = ok ? mid : lo;
        hi = ok ? hi : mid;
    }
    return clamp(oklab_to_linear(float3(L, lo * a, lo * b)), 0.0f, 1.0f);
}

static inline uint lineage_slot(uint lo, uint hi) {
    ulong z = (ulong(hi) << 32) | ulong(lo);
    z += 0x9E3779B97F4A7C15UL;
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9UL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBUL;
    z ^= (z >> 31);
    return uint(z % 12UL);
}

static inline float3 sample_ramp6_srgb(constant float3 (&stops)[6], float t) {
    float tc = clamp(t, 0.0f, 1.0f) * 5.0f;
    uint idx = min(uint(floor(tc)), 4u);
    float frac = tc - float(idx);
    return mix(stops[idx], stops[idx + 1], frac);
}

static inline float3 sample_ramp5_srgb(constant float3 (&stops)[5], float t) {
    float tc = clamp(t, 0.0f, 1.0f) * 4.0f;
    uint idx = min(uint(floor(tc)), 3u);
    float frac = tc - float(idx);
    return mix(stops[idx], stops[idx + 1], frac);
}

static inline float3 sample_ramp7_srgb(constant float3 (&stops)[7], float t) {
    float tc = clamp(t, 0.0f, 1.0f) * 6.0f;
    uint idx = min(uint(floor(tc)), 5u);
    float frac = tc - float(idx);
    return mix(stops[idx], stops[idx + 1], frac);
}

static inline float3 sample_ramp9_srgb(constant float3 (&stops)[9], float t) {
    float tc = clamp(t, 0.0f, 1.0f) * 8.0f;
    uint idx = min(uint(floor(tc)), 7u);
    float frac = tc - float(idx);
    return mix(stops[idx], stops[idx + 1], frac);
}

static inline float3 affinity_linear(float3 q) {
    float3 g = q * 6.531972647421808f;
    float squash = sqrt(1.0f + dot(g, g));
    float3 s = g / squash;
    float L = 0.46f + 0.12f * s.z;
    return oklab_gamut_clamped_linear(L, 0.26f * s.x, 0.26f * s.y);
}

static inline float3 compute_organism_linear_color(OrganismGPU org, constant FrameUniforms& u) {
    uint slot = lineage_slot(org.lineageLo, org.lineageHi);

    switch (u.colorMapping) {
        case 0: // Lineage
            return srgb_to_linear(LINEAGE_SRGB[slot]);

        case 1: { // Energy
            float e = max(org.energy, 0.0f);
            float t = e / (e + 2.0f);
            return srgb_to_linear(sample_ramp9_srgb(ENERGY_RAMP_SRGB, t));
        }

        case 2: { // Generation
            float t = log(1.0f + float(org.generation)) / log(257.0f);
            return srgb_to_linear(sample_ramp7_srgb(GENERATION_RAMP_SRGB, t));
        }

        case 3: { // Motion
            float speed = length(org.velocity);
            float frac = (u.maxSpeed > 0.0f) ? sqrt(clamp(speed / u.maxSpeed, 0.0f, 1.0f)) : 0.0f;
            float3 still = float3(0x46, 0x4B, 0x5A) / 255.0f;
            if (speed < 1e-6f) {
                return srgb_to_linear(still);
            }
            float turn = atan2(-org.velocity.y, org.velocity.x) / (2.0f * M_PI_F);
            if (turn < 0.0f) turn += 1.0f;
            float pos = turn * 6.0f;
            uint idx = uint(floor(pos)) % 6u;
            float f = pos - floor(pos);
            float3 hue = mix(MOTION_WHEEL_SRGB[idx], MOTION_WHEEL_SRGB[(idx + 1u) % 6u], f);
            return srgb_to_linear(mix(still, hue, frac));
        }

        case 4: { // Topology
            uint topo = org.packedMeta & 0xFFu;
            return srgb_to_linear(TOPOLOGY_SRGB[topo & 7u]);
        }

        case 5: // Affinity
            return affinity_linear(float3(org.aff0, org.aff1, org.aff2));

        case 6: { // Lineage-Energy
            float3 baseLab = linear_to_oklab(srgb_to_linear(LINEAGE_SRGB[slot]));
            float denom = max(u.attemptEnergy, 1e-5f);
            float L = clamp(0.55f * max(org.energy, 0.0f) / denom, 0.0f, 1.0f);
            return oklab_gamut_clamped_linear(L, baseLab.y, baseLab.z);
        }

        case 7: { // V2 Cell-State (h0 in bits 8..15 as signed int8)
            int raw = int(org.packedMeta << 16) >> 24;
            float s = clamp(float(raw) / 127.0f, -1.0f, 1.0f);
            float t = 0.5f * (s + 1.0f);
            float L = mix(0.36f, 0.93f, t);
            float3 baseLab = linear_to_oklab(srgb_to_linear(LINEAGE_SRGB[slot]));
            float ang = s * 0.30f;
            float cs = cos(ang), sn = sin(ang);
            float aRot = baseLab.y * cs - baseLab.z * sn;
            float bRot = baseLab.y * sn + baseLab.z * cs;
            float cScale = (s < 0.0f) ? (1.0f - 0.15f * s) : (1.0f - 0.45f * s);
            return oklab_gamut_clamped_linear(L, aRot * cScale, bRot * cScale);
        }

        case 8: { // V2 Polarity (signed int16 in bits 16..31; -32768 = unpolarized)
            int pRaw = int(org.packedMeta) >> 16;
            if (pRaw == -32768) {
                return srgb_to_linear(float3(0x58, 0x5E, 0x6E) / 255.0f);
            }
            float ang = (float(pRaw) / 32767.0f) * M_PI_F;
            return oklab_gamut_clamped_linear(0.74f, 0.145f * cos(ang), 0.145f * sin(ang));
        }

        default:
            return srgb_to_linear(LINEAGE_SRGB[slot]);
    }
}

/* ============================================================================
 * Pass 1: Seamless Periodic Toroidal Field Layer
 * ============================================================================ */

struct FullscreenOut {
    float4 position [[position]];
    float2 uv;
};

vertex FullscreenOut fullscreen_vertex(uint vid [[vertex_id]]) {
    float2 pos[3] = {
        float2(-1.0f, -1.0f),
        float2( 3.0f, -1.0f),
        float2(-1.0f,  3.0f)
    };
    FullscreenOut out;
    out.position = float4(pos[vid], 0.0f, 1.0f);
    out.uv = float2((pos[vid].x + 1.0f) * 0.5f, (1.0f - pos[vid].y) * 0.5f);
    return out;
}

fragment float4 field_fragment(
    FullscreenOut in [[stage_in]],
    constant FrameUniforms& u [[buffer(0)]],
    texture2d<float> energyTex [[texture(0)]],
    texture2d<float> compTex [[texture(1)]],
    texture2d<float> v2OverlayTex [[texture(2)]]
) {
    // Deep obsidian scientific backdrop
    float3 bg = (u.lookMode == 0)
        ? srgb_to_linear(float3(0.008f, 0.011f, 0.025f))
        : float3(0.004f, 0.004f, 0.006f);

    if (u.showField == 0) {
        return float4(bg, 1.0f);
    }

    constexpr sampler torSampler(
        mag_filter::linear,
        min_filter::linear,
        address::repeat
    );

    float2 pixelOffset = (in.uv - 0.5f) * u.viewportSize;
    float2 worldPos = u.cameraCenter + pixelOffset * u.worldUnitsPerPixel;
    float2 texUV = fract(worldPos / u.worldExtent);

    float energyDensity = max(energyTex.sample(torSampler, texUV).r, 0.0f);
    float scale = max(u.fieldScale, 1e-6f);
    float rawNorm = clamp(energyDensity / scale, 0.0f, 1.0f);
    float glowNorm = sqrt(rawNorm);

    bool isRawLook = (u.lookMode != 0); // Raw and Sphere share honest linear greyscale field

    float3 col = bg;
    if (u.fieldMode == 0) {
        // Carrier-0 Energy Density
        if (isRawLook) {
            col = float3(rawNorm * 0.65f);
        } else {
            // Subtle dark-ocean framing so bioluminescent colonies pop with rich contrast
            col = srgb_to_linear(sample_ramp6_srgb(FIELD_PALETTE_SRGB, glowNorm)) * 0.48f;
        }
    } else if (u.fieldMode == 1) {
        // Chemical Composition (Hadamard projection tinted by affinity color, lit by energy)
        float4 compSample = compTex.sample(torSampler, texUV);
        float3 hue = affinity_linear(compSample.rgb);
        float lum = isRawLook ? rawNorm : (glowNorm * 0.62f);
        col = hue * lum;
    } else if (u.fieldMode == 2) {
        // Single Moment Channel (RdYlBu diverging palette, lit by energy)
        float4 compSample = compTex.sample(torSampler, texUV);
        float chVal = clamp(compSample.a, -1.0f, 1.0f);
        float3 hue = srgb_to_linear(sample_ramp5_srgb(COMPONENT_PALETTE_SRGB, 0.5f * (chVal + 1.0f)));
        float lum = isRawLook ? rawNorm : (glowNorm * 0.62f);
        col = hue * lum;
    } else if (u.fieldMode == 3) {
        // V2 Carrier-1 Persistent Pheromone Trails
        float trailVal = max(v2OverlayTex.sample(torSampler, texUV).r, 0.0f);
        float norm = clamp(trailVal / scale, 0.0f, 1.0f);
        if (isRawLook) {
            col = float3(norm * 0.7f);
        } else {
            float logT = log(1.0f + 15.0f * norm) / log(16.0f);
            col = srgb_to_linear(sample_ramp6_srgb(TRAIL_PALETTE_SRGB, logT)) * 0.72f;
        }
    } else {
        // V2 Murray/Physarum Vascular Conductance
        float vascVal = max(v2OverlayTex.sample(torSampler, texUV).g, 0.0f);
        float norm = clamp(vascVal / scale, 0.0f, 1.0f);
        if (isRawLook) {
            col = float3(norm * 0.7f);
        } else {
            float logT = log(1.0f + 15.0f * norm) / log(16.0f);
            col = srgb_to_linear(sample_ramp6_srgb(VASCULAR_PALETTE_SRGB, logT)) * 0.72f;
        }
    }

    return float4(col, 1.0f);
}

/* ============================================================================
 * Pass 2: 1-Draw-Call Instanced SDF Organism Renderer (with 120Hz Interpolation)
 * ============================================================================ */

struct OrganismVertOut {
    float4 position [[position]];
    float2 localCoord;     // [-1, 1] inside billboard quad (extended for Glow halo / polarity tick)
    float3 linearColor;
    float  radiusPx;
    float  energyRatio;
    float  alpha;
    float  polarityAngle;  // NaN if unpolarized
    uint   hasPolarity;
    uint   flags;
};

vertex OrganismVertOut organism_vertex(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    constant FrameUniforms& u [[buffer(0)]],
    const device OrganismGPU* organisms [[buffer(1)]]
) {
    uint orgCount = max(u.organismCount, 1u);
    uint orgIdx = iid % orgCount;
    uint tileIdx = iid / orgCount;
    uint tx = max(u.tilesX, 1u);
    uint ty = max(u.tilesY, 1u);
    int tileOffsetX = int(tileIdx % tx) - int(tx / 2u);
    int tileOffsetY = int(tileIdx / tx) - int(ty / 2u);

    OrganismGPU org = organisms[orgIdx];

    // Toroidal minimum-image interpolation between previous and current epoch (silky 120 FPS!)
    float2 delta = org.currPos - org.prevPos;
    delta -= u.worldExtent * round(delta / u.worldExtent);
    float2 worldPos = org.prevPos + delta * u.interpAlpha;

    // Seamless periodic displacement tiled cleanly across the entire portrait/landscape viewport
    float2 rel = worldPos - u.cameraCenter;
    rel -= u.worldExtent * round(rel / u.worldExtent);
    rel += float2(float(tileOffsetX), float(tileOffsetY)) * u.worldExtent;

    float2 screenPx = rel / u.worldUnitsPerPixel;

    // Energy-modulated body size in Glow mode; consistent true-dot size in Sphere/Raw
    float eNorm = clamp(org.energy / max(u.attemptEnergy, 1.0f), 0.15f, 2.2f);
    float bodyRadiusPx = u.pointRadiusPx * u.pixelScale;
    if (u.lookMode == 0) {
        bodyRadiusPx *= mix(0.76f, 1.28f, clamp(eNorm * 0.65f, 0.0f, 1.0f));
    }

    // Billboard expansion factor: crisp Glow aura without muddy overlap; Sphere leaves room for apical tick
    float quadScale = (u.lookMode == 0) ? 2.15f : ((u.lookMode == 1) ? 1.65f : 1.25f);
    float quadHalfPx = max(bodyRadiusPx * quadScale, 2.0f);

    float2 quadCorners[6] = {
        float2(-1.0f, -1.0f), float2( 1.0f, -1.0f), float2(-1.0f,  1.0f),
        float2(-1.0f,  1.0f), float2( 1.0f, -1.0f), float2( 1.0f,  1.0f)
    };
    float2 corner = quadCorners[vid];
    float2 pixelPos = screenPx + corner * quadHalfPx;

    float2 ndc = float2(
        (pixelPos.x / (0.5f * u.viewportSize.x)),
        -(pixelPos.y / (0.5f * u.viewportSize.y))
    );

    int pRaw = int(org.packedMeta) >> 16;
    bool hasPol = (pRaw != -32768);
    float polAngle = hasPol ? ((float(pRaw) / 32767.0f) * M_PI_F) : 0.0f;

    OrganismVertOut out;
    out.position = float4(ndc, 0.0f, 1.0f);
    out.localCoord = corner * quadScale; // unit circle is r = 1.0
    out.linearColor = compute_organism_linear_color(org, u);
    out.radiusPx = bodyRadiusPx;
    out.energyRatio = eNorm;
    out.alpha = org.birthAlpha;
    out.polarityAngle = polAngle;
    out.hasPolarity = hasPol ? 1u : 0u;
    out.flags = org.flags;
    return out;
}

fragment float4 organism_fragment(
    OrganismVertOut in [[stage_in]],
    constant FrameUniforms& u [[buffer(0)]]
) {
    float r = length(in.localCoord);
    float aa = max(1.25f / max(in.radiusPx, 1.0f), 0.04f);

    if (u.lookMode == 0) {
        // GLOW LOOK: Crisp jewel-toned bioluminescent cell + restrained luminous rim (zero white washout)
        if (r > 2.15f) discard_fragment();

        float core = 1.0f - smoothstep(0.62f - aa, 0.96f + aa, r);
        float innerGlow = exp(-3.4f * r * r) * 0.42f;
        float outerHalo = exp(-1.25f * r * r) * 0.14f;

        // Subtle luminous center highlight keeping full chromatic saturation
        float edrBoost = mix(1.0f, min(u.edrHeadroom, 1.65f), 0.45f);
        float3 cellCol = mix(in.linearColor, float3(1.0f, 0.99f, 0.98f), 0.14f * core * clamp(in.energyRatio * 0.5f, 0.0f, 1.0f));
        float3 rgb = cellCol * (core * 0.92f + innerGlow) * edrBoost
                   + in.linearColor * outerHalo;

        float alpha = clamp((core * 0.96f + innerGlow * 0.65f + outerHalo * 0.45f) * in.alpha, 0.0f, 1.0f);
        if (alpha < 0.004f) discard_fragment();
        return float4(rgb * in.alpha, alpha);

    } else if (u.lookMode == 1) {
        // SPHERE LOOK: 3D microscope true-dot shading + V2 apical polarity orientation tick
        float bodyAlpha = 1.0f - smoothstep(1.0f - aa, 1.0f + aa, r);
        float3 col = float3(0.0f);
        float finalAlpha = 0.0f;

        if (bodyAlpha > 0.001f) {
            float2 p = in.localCoord;
            float z = sqrt(max(1.0f - dot(p, p), 0.0f));
            float3 N = normalize(float3(p.x, -p.y, z));
            float3 L = normalize(float3(-0.38f, 0.48f, 0.79f));
            float3 V = float3(0.0f, 0.0f, 1.0f);
            float3 H = normalize(L + V);

            float diff = max(dot(N, L), 0.0f);
            float spec = pow(max(dot(N, H), 0.0f), 28.0f) * 0.42f;
            float fresnel = pow(1.0f - max(N.z, 0.0f), 2.5f) * 0.26f;
            float ambient = 0.24f;

            col = in.linearColor * (ambient + 0.88f * diff)
                + float3(spec)
                + mix(in.linearColor, float3(1.0f), 0.5f) * fresnel;
            finalAlpha = bodyAlpha * in.alpha;
        }

        // Crisp V2 apical-basal polarity tick when polarity view is active
        if (u.colorMapping == 8 && in.hasPolarity != 0u) {
            float2 dir = float2(cos(in.polarityAngle), sin(in.polarityAngle));
            float2 perp = float2(-dir.y, dir.x);
            float along = dot(in.localCoord, dir);
            float across = abs(dot(in.localCoord, perp));
            float inShaft = step(0.35f, along) * (1.0f - smoothstep(1.42f, 1.48f, along))
                          * (1.0f - smoothstep(0.14f, 0.20f, across));
            if (inShaft > 0.01f) {
                float3 tickCol = float3(0.96f, 0.96f, 0.92f);
                col = mix(col, tickCol, inShaft * 0.95f);
                finalAlpha = max(finalAlpha, inShaft * 0.95f * in.alpha);
            }
        }

        if (finalAlpha < 0.003f) discard_fragment();
        return float4(col * finalAlpha, finalAlpha);

    } else {
        // RAW LOOK (Gate G5): Honest flat unblended circle, zero halo, zero bloom
        float a = (1.0f - smoothstep(1.0f - aa, 1.0f + aa, r)) * in.alpha;
        if (a < 0.003f) discard_fragment();
        return float4(in.linearColor * a, a);
    }
}

/* ============================================================================
 * Pass 3: Dual-Kawase HDR Bloom Pyramid (runs only in Glow look)
 * ============================================================================ */

fragment float4 bloom_prefilter_fragment(
    FullscreenOut in [[stage_in]],
    texture2d<float> sceneTex [[texture(0)]]
) {
    constexpr sampler s(mag_filter::linear, min_filter::linear, address::clamp_to_edge);
    float3 c = sceneTex.sample(s, in.uv).rgb;
    float lum = dot(c, float3(0.2126f, 0.7152f, 0.0722f));
    float threshold = 0.58f;
    float knee = 0.24f;
    float soft = clamp(lum - threshold + knee, 0.0f, 2.0f * knee);
    soft = (soft * soft) / (4.0f * knee + 1e-5f);
    float contrib = max(soft, lum - threshold) / max(lum, 1e-5f);
    return float4(c * contrib, 1.0f);
}

fragment float4 bloom_downsample_fragment(
    FullscreenOut in [[stage_in]],
    texture2d<float> srcTex [[texture(0)]]
) {
    constexpr sampler s(mag_filter::linear, min_filter::linear, address::clamp_to_edge);
    float2 texel = 1.0f / float2(srcTex.get_width(), srcTex.get_height());
    float3 sum = srcTex.sample(s, in.uv).rgb * 4.0f;
    sum += srcTex.sample(s, in.uv + float2(-texel.x, -texel.y)).rgb;
    sum += srcTex.sample(s, in.uv + float2( texel.x, -texel.y)).rgb;
    sum += srcTex.sample(s, in.uv + float2(-texel.x,  texel.y)).rgb;
    sum += srcTex.sample(s, in.uv + float2( texel.x,  texel.y)).rgb;
    return float4(sum * 0.125f, 1.0f);
}

fragment float4 bloom_upsample_fragment(
    FullscreenOut in [[stage_in]],
    texture2d<float> srcTex [[texture(0)]],
    texture2d<float> baseTex [[texture(1)]]
) {
    constexpr sampler s(mag_filter::linear, min_filter::linear, address::clamp_to_edge);
    float2 t = 1.25f / float2(srcTex.get_width(), srcTex.get_height());
    float3 sum = float3(0.0f);
    sum += srcTex.sample(s, in.uv + float2(-t.x * 2.0f, 0.0f)).rgb;
    sum += srcTex.sample(s, in.uv + float2(-t.x,  t.y)).rgb * 2.0f;
    sum += srcTex.sample(s, in.uv + float2(0.0f,  t.y * 2.0f)).rgb;
    sum += srcTex.sample(s, in.uv + float2( t.x,  t.y)).rgb * 2.0f;
    sum += srcTex.sample(s, in.uv + float2( t.x * 2.0f, 0.0f)).rgb;
    sum += srcTex.sample(s, in.uv + float2( t.x, -t.y)).rgb * 2.0f;
    sum += srcTex.sample(s, in.uv + float2(0.0f, -t.y * 2.0f)).rgb;
    sum += srcTex.sample(s, in.uv + float2(-t.x, -t.y)).rgb * 2.0f;
    float3 blurred = sum / 12.0f;
    float3 base = baseTex.sample(s, in.uv).rgb;
    return float4(base + blurred * 0.72f, 1.0f);
}

/* ============================================================================
 * Pass 4: EDR-Aware Composite + Crisp Scientific Vector Overlays
 * ============================================================================ */

static inline float3 soft_edr_tonemap(float3 x, float headroom) {
    // Preserve linear response below 0.85, then smoothly compress toward EDR headroom peak.
    float peak = max(headroom, 1.0f);
    float3 scaled = x / peak;
    float3 mapped = (scaled * (2.51f * scaled + 0.03f)) / (scaled * (2.43f * scaled + 0.59f) + 0.14f);
    return clamp(mapped, 0.0f, 1.0f) * peak;
}

static inline float toroidal_dist(float2 a, float2 b, float2 extent) {
    float2 d = a - b;
    d -= extent * round(d / extent);
    return length(d);
}

fragment float4 composite_fragment(
    FullscreenOut in [[stage_in]],
    constant FrameUniforms& u [[buffer(0)]],
    texture2d<float> sceneTex [[texture(0)]],
    texture2d<float> bloomTex [[texture(1)]]
) {
    constexpr sampler s(mag_filter::linear, min_filter::linear, address::clamp_to_edge);
    float3 col = sceneTex.sample(s, in.uv).rgb;

    if (u.lookMode == 0 && u.bloomStrength > 0.005f) {
        float3 bloom = bloomTex.sample(s, in.uv).rgb;
        col += bloom * (u.bloomStrength * 0.55f);
        col = soft_edr_tonemap(col, u.edrHeadroom);
    } else {
        // Honest un-tonemapped linear clamp in Raw / Sphere looks
        col = clamp(col, 0.0f, max(u.edrHeadroom, 1.0f));
    }

    // World position of this pixel for crisp GPU vector overlays
    float2 pixelOffset = (in.uv - 0.5f) * u.viewportSize;
    float2 worldPos = u.cameraCenter + pixelOffset * u.worldUnitsPerPixel;
    float pxWorld = u.worldUnitsPerPixel;

    // 1. Selected organism animated precision reticle (120Hz toroidal interpolation matching organism_vertex)
    if (u.hasSelected != 0u) {
        float2 selDelta = u.selectedPos - u.selectedPrevPos;
        selDelta -= u.worldExtent * round(selDelta / u.worldExtent);
        float2 selPos = u.selectedPrevPos + selDelta * u.interpAlpha;
        float dWorld = toroidal_dist(worldPos, selPos, u.worldExtent);
        float dPx = dWorld / pxWorld;
        float pulse = 1.0f + 0.08f * sin(u.timeSeconds * 4.5f);
        float ringR = max(14.0f * u.pixelScale, u.pointRadiusPx * u.pixelScale * 2.2f) * pulse;
        float stroke = 1.6f * u.pixelScale;
        float ring = 1.0f - smoothstep(stroke * 0.4f, stroke, abs(dPx - ringR));
        col = mix(col, float3(0.45f, 0.95f, 1.0f) * min(u.edrHeadroom, 1.4f), ring * 0.92f);
    }

    // 2. Followed lineage heir golden beacon ring (120Hz toroidal interpolation)
    if (u.hasFollowed != 0u) {
        float2 folDelta = u.followedPos - u.followedPrevPos;
        folDelta -= u.worldExtent * round(folDelta / u.worldExtent);
        float2 folPos = u.followedPrevPos + folDelta * u.interpAlpha;
        float dWorld = toroidal_dist(worldPos, folPos, u.worldExtent);
        float dPx = dWorld / pxWorld;
        float ringR = max(18.0f * u.pixelScale, u.pointRadiusPx * u.pixelScale * 2.7f);
        float stroke = 1.8f * u.pixelScale;
        float ring = 1.0f - smoothstep(stroke * 0.4f, stroke, abs(dPx - ringR));
        col = mix(col, float3(1.0f, 0.84f, 0.25f) * min(u.edrHeadroom, 1.4f), ring * 0.88f);
    }

    // 3. Non-invasive region measurement disk
    if (u.measureRadius > 0.0f) {
        float dWorld = toroidal_dist(worldPos, u.measureCenter, u.worldExtent);
        float edgePx = abs(dWorld - u.measureRadius) / pxWorld;
        float fill = (dWorld <= u.measureRadius) ? 0.10f : 0.0f;
        float border = 1.0f - smoothstep(1.0f * u.pixelScale, 2.2f * u.pixelScale, edgePx);
        float3 measCol = float3(0.32f, 0.88f, 0.98f);
        col = mix(col, measCol, fill);
        col = mix(col, measCol * min(u.edrHeadroom, 1.35f), border * 0.9f);
    }

    // 4. Interactive touch brush preview ring
    if (u.brushRadius > 0.0f) {
        float dWorld = toroidal_dist(worldPos, u.brushCenter, u.worldExtent);
        float edgePx = abs(dWorld - u.brushRadius) / pxWorld;
        float border = 1.0f - smoothstep(1.0f * u.pixelScale, 2.0f * u.pixelScale, edgePx);
        col = mix(col, float3(1.0f, 0.72f, 0.28f), border * 0.75f);
    }

    return float4(col, 1.0f);
}
