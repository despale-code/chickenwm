//! Phase 3.2, 3.3 & 3.5: Premultiplied alpha blending pipeline, Catmull-Rom bicubic filter,
//! vertex deformation push constants, and Wallpaper / Exposé layer state.

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

/// Configures Vulkan pipeline states for compositing window quads.
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
