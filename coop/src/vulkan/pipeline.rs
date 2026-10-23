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

/// Configures Vulkan pipeline states for compositing window quads and GPU border accents.
pub struct CompositorPipeline {
    pub blend_attachment: vk::PipelineColorBlendAttachmentState,
    pub layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
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

    /// Records a crisp animated GPU border frame around a `WindowQuad` inside an active render pass
    /// using `vkCmdClearAttachments`, providing visual spring-damper feedback at 240Hz+.
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
    ) {
        let local_x = (quad.physics.position.x.round() as i32) - mon_x;
        let local_y = (quad.physics.position.y.round() as i32) - mon_y;
        let w = quad.width.round() as i32;
        let h = quad.height.round() as i32;
        let t = thickness.max(2);

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

        let mut rects = Vec::with_capacity(4);
        // Top, Bottom, Left, Right border strips
        if let Some(r) = clamp_rect(local_x - t, local_y - t, w + 2 * t, t) {
            rects.push(r);
        }
        if let Some(r) = clamp_rect(local_x - t, local_y + h, w + 2 * t, t) {
            rects.push(r);
        }
        if let Some(r) = clamp_rect(local_x - t, local_y, t, h) {
            rects.push(r);
        }
        if let Some(r) = clamp_rect(local_x + w, local_y, t, h) {
            rects.push(r);
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
