// The outline cage around the targeted block.
//
// Shares the camera uniform with the terrain pass, so the outline sits in the
// world and shrinks with distance like everything else.

struct Camera {
    view_projection: mat4x4<f32>,
    position: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) colour: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_projection * vec4<f32>(in.position, 1.0);
    out.colour = in.colour;
    return out;
}

// The edges standing clear of the world, at full strength.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.colour;
}

// The edges buried inside terrain. Same geometry, same colour, drawn faintly
// by the pass whose depth test only accepts occluded fragments — so the cage
// stays a complete cube even when five of its faces are inside rock.
const GHOST_ALPHA: f32 = 0.25;

@fragment
fn fs_ghost(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.colour.rgb, in.colour.a * GHOST_ALPHA);
}
