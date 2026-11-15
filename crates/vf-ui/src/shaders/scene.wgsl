struct Uniforms {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    fog_color: vec4<f32>,
    sand: vec4<f32>,
    grass: vec4<f32>,
    rock: vec4<f32>,
    snow: vec4<f32>,
    water_deep: vec4<f32>,
    water_shallow: vec4<f32>,
    params: vec4<f32>, // tan_half_fov, aspect, exposure, encode_srgb
    misc: vec4<f32>,   // sea_level_rel, relief, fog_density, time
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> SkyOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: SkyOut;
    let p = positions[index];
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let ray = normalize(
        u.forward.xyz
            + u.right.xyz * in.ndc.x * u.params.x * u.params.y
            + u.up.xyz * in.ndc.y * u.params.x
    );
    let height = smoothstep(-0.03, 0.55, ray.z);
    var color = mix(u.sky_horizon.rgb, u.sky_zenith.rgb, height);
    let sun_dot = max(dot(ray, u.sun_dir.xyz), 0.0);
    color += u.sun_color.rgb * (pow(sun_dot, 900.0) * 3.0 + pow(sun_dot, 10.0) * 0.06);
    let haze = 1.0 - smoothstep(-0.02, 0.14, ray.z);
    color = mix(color, u.fog_color.rgb, haze * 0.35);
    color *= u.params.z;
    if (u.params.w > 0.5) {
        color = pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(color, 1.0);
}

struct TerrainIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) kind: f32,
    @location(3) height: f32,
};

struct TerrainOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) rel: vec3<f32>,
    @location(2) kind: f32,
    @location(3) height: f32,
    @location(4) distance: f32,
};

@vertex
fn vs_terrain(in: TerrainIn) -> TerrainOut {
    var out: TerrainOut;
    out.clip = u.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;
    out.rel = in.position;
    out.kind = in.kind;
    out.height = in.height;
    out.distance = length(in.position - u.eye.xyz);
    return out;
}

@fragment
fn fs_terrain(in: TerrainOut) -> @location(0) vec4<f32> {
    let normal = normalize(in.normal);
    let slope = 1.0 - clamp(normal.z, 0.0, 1.0);
    let altitude = clamp(in.height / max(u.misc.y, 1.0), 0.0, 1.0);
    let view = normalize(u.eye.xyz - in.rel);

    var color: vec3<f32>;
    if (in.kind > 0.5) {
        // Opaque water: Fresnel sky reflection, sun glint and a gentle ripple.
        let ripple = vec3<f32>(
            sin(in.rel.x * 0.013 + u.misc.w) * 0.02,
            cos(in.rel.y * 0.011 + u.misc.w * 0.7) * 0.02,
            1.0
        );
        let n = normalize(ripple);
        let fresnel = pow(1.0 - max(dot(n, view), 0.0), 4.0);
        let water = mix(u.water_deep.rgb, u.water_shallow.rgb, 0.10 + fresnel * 0.16);
        let reflected = mix(water, u.sky_horizon.rgb, fresnel * 0.05);
        let half_vec = normalize(u.sun_dir.xyz + view);
        let glint = pow(max(dot(n, half_vec), 0.0), 320.0) * 0.9;
        let ripple_tint = 1.0 + 0.04 * sin(in.rel.x * 0.02 + in.rel.y * 0.013);
        color = reflected * (0.58 + 0.42 * max(u.sun_dir.z, 0.0)) * ripple_tint
            + u.sun_color.rgb * glint;
    } else {
        var albedo = mix(u.sand.rgb, u.grass.rgb, smoothstep(0.01, 0.10, slope));
        albedo = mix(albedo, u.rock.rgb, smoothstep(0.18, 0.55, slope));
        albedo = mix(
            albedo,
            u.snow.rgb,
            smoothstep(0.62, 0.82, altitude) * (1.0 - smoothstep(0.35, 0.70, slope))
        );
        let shore = smoothstep(u.misc.x + 0.001, u.misc.x + 0.035, in.height);
        albedo = mix(u.sand.rgb, albedo, shore);

        let diffuse = max(dot(normal, u.sun_dir.xyz), 0.0);
        let sky = mix(u.sky_zenith.rgb, u.sky_horizon.rgb, 0.55);
        color = albedo * (u.sun_color.rgb * (0.22 + diffuse * 0.95) + sky * 0.28);
    }

    let fog = 1.0 - exp(-max(in.distance - 10.0, 0.0) * u.misc.z);
    let fog_amount = select(0.85, 0.30, in.kind > 0.5);
    color = mix(color, u.fog_color.rgb, clamp(fog, 0.0, 0.90) * fog_amount);
    color *= u.params.z;
    if (u.params.w > 0.5) {
        color = pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(color, 1.0);
}
