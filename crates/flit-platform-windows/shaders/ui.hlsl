cbuffer Globals : register(b0) {
    float2 viewport;
    float text_contrast;
    float pad;
};

struct VSIn {
    float4 rect   : RECT;
    float4 uv     : UV;
    float4 clip   : CLIP;
    float4 color  : COLOR;
    float2 params : PARAMS;
    uint   kind   : KIND;
    uint   vid    : SV_VertexID;
};

struct PSIn {
    float4 pos : SV_Position;
    float2 local : LOCAL;
    float2 uv : UV;
    nointerpolation float4 color : COLOR;
    nointerpolation float2 size : SIZE;
    nointerpolation float4 clip : CLIP;
    nointerpolation float2 params : PARAMS;
    nointerpolation uint kind : KIND;
};

PSIn vs_main(VSIn i) {
    float2 corner = float2(i.vid & 1, i.vid >> 1);
    float2 p = i.rect.xy + corner * i.rect.zw;
    PSIn o;
    o.pos = float4(p / viewport * float2(2, -2) + float2(-1, 1), 0, 1);
    o.local = corner * i.rect.zw;
    o.uv = lerp(i.uv.xy, i.uv.zw, corner);
    o.color = float4(i.color.rgb * i.color.a, i.color.a);
    o.size = i.rect.zw;
    o.clip = i.clip;
    o.params = i.params;
    o.kind = i.kind;
    return o;
}

Texture2D atlas : register(t0);
Texture2D thumbs : register(t1);
Texture2D preview : register(t2);
SamplerState samp : register(s0);

float rounded_box(float2 p, float2 half_size, float r) {
    float2 q = abs(p) - half_size + r;
    return length(max(q, 0)) + min(max(q.x, q.y), 0) - r;
}

float4 ps_main(PSIn i) : SV_Target {
    float2 px = i.pos.xy;
    if (px.x < i.clip.x || px.y < i.clip.y || px.x >= i.clip.z || px.y >= i.clip.w) discard;
    if (i.kind == 1) {
        float a = atlas.Sample(samp, i.uv).a;
        float k = text_contrast;
        a = a * (k + 1) / (a * k + 1);
        return i.color * a;
    }
    if (i.kind == 2) {
        return atlas.Sample(samp, i.uv) * i.color.a;
    }
    if (i.kind == 3) {
        return thumbs.Sample(samp, i.uv) * i.color.a;
    }
    if (i.kind == 4) {
        return preview.Sample(samp, i.uv) * i.color.a;
    }
    float r = i.params.x;
    float border = i.params.y;
    float2 half_size = i.size * 0.5;
    if (i.kind == 5) {
        float2 shape = max(half_size - border, 0);
        float d = rounded_box(i.local - half_size, shape, min(r, min(shape.x, shape.y)));
        float a = 1 - smoothstep(-border, border, d);
        return i.color * (a * a);
    }
    if (r <= 0 && border <= 0) return i.color;
    float d = rounded_box(i.local - half_size, half_size, min(r, min(half_size.x, half_size.y)));
    float a = saturate(0.5 - d);
    if (border > 0) a *= saturate(0.5 + d + border);
    return i.color * a;
}
