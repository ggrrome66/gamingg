// Screen-space overlay: crosshair, HUD and menus.
//
// Positions arrive already in clip space, so there is no transform here — the
// CPU side owns the pixel-to-clip conversion because that is where the layout
// lives.

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) colour: vec4<f32>,
    @location(3) textured: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
    // Flat: a per-quad mode flag must not be interpolated into a fraction.
    @location(2) @interpolate(flat) textured: u32,
};

@group(0) @binding(0) var font_atlas: texture_2d<f32>;
@group(0) @binding(1) var font_sampler: sampler;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    // z = 0 with the depth test disabled; the overlay always draws on top.
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.colour = in.colour;
    out.textured = in.textured;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if (in.textured == 0u) {
        return in.colour;
    }

    // The atlas is a coverage mask, not colour: it decides where the glyph is
    // inked and the vertex colour decides what shade it is.
    let coverage = textureSample(font_atlas, font_sampler, in.uv).r;
    if (coverage < 0.5) {
        discard;
    }
    return vec4<f32>(in.colour.rgb, in.colour.a * coverage);
}
