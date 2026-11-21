Texture2D<float> luma : register(t0);
Texture2D<float2> chroma : register(t1);
SamplerState linear_clamp : register(s0);

cbuffer Color : register(b0) {
    float hlg;
    float peak;
    float full_range;
    float padding;
};

float4 vs_main(uint vertex : SV_VertexID) : SV_Position {
    return float4(vertex == 1 ? 3.0 : -1.0, vertex == 2 ? -3.0 : 1.0, 0.0, 1.0);
}

float3 pq_to_linear(float3 rgb) {
    float3 p = pow(max(rgb, 0.0), 32.0 / 2523.0);
    return 100.0 * pow(max(p - 3424.0 / 4096.0, 0.0) / max(2413.0 / 128.0 - 2392.0 / 128.0 * p, 0.00001), 16384.0 / 2610.0);
}

float hlg_to_scene(float v) {
    return v <= 0.5 ? v * v / 3.0 : (exp((v - 0.55991073) / 0.17883277) + 0.28466892) / 12.0;
}

float hable(float x) {
    return (0.14 * x + 1.0 / 60.0) / (0.15 * x + 0.5 + 0.06 / x);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target {
    uint width, height;
    luma.GetDimensions(width, height);
    float y = luma.Load(int3(int2(pos.xy), 0)) * (65535.0 / 64.0);
    float2 uv = chroma.SampleLevel(linear_clamp, (pos.xy + float2(0.5, 0.0)) / float2(width, height), 0) * (65535.0 / 64.0);
    y = full_range != 0.0 ? y / 1023.0 : (y - 64.0) / 876.0;
    uv = (uv - 512.0) / (full_range != 0.0 ? 1023.0 : 896.0);
    float3 rgb = max(float3(y + 1.4746 * uv.y, y - 0.16455313 * uv.x - 0.57135313 * uv.y, y + 1.8814 * uv.x), 0.0);
    if (hlg != 0.0) {
        rgb = float3(hlg_to_scene(rgb.r), hlg_to_scene(rgb.g), hlg_to_scene(rgb.b));
        rgb *= 10.0 * pow(dot(rgb, float3(0.2627, 0.6780, 0.0593)), 0.2);
    } else {
        rgb = pq_to_linear(rgb);
    }
    float3 rec709 = float3(dot(rgb, float3(1.660491, -0.587641, -0.072850)),
                           dot(rgb, float3(-0.124550, 1.132900, -0.008349)),
                           dot(rgb, float3(-0.018151, -0.100579, 1.118730)));
    float signal = max(max(max(rec709.r, rec709.g), rec709.b), 0.000001);
    float3 mapped = saturate(rec709 * (hable(signal) / (signal * hable(peak))));
    return float4(pow(mapped, 1.0 / 2.4), 1.0);
}
