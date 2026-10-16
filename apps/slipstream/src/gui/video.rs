//! GPU video presentation: decoded I420 planes are uploaded as three R8
//! textures and converted to RGB in a fragment shader, so no CPU colour
//! conversion or RGBA copies happen per frame.

use std::{collections::HashMap, sync::Arc};

use eframe::{
    egui,
    egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor, wgpu},
};
use slipstream_session::YuvFrame;

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle covering the viewport (egui sets it to our rect).
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var tex_y: texture_2d<f32>;
@group(0) @binding(1) var tex_u: texture_2d<f32>;
@group(0) @binding(2) var tex_v: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // BT.601 limited range, matching the encoder.
    let y = (textureSample(tex_y, samp, in.uv).r - 0.0625) * 1.164;
    let u = textureSample(tex_u, samp, in.uv).r - 0.5;
    let v = textureSample(tex_v, samp, in.uv).r - 0.5;
    let rgb = vec3<f32>(y + 1.596 * v, y - 0.392 * u - 0.813 * v, y + 2.017 * u);
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// Shared GPU objects, stored in egui-wgpu's callback resources.
pub struct VideoPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    streams: HashMap<u64, StreamTextures>,
}

struct StreamTextures {
    size: (u32, u32),
    textures: [wgpu::Texture; 3],
    bind_group: wgpu::BindGroup,
    serial: u64,
}

/// Registers the pipeline. Call once at startup with eframe's render state.
pub fn install(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("slipstream-yuv"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("slipstream-yuv"),
        entries: &[
            tex_entry(0),
            tex_entry(1),
            tex_entry(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("slipstream-yuv"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("slipstream-yuv"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(render_state.target_format.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("slipstream-yuv"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    render_state.renderer.write().callback_resources.insert(VideoPipeline {
        pipeline,
        layout,
        sampler,
        streams: HashMap::new(),
    });
}

/// Paint callback for one stream's current frame.
pub struct VideoCallback {
    pub stream: u64,
    pub serial: u64,
    pub frame: Arc<YuvFrame>,
}

impl VideoPipeline {
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, cb: &VideoCallback) {
        let f = &cb.frame;
        let size = (f.width, f.height);
        let needs_new = self.streams.get(&cb.stream).is_none_or(|s| s.size != size);
        if needs_new {
            let make = |w: u32, h: u32| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("slipstream-plane"),
                    size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            };
            let (cw, ch) = (f.width.div_ceil(2), f.height.div_ceil(2));
            let textures = [make(f.width, f.height), make(cw, ch), make(cw, ch)];
            let views: Vec<_> = textures.iter().map(|t| t.create_view(&Default::default())).collect();
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("slipstream-yuv"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[0]) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[1]) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[2]) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.streams.insert(cb.stream, StreamTextures { size, textures, bind_group, serial: 0 });
        }
        let s = self.streams.get_mut(&cb.stream).expect("inserted");
        if s.serial == cb.serial {
            return;
        }
        s.serial = cb.serial;
        let (cw, ch) = (f.width.div_ceil(2), f.height.div_ceil(2));
        let planes: [(&[u8], u32, u32, u32); 3] = [
            (&f.y, f.y_stride, f.width, f.height),
            (&f.u, f.uv_stride, cw, ch),
            (&f.v, f.uv_stride, cw, ch),
        ];
        for (tex, (data, stride, w, h)) in s.textures.iter().zip(planes) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
    }

    /// Frees textures for streams that are gone.
    pub fn retain(&mut self, alive: impl Fn(u64) -> bool) {
        self.streams.retain(|id, _| alive(*id));
    }
}

impl CallbackTrait for VideoCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(p) = resources.get_mut::<VideoPipeline>() {
            p.upload(device, queue, self);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(p) = resources.get::<VideoPipeline>() else { return };
        let Some(s) = p.streams.get(&self.stream) else { return };
        render_pass.set_pipeline(&p.pipeline);
        render_pass.set_bind_group(0, &s.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

/// Largest rect with the video's aspect ratio that fits in `area`.
pub fn fit_rect(area: egui::Rect, video: (u32, u32)) -> egui::Rect {
    let (vw, vh) = (video.0.max(1) as f32, video.1.max(1) as f32);
    let scale = (area.width() / vw).min(area.height() / vh);
    egui::Rect::from_center_size(area.center(), egui::vec2(vw * scale, vh * scale))
}

pub fn retain_streams(render_state: &egui_wgpu::RenderState, alive: impl Fn(u64) -> bool) {
    if let Some(p) = render_state.renderer.write().callback_resources.get_mut::<VideoPipeline>() {
        p.retain(alive);
    }
}
