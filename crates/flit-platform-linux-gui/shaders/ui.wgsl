struct Globals {
    viewport: vec2<f32>,
    text_contrast: f32,
    pad: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var thumbs: texture_2d<f32>;
@group(0) @binding(3) var preview: texture_2d<f32>;
@group(0) @binding(4) var backdrop: texture_2d<f32>;
@group(0) @binding(5) var samp: sampler;

struct VsIn {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) clip: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) params: vec2<f32>,
    @location(5) kind: u32,
    @builtin(vertex_index) vid: u32,
}

struct PsIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) size: vec2<f32>,
    @location(4) @interpolate(flat) clip: vec4<f32>,
    @location(5) @interpolate(flat) params: vec2<f32>,
    @location(6) @interpolate(flat) kind: u32,
}

@vertex
fn vs_main(i: VsIn) -> PsIn {
    let corner = vec2<f32>(f32(i.vid & 1u), f32(i.vid >> 1u));
    let p = i.rect.xy + corner * i.rect.zw;
    var o: PsIn;
    o.pos = vec4<f32>(p / globals.viewport * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    o.local = corner * i.rect.zw;
    o.uv = mix(i.uv.xy, i.uv.zw, corner);
    o.color = vec4<f32>(i.color.rgb * i.color.a, i.color.a);
    o.size = i.rect.zw;
    o.clip = i.clip;
    o.params = i.params;
    o.kind = i.kind;
    return o;
}

fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half_size + r;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(i: PsIn) -> @location(0) vec4<f32> {
    let px = vec2<f32>(i.pos.x, globals.viewport.y - i.pos.y);
    if (px.x < i.clip.x || px.y < i.clip.y || px.x >= i.clip.z || px.y >= i.clip.w) {
        discard;
    }
    if (i.kind == 1u) {
        var a = textureSampleLevel(atlas, samp, i.uv, 0.0).a;
        let k = globals.text_contrast;
        a = a * (k + 1.0) / (a * k + 1.0);
        return i.color * a;
    }
    if (i.kind == 2u) {
        return textureSampleLevel(atlas, samp, i.uv, 0.0) * i.color.a;
    }
    if (i.kind == 3u) {
        return textureSampleLevel(thumbs, samp, i.uv, 0.0) * i.color.a;
    }
    if (i.kind == 4u) {
        return textureSampleLevel(preview, samp, i.uv, 0.0) * i.color.a;
    }
    if (i.kind == 6u) {
        return textureSampleLevel(backdrop, samp, i.uv, 0.0) * i.color.a;
    }
    let r = i.params.x;
    let border = i.params.y;
    let half_size = i.size * 0.5;
    if (i.kind == 5u) {
        let shape = max(half_size - border, vec2<f32>(0.0));
        let d = rounded_box(i.local - half_size, shape, min(r, min(shape.x, shape.y)));
        let a = 1.0 - smoothstep(-border, border, d);
        return i.color * (a * a);
    }
    if (r <= 0.0 && border <= 0.0) {
        return i.color;
    }
    let d = rounded_box(i.local - half_size, half_size, min(r, min(half_size.x, half_size.y)));
    var a = saturate(0.5 - d);
    if (border > 0.0) {
        a *= saturate(0.5 + d + border);
    }
    return i.color * a;
}
