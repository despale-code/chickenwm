//! Phase 3.2, 3.3 & 3.5: Premultiplied alpha blending pipeline, Catmull-Rom bicubic filter,
//! vertex deformation push constants, Wallpaper / Exposé layer state, and GPU quad/border command recording.

use crate::scene::WindowQuad;
use anyhow::Result;
use ash::vk;

/// Push constants sent to the vertex and fragment shaders per window quad.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct QuadPushConstants {
    /// 4x4 Model-View-Projection matrix.
    pub mvp: [[f32; 4]; 4],
    /// Spring velocity vector used for non-rigid wobbly/jelly vertex displacement.
    pub velocity: [f32; 2],
    /// UV scale factors for live interactive resize (anchor-and-pad / stretch).
    pub uv_scale: [f32; 2],
    /// Source texture resolution in pixels (for 4-tap Catmull-Rom bicubic sampling).
    pub tex_size: [f32; 2],
    /// Corner radius in pixels and opacity [0.0..1.0].
    pub corner_radius: f32,
    pub opacity: f32,
}

impl Default for QuadPushConstants {
    fn default() -> Self {
        Self {
            mvp: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            velocity: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            tex_size: [1920.0, 1080.0],
            corner_radius: 10.0,
            opacity: 1.0,
        }
    }
}

/// Phase 3.3: Evaluates the 4 Catmull-Rom cubic spline weights for fractional texel offset `t` in `[0, 1]`.
/// Used for crisp 150–200% virtual DPI downsampling without bilinear blur.
#[inline]
pub fn catmull_rom_weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

/// Phase 3.2: Returns the exact Vulkan color blend attachment state for DWM/WGC premultiplied alpha surfaces.
/// Eliminates dark/black halos around translucent window shadows and rounded corners.
pub fn premultiplied_alpha_blend_attachment() -> vk::PipelineColorBlendAttachmentState {
    vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(vk::ColorComponentFlags::RGBA)
}

/// Push constants for the anti-aliased SDF macOS Shadowless Dual-Rim rounded border shader.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BorderPushConstants {
    /// Window rectangle `(x, y, width, height)` in monitor-local pixels.
    pub rect: [f32; 4],
    /// `(screen_width, screen_height, corner_radius, 0.0)`.
    pub screen_and_radius: [f32; 4],
    /// Outer 1px perimeter stroke color `[r, g, b, a]` (unpremultiplied).
    pub outer_color: [f32; 4],
    /// Inner top 1px specular bevel highlight `[r, g, b, a]` (unpremultiplied).
    pub inner_top_color: [f32; 4],
    /// Background color `[r, g, b, a]` used to mask sharp 90-degree blitted corners outside the rounded arc.
    pub corner_mask_color: [f32; 4],
}

const BORDER_SHADER_WGSL: &str = r#"
struct BorderPushConstants {
    rect: vec4<f32>,
    screen_and_radius: vec4<f32>,
    outer_color: vec4<f32>,
    inner_top_color: vec4<f32>,
    corner_mask_color: vec4<f32>,
};

var<push_constant> pc: BorderPushConstants;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local_p: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOutput {
    var uv = vec2<f32>(0.0, 0.0);
    switch vid {
        case 0u: { uv = vec2<f32>(0.0, 0.0); }
        case 1u: { uv = vec2<f32>(1.0, 0.0); }
        case 2u: { uv = vec2<f32>(0.0, 1.0); }
        case 3u: { uv = vec2<f32>(0.0, 1.0); }
        case 4u: { uv = vec2<f32>(1.0, 0.0); }
        default: { uv = vec2<f32>(1.0, 1.0); }
    }

    let margin = 2.0;
    let box_min = pc.rect.xy - vec2<f32>(margin, margin);
    let box_size = pc.rect.zw + vec2<f32>(margin * 2.0, margin * 2.0);
    let pixel_pos = box_min + uv * box_size;

    let center = pc.rect.xy + pc.rect.zw * 0.5;
    let screen_size = max(pc.screen_and_radius.xy, vec2<f32>(1.0, 1.0));
    let ndc = (pixel_pos / screen_size) * 2.0 - vec2<f32>(1.0, 1.0);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.local_p = pixel_pos - center;
    return out;
}

fn sd_rounded_box(p: vec2<f32>, half_size: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - (half_size - vec2<f32>(r, r));
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let half_size = pc.rect.zw * 0.5;
    let max_r = min(half_size.x, half_size.y);
    let r = clamp(pc.screen_and_radius.z, 0.0, max_r);
    let p = in.local_p;

    let d = sd_rounded_box(p, half_size, r);

    if (d < -1.5 || d > 1.5) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // 1. Anti-aliased corner clipping mask inside the 4 corner regions of the blitted rectangular window
    var mask_cov = 0.0;
    if (r > 0.5 && abs(p.x) <= half_size.x && abs(p.y) <= half_size.y) {
        if (abs(p.x) > half_size.x - r && abs(p.y) > half_size.y - r) {
            mask_cov = clamp(d + 0.5, 0.0, 1.0);
        }
    }
    let a_m = pc.corner_mask_color.a * mask_cov;
    let rgb_m = pc.corner_mask_color.rgb;

    // 2. Anti-aliased 1px outer perimeter stroke: d in [0.0, 1.0]
    let outer_cov = clamp(min(d + 0.5, 1.5 - d), 0.0, 1.0);

    // 3. Anti-aliased 1px inner top specular bevel: inset 0 1px 0 0
    // Inside original rounded box (d <= 0) and outside rounded box shifted down by +1px (d_shift >= 0)
    let d_shift = sd_rounded_box(p - vec2<f32>(0.0, 1.0), half_size, r);
    let inner_cov = clamp(min(0.5 - d, 0.5 + d_shift), 0.0, 1.0);

    let a_outer = pc.outer_color.a * outer_cov;
    let a_inner = pc.inner_top_color.a * inner_cov;

    // Composite inner top bevel and outer perimeter stroke
    let a_s = a_outer + (1.0 - a_outer) * a_inner;
    let premul_s = pc.outer_color.rgb * a_outer + (1.0 - a_outer) * (pc.inner_top_color.rgb * a_inner);

    // Composite dual-rim stroke over corner mask in premultiplied alpha space
    let out_premul_rgb = premul_s + (1.0 - a_s) * (rgb_m * a_m);
    let out_a = a_s + (1.0 - a_s) * a_m;

    return vec4<f32>(out_premul_rgb, out_a);
}
"#;

/// Compiles the embedded WGSL rounded-border shader into Vertex and Fragment SPIR-V words.
pub fn compile_border_spirv() -> Result<(Vec<u32>, Vec<u32>)> {
    let module = naga::front::wgsl::parse_str(BORDER_SHADER_WGSL)
        .map_err(|e| anyhow::anyhow!("WGSL parse error: {}", e))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::PUSH_CONSTANT,
    )
    .validate(&module)
    .map_err(|e| anyhow::anyhow!("WGSL validation error: {:?}", e))?;

    let options = naga::back::spv::Options {
        lang_version: (1, 0),
        flags: naga::back::spv::WriterFlags::LABEL_VARYINGS,
        ..Default::default()
    };

    let vs_pipe = naga::back::spv::PipelineOptions {
        shader_stage: naga::ShaderStage::Vertex,
        entry_point: "vs_main".to_string(),
    };
    let fs_pipe = naga::back::spv::PipelineOptions {
        shader_stage: naga::ShaderStage::Fragment,
        entry_point: "fs_main".to_string(),
    };

    let vs_words = naga::back::spv::write_vec(&module, &info, &options, Some(&vs_pipe))?;
    let fs_words = naga::back::spv::write_vec(&module, &info, &options, Some(&fs_pipe))?;
    Ok((vs_words, fs_words))
}

/// Configures Vulkan pipeline states for compositing window quads and GPU border accents.
pub struct CompositorPipeline {
    pub blend_attachment: vk::PipelineColorBlendAttachmentState,
    pub layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    pub border_layout: vk::PipelineLayout,
    pub border_pipeline: vk::Pipeline,
}

impl CompositorPipeline {
    /// Creates the quad rendering pipeline configured with premultiplied alpha:
    /// - `srcColorBlendFactor = VK_BLEND_FACTOR_ONE`
    /// - `dstColorBlendFactor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA`
    /// - `srcAlphaBlendFactor = VK_BLEND_FACTOR_ONE`
    /// - `dstAlphaBlendFactor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA`
    pub fn new() -> Result<Self> {
        Ok(Self {
            blend_attachment: premultiplied_alpha_blend_attachment(),
            layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
            border_layout: vk::PipelineLayout::null(),
            border_pipeline: vk::Pipeline::null(),
        })
    }

    /// Initializes the Vulkan `VkPipelineLayout` with `QuadPushConstants` range.
    pub fn init_layout(&mut self, device: &ash::Device) -> Result<()> {
        unsafe {
            let push_constant_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<QuadPushConstants>() as u32)];

            let layout_info = vk::PipelineLayoutCreateInfo::default()
                .push_constant_ranges(&push_constant_ranges);

            self.layout = device.create_pipeline_layout(&layout_info, None)?;
        }
        Ok(())
    }

    /// Compiles and initializes the anti-aliased SDF rounded border `VkPipeline` for `render_pass`.
    pub fn init_border_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
    ) -> Result<()> {
        if self.border_pipeline != vk::Pipeline::null() {
            return Ok(());
        }

        let (vs_words, fs_words) = compile_border_spirv()?;

        unsafe {
            let push_constant_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<BorderPushConstants>() as u32)];

            let layout_info = vk::PipelineLayoutCreateInfo::default()
                .push_constant_ranges(&push_constant_ranges);
            self.border_layout = device.create_pipeline_layout(&layout_info, None)?;

            let vs_info = vk::ShaderModuleCreateInfo::default().code(&vs_words);
            let fs_info = vk::ShaderModuleCreateInfo::default().code(&fs_words);
            let vs_module = device.create_shader_module(&vs_info, None)?;
            let fs_module = device.create_shader_module(&fs_info, None)?;

            let vs_entry = std::ffi::CString::new("vs_main")?;
            let fs_entry = std::ffi::CString::new("fs_main")?;

            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vs_module)
                    .name(&vs_entry),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(fs_module)
                    .name(&fs_entry),
            ];

            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
            let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
                .primitive_restart_enable(false);

            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);

            let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
                .depth_clamp_enable(false)
                .rasterizer_discard_enable(false)
                .polygon_mode(vk::PolygonMode::FILL)
                .line_width(1.0)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::CLOCKWISE)
                .depth_bias_enable(false);

            let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
                .sample_shading_enable(false)
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);

            let color_blend_attachments = [self.blend_attachment];
            let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
                .logic_op_enable(false)
                .attachments(&color_blend_attachments);

            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic_state = vk::PipelineDynamicStateCreateInfo::default()
                .dynamic_states(&dynamic_states);

            let pipeline_info = [vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterizer)
                .multisample_state(&multisampling)
                .color_blend_state(&color_blending)
                .dynamic_state(&dynamic_state)
                .layout(self.border_layout)
                .render_pass(render_pass)
                .subpass(0)];

            let pipelines = device
                .create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_info, None)
                .map_err(|(_, e)| e)?;
            self.border_pipeline = pipelines[0];

            device.destroy_shader_module(vs_module, None);
            device.destroy_shader_module(fs_module, None);
        }
        Ok(())
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.border_pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.border_pipeline, None);
                self.border_pipeline = vk::Pipeline::null();
            }
            if self.border_layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.border_layout, None);
                self.border_layout = vk::PipelineLayout::null();
            }
            if self.pipeline != vk::Pipeline::null() {
                device.destroy_pipeline(self.pipeline, None);
                self.pipeline = vk::Pipeline::null();
            }
            if self.layout != vk::PipelineLayout::null() {
                device.destroy_pipeline_layout(self.layout, None);
                self.layout = vk::PipelineLayout::null();
            }
        }
    }

    /// Records a crisp animated GPU border frame around a `WindowQuad` inside an active render pass
    /// using `vkCmdClearAttachments`, with exact circular fillet corner arcs based on `radius`.
    pub fn record_quad_border(
        &self,
        device: &ash::Device,
        cmd_buf: vk::CommandBuffer,
        quad: &WindowQuad,
        mon_x: i32,
        mon_y: i32,
        mon_w: u32,
        mon_h: u32,
        rgba: [f32; 4],
        thickness: i32,
        radius: i32,
    ) {
        let local_x = (quad.physics.position.x.round() as i32) - mon_x;
        let local_y = (quad.physics.position.y.round() as i32) - mon_y;
        let w = quad.width.round() as i32;
        let h = quad.height.round() as i32;
        let t = thickness.max(1);
        let r = radius.clamp(0, (w / 2).min(h / 2));

        let clamp_rect = |rx: i32, ry: i32, rw: i32, rh: i32| -> Option<vk::ClearRect> {
            let x0 = rx.clamp(0, mon_w as i32);
            let y0 = ry.clamp(0, mon_h as i32);
            let x1 = (rx + rw).clamp(0, mon_w as i32);
            let y1 = (ry + rh).clamp(0, mon_h as i32);
            if x1 > x0 && y1 > y0 {
                Some(vk::ClearRect {
                    rect: vk::Rect2D {
                        offset: vk::Offset2D { x: x0, y: y0 },
                        extent: vk::Extent2D {
                            width: (x1 - x0) as u32,
                            height: (y1 - y0) as u32,
                        },
                    },
                    base_array_layer: 0,
                    layer_count: 1,
                })
            } else {
                None
            }
        };

        let mut rects = Vec::with_capacity(40);

        if r <= 1 {
            // Flat rectangular border strips
            if let Some(rect) = clamp_rect(local_x - t, local_y - t, w + 2 * t, t) {
                rects.push(rect);
            }
            if let Some(rect) = clamp_rect(local_x - t, local_y + h, w + 2 * t, t) {
                rects.push(rect);
            }
            if let Some(rect) = clamp_rect(local_x - t, local_y, t, h) {
                rects.push(rect);
            }
            if let Some(rect) = clamp_rect(local_x + w, local_y, t, h) {
                rects.push(rect);
            }
        } else {
            // 1. Straight edge segments between rounded corners
            if w > 2 * r {
                if let Some(rect) = clamp_rect(local_x + r, local_y - t, w - 2 * r, t) {
                    rects.push(rect);
                }
                if let Some(rect) = clamp_rect(local_x + r, local_y + h, w - 2 * r, t) {
                    rects.push(rect);
                }
            }
            if h > 2 * r {
                if let Some(rect) = clamp_rect(local_x - t, local_y + r, t, h - 2 * r) {
                    rects.push(rect);
                }
                if let Some(rect) = clamp_rect(local_x + w, local_y + r, t, h - 2 * r) {
                    rects.push(rect);
                }
            }

            // 2. Discretized circular arc fillet corners
            let cx_left = local_x + r;
            let cx_right = local_x + w - r;
            let cy_top = local_y + r;
            let cy_bottom = local_y + h - r;

            let r_outer = (r + t) as f32;
            let r_inner = r as f32;
            let total_rows = r + t;

            for row in 0..total_rows {
                let dy = (total_rows - 1 - row) as f32 + 0.5;
                if dy < r_outer {
                    let dx_outer = (r_outer * r_outer - dy * dy).sqrt().round() as i32;
                    let dx_inner = if dy < r_inner {
                        (r_inner * r_inner - dy * dy).sqrt().round() as i32
                    } else {
                        0
                    };
                    let span_w = (dx_outer - dx_inner).max(1);

                    // Top-Left corner
                    if let Some(rect) = clamp_rect(cx_left - dx_outer, cy_top - total_rows + row, span_w, 1) {
                        rects.push(rect);
                    }
                    // Top-Right corner
                    if let Some(rect) = clamp_rect(cx_right + dx_inner, cy_top - total_rows + row, span_w, 1) {
                        rects.push(rect);
                    }
                    // Bottom-Left corner
                    if let Some(rect) = clamp_rect(cx_left - dx_outer, cy_bottom + (total_rows - 1 - row), span_w, 1) {
                        rects.push(rect);
                    }
                    // Bottom-Right corner
                    if let Some(rect) = clamp_rect(cx_right + dx_inner, cy_bottom + (total_rows - 1 - row), span_w, 1) {
                        rects.push(rect);
                    }
                }
            }
        }

        if rects.is_empty() {
            return;
        }

        let clear_att = [vk::ClearAttachment {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            color_attachment: 0,
            clear_value: vk::ClearValue {
                color: vk::ClearColorValue { float32: rgba },
            },
        }];

        unsafe {
            device.cmd_clear_attachments(cmd_buf, &clear_att, &rects);
        }
    }

    /// Records the anti-aliased macOS shadowless dual-rim border using the SDF graphics pipeline:
    /// - Outer perimeter: 1px anti-aliased stroke (`0 0 0 1px`) with corner radius (12px default).
    /// - Inner top: 1px anti-aliased specular bevel (`inset 0 1px 0 0`) along the inside top edge & corners.
    /// - Corner clip mask: Smoothly clips sharp 90-degree blitted window texture corners outside the rounded arc.
    pub fn record_mac_dual_rim_border(
        &self,
        device: &ash::Device,
        cmd_buf: vk::CommandBuffer,
        quad: &WindowQuad,
        mon_x: i32,
        mon_y: i32,
        mon_w: u32,
        mon_h: u32,
        colors: MacBorderColors,
        radius: i32,
    ) {
        let local_x = (quad.physics.position.x.round() as i32) - mon_x;
        let local_y = (quad.physics.position.y.round() as i32) - mon_y;
        let w = quad.width.round() as i32;
        let h = quad.height.round() as i32;
        if w <= 0 || h <= 0 {
            return;
        }
        let r = radius.clamp(0, (w / 2).min(h / 2));

        if self.border_pipeline != vk::Pipeline::null()
            && self.border_layout != vk::PipelineLayout::null()
        {
            let pc = BorderPushConstants {
                rect: [local_x as f32, local_y as f32, w as f32, h as f32],
                screen_and_radius: [mon_w.max(1) as f32, mon_h.max(1) as f32, r as f32, 0.0],
                outer_color: colors.outer,
                inner_top_color: colors.inner_top,
                corner_mask_color: colors.corner_mask,
            };

            unsafe {
                device.cmd_bind_pipeline(
                    cmd_buf,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.border_pipeline,
                );
                let pc_bytes = std::slice::from_raw_parts(
                    &pc as *const BorderPushConstants as *const u8,
                    std::mem::size_of::<BorderPushConstants>(),
                );
                device.cmd_push_constants(
                    cmd_buf,
                    self.border_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    pc_bytes,
                );
                device.cmd_draw(cmd_buf, 6, 1, 0, 0);
            }
            return;
        }
    }
}

/// Theme mode (Dark vs Light) for macOS Dual-Rim border styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacBorderTheme {
    Dark,
    Light,
}

/// macOS Shadowless Dual-Rim border colors: outer perimeter stroke, inner top specular bevel,
/// and optional background mask color to clip sharp 90-degree window texture corners.
#[derive(Debug, Clone, Copy)]
pub struct MacBorderColors {
    pub outer: [f32; 4],
    pub inner_top: [f32; 4],
    pub corner_mask: [f32; 4],
}

impl MacBorderColors {
    pub fn for_state(theme: MacBorderTheme, focused: bool) -> Self {
        match (theme, focused) {
            (MacBorderTheme::Light, true) => Self {
                outer: [0.0, 0.0, 0.0, 0.18],
                inner_top: [1.0, 1.0, 1.0, 0.75],
                corner_mask: [0.0, 0.0, 0.0, 0.0],
            },
            (MacBorderTheme::Light, false) => Self {
                outer: [0.0, 0.0, 0.0, 0.10],
                inner_top: [1.0, 1.0, 1.0, 0.40],
                corner_mask: [0.0, 0.0, 0.0, 0.0],
            },
            (MacBorderTheme::Dark, true) => Self {
                outer: [1.0, 1.0, 1.0, 0.22],
                inner_top: [1.0, 1.0, 1.0, 0.12],
                corner_mask: [0.0, 0.0, 0.0, 0.0],
            },
            (MacBorderTheme::Dark, false) => Self {
                outer: [1.0, 1.0, 1.0, 0.08],
                inner_top: [1.0, 1.0, 1.0, 0.04],
                corner_mask: [0.0, 0.0, 0.0, 0.0],
            },
        }
    }

    pub fn with_corner_mask(mut self, mask_rgba: [f32; 4]) -> Self {
        self.corner_mask = mask_rgba;
        self
    }
}

/// Phase 3.5: Background wallpaper layer supporting high-res static images and animated shader uniforms.
#[derive(Debug, Clone)]
pub struct WallpaperLayer {
    pub image_path: Option<String>,
    pub custom_shader_path: Option<String>,
    pub time_seconds: f32,
    pub resolution: [f32; 2],
}

impl Default for WallpaperLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl WallpaperLayer {
    pub fn new() -> Self {
        Self {
            image_path: None,
            custom_shader_path: None,
            time_seconds: 0.0,
            resolution: [1920.0, 1080.0],
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.time_seconds += dt;
    }
}

/// Phase 3.5: 3D Exposé / Bird's-Eye window overview layout calculator.
#[derive(Debug, Clone)]
pub struct ExposeOverview {
    pub active: bool,
    pub transition_progress: f32,
}

impl Default for ExposeOverview {
    fn default() -> Self {
        Self::new()
    }
}

impl ExposeOverview {
    pub fn new() -> Self {
        Self {
            active: false,
            transition_progress: 0.0,
        }
    }

    pub fn toggle(&mut self) {
        self.active = !self.active;
    }

    /// Computes the target `(x, y, width, height)` for window `index` out of `total` windows
    /// arranged in a padded bird's-eye grid across a monitor of `(mon_w, mon_h)`.
    pub fn compute_slot(
        &self,
        index: usize,
        total: usize,
        mon_w: f32,
        mon_h: f32,
    ) -> (f32, f32, f32, f32) {
        if total == 0 {
            return (0.0, 0.0, mon_w, mon_h);
        }
        let cols = (total as f32).sqrt().ceil() as usize;
        let rows = total.div_ceil(cols);
        let pad = 32.0;

        let cell_w = (mon_w - pad * (cols as f32 + 1.0)) / cols as f32;
        let cell_h = (mon_h - pad * (rows as f32 + 1.0)) / rows as f32;

        let col = index % cols;
        let row = index / cols;

        let x = pad + col as f32 * (cell_w + pad);
        let y = pad + row as f32 * (cell_h + pad);
        (x, y, cell_w.max(64.0), cell_h.max(64.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_border_spirv_valid() {
        let (vs, fs) = compile_border_spirv().expect("WGSL border shader must compile to SPIR-V");
        assert!(!vs.is_empty());
        assert!(!fs.is_empty());
        // SPIR-V magic number: 0x07230203
        assert_eq!(vs[0], 0x07230203);
        assert_eq!(fs[0], 0x07230203);
        assert_eq!(std::mem::size_of::<BorderPushConstants>(), 80);
    }
}
