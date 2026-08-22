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

// The sun, the sky and the light level. One value drives both the diffuse
// term and the fog, so the horizon always agrees with the sky behind it.
// `light.z` is the view mode: 0 plain, 1 night vision, 2 thermal.
// The lamp is the player's hand lamp: `lamp_position.w` is its strength
// (zero when off) and `lamp_direction.w` its reach in blocks.
struct Sun {
    direction: vec4<f32>,
    sky: vec4<f32>,
    light: vec4<f32>,
    lamp_position: vec4<f32>,
    lamp_direction: vec4<f32>,
};

@group(2) @binding(0) var<uniform> sun: Sun;

// Where this chunk's (0,0,0) corner sits. Quad positions are chunk-local, so
// this is added once per vertex here instead of being baked into every one of
// them on the CPU — which is what keeps geometry out of the f32 precision wall
// far from the origin.
@group(3) @binding(0) var<uniform> chunk_origin: vec4<f32>;

// One instance per quad, eight bytes:
//   kind:4 | plane:9 | iu:9 | iv:9 | w:9 | h:9 | tile:8
// There is no vertex buffer and no index buffer; the six vertices of a quad's
// two triangles are synthesised from `vertex_index`. Must stay in step with
// `vx_mesh::PackedQuad`, whose `corners`/`normal`/`uvs` are the same arithmetic
// on the CPU and are held to it by test.
struct QuadInput {
    @location(0) packed: vec2<u32>,
};

// The object pipeline still draws real vertices: a drone is a handful of
// cuboids, not a chunk of blocks, so packing buys nothing there and the mesh
// comes from `object::unit_cube` rather than the greedy mesher.
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
    // Baked sky exposure, 0 buried .. 1 open sky. Interpolated so a lit quad
    // meeting a dark one shades across the shared edge instead of stepping.
    @location(4) light: f32,
    // 0 terrain, 1 object — thermal renders warm bodies, and only objects
    // are warm bodies.
    @location(5) @interpolate(flat) source: u32,
};

@vertex
fn vs_main(quad: QuadInput, @builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let low = quad.packed.x;
    let high = quad.packed.y;

    let kind = low & 0xfu;
    let plane = f32((low >> 4u) & 0x1ffu);
    let iu = f32((low >> 13u) & 0x1ffu);
    let iv = f32((low >> 22u) & 0x1ffu);
    // `w` straddles the word boundary: one bit at the top of `low`, eight at
    // the bottom of `high`.
    let w = f32(((low >> 31u) & 1u) | ((high & 0xffu) << 1u));
    let h = f32((high >> 8u) & 0x1ffu);
    let tile = (high >> 17u) & 0xffu;
    let sky_exposure = f32((high >> 25u) & 0xfu) / 15.0;

    // Two triangles per quad: 0,1,2 and 0,2,3.
    var winding = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let corner = winding[vertex_index];

    // uv spans the merged quad in tile units, so the sampler's Repeat mode
    // tiles the texture across it. Indexed by the output corner.
    var uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(w, 0.0),
        vec2<f32>(w, h),
        vec2<f32>(0.0, h),
    );

    var local = vec3<f32>(0.0, 0.0, 0.0);
    var normal = vec3<f32>(0.0, 1.0, 0.0);

    if kind >= 6u {
        // A plant's crossed quads. Always one block square, so plane/iu/iv are
        // y/x/z. Lit as if lying flat: plants take the ground's light, and
        // neither diagonal shows a dark "back of the leaf".
        let x = iu;
        let y = plane;
        let z = iv;
        var first = array<vec3<f32>, 4>(
            vec3<f32>(x, y + 1.0, z),
            vec3<f32>(x + 1.0, y + 1.0, z + 1.0),
            vec3<f32>(x + 1.0, y, z + 1.0),
            vec3<f32>(x, y, z),
        );
        var second = array<vec3<f32>, 4>(
            vec3<f32>(x + 1.0, y + 1.0, z),
            vec3<f32>(x, y + 1.0, z + 1.0),
            vec3<f32>(x, y, z + 1.0),
            vec3<f32>(x + 1.0, y, z),
        );
        let variant = kind - 6u;
        // The odd variant of each pair is the same quad wound backwards, so a
        // plant carries its own back under backface culling.
        var pick = corner;
        if (variant & 1u) == 1u {
            pick = 3u - corner;
        }
        if variant < 2u {
            local = first[pick];
        } else {
            local = second[pick];
        }
    } else {
        // A merged cube face. `kind` indexes vx_core::Face::ALL, which runs
        // NegX, PosX, NegY, PosY, NegZ, PosZ — so the axis is kind/2 and the
        // sign is kind&1.
        let axis = kind / 2u;
        let positive = (kind & 1u) == 1u;

        // Counter-clockwise seen from outside; reversed for negative faces,
        // whose outward direction is the other way along the axis.
        var forward = array<vec2<f32>, 4>(
            vec2<f32>(0.0, 0.0),
            vec2<f32>(w, 0.0),
            vec2<f32>(w, h),
            vec2<f32>(0.0, h),
        );
        var backward = array<vec2<f32>, 4>(
            vec2<f32>(0.0, 0.0),
            vec2<f32>(0.0, h),
            vec2<f32>(w, h),
            vec2<f32>(w, 0.0),
        );
        var offset = backward[corner];
        if positive {
            offset = forward[corner];
        }

        let a = iu + offset.x;
        let b = iv + offset.y;
        // u and v are the two axes after this one, cyclically, matching the
        // mesher's choice that makes u x v = +axis.
        if axis == 0u {
            local = vec3<f32>(plane, a, b);
            normal = vec3<f32>(1.0, 0.0, 0.0);
        } else if axis == 1u {
            local = vec3<f32>(b, plane, a);
            normal = vec3<f32>(0.0, 1.0, 0.0);
        } else {
            local = vec3<f32>(a, b, plane);
            normal = vec3<f32>(0.0, 0.0, 1.0);
        }
        if !positive {
            normal = -normal;
        }
    }

    let world = local + chunk_origin.xyz;

    var out: VertexOutput;
    out.clip_position = camera.view_projection * vec4<f32>(world, 1.0);
    out.normal = normal;
    out.uv = uvs[corner];
    out.tile = tile;
    out.world_position = world;
    out.light = sky_exposure;
    out.source = 0u;
    return out;
}


// Per-instance data for objects — drones, markers, anything that is not
// terrain. A mat4x4 cannot be a single vertex attribute, so it arrives as four
// vec4 columns.
struct InstanceInput {
    @location(4) model_0: vec4<f32>,
    @location(5) model_1: vec4<f32>,
    @location(6) model_2: vec4<f32>,
    @location(7) model_3: vec4<f32>,
    // Inverse-transpose of the model's upper 3x3, for normals. Computed on
    // the CPU: rotating a normal by the raw model matrix is wrong the moment
    // a transform mixes rotation with non-uniform scale.
    @location(8) normal_0: vec4<f32>,
    @location(9) normal_1: vec4<f32>,
    @location(10) normal_2: vec4<f32>,
    @location(11) tile: u32,
    // Sky exposure at the object's position, computed on the CPU from the
    // same column-depth rule the mesher bakes into terrain, so a machine
    // in a cave darkens with the ground it stands on.
    @location(12) light: f32,
};

// Objects deliberately emit the same VertexOutput as terrain and share fs_main
// below. An object lit by different code from the ground beneath it would drift
// out of step the first time the lighting changed.
@vertex
fn vs_object(in: VertexInput, instance: InstanceInput) -> VertexOutput {
    let model = mat4x4<f32>(
        instance.model_0,
        instance.model_1,
        instance.model_2,
        instance.model_3,
    );
    let world = model * vec4<f32>(in.position, 1.0);

    let normal_matrix = mat3x3<f32>(
        instance.normal_0.xyz,
        instance.normal_1.xyz,
        instance.normal_2.xyz,
    );

    var out: VertexOutput;
    out.clip_position = camera.view_projection * world;
    out.normal = normalize(normal_matrix * in.normal);
    out.uv = in.uv;
    // The instance's tile wins; the cube's per-vertex tile is unused padding
    // inherited from sharing the terrain vertex layout.
    out.tile = instance.tile;
    out.world_position = world.xyz;
    out.light = instance.light;
    out.source = 1u;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let albedo = textureSample(tiles, tile_sampler, in.uv, in.tile);

    // Discard fully transparent texels rather than blending them, so they do
    // not write depth and hide geometry behind them.
    if (albedo.a < 0.05) {
        discard;
    }

    let normal = normalize(in.normal);
    let light = normalize(sun.direction.xyz);
    let diffuse = max(dot(normal, light), 0.0);

    // A small per-axis bias keeps the four side faces from reading as one flat
    // silhouette when the light is nearly overhead.
    let axis_bias = 0.06 * abs(normal.x) - 0.03 * abs(normal.z);

    // Daylight only reaches what the sky can see: the baked exposure scales
    // the whole sun term, and a tiny floor keeps buried geometry from being
    // mathematically invisible rather than dark.
    let day = sun.light.y + sun.light.x * diffuse + axis_bias;
    var lighting = day * in.light + 0.015;

    // The hand lamp: a warm spot cone from wherever the player's eye is.
    let lamp_strength = sun.lamp_position.w;
    var lamp = 0.0;
    if lamp_strength > 0.0 {
        let to_fragment = in.world_position - sun.lamp_position.xyz;
        let lamp_distance = length(to_fragment);
        let reach = sun.lamp_direction.w;
        if lamp_distance > 0.01 && lamp_distance < reach {
            let along = to_fragment / lamp_distance;
            let cone = smoothstep(0.78, 0.92, dot(along, normalize(sun.lamp_direction.xyz)));
            var falloff = 1.0 - lamp_distance / reach;
            falloff = falloff * falloff;
            // Grazing surfaces still catch a little, so the cone reads as a
            // pool of light rather than a hard stencil.
            let facing = max(dot(normal, -along), 0.0) * 0.75 + 0.25;
            lamp = lamp_strength * cone * falloff * facing;
        }
    }
    lighting = lighting + lamp;

    var colour = albedo.rgb * clamp(lighting, 0.0, 1.4);
    // The lamp is warm; sunlight is not. Tint only what the lamp added.
    colour = colour + albedo.rgb * lamp * vec3<f32>(0.22, 0.14, 0.02);

    // Distance fog, so the far edge of the loaded world fades out instead of
    // ending in a hard wall. Scaled by exposure: underground there is no sky
    // to fade into, and haze glowing in a black gallery would be a light
    // source the world does not have.
    let distance = length(in.world_position - camera.position.xyz);
    let fog = clamp((distance - 140.0) / 180.0, 0.0, 1.0) * in.light;
    colour = mix(colour, sun.sky.xyz, fog);

    // View modes, applied last: they transform what the eye receives.
    let mode = sun.light.z;
    if mode > 1.5 {
        // Thermal: lighting is irrelevant — that is the whole point. Terrain
        // is cold, graded faintly by its own tone; objects are warm bodies.
        if in.source == 1u {
            let heat = clamp(0.45 + 0.55 * diffuse, 0.0, 1.0);
            colour = mix(vec3<f32>(0.85, 0.25, 0.05), vec3<f32>(1.0, 0.95, 0.55), heat);
        } else {
            let tone = dot(albedo.rgb, vec3<f32>(0.3, 0.55, 0.15));
            colour = mix(vec3<f32>(0.02, 0.01, 0.08), vec3<f32>(0.3, 0.06, 0.38), tone);
        }
    } else if mode > 0.5 {
        // Night vision: an intensifier, not a light. It amplifies what little
        // reaches the eye plus a floor read off the surface itself, shaped by
        // the face's orientation and dimming with distance — without those
        // two, a pitch-black gallery amplifies into one flat green wash
        // instead of resolving into geometry.
        let received = dot(colour, vec3<f32>(0.35, 0.5, 0.15));
        let surface = dot(albedo.rgb, vec3<f32>(0.3, 0.55, 0.15));
        let form = 0.25 + 0.75 * diffuse + 0.35 * abs(normal.x) + 0.15 * abs(normal.z);
        let range_fade = clamp(1.0 - distance / 70.0, 0.15, 1.0);
        let amplified = clamp((received * 2.2 + surface * 0.55) * form * range_fade + 0.02, 0.0, 1.0);
        colour = vec3<f32>(0.07, 1.0, 0.28) * amplified;
    }

    return vec4<f32>(colour, albedo.a);
}
