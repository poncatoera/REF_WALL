//! Drawing into an off-screen picture instead of the window, for exports.

use glow::HasContext;

use crate::{RenderError, Renderer};

impl Renderer {
    /// Largest picture [`Renderer::render_to_pixels`] can draw on this GPU
    /// (at least 2048 on every WebGL2 / GLES 3.0 device).
    pub fn max_picture_size(&self) -> u32 {
        let gl = self.gl();
        let tex = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        let rb = unsafe { gl.get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE) };
        tex.min(rb).max(2048) as u32
    }

    /// Runs `draw` with an off-screen `size` picture as the target and
    /// returns its pixels as straight RGBA8, top row first. The window's own
    /// framebuffer is bound again afterwards.
    pub fn render_to_pixels(&mut self, size: [u32; 2], draw: impl FnOnce(&mut Renderer)) -> Result<Vec<u8>, RenderError> {
        let [w, h] = size;
        if w == 0 || h == 0 || w > self.max_picture_size() || h > self.max_picture_size() {
            return Err(RenderError::Gl(format!("picture size {w}×{h} is not possible here")));
        }
        let (fbo, tex) = unsafe {
            let gl = self.gl();
            let tex = gl.create_texture().map_err(RenderError::Gl)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, w as i32, h as i32);
            let fbo = gl.create_framebuffer().map_err(RenderError::Gl)?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(fbo);
                gl.delete_texture(tex);
                return Err(RenderError::Gl(format!("off-screen picture incomplete ({status:#x})")));
            }
            (fbo, tex)
        };
        draw(self);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        unsafe {
            let gl = self.gl();
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_framebuffer(fbo);
            gl.delete_texture(tex);
        }
        // GL's rows run bottom-up.
        let row = (w * 4) as usize;
        let mut flipped = Vec::with_capacity(pixels.len());
        for r in pixels.chunks_exact(row).rev() {
            flipped.extend_from_slice(r);
        }
        Ok(flipped)
    }
}
