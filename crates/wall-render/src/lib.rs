//! GPU renderer shared by desktop and web.
//!
//! One backend: OpenGL ES 3.0 / WebGL2 through `glow`. Desktop can also run
//! the same shaders on desktop OpenGL 3.3+ (the version line is swapped), which
//! is what the first spike uses before ANGLE is wired in.
//!
//! Shaders are written in Slang (`shaders/*.slang`) and compiled ahead of time
//! into `shaders/generated/*.glsl`, which are embedded here.

use bytemuck::{Pod, Zeroable};
use glow::HasContext;

const BOX_VS: &str = include_str!("../../../shaders/generated/sdf_box.vert.glsl");
const BOX_FS: &str = include_str!("../../../shaders/generated/sdf_box.frag.glsl");

/// Which GLSL dialect the current context accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlslTarget {
    /// WebGL2, OpenGL ES 3.0, ANGLE.
    Es300,
    /// Desktop OpenGL 3.3 core (macOS 4.1 and Windows drivers accept it).
    Core330,
}

/// Rewrites the `#version` line of a generated GLSL ES 3.00 shader.
pub fn adapt_glsl(src: &str, target: GlslTarget) -> String {
    match target {
        GlslTarget::Es300 => src.to_owned(),
        GlslTarget::Core330 => src.replacen("#version 300 es", "#version 330 core", 1),
    }
}

/// One rounded box. Colours are premultiplied RGBA in 0..1.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BoxInstance {
    /// x, y, width, height in board units.
    pub rect: [f32; 4],
    pub fill: [f32; 4],
    pub stroke: [f32; 4],
    /// Corner radius, stroke width (board units).
    pub shape: [f32; 2],
}

/// Board-to-clip transform, laid out as the shader's std140 `View` block.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ViewUniform {
    pub scale: [f32; 2],
    pub offset: [f32; 2],
    pub pixel: f32,
    pub _pad: [f32; 3],
}

impl ViewUniform {
    /// `center` is the board point at the middle of the screen; `zoom` is
    /// screen pixels per board unit; `size_px` the framebuffer size.
    pub fn new(center: [f32; 2], zoom: f32, size_px: [f32; 2]) -> Self {
        let sx = 2.0 * zoom / size_px[0];
        let sy = -2.0 * zoom / size_px[1]; // board y grows downwards
        Self { scale: [sx, sy], offset: [-center[0] * sx, -center[1] * sy], pixel: 1.0 / zoom, _pad: [0.0; 3] }
    }
}

#[derive(Debug)]
pub enum RenderError {
    Shader(String),
    Link(String),
    Gl(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shader(s) => write!(f, "shader compile failed: {s}"),
            Self::Link(s) => write!(f, "shader link failed: {s}"),
            Self::Gl(s) => write!(f, "GL error: {s}"),
        }
    }
}

impl std::error::Error for RenderError {}

pub struct Renderer {
    gl: glow::Context,
    box_program: glow::Program,
    box_vao: glow::VertexArray,
    box_instances: glow::Buffer,
    box_capacity: usize,
    view_ubo: glow::Buffer,
}

impl Renderer {
    /// # Safety
    /// `gl` must be current on this thread for the renderer's whole life.
    pub unsafe fn new(gl: glow::Context, target: GlslTarget) -> Result<Self, RenderError> {
        let box_program = link(&gl, &adapt_glsl(BOX_VS, target), &adapt_glsl(BOX_FS, target))?;

        // Bind the program's first uniform block to slot 0, whatever the
        // shader compiler named it.
        gl.uniform_block_binding(box_program, 0, 0);

        let view_ubo = gl.create_buffer().map_err(RenderError::Gl)?;
        gl.bind_buffer(glow::UNIFORM_BUFFER, Some(view_ubo));
        gl.buffer_data_size(glow::UNIFORM_BUFFER, std::mem::size_of::<ViewUniform>() as i32, glow::DYNAMIC_DRAW);

        let box_vao = gl.create_vertex_array().map_err(RenderError::Gl)?;
        let box_instances = gl.create_buffer().map_err(RenderError::Gl)?;
        gl.bind_vertex_array(Some(box_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(box_instances));
        let stride = std::mem::size_of::<BoxInstance>() as i32;
        let attrs: [(u32, i32, i32); 4] = [(0, 4, 0), (1, 4, 16), (2, 4, 32), (3, 2, 48)];
        for (loc, size, offset) in attrs {
            gl.enable_vertex_attrib_array(loc);
            gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, offset);
            gl.vertex_attrib_divisor(loc, 1);
        }
        gl.bind_vertex_array(None);

        Ok(Self { gl, box_program, box_vao, box_instances, box_capacity: 0, view_ubo })
    }

    pub fn gl(&self) -> &glow::Context {
        &self.gl
    }

    /// Clears the frame. `rgba` is premultiplied.
    pub fn begin_frame(&mut self, size_px: [u32; 2], rgba: [f32; 4]) {
        unsafe {
            self.gl.viewport(0, 0, size_px[0] as i32, size_px[1] as i32);
            self.gl.clear_color(rgba[0], rgba[1], rgba[2], rgba[3]);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    pub fn draw_boxes(&mut self, view: &ViewUniform, boxes: &[BoxInstance]) {
        if boxes.is_empty() {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);

            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.view_ubo));
            gl.buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 0, bytemuck::bytes_of(view));
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.view_ubo));

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.box_instances));
            let bytes: &[u8] = bytemuck::cast_slice(boxes);
            if boxes.len() > self.box_capacity {
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::DYNAMIC_DRAW);
                self.box_capacity = boxes.len();
            } else {
                gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
            }

            gl.use_program(Some(self.box_program));
            gl.bind_vertex_array(Some(self.box_vao));
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, boxes.len() as i32);
            gl.bind_vertex_array(None);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.box_program);
            self.gl.delete_vertex_array(self.box_vao);
            self.gl.delete_buffer(self.box_instances);
            self.gl.delete_buffer(self.view_ubo);
        }
    }
}

unsafe fn link(gl: &glow::Context, vs: &str, fs: &str) -> Result<glow::Program, RenderError> {
    let program = gl.create_program().map_err(RenderError::Gl)?;
    let mut shaders = Vec::new();
    for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
        let sh = gl.create_shader(kind).map_err(RenderError::Gl)?;
        gl.shader_source(sh, src);
        gl.compile_shader(sh);
        if !gl.get_shader_compile_status(sh) {
            return Err(RenderError::Shader(gl.get_shader_info_log(sh)));
        }
        gl.attach_shader(program, sh);
        shaders.push(sh);
    }
    gl.link_program(program);
    let ok = gl.get_program_link_status(program);
    for sh in shaders {
        gl.detach_shader(program, sh);
        gl.delete_shader(sh);
    }
    if ok { Ok(program) } else { Err(RenderError::Link(gl.get_program_info_log(program))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_layout_matches_attribute_offsets() {
        assert_eq!(std::mem::size_of::<BoxInstance>(), 56);
        assert_eq!(std::mem::size_of::<ViewUniform>(), 32);
    }

    #[test]
    fn view_maps_center_to_origin_and_edges_to_clip_edges() {
        let v = ViewUniform::new([100.0, 50.0], 2.0, [800.0, 600.0]);
        let clip = |p: [f32; 2]| [p[0] * v.scale[0] + v.offset[0], p[1] * v.scale[1] + v.offset[1]];
        assert_eq!(clip([100.0, 50.0]), [0.0, 0.0]);
        // 200 board units right = 400 px = right edge.
        assert_eq!(clip([300.0, 50.0])[0], 1.0);
        // Board y down maps to clip y down (negative).
        assert_eq!(clip([100.0, 200.0])[1], -1.0);
        assert_eq!(v.pixel, 0.5);
    }

    #[test]
    fn version_line_swaps_for_desktop_gl() {
        let out = adapt_glsl(BOX_VS, GlslTarget::Core330);
        assert!(out.starts_with("#version 330 core"));
        assert!(!out.contains("300 es"));
    }
}
