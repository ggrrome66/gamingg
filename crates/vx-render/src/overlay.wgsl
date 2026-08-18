// A screen-space textured rectangle: the engine's first 2D surface.
//
// No camera, no depth — the quad lives in normalized device coordinates,
// mapped from a pixel rectangle by the uniform below. The minimap is the
// first user; anything that wants to be a flat picture on the screen (a
// crosshair, a terminal, a very small arcade machine) goes through here.

struct OverlayRect {
    // x, y, width, height of the quad in pixels, top-left origin.
    rect: vec4<f32>,
    // Width and height of the whole target, for the pixel-to-NDC mapping.
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> overlay: OverlayRect;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var picture_sampler: sampler;

struct OverlayOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Two triangles from the vertex index alone; no vertex buffer at all.
@vertex
fn vs_overlay(@builtin(vertex_index) index: u32) -> OverlayOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[index];

    let pixel = overlay.rect.xy + corner * overlay.rect.zw;
    // Pixel space is y-down from the top-left; NDC is y-up from the centre.
    let ndc = vec2<f32>(
        pixel.x / overlay.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / overlay.screen.y * 2.0,
    );

    var out: OverlayOutput;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    return out;
}

@fragment
fn fs_overlay(in: OverlayOutput) -> @location(0) vec4<f32> {
    return textureSample(picture, picture_sampler, in.uv);
}
