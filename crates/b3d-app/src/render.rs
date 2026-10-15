//! wgpu viewport renderer, drawn inside egui's render pass via a paint callback.

use b3d_core::scene::MeshHandle;
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::{Mat4, Vec3};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;

/// Must match `NativeOptions::multisampling` / `depth_buffer` in main.rs.
pub const MSAA: u32 = 4;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const MODEL_STRIDE: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub light: [f32; 4],
    /// Section plane: fragments with dot(xyz, p) > w are cut away. Zero normal = off.
    pub clip: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelUniform {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

impl LineVertex {
    pub fn new(p: Vec3, color: [f32; 4]) -> Self {
        Self { pos: p.to_array(), color }
    }
}

pub struct DrawItem {
    pub mesh: MeshHandle,
    pub model: Mat4,
    /// Linear RGBA.
    pub color: [f32; 4],
    /// selected, hovered, mirrored, show-backfaces
    pub params: [f32; 4],
}

struct GpuMesh {
    vbuf: wgpu::Buffer,
    /// Per-vertex paint (sRGBA8, alpha 0 = object colour).
    cbuf: wgpu::Buffer,
    /// None = painted mesh, drawn unindexed with 3 vertices per triangle.
    ibuf: Option<wgpu::Buffer>,
    count: u32,
    rev: u64,
}

impl GpuMesh {
    fn new(device: &wgpu::Device, h: &MeshHandle) -> GpuMesh {
        let m = &h.mesh;
        let init = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        let vertex = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
        if m.is_painted() {
            let pos: Vec<glam::Vec3> = m.triangles.iter().flat_map(|t| t.map(|i| m.positions[i as usize])).collect();
            GpuMesh {
                vbuf: init("mesh vb", bytemuck::cast_slice(&pos), wgpu::BufferUsages::VERTEX),
                cbuf: init("mesh paint", bytemuck::cast_slice(&paint_vertices(m)), vertex),
                ibuf: None,
                count: pos.len() as u32,
                rev: h.paint_rev,
            }
        } else {
            GpuMesh {
                vbuf: init("mesh vb", bytemuck::cast_slice(&m.positions), wgpu::BufferUsages::VERTEX),
                cbuf: init("mesh paint", &vec![0u8; m.positions.len().max(1) * 4], vertex),
                ibuf: Some(init("mesh ib", bytemuck::cast_slice(&m.triangles), wgpu::BufferUsages::INDEX)),
                count: (m.triangles.len() * 3) as u32,
                rev: h.paint_rev,
            }
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_vertex_buffer(1, self.cbuf.slice(..));
        match &self.ibuf {
            Some(ib) => {
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.count, 0, 0..1);
            }
            None => pass.draw(0..self.count, 0..1),
        }
    }
}

/// Paint colour for each of the 3 vertices of every triangle.
fn paint_vertices(m: &b3d_core::Mesh) -> Vec<u32> {
    (0..m.triangles.len()).flat_map(|i| [m.paint_of(i); 3]).collect()
}

struct DynBuf {
    buf: wgpu::Buffer,
    cap: u64,
}

impl DynBuf {
    fn new(device: &wgpu::Device, label: &str, usage: wgpu::BufferUsages, cap: u64) -> Self {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: cap,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buf, cap }
    }
    /// Returns true if the buffer was re-created.
    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, usage: wgpu::BufferUsages, data: &[u8]) -> bool {
        let mut grown = false;
        if data.len() as u64 > self.cap {
            *self = DynBuf::new(device, label, usage, (data.len() as u64).next_power_of_two());
            grown = true;
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buf, 0, data);
        }
        grown
    }
}

pub struct ViewportRenderer {
    mesh_pipeline: wgpu::RenderPipeline,
    tri_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    ghost_pipeline: wgpu::RenderPipeline,
    xray_pipeline: wgpu::RenderPipeline,
    cam_buf: wgpu::Buffer,
    cam_bg: wgpu::BindGroup,
    model_bgl: wgpu::BindGroupLayout,
    models: DynBuf,
    model_bg: wgpu::BindGroup,
    lines: DynBuf,
    meshes: HashMap<u64, GpuMesh>,
    // per-frame draw list computed in prepare()
    frame: Vec<(u64, u32)>,
    ghost_frame: Vec<(u64, u32)>,
    line_ranges: [std::ops::Range<u32>; 3],
}

impl ViewportRenderer {
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("b3d shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let cam_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cam bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<ModelUniform>() as u64),
                },
                count: None,
            }],
        });
        let cam_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&CameraUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cam bg"),
            layout: &cam_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: cam_buf.as_entire_binding() }],
        });
        let models = DynBuf::new(device, "models", wgpu::BufferUsages::UNIFORM, MODEL_STRIDE * 64);
        let model_bg = Self::make_model_bg(device, &model_bgl, &models.buf);
        let lines = DynBuf::new(device, "lines", wgpu::BufferUsages::VERTEX, 1 << 16);

        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh layout"),
            bind_group_layouts: &[Some(&cam_bgl), Some(&model_bgl)],
            immediate_size: 0,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line layout"),
            bind_group_layouts: &[Some(&cam_bgl)],
            immediate_size: 0,
        });
        let ms = wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false };
        let depth = |write: bool, cmp: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(cmp),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let blend_target = [Some(wgpu::ColorTargetState {
            format: target,
            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let opaque_target = [Some(wgpu::ColorTargetState { format: target, blend: None, write_mask: wgpu::ColorWrites::ALL })];

        let mesh_vb = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 4,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![1 => Unorm8x4],
            }),
        ];
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &mesh_vb,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth(true, wgpu::CompareFunction::Less),
            multisample: ms,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &opaque_target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let make_ghost = |label: &str, entry: &str, cmp: wgpu::CompareFunction| {
            // pushed back a little so a ghost lying exactly on a surface doesn't flicker
            let mut ds = depth(false, cmp);
            if let Some(d) = ds.as_mut() {
                d.bias = wgpu::DepthBiasState { constant: 8, slope_scale: 2.0, clamp: 0.0 };
            }
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&mesh_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_mesh"),
                    compilation_options: Default::default(),
                    buffers: &mesh_vb,
                },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: ds,
                multisample: ms,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &blend_target,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let ghost_pipeline = make_ghost("ghost pipeline", "fs_ghost", wgpu::CompareFunction::LessEqual);
        let xray_pipeline = make_ghost("x-ray pipeline", "fs_ghost_xray", wgpu::CompareFunction::Always);
        let line_vb = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
        })];
        let make_line = |label: &str, topo: wgpu::PrimitiveTopology, ds: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&line_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_line"),
                    compilation_options: Default::default(),
                    buffers: &line_vb,
                },
                primitive: wgpu::PrimitiveState { topology: topo, cull_mode: None, ..Default::default() },
                depth_stencil: ds,
                multisample: ms,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_line"),
                    compilation_options: Default::default(),
                    targets: &blend_target,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri_pipeline = make_line("tri pipeline", wgpu::PrimitiveTopology::TriangleList, depth(false, wgpu::CompareFunction::LessEqual));
        let line_pipeline = make_line("line pipeline", wgpu::PrimitiveTopology::LineList, depth(false, wgpu::CompareFunction::LessEqual));
        let overlay_pipeline =
            make_line("overlay pipeline", wgpu::PrimitiveTopology::LineList, depth(false, wgpu::CompareFunction::Always));

        Self {
            mesh_pipeline,
            tri_pipeline,
            line_pipeline,
            overlay_pipeline,
            ghost_pipeline,
            xray_pipeline,
            cam_buf,
            cam_bg,
            model_bgl,
            models,
            model_bg,
            lines,
            meshes: HashMap::new(),
            frame: vec![],
            ghost_frame: vec![],
            line_ranges: [0..0, 0..0, 0..0],
        }
    }

    fn make_model_bg(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, buf: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model bg"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: buf,
                    offset: 0,
                    size: NonZeroU64::new(std::mem::size_of::<ModelUniform>() as u64),
                }),
            }],
        })
    }
}

/// Everything needed to draw one frame of the viewport.
pub struct ViewportFrame {
    pub camera: CameraUniform,
    pub items: Vec<DrawItem>,
    /// Translucent meshes (previews of text, originals behind a preview).
    pub ghosts: Vec<DrawItem>,
    /// Translucent triangles drawn after meshes (build plate, cut plane).
    pub tris: Vec<LineVertex>,
    /// Depth-tested lines (grid, axes).
    pub lines: Vec<LineVertex>,
    /// Always-on-top lines (measurements, bounding boxes).
    pub overlay: Vec<LineVertex>,
}

impl egui_wgpu::CallbackTrait for ViewportFrame {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<ViewportRenderer>() else { return vec![] };
        queue.write_buffer(&r.cam_buf, 0, bytemuck::bytes_of(&self.camera));

        // upload new meshes, drop unused ones
        let mut used = HashSet::new();
        for it in self.items.iter().chain(&self.ghosts) {
            used.insert(it.mesh.id);
            let painted = it.mesh.mesh.is_painted();
            match r.meshes.get_mut(&it.mesh.id) {
                Some(g) if g.ibuf.is_none() == painted => {
                    if g.rev != it.mesh.paint_rev {
                        if painted {
                            queue.write_buffer(&g.cbuf, 0, bytemuck::cast_slice(&paint_vertices(&it.mesh.mesh)));
                        }
                        g.rev = it.mesh.paint_rev;
                    }
                }
                _ => {
                    r.meshes.insert(it.mesh.id, GpuMesh::new(device, &it.mesh));
                }
            }
        }
        r.meshes.retain(|k, _| used.contains(k));

        // per-object uniforms
        let n = self.items.len() + self.ghosts.len();
        let mut bytes = vec![0u8; n.max(1) * MODEL_STRIDE as usize];
        r.frame.clear();
        r.ghost_frame.clear();
        for (i, it) in self.items.iter().chain(&self.ghosts).enumerate() {
            let u = ModelUniform { model: it.model.to_cols_array_2d(), color: it.color, params: it.params };
            let o = i * MODEL_STRIDE as usize;
            bytes[o..o + std::mem::size_of::<ModelUniform>()].copy_from_slice(bytemuck::bytes_of(&u));
            let entry = (it.mesh.id, (i as u64 * MODEL_STRIDE) as u32);
            if i < self.items.len() {
                r.frame.push(entry);
            } else {
                r.ghost_frame.push(entry);
            }
        }
        if r.models.write(device, queue, "models", wgpu::BufferUsages::UNIFORM, &bytes) {
            r.model_bg = ViewportRenderer::make_model_bg(device, &r.model_bgl, &r.models.buf);
        }

        // lines
        let mut lv: Vec<LineVertex> = Vec::with_capacity(self.tris.len() + self.lines.len() + self.overlay.len());
        let a = self.tris.len() as u32;
        lv.extend_from_slice(&self.tris);
        let b = a + self.lines.len() as u32;
        lv.extend_from_slice(&self.lines);
        let c = b + self.overlay.len() as u32;
        lv.extend_from_slice(&self.overlay);
        r.line_ranges = [0..a, a..b, b..c];
        r.lines.write(device, queue, "lines", wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&lv));
        vec![]
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(r) = resources.get::<ViewportRenderer>() else { return };
        pass.set_bind_group(0, &r.cam_bg, &[]);

        pass.set_pipeline(&r.mesh_pipeline);
        for (mesh_id, offset) in &r.frame {
            let Some(g) = r.meshes.get(mesh_id) else { continue };
            pass.set_bind_group(1, &r.model_bg, &[*offset]);
            g.draw(pass);
        }

        let draw_lines = |pass: &mut wgpu::RenderPass<'static>, pipe: &wgpu::RenderPipeline, range: &std::ops::Range<u32>| {
            if !range.is_empty() {
                pass.set_pipeline(pipe);
                pass.set_vertex_buffer(0, r.lines.buf.slice(..));
                pass.draw(range.clone(), 0..1);
            }
        };
        draw_lines(pass, &r.tri_pipeline, &r.line_ranges[0]);
        draw_lines(pass, &r.line_pipeline, &r.line_ranges[1]);
        // ghosts: a faint x-ray pass (visible through other objects), then a depth-tested pass
        for pipe in [&r.xray_pipeline, &r.ghost_pipeline] {
            pass.set_pipeline(pipe);
            for (mesh_id, offset) in &r.ghost_frame {
                let Some(g) = r.meshes.get(mesh_id) else { continue };
                pass.set_bind_group(1, &r.model_bg, &[*offset]);
                g.draw(pass);
            }
        }
        draw_lines(pass, &r.overlay_pipeline, &r.line_ranges[2]);
    }
}
