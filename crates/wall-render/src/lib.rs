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
const GLYPH_VS: &str = include_str!("../../../shaders/generated/glyph.vert.glsl");
const GLYPH_FS: &str = include_str!("../../../shaders/generated/glyph.frag.glsl");

/// Width of the glyph data textures (must match `wall_text::TEXTURE_WIDTH`).
pub const GLYPH_TEXTURE_WIDTH: u32 = 4096;

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

/// One glyph quad (see `shaders/glyph.slang`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GlyphInstance {
    /// Glyph box on the board: x, y, w, h (y down).
    pub rect: [f32; 4],
    /// Glyph bounds in em units: x0, y0, x1, y1 (y up).
    pub em: [f32; 4],
    /// Premultiplied RGBA.
    pub color: [f32; 4],
    /// curve_base, band_base, bands_h, bands_v.
    pub data: [f32; 4],
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

struct InstancedPass {
    program: glow::Program,
    vao: glow::VertexArray,
    instances: glow::Buffer,
    capacity: usize,
}

pub struct Renderer {
    gl: glow::Context,
    boxes: InstancedPass,
    glyphs: InstancedPass,
    curve_tex: glow::Texture,
    band_tex: glow::Texture,
    view_ubo: glow::Buffer,
}

impl Renderer {
    /// # Safety
    /// `gl` must be current on this thread for the renderer's whole life.
    pub unsafe fn new(gl: glow::Context, target: GlslTarget) -> Result<Self, RenderError> {
        let view_ubo = gl.create_buffer().map_err(RenderError::Gl)?;
        gl.bind_buffer(glow::UNIFORM_BUFFER, Some(view_ubo));
        gl.buffer_data_size(glow::UNIFORM_BUFFER, std::mem::size_of::<ViewUniform>() as i32, glow::DYNAMIC_DRAW);

        let boxes = InstancedPass::new(&gl, BOX_VS, BOX_FS, target, std::mem::size_of::<BoxInstance>(), &[4, 4, 4, 2])?;
        let glyphs = InstancedPass::new(&gl, GLYPH_VS, GLYPH_FS, target, std::mem::size_of::<GlyphInstance>(), &[4, 4, 4, 4])?;

        // Glyph samplers: texture unit 0 = curves, 1 = bands, matched by name
        // so this keeps working whatever the shader compiler calls them.
        gl.use_program(Some(glyphs.program));
        let n = gl.get_active_uniforms(glyphs.program);
        for i in 0..n {
            if let Some(u) = gl.get_active_uniform(glyphs.program, i) {
                if u.utype == glow::SAMPLER_2D {
                    let unit = if u.name.contains("band") { 1 } else { 0 };
                    let loc = gl.get_uniform_location(glyphs.program, &u.name);
                    gl.uniform_1_i32(loc.as_ref(), unit);
                }
            }
        }
        gl.use_program(None);

        let curve_tex = data_texture(&gl, glow::RGBA32F, glow::RGBA)?;
        let band_tex = data_texture(&gl, glow::R32F, glow::RED)?;
        Ok(Self { gl, boxes, glyphs, curve_tex, band_tex, view_ubo })
    }

    /// Uploads the glyph curve and band arrays (from `wall_text::Atlas`),
    /// already padded to whole rows of `GLYPH_TEXTURE_WIDTH` texels.
    pub fn upload_glyph_data(&mut self, curves_rgba: &[f32], bands_r: &[f32]) {
        let w = GLYPH_TEXTURE_WIDTH as usize;
        let gl = &self.gl;
        unsafe {
            for (tex, data, comps, ifmt, fmt) in [
                (self.curve_tex, curves_rgba, 4, glow::RGBA32F, glow::RGBA),
                (self.band_tex, bands_r, 1, glow::R32F, glow::RED),
            ] {
                let rows = (data.len() / (w * comps)).max(1);
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_image_2d(
                    glow::TEXTURE_2D, 0, ifmt as i32, w as i32, rows as i32, 0, fmt, glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(data))),
                );
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
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

    fn set_view(&self, view: &ViewUniform) {
        unsafe {
            self.gl.enable(glow::BLEND);
            self.gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            self.gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.view_ubo));
            self.gl.buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 0, bytemuck::bytes_of(view));
            self.gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.view_ubo));
        }
    }

    pub fn draw_boxes(&mut self, view: &ViewUniform, boxes: &[BoxInstance]) {
        if boxes.is_empty() {
            return;
        }
        self.set_view(view);
        unsafe { self.boxes.draw(&self.gl, bytemuck::cast_slice(boxes), boxes.len()) };
    }

    pub fn draw_glyphs(&mut self, view: &ViewUniform, glyphs: &[GlyphInstance]) {
        if glyphs.is_empty() {
            return;
        }
        self.set_view(view);
        unsafe {
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.curve_tex));
            self.gl.active_texture(glow::TEXTURE1);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.band_tex));
            self.gl.active_texture(glow::TEXTURE0);
            self.glyphs.draw(&self.gl, bytemuck::cast_slice(glyphs), glyphs.len());
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            self.boxes.delete(&self.gl);
            self.glyphs.delete(&self.gl);
            self.gl.delete_texture(self.curve_tex);
            self.gl.delete_texture(self.band_tex);
            self.gl.delete_buffer(self.view_ubo);
        }
    }
}

impl InstancedPass {
    /// A program drawing one 6-vertex quad per instance. `sizes` are the float
    /// counts of the instance attributes at locations 0, 1, 2…
    unsafe fn new(gl: &glow::Context, vs: &str, fs: &str, target: GlslTarget, stride: usize, sizes: &[i32]) -> Result<Self, RenderError> {
        let program = link(gl, &adapt_glsl(vs, target), &adapt_glsl(fs, target))?;
        // Bind the first uniform block to slot 0, whatever the compiler named it.
        gl.uniform_block_binding(program, 0, 0);
        let vao = gl.create_vertex_array().map_err(RenderError::Gl)?;
        let instances = gl.create_buffer().map_err(RenderError::Gl)?;
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
        let mut offset = 0;
        for (loc, &size) in sizes.iter().enumerate() {
            gl.enable_vertex_attrib_array(loc as u32);
            gl.vertex_attrib_pointer_f32(loc as u32, size, glow::FLOAT, false, stride as i32, offset);
            gl.vertex_attrib_divisor(loc as u32, 1);
            offset += size * 4;
        }
        gl.bind_vertex_array(None);
        Ok(Self { program, vao, instances, capacity: 0 })
    }

    unsafe fn draw(&mut self, gl: &glow::Context, bytes: &[u8], count: usize) {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
        if bytes.len() > self.capacity {
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::DYNAMIC_DRAW);
            self.capacity = bytes.len();
        } else {
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
        }
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, count as i32);
        gl.bind_vertex_array(None);
    }

    unsafe fn delete(&self, gl: &glow::Context) {
        gl.delete_program(self.program);
        gl.delete_vertex_array(self.vao);
        gl.delete_buffer(self.instances);
    }
}

/// A float data texture read with texelFetch: nearest sampling, no mips.
unsafe fn data_texture(gl: &glow::Context, ifmt: u32, fmt: u32) -> Result<glow::Texture, RenderError> {
    let tex = gl.create_texture().map_err(RenderError::Gl)?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    for (k, v) in [
        (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
        (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
    ] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
    }
    let zeros = [0f32; 4];
    let comps = if fmt == glow::RGBA { 4 } else { 1 };
    gl.tex_image_2d(glow::TEXTURE_2D, 0, ifmt as i32, 1, 1, 0, fmt, glow::FLOAT,
        glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&zeros[..comps]))));
    gl.bind_texture(glow::TEXTURE_2D, None);
    Ok(tex)
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
        assert_eq!(std::mem::size_of::<GlyphInstance>(), 64);
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
