struct Camera {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // xyz = key light direction (towards light), w = 1 if output needs manual sRGB encoding
    light: vec4<f32>,
};

struct Model {
    model: mat4x4<f32>,
    color: vec4<f32>,
    // x = selected, y = hovered, z = winding flipped (mirror), w = show back faces as errors
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var<uniform> obj: Model;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_mesh(@location(0) p: vec3<f32>) -> VOut {
    var out: VOut;
    let w = obj.model * vec4<f32>(p, 1.0);
    out.world = w.xyz;
    out.pos = cam.view_proj * w;
    return out;
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn encode(c: vec3<f32>) -> vec3<f32> {
    if (cam.light.w > 0.5) {
        return to_srgb(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    return c;
}

@fragment
fn fs_mesh(in: VOut, @builtin(front_facing) ff_raw: bool) -> @location(0) vec4<f32> {
    // flat normal from screen-space derivatives
    var n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let v = normalize(cam.eye.xyz - in.world);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    var ff = ff_raw;
    if (obj.params.z > 0.5) {
        ff = !ff;
    }
    var base = obj.color.rgb;
    if (!ff && obj.params.w > 0.5) {
        // looking at the inside of a surface: open / inverted mesh -> show as red
        base = vec3<f32>(0.85, 0.08, 0.08);
    }
    let l = normalize(cam.light.xyz);
    let key = max(dot(n, l), 0.0);
    let fill = max(dot(n, normalize(v + vec3<f32>(0.0, 0.0, 0.6))), 0.0);
    let sky = 0.5 + 0.5 * n.z;
    var c = base * (0.18 + 0.12 * sky + 0.55 * key + 0.30 * fill);
    // subtle specular
    let h = normalize(l + v);
    c += vec3<f32>(0.08) * pow(max(dot(n, h), 0.0), 40.0);

    let rim = pow(1.0 - max(dot(n, v), 0.0), 2.0);
    if (obj.params.x > 0.5) {
        let accent = vec3<f32>(0.10, 0.45, 1.0);
        c = mix(c, accent, 0.10) + accent * rim * 0.55;
    } else if (obj.params.y > 0.5) {
        c += vec3<f32>(0.06) + vec3<f32>(0.18) * rim;
    }
    return vec4<f32>(encode(c), obj.color.a);
}

// ---- lines (grid, axes, overlays) ----

struct LOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(@location(0) p: vec3<f32>, @location(1) color: vec4<f32>) -> LOut {
    var out: LOut;
    out.pos = cam.view_proj * vec4<f32>(p, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_line(in: LOut) -> @location(0) vec4<f32> {
    return vec4<f32>(encode(in.color.rgb) * in.color.a, in.color.a);
}
