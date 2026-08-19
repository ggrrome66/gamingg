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
struct Sun {
    direction: vec4<f32>,
    sky: vec4<f32>,
    light: vec4<f32>,
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
    let lighting = sun.light.y + sun.light.x * diffuse + axis_bias;

    var colour = albedo.rgb * clamp(lighting, 0.0, 1.4);

    // Distance fog, so the far edge of the loaded world fades out instead of
    // ending in a hard wall.
    let distance = length(in.world_position - camera.position.xyz);
    let fog = clamp((distance - 140.0) / 180.0, 0.0, 1.0);
    colour = mix(colour, sun.sky.xyz, fog);

    return vec4<f32>(colour, albedo.a);
}
