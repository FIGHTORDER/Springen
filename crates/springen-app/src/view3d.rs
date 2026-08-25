//! The 3D terrain viewport.
//!
//! A single indexed grid is uploaded once and displaced in the vertex shader
//! from an R32F height texture, so changing a node updates the view by
//! re-uploading one texture rather than rebuilding a mesh. At 1025² a heightmap
//! is about a million vertices, which needs no LOD.
//!
//! Colour comes from a second texture the CPU fills per view mode, which keeps
//! the shader to one branch and lets every mode reuse the analysis code that
//! already exists in `springen-core`.

use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect, Vec2};
use eframe::glow::{self, HasContext};

/// What the terrain is coloured by. The painting itself lives in
/// `springen_core::preview`, so the CLI's `preview` command and this viewport
/// cannot drift apart.
pub use springen_core::preview::ViewMode;

/// Orbit camera, Spring-style: drag to turn, wheel to zoom, middle or shift
/// drag to pan the target across the map.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    /// Distance from the target, in elmos.
    pub distance: f32,
    pub target: [f32; 3],
    /// Vertical exaggeration. 1.0 is true scale.
    pub exaggeration: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            yaw: 0.6,
            pitch: 0.62,
            distance: 9000.0,
            target: [0.0, 0.0, 0.0],
            exaggeration: 1.0,
        }
    }
}

impl Camera {
    /// Frame the whole world. Both extents, because a 16x8 map is not a
    /// square and framing it as one puts half of it off screen.
    pub fn frame_map(&mut self, world: [f32; 2]) {
        self.target = [world[0] * 0.5, 0.0, world[1] * 0.5];
        self.distance = world[0].max(world[1]) * 1.35;
        self.yaw = 0.6;
        self.pitch = 0.62;
    }

    pub fn eye(&self) -> [f32; 3] {
        let cp = self.pitch.cos();
        [
            self.target[0] + self.distance * cp * self.yaw.cos(),
            self.target[1] + self.distance * self.pitch.sin(),
            self.target[2] + self.distance * cp * self.yaw.sin(),
        ]
    }

    /// Column-major view-projection, ready for `uniform_matrix_4_f32_slice`.
    pub fn view_projection(&self, aspect: f32, far: f32) -> [f32; 16] {
        let eye = self.eye();
        let f = normalise(sub(self.target, eye));
        let s = normalise(cross(f, [0.0, 1.0, 0.0]));
        let u = cross(s, f);
        let view = [
            s[0],
            u[0],
            -f[0],
            0.0, //
            s[1],
            u[1],
            -f[1],
            0.0, //
            s[2],
            u[2],
            -f[2],
            0.0, //
            -dot(s, eye),
            -dot(u, eye),
            dot(f, eye),
            1.0,
        ];
        let fovy = 50.0f32.to_radians();
        let t = 1.0 / (fovy / 2.0).tan();
        let near = (self.distance * 0.01).max(1.0);
        let proj = [
            t / aspect,
            0.0,
            0.0,
            0.0,
            0.0,
            t,
            0.0,
            0.0,
            0.0,
            0.0,
            (far + near) / (near - far),
            -1.0,
            0.0,
            0.0,
            2.0 * far * near / (near - far),
            0.0,
        ];
        mul4(proj, view)
    }

    /// The ray leaving the camera through a screen point, as origin and unit
    /// direction in world space.
    fn ray(&self, at: Pos2, rect: Rect, aspect: f32) -> ([f32; 3], [f32; 3]) {
        let eye = self.eye();
        let f = normalise(sub(self.target, eye));
        let s = normalise(cross(f, [0.0, 1.0, 0.0]));
        let u = cross(s, f);
        // Normalised device coordinates, y up.
        let ndc_x = 2.0 * (at.x - rect.min.x) / rect.width().max(1.0) - 1.0;
        let ndc_y = 1.0 - 2.0 * (at.y - rect.min.y) / rect.height().max(1.0);
        let fovy = 50.0f32.to_radians();
        let th = (fovy / 2.0).tan();
        let dir = normalise([
            f[0] + s[0] * ndc_x * th * aspect + u[0] * ndc_y * th,
            f[1] + s[1] * ndc_x * th * aspect + u[1] * ndc_y * th,
            f[2] + s[2] * ndc_x * th * aspect + u[2] * ndc_y * th,
        ]);
        (eye, dir)
    }

    /// Where the cursor is pointing *on the terrain*, in elmos.
    ///
    /// `screen_to_ground` solves the projection's Jacobian, which answers "how
    /// far did this drag move the ground" and is the right tool for dragging an
    /// object that is already picked. A brush needs the other question — what
    /// is under the cursor — and there is no closed form for that against a
    /// height field, so the ray is marched and the crossing bisected.
    ///
    /// `height` returns the terrain height in elmos at a world position, and is
    /// asked only about points inside the world; outside it, the surface is
    /// treated as the waterline plane so a stroke started off the map still has
    /// somewhere to land.
    pub fn pick_terrain(
        &self,
        at: Pos2,
        rect: Rect,
        world: [f32; 2],
        height: &dyn Fn(f32, f32) -> f32,
    ) -> Option<(f32, f32)> {
        let aspect = rect.width() / rect.height().max(1.0);
        let (eye, dir) = self.ray(at, rect, aspect);
        let span = world[0].max(world[1]);
        // Fine enough that a step cannot pass through a ridge unseen at any
        // sane camera distance, and bounded so a ray along the horizon gives up
        // rather than marching forever.
        let step = (span / 900.0).max(1.0);
        let far = span * 4.0 + self.distance * 2.0;
        let sample = |t: f32| -> f32 {
            let (x, z) = (eye[0] + dir[0] * t, eye[2] + dir[2] * t);
            let y = eye[1] + dir[1] * t;
            let ground = if x < 0.0 || z < 0.0 || x > world[0] || z > world[1] {
                0.0
            } else {
                height(x, z)
            };
            y - ground
        };
        let mut t = 0.0f32;
        let mut prev = sample(t);
        if prev < 0.0 {
            // The camera is already under the terrain; nothing sensible to
            // pick.
            return None;
        }
        while t < far {
            let next_t = t + step;
            let next = sample(next_t);
            if next <= 0.0 {
                // Bisect the crossing. Twenty halvings takes a step of a few
                // elmos down to well under one.
                let (mut lo, mut hi) = (t, next_t);
                for _ in 0..20 {
                    let mid = 0.5 * (lo + hi);
                    if sample(mid) > 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let tf = 0.5 * (lo + hi);
                let (x, z) = (eye[0] + dir[0] * tf, eye[2] + dir[2] * tf);
                if x < 0.0 || z < 0.0 || x > world[0] || z > world[1] {
                    return None;
                }
                return Some((x, z));
            }
            prev = next;
            t = next_t;
        }
        let _ = prev;
        None
    }

    /// Turn a screen drag into a movement across the ground, at a world point.
    ///
    /// Solves the projection's local 2×2 Jacobian rather than casting a ray:
    /// `project` is the transform we have, and two extra projections a step
    /// apart give its derivative exactly where the drag is happening — which
    /// is also what makes the mex follow the cursor at any camera angle
    /// instead of at one calibrated one.
    pub fn screen_to_ground(
        &self,
        at: [f32; 3],
        drag: Vec2,
        rect: Rect,
        far: f32,
    ) -> Option<(f32, f32)> {
        const STEP: f32 = 32.0;
        let p0 = self.project(at, rect, far)?;
        let px = self.project([at[0] + STEP, at[1], at[2]], rect, far)?;
        let pz = self.project([at[0], at[1], at[2] + STEP], rect, far)?;
        let a = (px - p0) / STEP;
        let b = (pz - p0) / STEP;
        let det = a.x * b.y - a.y * b.x;
        // Edge on: the ground is a line on screen and a drag across it says
        // nothing about which way to go.
        if det.abs() < 1e-9 {
            return None;
        }
        Some((
            (drag.x * b.y - drag.y * b.x) / det,
            (-drag.x * a.y + drag.y * a.x) / det,
        ))
    }

    /// Project a world point to screen, or `None` if it is behind the camera.
    pub fn project(&self, world: [f32; 3], rect: Rect, far: f32) -> Option<Pos2> {
        let m = self.view_projection(rect.width() / rect.height().max(1.0), far);
        let x = m[0] * world[0] + m[4] * world[1] + m[8] * world[2] + m[12];
        let y = m[1] * world[0] + m[5] * world[1] + m[9] * world[2] + m[13];
        let w = m[3] * world[0] + m[7] * world[1] + m[11] * world[2] + m[15];
        if w <= 1e-4 {
            return None;
        }
        Some(Pos2::new(
            rect.center().x + (x / w) * rect.width() / 2.0,
            rect.center().y - (y / w) * rect.height() / 2.0,
        ))
    }

    /// Apply one frame of mouse input. Returns true if the view moved.
    pub fn interact(&mut self, ui: &egui::Ui, response: &egui::Response, elmos: f32) -> bool {
        let mut moved = false;
        let shift = ui.input(|i| i.modifiers.shift);
        let middle = ui.input(|i| i.pointer.middle_down());
        if response.dragged() {
            let d = response.drag_delta();
            if d != Vec2::ZERO {
                moved = true;
                if shift || middle {
                    // Pan across the ground plane, scaled so the terrain keeps
                    // up with the pointer at any zoom.
                    let k = self.distance * 0.0016;
                    let (sy, cy) = (self.yaw.sin(), self.yaw.cos());
                    self.target[0] += (d.x * sy - d.y * cy) * k;
                    self.target[2] += (-d.x * cy - d.y * sy) * k;
                    let lim = elmos * 1.5;
                    self.target[0] = self.target[0].clamp(-lim, lim * 1.5);
                    self.target[2] = self.target[2].clamp(-lim, lim * 1.5);
                } else {
                    self.yaw -= d.x * 0.006;
                    // Stop just short of the poles so the up vector stays sane.
                    self.pitch = (self.pitch + d.y * 0.006).clamp(0.03, 1.52);
                }
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.01 {
                self.distance =
                    (self.distance * (1.0 - scroll * 0.0015)).clamp(elmos * 0.02, elmos * 4.0);
                moved = true;
            }
        }
        moved
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalise(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}
fn mul4(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    let mut o = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + r] * b[c * 4 + k];
            }
            o[c * 4 + r] = s;
        }
    }
    o
}

const VERT: &str = r#"#version 330 core
layout(location = 0) in vec2 grid;      // 0..1 across the map
uniform mat4 u_view_proj;
uniform sampler2D u_height;             // normalised height, R channel
uniform vec2 u_world;                   // world extent in elmos, per axis
uniform float u_range;                  // maxHeight - minHeight
uniform float u_sea;                    // normalised sea level
uniform float u_exaggeration;
out vec2 v_uv;
out float v_h;
out float v_wet;
void main() {
    v_uv = grid;
    float h = texture(u_height, grid).r;
    v_h = h;
    // Water is a real surface: the ground is lifted to sea level so the plane
    // reads as water rather than as a hole in the terrain.
    v_wet = h < u_sea ? 1.0 : 0.0;
    float y = (max(h, u_sea) - u_sea) * u_range * u_exaggeration;
    gl_Position = u_view_proj * vec4(grid.x * u_world.x, y, grid.y * u_world.y, 1.0);
}
"#;

const FRAG: &str = r#"#version 330 core
in vec2 v_uv;
in float v_h;
in float v_wet;
uniform sampler2D u_height;
uniform sampler2D u_colour;
uniform vec3 u_sun;
uniform vec2 u_world;
uniform float u_range;
uniform float u_texel;                  // 1 / height texture size
uniform float u_exaggeration;
uniform vec3 u_water;
out vec4 frag;
void main() {
    // Normal from the height field, in world units so the shading matches the
    // real slope rather than the texture resolution.
    vec2 step_world = u_texel * u_world;
    float hl = texture(u_height, v_uv - vec2(u_texel, 0.0)).r;
    float hr = texture(u_height, v_uv + vec2(u_texel, 0.0)).r;
    float hd = texture(u_height, v_uv - vec2(0.0, u_texel)).r;
    float hu = texture(u_height, v_uv + vec2(0.0, u_texel)).r;
    float gx = (hr - hl) * u_range * u_exaggeration / (2.0 * step_world.x);
    float gy = (hu - hd) * u_range * u_exaggeration / (2.0 * step_world.y);
    vec3 n = normalize(vec3(-gx, 1.0, -gy));
    float lambert = max(dot(n, normalize(u_sun)), 0.0);
    vec3 base = texture(u_colour, v_uv).rgb;
    vec3 lit = base * (0.42 + 0.72 * lambert);
    if (v_wet > 0.5) {
        // Tint by depth, and keep a little of the seabed visible.
        float depth = clamp((0.0 - (v_h - 0.0)) * 0.0 + 1.0, 0.0, 1.0);
        lit = mix(lit * 0.55, u_water, 0.72);
    }
    frag = vec4(lit, 1.0);
}
"#;

pub struct Renderer {
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    height_tex: glow::Texture,
    colour_tex: glow::Texture,
    index_count: i32,
    tex_size: usize,
}

impl Renderer {
    /// `grid` is the mesh resolution; 256 gives 65k quads, which is plenty for
    /// a preview and cheap on a software rasteriser.
    pub fn new(gl: &glow::Context, grid: usize) -> Result<Renderer, String> {
        unsafe {
            let program = gl.create_program().map_err(|e| e.to_string())?;
            for (kind, src) in [(glow::VERTEX_SHADER, VERT), (glow::FRAGMENT_SHADER, FRAG)] {
                let sh = gl.create_shader(kind).map_err(|e| e.to_string())?;
                gl.shader_source(sh, src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    return Err(gl.get_shader_info_log(sh));
                }
                gl.attach_shader(program, sh);
                gl.delete_shader(sh);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }

            // A static unit grid. Only the height texture changes.
            let n = grid + 1;
            let mut verts: Vec<f32> = Vec::with_capacity(n * n * 2);
            for y in 0..n {
                for x in 0..n {
                    verts.push(x as f32 / grid as f32);
                    verts.push(y as f32 / grid as f32);
                }
            }
            let mut idx: Vec<u32> = Vec::with_capacity(grid * grid * 6);
            for y in 0..grid {
                for x in 0..grid {
                    let a = (y * n + x) as u32;
                    let b = a + 1;
                    let c = a + n as u32;
                    let d = c + 1;
                    idx.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }

            let vao = gl.create_vertex_array().map_err(|e| e.to_string())?;
            gl.bind_vertex_array(Some(vao));
            let vbo = gl.create_buffer().map_err(|e| e.to_string())?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(&verts), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            let ebo = gl.create_buffer().map_err(|e| e.to_string())?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytes_of(&idx),
                glow::STATIC_DRAW,
            );
            gl.bind_vertex_array(None);

            let height_tex = gl.create_texture().map_err(|e| e.to_string())?;
            let colour_tex = gl.create_texture().map_err(|e| e.to_string())?;
            for t in [height_tex, colour_tex] {
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
            }

            Ok(Renderer {
                program,
                vao,
                vbo,
                ebo,
                height_tex,
                colour_tex,
                index_count: idx.len() as i32,
                tex_size: 0,
            })
        }
    }

    /// Upload the normalised height field and the per-mode colour.
    pub fn upload(&mut self, gl: &glow::Context, size: usize, height: &[f32], colour: &[u8]) {
        unsafe {
            // Rows are not 4-byte aligned -- an RGB8 row at 257 wide is 771
            // bytes -- and the default unpack alignment of 4 shears the image
            // into diagonal stripes.
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.height_tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R32F as i32,
                size as i32,
                size as i32,
                0,
                glow::RED,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(bytes_of(height))),
            );
            gl.bind_texture(glow::TEXTURE_2D, Some(self.colour_tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGB8 as i32,
                size as i32,
                size as i32,
                0,
                glow::RGB,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(colour)),
            );
        }
        self.tex_size = size;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        gl: &glow::Context,
        camera: &Camera,
        aspect: f32,
        world: [f32; 2],
        range: f32,
        sea: f32,
        sun: [f32; 3],
        water: [f32; 3],
    ) {
        if self.tex_size == 0 {
            return;
        }
        unsafe {
            gl.use_program(Some(self.program));
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::CULL_FACE);

            let vp = camera.view_projection(aspect, world[0].max(world[1]) * 8.0);
            let u = |n: &str| gl.get_uniform_location(self.program, n);
            gl.uniform_matrix_4_f32_slice(u("u_view_proj").as_ref(), false, &vp);
            gl.uniform_2_f32(u("u_world").as_ref(), world[0], world[1]);
            gl.uniform_1_f32(u("u_range").as_ref(), range);
            gl.uniform_1_f32(u("u_sea").as_ref(), sea);
            gl.uniform_1_f32(u("u_exaggeration").as_ref(), camera.exaggeration);
            gl.uniform_1_f32(u("u_texel").as_ref(), 1.0 / self.tex_size as f32);
            gl.uniform_3_f32(u("u_sun").as_ref(), sun[0], sun[1], sun[2]);
            gl.uniform_3_f32(u("u_water").as_ref(), water[0], water[1], water[2]);

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.height_tex));
            gl.uniform_1_i32(u("u_height").as_ref(), 0);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.colour_tex));
            gl.uniform_1_i32(u("u_colour").as_ref(), 1);

            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements(glow::TRIANGLES, self.index_count, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
            gl.disable(glow::DEPTH_TEST);
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_buffer(self.ebo);
            gl.delete_texture(self.height_tex);
            gl.delete_texture(self.colour_tex);
        }
    }
}

fn bytes_of<T>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// Shared handle, because the paint callback runs on the render thread.
pub type Shared = Arc<std::sync::Mutex<Renderer>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_camera_orbits_around_its_target() {
        let mut c = Camera::default();
        c.frame_map([6144.0, 6144.0]);
        assert_eq!(c.target, [3072.0, 0.0, 3072.0]);
        // The eye stays at a fixed distance whatever the yaw.
        for yaw in [0.0, 1.0, 2.5, 4.0] {
            c.yaw = yaw;
            let e = c.eye();
            let d = ((e[0] - c.target[0]).powi(2)
                + (e[1] - c.target[1]).powi(2)
                + (e[2] - c.target[2]).powi(2))
            .sqrt();
            assert!((d - c.distance).abs() < 0.5, "yaw {yaw}: {d}");
        }
        // Above the ground for every legal pitch.
        for pitch in [0.03, 0.5, 1.52] {
            c.pitch = pitch;
            assert!(c.eye()[1] > 0.0, "pitch {pitch}");
        }
    }

    #[test]
    fn the_map_centre_projects_to_the_middle_of_the_viewport() {
        let mut c = Camera::default();
        c.frame_map([6144.0, 6144.0]);
        let rect = Rect::from_min_size(Pos2::new(212.0, 44.0), Vec2::new(900.0, 600.0));
        let p = c
            .project([3072.0, 0.0, 3072.0], rect, 6144.0 * 8.0)
            .unwrap();
        assert!((p.x - rect.center().x).abs() < 1.0, "{p:?}");
        assert!((p.y - rect.center().y).abs() < 1.0, "{p:?}");
    }

    #[test]
    fn points_behind_the_camera_do_not_project() {
        let mut c = Camera::default();
        c.frame_map([6144.0, 6144.0]);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let behind = {
            let e = c.eye();
            [
                e[0] + (e[0] - c.target[0]),
                e[1] + (e[1] - c.target[1]),
                e[2] + (e[2] - c.target[2]),
            ]
        };
        assert!(c.project(behind, rect, 6144.0 * 8.0).is_none());
    }

    #[test]
    fn framing_scales_with_the_map() {
        let mut small = Camera::default();
        small.frame_map([2048.0, 2048.0]);
        let mut big = Camera::default();
        big.frame_map([16384.0, 16384.0]);
        assert!(big.distance > small.distance * 4.0);
    }
}
