// Terrain shading.
//
// Block textures live in a texture array, one layer per tile, so the sampler's
// Repeat address mode tiles them across greedy-merged quads whose UVs span the
// whole quad. That is why there is no atlas arithmetic here.

struct Camera {
    view_projection: mat4x4<f32>,
    position: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var tiles: texture_2d_array<f32>;
@group(1) @binding(1) var tile_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tile: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // Flat: the tile index is constant per quad and must not be interpolated
    // into a fractional value between two layers.
    @location(2) @interpolate(flat) tile: u32,
    @location(3) world_position: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_projection * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;
    out.uv = in.uv;
    out.tile = in.tile;
    out.world_position = in.position;
    return out;
}

// Direction the key light arrives from.
const LIGHT_DIRECTION: vec3<f32> = vec3<f32>(0.42, 0.86, 0.29);
const AMBIENT: f32 = 0.42;
const FOG_COLOUR: vec3<f32> = vec3<f32>(0.62, 0.74, 0.88);

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let albedo = textureSample(tiles, tile_sampler, in.uv, in.tile);

    // Discard fully transparent texels rather than blending them, so they do
    // not write depth and hide geometry behind them.
    if (albedo.a < 0.05) {
        discard;
    }

    let normal = normalize(in.normal);
    let light = normalize(LIGHT_DIRECTION);
    let diffuse = max(dot(normal, light), 0.0);

    // A small per-axis bias keeps the four side faces from reading as one flat
    // silhouette when the light is nearly overhead.
    let axis_bias = 0.06 * abs(normal.x) - 0.03 * abs(normal.z);
    let lighting = AMBIENT + 0.58 * diffuse + axis_bias;

    var colour = albedo.rgb * clamp(lighting, 0.0, 1.4);

    // Distance fog, so the far edge of the loaded world fades out instead of
    // ending in a hard wall.
    let distance = length(in.world_position - camera.position.xyz);
    let fog = clamp((distance - 140.0) / 180.0, 0.0, 1.0);
    colour = mix(colour, FOG_COLOUR, fog);

    return vec4<f32>(colour, albedo.a);
}
