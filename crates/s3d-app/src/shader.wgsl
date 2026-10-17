struct Camera {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // xyz = key light direction (towards light), w = 1 if output needs manual sRGB encoding
    light: vec4<f32>,
    // section plane: fragments with dot(xyz, p) > w are cut away; zero normal = off
    clip: vec4<f32>,
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
    // painted colour (sRGB), alpha 0 = use the object colour
    @location(1) @interpolate(flat) paint: vec4<f32>,
    // smooth-shading normal in world space; zero = flat shading
    @location(2) normal: vec3<f32>,
};

@vertex
fn vs_mesh(@location(0) p: vec3<f32>, @location(1) paint: vec4<f32>, @location(2) n: vec4<f32>) -> VOut {
    var out: VOut;
    let w = obj.model * vec4<f32>(p, 1.0);
    out.world = w.xyz;
    out.paint = paint;
    out.pos = cam.view_proj * w;
    // cofactor matrix: transforms normals correctly under non-uniform scale
    let c0 = obj.model[0].xyz;
    let c1 = obj.model[1].xyz;
    let c2 = obj.model[2].xyz;
    let nm = mat3x3<f32>(cross(c1, c2), cross(c2, c0), cross(c0, c1));
    out.normal = nm * n.xyz;
    return out;
}

/// Shading normal: the smooth one when there is one, kept on the same side as the face.
fn shading_normal(in: VOut, flat: vec3<f32>) -> vec3<f32> {
    if (dot(in.normal, in.normal) < 1e-12) {
        return flat;
    }
    var n = normalize(in.normal);
    if (dot(n, flat) < 0.0) {
        n = -n;
    }
    return n;
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

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn surface_color(in: VOut) -> vec3<f32> {
    if (in.paint.a > 0.5) {
        return to_linear(in.paint.rgb);
    }
    return obj.color.rgb;
}

fn section_on() -> bool {
    return dot(cam.clip.xyz, cam.clip.xyz) > 0.25;
}

fn clipped(p: vec3<f32>) -> bool {
    return section_on() && dot(cam.clip.xyz, p) > cam.clip.w;
}

fn ghost_color(world: vec3<f32>, n_raw: vec3<f32>) -> vec3<f32> {
    var n = n_raw;
    let v = normalize(cam.eye.xyz - world);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let key = max(dot(n, normalize(cam.light.xyz)), 0.0);
    return obj.color.rgb * (0.55 + 0.6 * key);
}

@fragment
fn fs_ghost(in: VOut) -> @location(0) vec4<f32> {
    let n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    if (clipped(in.world)) {
        discard;
    }
    let a = obj.color.a;
    return vec4<f32>(encode(ghost_color(in.world, n)) * a, a);
}

@fragment
fn fs_ghost_xray(in: VOut) -> @location(0) vec4<f32> {
    let n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    if (clipped(in.world)) {
        discard;
    }
    let a = obj.color.a * obj.params.x;
    return vec4<f32>(encode(ghost_color(in.world, n)) * a, a);
}

@fragment
fn fs_mesh(in: VOut, @builtin(front_facing) ff_raw: bool) -> @location(0) vec4<f32> {
    // flat normal from screen-space derivatives
    var f = normalize(cross(dpdx(in.world), dpdy(in.world)));
    if (clipped(in.world)) {
        discard;
    }
    let v = normalize(cam.eye.xyz - in.world);
    if (dot(f, v) < 0.0) {
        f = -f;
    }
    let n = shading_normal(in, f);
    var ff = ff_raw;
    if (obj.params.z > 0.5) {
        ff = !ff;
    }
    if (!ff && section_on()) {
        // inside of a solid seen through the section plane: flat hatched cap
        let s = fract((in.pos.x + in.pos.y) / 9.0);
        let sc = surface_color(in);
        var cap = sc * 0.62;
        if (s < 0.3) {
            cap = sc * 0.40;
        }
        return vec4<f32>(encode(cap), 1.0);
    }
    var base = surface_color(in);
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

// ---- wireframe ----

@vertex
fn vs_wire(@location(0) p: vec3<f32>) -> LOut {
    var out: LOut;
    let w = obj.model * vec4<f32>(p, 1.0);
    out.pos = cam.view_proj * w;
    out.color = vec4<f32>(w.xyz, 1.0);
    return out;
}

@fragment
fn fs_wire(in: LOut) -> @location(0) vec4<f32> {
    if (clipped(in.color.xyz)) {
        discard;
    }
    let c = obj.color.rgb * 0.35;
    let a = 0.75;
    return vec4<f32>(encode(c) * a, a);
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
